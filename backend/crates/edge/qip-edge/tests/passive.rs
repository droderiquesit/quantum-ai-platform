//! §32.1's passive-first mechanism, driven through the cell.
//!
//! The subject is the same two-venue spatial cycle `dispersion.rs` drives —
//! buy where the instrument is cheap, sell where it is dear — and the question
//! is what the cell does when one of those two venues is, on its own, slower
//! to answer than the operator says a cycle's legs may be apart. Under the
//! all-at-once discipline both legs go out together and the cycle is a
//! position until the slow one fills. Under this one the slow leg goes alone
//! and the fast leg is crossed against what it actually did.
//!
//! Nothing here injects a latency into the cell. Every fill time these tests
//! turn on is one the cell measured from its own orders on an earlier pass,
//! which is the only source §32.1's machinery has.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{BeliefPriors, CausalDigest, EpisodicDigest, PolicyPayload, Slot};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer};
use qip_edge::dispersion::DispersionPolicy;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::telemetry::EDGE_PASSIVE_CYCLES;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels};
use qip_orderbook::venue::VenueState;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
/// The cheap venue, which answers quickly.
const NEAR: &str = "CX";
/// The dear venue, and the slow one: the leg that rests.
const FAR: &str = "DX";
const MARKET: &str = "ETHUSDT";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";
/// See `graph`: the transfers are not re-quoted from a book, so they carry
/// their own observation count and must not be the path's weakest link.
const TRANSFER_OBSERVATIONS: u32 = 1_000;
/// How long a leg stays priced. Past this the cycle's remaining legs are a
/// price the market has left, and the cell says so rather than crossing them.
const LEG_VALIDITY_SECS: i64 = 30;

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn near() -> VenueId {
    VenueId::new(NEAR)
}

fn far() -> VenueId {
    VenueId::new(FAR)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn book(venue: &VenueId, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(MARKET), venue.clone(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(MARKET),
            Origin::new(venue.clone(), "feed-a", 0, index as u64),
            MessageBody::LevelSet {
                side,
                price: d(price),
                quantity: d(size),
                order_count: None,
            },
            when,
            when,
        ))?;
    }
    Ok(state)
}

fn books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book(&near(), ("3000", "200"), ("3000.1", "200"))?,
        book(&far(), ("3030", "200"), ("3030.1", "200"))?,
    ])
}

fn graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    for venue in [near(), far()] {
        graph.register_venue(
            venue,
            VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
        );
    }
    let fee = d("0.0004");
    graph.add_trade(
        Node::new(object("USDT"), near()),
        Node::new(object("ETH"), near()),
        Decimal::ONE,
        fee,
        object(MARKET),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("ETH"),
        near(),
        far(),
        Decimal::ZERO,
        t(0),
        TRANSFER_OBSERVATIONS,
    )?;
    graph.add_trade(
        Node::new(object("ETH"), far()),
        Node::new(object("USDT"), far()),
        Decimal::ONE,
        fee,
        object(MARKET),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_transfer(
        object("USDT"),
        far(),
        near(),
        Decimal::ZERO,
        t(0),
        TRANSFER_OBSERVATIONS,
    )?;
    Ok(graph)
}

fn sizes() -> SizePolicy {
    SizePolicy::uniform(d("10000")).with(object("ETH"), d("3.3"))
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![near(), far()],
            t(0),
            t(3600),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

fn desk() -> Result<ArbitrageDesk> {
    ArbitrageDesk::new(
        StrategyId::new(DESK),
        OpportunityScanner::new(
            SearchSettings::default(),
            EdgeAssumptions::default(),
            PlanSettings::with_budget(d("50000")),
        ),
        graph()?,
        sizes(),
        signed_envelope(DESK)?,
        4,
        Duration::from_secs(LEG_VALIDITY_SECS),
    )
}

fn fresh_policy(issued_at: Timestamp) -> Result<VerifiedPolicy> {
    let mut payload = PolicyPayload::unproduced(1, CELL, issued_at);
    payload.belief_priors = Slot::produced(
        BeliefPriors {
            priors: BTreeMap::new(),
        },
        issued_at,
    );
    payload.causal_digest = Slot::produced(
        CausalDigest {
            active_edges: Vec::new(),
        },
        issued_at,
    );
    payload.episodic_digest = Slot::produced(
        EpisodicDigest {
            digest: "d".to_string(),
            episodes: 0,
        },
        issued_at,
    );
    VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, issued_at)
}

/// A cell holding both books, a fresh policy, the desk, and a dispersion
/// policy that judges a venue on its first fill — the subject is what the
/// cell does with a measurement, not how many samples make one.
fn cell_with(bound_millis: i64) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION)
        .with_venue(near())
        .with_venue(far());
    config.dispersion = DispersionPolicy::new(Duration::from_millis(bound_millis), 1, 8)?;
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk()?)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    for state in books()? {
        cell.track(state);
    }
    Ok((cell, metrics))
}

/// A venue that answers every order it is given, each venue taking its own
/// time to say so, and that can withdraw a resting one.
///
/// `stops_after` is how many orders it will answer at all. Past that it
/// accepts and reports nothing, which is a venue whose book has walked away
/// from the price — the state the abandonment arm exists for.
#[derive(Debug)]
struct SettlingGateway {
    placed: Vec<(String, VenueId)>,
    pending: Vec<ExecutionReport>,
    latency_millis: BTreeMap<String, i64>,
    stops_after: usize,
    answered: usize,
    cancelled: Vec<String>,
    /// Quantity still open at the venue for each order it has not answered.
    open: BTreeMap<String, Decimal>,
}

impl SettlingGateway {
    fn taking(latencies: &[(&str, i64)]) -> Self {
        Self {
            placed: Vec::new(),
            pending: Vec::new(),
            latency_millis: latencies
                .iter()
                .map(|(venue, millis)| ((*venue).to_string(), *millis))
                .collect(),
            stops_after: usize::MAX,
            answered: 0,
            cancelled: Vec::new(),
            open: BTreeMap::new(),
        }
    }

    fn answering_only(mut self, orders: usize) -> Self {
        self.stops_after = orders;
        self
    }

    fn placed_on(&self, venue: &str) -> usize {
        self.placed
            .iter()
            .filter(|(_, at)| at.as_str() == venue)
            .count()
    }
}

impl Placer for SettlingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if self.answered < self.stops_after {
            let millis = self
                .latency_millis
                .get(venue.as_str())
                .copied()
                .unwrap_or_default();
            self.pending.push(ExecutionReport {
                order_id: order_id.to_string(),
                venue: venue.clone(),
                quantity,
                price,
                at: at.saturating_add(Duration::from_millis(millis)),
            });
            self.answered += 1;
        } else {
            self.open.insert(order_id.to_string(), quantity);
        }
        self.placed.push((order_id.to_string(), venue.clone()));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }

    fn can_cancel(&self) -> bool {
        true
    }

    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        let Some(remaining) = self.open.remove(order_id) else {
            return Err(Error::denied(format!(
                "this venue is not holding order {order_id}"
            )));
        };
        self.cancelled.push(order_id.to_string());
        Ok(remaining)
    }
}

/// A venue with no cancel path at all: it accepts and answers, and cannot be
/// asked to take an order back.
#[derive(Debug)]
struct UnwithdrawableGateway {
    inner: SettlingGateway,
}

impl Placer for UnwithdrawableGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        self.inner
            .place(order_id, object_id, venue, side, quantity, price, at)
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        self.inner.execution_reports()
    }
}

fn kinds(cell: &Cell) -> Vec<&'static str> {
    cell.journal()
        .entries()
        .iter()
        .map(|entry| entry.decision.kind())
        .collect()
}

fn outcome(metrics: &Metrics, outcome: &str) -> u64 {
    metrics.snapshot().counter(
        EDGE_PASSIVE_CYCLES,
        &labels([("cell", CELL), ("region", REGION), ("outcome", outcome)]),
    )
}

/// The latencies every test below turns on: four milliseconds at the near
/// venue and eight at the far one, under a five-millisecond bound. The spread
/// is four, inside the bound, so the dispersion gate admits the cycle — and
/// the far venue's own eight is past it, so passive-first rests there. Both
/// halves matter: a spread outside the bound would be refused whole before
/// this mechanism was ever consulted.
fn slow_far_venue() -> SettlingGateway {
    SettlingGateway::taking(&[(NEAR, 4), (FAR, 8)])
}

#[test]
fn a_cell_that_has_measured_nothing_sends_a_cycle_whole_and_says_it_was_unmeasured() -> Result<()> {
    // The state every cell starts in, and the reason the declining arms are
    // on the series at all: without them a cell that never rests a leg and a
    // cell that never ran a cycle are the same empty counter.
    let (mut cell, metrics) = cell_with(5)?;
    let mut gateway = slow_far_venue();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the spatial cycle did not go out as two legs: {:?}",
        first.refusals
    );
    assert_eq!(
        outcome(&metrics, "unmeasured"),
        1,
        "a cycle sent whole for want of a measurement did not say so on the series"
    );
    assert_eq!(
        outcome(&metrics, "rested"),
        0,
        "a cell with no fill times rested a leg on a venue it has never measured"
    );
    Ok(())
}

#[test]
fn a_cycle_whose_far_venue_is_slower_than_the_bound_sends_only_that_leg_and_waits() -> Result<()> {
    // The failure this prevents: crossing the near leg of a spatial cycle
    // while the far venue — which the cell has already watched take twice the
    // bound to answer — has said nothing. The near leg is then an outright
    // position for as long as the far one takes, at a price chosen for an
    // arbitrage that only exists if both fill.
    //
    // The first pass is the premise and the measurement at once: the cell has
    // no fill times, sends the cycle whole, and learns both venues from its
    // own legs. The second pass is the property.
    let (mut cell, metrics) = cell_with(5)?;
    let mut gateway = slow_far_venue();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the cycle did not go out whole on the unmeasured pass: {:?}",
        first.refusals
    );
    assert_eq!(gateway.placed_on(NEAR), 1);
    assert_eq!(gateway.placed_on(FAR), 1);

    let second = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        second.orders.len(),
        1,
        "the cycle did not rest: both legs went out although the far venue answers past the \
         bound: {:?}",
        second.refusals
    );
    assert_eq!(
        second.orders[0].venue.as_str(),
        FAR,
        "the leg that rested was not the one at the slow venue"
    );
    assert_eq!(
        gateway.placed_on(NEAR),
        1,
        "the near leg was crossed while the far venue had said nothing"
    );
    assert_eq!(gateway.placed_on(FAR), 2);
    assert_eq!(
        outcome(&metrics, "rested"),
        1,
        "the resting leg did not reach the series"
    );
    assert!(
        kinds(&cell).contains(&"cycle_rested"),
        "the chain does not say why only one leg of the cycle reached a venue"
    );
    Ok(())
}

#[test]
fn the_near_leg_is_crossed_on_the_pass_the_far_venue_answers_and_not_before() -> Result<()> {
    // The other half, and the half that separates a mechanism from a new
    // refusal: the cycle does complete. A control that rested a leg and never
    // crossed the rest would pass the test above and have stopped the cell
    // trading cycles altogether.
    let (mut cell, metrics) = cell_with(5)?;
    let mut gateway = slow_far_venue();

    cell.work(t(10), &mut gateway)?;
    let rested = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        rested.orders.len(),
        1,
        "the premise failed: no leg rested, so there is nothing to complete: {:?}",
        rested.refusals
    );
    let near_before = gateway.placed_on(NEAR);

    let third = cell.work(t(25), &mut gateway)?;
    assert!(
        gateway.placed_on(NEAR) > near_before,
        "the far venue answered and the near leg was never crossed behind it: {:?}",
        third.refusals
    );
    assert_eq!(
        outcome(&metrics, "completed"),
        1,
        "the completed cycle did not reach the series"
    );
    assert_eq!(
        outcome(&metrics, "abandoned"),
        0,
        "a cycle whose resting leg filled whole was recorded as abandoned"
    );
    Ok(())
}

#[test]
fn a_resting_leg_that_fills_nothing_is_withdrawn_and_the_near_leg_is_never_crossed() -> Result<()> {
    // The outcome the mechanism exists to produce, and the one the
    // all-at-once discipline cannot reach. The far venue answers the first
    // cycle — which is how the cell comes to know it is slow — and then stops
    // answering. Its resting leg is withdrawn when the cycle's own legs
    // expire, nothing of that cycle ever filled, and the near leg is never
    // sent. Under the previous discipline the near leg would already have
    // been crossed and the cell would be holding it outright.
    let (mut cell, metrics) = cell_with(5)?;
    let mut gateway = slow_far_venue().answering_only(2);

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the measuring cycle did not go out: {:?}",
        first.refusals
    );
    let rested = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        rested.orders.len(),
        1,
        "the premise failed: no leg rested: {:?}",
        rested.refusals
    );
    let near_before = gateway.placed_on(NEAR);

    // Past the leg validity the cycle was priced with, so the resting order's
    // own time to live has elapsed and the expiry sweep withdraws it.
    let after = cell.work(t(20 + LEG_VALIDITY_SECS + 1), &mut gateway)?;
    assert!(
        !gateway.cancelled.is_empty(),
        "the resting leg was never withdrawn although its time to live had elapsed: {:?}",
        after.refusals
    );
    assert_eq!(
        gateway.placed_on(NEAR),
        near_before,
        "the near leg was crossed against a far leg that filled nothing"
    );
    assert_eq!(
        outcome(&metrics, "abandoned"),
        1,
        "the abandoned cycle did not reach the series"
    );
    assert_eq!(
        outcome(&metrics, "completed"),
        0,
        "a cycle whose resting leg filled nothing was recorded as completed"
    );
    assert!(
        kinds(&cell).contains(&"cycle_abandoned"),
        "the chain does not record that a cycle was given up on having filled nothing"
    );
    assert!(
        !cell.is_halted(),
        "a cycle abandoned before it became a position halted the cell; nothing went wrong"
    );
    Ok(())
}

#[test]
fn a_gateway_that_cannot_withdraw_an_order_is_never_given_a_resting_leg() -> Result<()> {
    // A leg may only rest where the cell could take it back. The same guard
    // `resolve_pricing` puts on a resting net: an order nothing can withdraw
    // sits at a price the market has since left — and here it would strand
    // the rest of the cycle behind it for as long as the venue kept it. The
    // cycle goes out whole instead, which is what the cell did before this
    // mechanism existed.
    let (mut cell, metrics) = cell_with(5)?;
    let mut gateway = UnwithdrawableGateway {
        inner: slow_far_venue(),
    };

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the measuring cycle did not go out: {:?}",
        first.refusals
    );
    let second = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        second.orders.len(),
        2,
        "a cycle rested a leg at a gateway that could never withdraw it: {:?}",
        second.refusals
    );
    assert_eq!(
        outcome(&metrics, "no_withdrawal"),
        1,
        "the reason the cycle went out whole did not reach the series"
    );
    assert_eq!(
        outcome(&metrics, "rested"),
        0,
        "a leg rested where nothing could withdraw it"
    );
    Ok(())
}

#[test]
fn two_venues_inside_the_bound_keep_sending_their_cycle_in_one_pass() -> Result<()> {
    // The threshold's own test. Same shape, same measurement, a bound wide
    // enough to cover the far venue's answer — and the cycle goes out whole,
    // because waiting a pass to remove eight milliseconds from a window the
    // operator has said may be fifty is a cost with nothing bought by it.
    let (mut cell, metrics) = cell_with(50)?;
    let mut gateway = slow_far_venue();

    cell.work(t(10), &mut gateway)?;
    let second = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        second.orders.len(),
        2,
        "a cycle whose slowest venue answers well inside the bound rested a leg anyway: {:?}",
        second.refusals
    );
    assert_eq!(
        outcome(&metrics, "within_bound"),
        1,
        "the reason the cycle was sent whole did not reach the series"
    );
    assert_eq!(
        outcome(&metrics, "rested"),
        0,
        "a leg rested on a venue answering inside the bound"
    );
    Ok(())
}

#[test]
fn a_cycle_already_resting_a_leg_is_never_opened_a_second_time() -> Result<()> {
    // The scanner re-quotes the graph on every pass and will happily find the
    // same dislocation again. Opening it twice would double the position the
    // first one is waiting to complete, at a venue that has answered neither.
    // The guard is the cycle's own id, and the refusal is its own gate rather
    // than `open_orders` or a break: nothing is wrong, the cell is waiting.
    let (mut cell, _metrics) = cell_with(5)?;
    let mut gateway = slow_far_venue().answering_only(2);

    cell.work(t(10), &mut gateway)?;
    let rested = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        rested.orders.len(),
        1,
        "the premise failed: no leg rested: {:?}",
        rested.refusals
    );
    let far_before = gateway.placed_on(FAR);

    // The same instant, so the scanner derives the same cycle id it did on
    // the pass that rested — which is the only way this guard can be reached.
    let again = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        gateway.placed_on(FAR),
        far_before,
        "a second leg was sent for a cycle already resting one: {:?}",
        again.refusals
    );
    assert!(
        again
            .refusals
            .iter()
            .any(|(gate, _)| gate == qip_edge::cell::GATE_CYCLE_RESTING),
        "the second admission was not refused under its own gate: {:?}",
        again.refusals
    );
    Ok(())
}
