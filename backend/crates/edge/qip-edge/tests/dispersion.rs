//! §32.1's fill-time dispersion gate, driven through the cell.
//!
//! The subject is a real two-venue spatial cycle: buy the instrument where it
//! is cheap, sell it where it is dear, and be a position for however long the
//! two venues disagree about when they will tell you. That interval is the
//! whole risk §32.1 is about, and it is one the cell can measure on its own
//! orders — which is what these tests drive. Nothing here injects a latency:
//! the cell learns each venue's fill time from its own first cycle and
//! refuses the second.
//!
//! The books are seeded through the feed path, because there is no setter
//! that bypasses it, and the graph carries placeholder rates the desk
//! re-quotes from those books before every scan.

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
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{
    Cell, CellConfig, ExecutionReport, GATE_FILL_DISPERSION, GATE_QUOTE_BUDGET, Placer, WorkReport,
};
use qip_edge::dispersion::DispersionPolicy;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::VerifiedPolicy;
use qip_edge::quoting::RateLimits;
use qip_edge::telemetry::{EDGE_FILL_TIME_MILLIS, EDGE_FILL_TIME_UNMEASURED};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
/// The cheap venue, which answers quickly.
const NEAR: &str = "CX";
/// The dear venue, whose fill times are the finding.
const FAR: &str = "DX";
const MARKET: &str = "ETHUSDT";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";
/// See fn graph: the transfers are not re-quoted from a book, so they carry
/// their own observation count and must not be the path's weakest link.
const TRANSFER_OBSERVATIONS: u32 = 1_000;

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

/// A two-sided book for one market at one venue, built from feed messages.
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

/// The same instrument a percent dearer on the far venue: the spatial
/// dislocation the cycle exists to take.
fn books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book(&near(), ("3000", "200"), ("3000.1", "200"))?,
        book(&far(), ("3030", "200"), ("3030.1", "200"))?,
    ])
}

/// Buy ETH with USDT on the near venue, move it, sell it on the far venue,
/// move the proceeds back. Four edges, two of them trades — one per venue,
/// which is what makes the cycle's fill-time spread a thing that exists.
///
/// The transfer edges are free on purpose, and well observed on purpose: a
/// transfer cost would be a second reason for the scan to refuse, and the
/// scanner prices its uncertainty haircut off the *fewest* observations on
/// the path — so a transfer left at zero observations takes the whole net
/// edge to nothing and the cycle never reaches the gate under test. The
/// trade edges are re-quoted from the books and carry the books' own
/// observation counts, which is what the haircut is meant to read.
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
        Duration::from_secs(30),
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

/// A cell holding both books, a fresh policy, the desk, and the dispersion
/// policy the test is about, under a message budget nothing here can spend.
fn cell_with(dispersion: DispersionPolicy) -> Result<(Cell, Arc<Metrics>)> {
    cell_under(dispersion, RateLimits::default())
}

/// The same cell under a message budget the test chooses.
fn cell_under(
    dispersion: DispersionPolicy,
    quote_limits: RateLimits,
) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION)
        .with_venue(near())
        .with_venue(far());
    config.dispersion = dispersion;
    config.quote_limits = quote_limits;
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

/// A venue that fills everything it is given, each venue taking its own time
/// to say so.
#[derive(Debug, Default)]
struct SettlingGateway {
    placed: Vec<(String, VenueId)>,
    pending: Vec<ExecutionReport>,
    /// Milliseconds between the cell's send and the venue's report, by venue.
    latency_millis: BTreeMap<String, i64>,
}

impl SettlingGateway {
    fn taking(latencies: &[(&str, i64)]) -> Self {
        Self {
            latency_millis: latencies
                .iter()
                .map(|(venue, millis)| ((*venue).to_string(), *millis))
                .collect(),
            ..Self::default()
        }
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
        self.placed.push((order_id.to_string(), venue.clone()));
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// A dispersion policy that judges a venue on its first fill, so one cycle is
/// enough to measure both — the subject is the gate, not how many samples
/// make a median.
fn judged_on_one_fill(bound_millis: i64) -> Result<DispersionPolicy> {
    DispersionPolicy::new(Duration::from_millis(bound_millis), 1, 8)
}

#[test]
fn a_cell_that_has_filled_nothing_publishes_every_venue_as_unmeasured_rather_than_fast()
-> Result<()> {
    // The idle state, and the reason the series exists. With no fill times
    // the gate admits every cycle, which on a chart is indistinguishable from
    // a gate that is passing — unless the number of venues it cannot judge is
    // itself published. Written on every pass, including this one, where the
    // cell has done nothing at all.
    let (mut cell, metrics) = cell_with(judged_on_one_fill(5)?)?;
    let mut gateway = SettlingGateway::default();

    assert_eq!(
        cell.fill_times().len(),
        2,
        "the premise failed: the cell was not given two venues to measure"
    );
    assert!(
        cell.fill_times().iter().all(|state| state.median.is_none()),
        "the premise failed: a cell that has filled nothing already has a median"
    );

    cell.work(t(10), &mut gateway)?;
    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.gauge(
            EDGE_FILL_TIME_UNMEASURED,
            &labels([("cell", CELL), ("region", REGION)])
        ),
        Some(2.0),
        "a cell that can judge neither of its venues published no such number, so the gate's \
         silence is invisible"
    );
    Ok(())
}

#[test]
fn a_cycle_whose_two_venues_fill_far_apart_is_refused_whole_on_the_pass_after_it_learns_that()
-> Result<()> {
    // The failure this prevents: sending both legs of a spatial cycle into a
    // pair of venues the cell has already watched answer twenty milliseconds
    // apart. The cycle is one position until its second leg fills, and the
    // unwind cost over that window is whatever the market did — which is
    // §32.1's "dominant risk on any multi-venue execution".
    //
    // The first pass is the premise and the measurement at once: the cell has
    // no fill times, admits the cycle, and learns both venues from its own
    // legs. The second pass is the property.
    let (mut cell, metrics) = cell_with(judged_on_one_fill(5)?)?;
    let mut gateway = SettlingGateway::taking(&[(NEAR, 1), (FAR, 30)]);

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the spatial cycle did not go out as two legs: {:?}",
        first.refusals
    );
    assert_eq!(
        gateway.placed_on(NEAR),
        1,
        "the premise failed: no leg reached the near venue"
    );
    assert_eq!(
        gateway.placed_on(FAR),
        1,
        "the premise failed: no leg reached the far venue"
    );
    assert!(
        refusals_under(&first, GATE_FILL_DISPERSION).is_empty(),
        "the first cycle was refused on a dispersion nobody had measured yet: {:?}",
        first.refusals
    );

    // Both legs filled, each at its own venue's pace, so both are measured.
    let measured = cell.fill_times();
    assert!(
        measured.iter().all(|state| state.median.is_some()),
        "the premise failed: a leg on each venue did not produce a fill time: {measured:?}"
    );
    let second = cell.work(t(20), &mut gateway)?;
    // Published at the *start* of a pass, like the region allocation beside
    // it, so this is the second pass reporting what the first pass taught it.
    assert_eq!(
        metrics.snapshot().gauge(
            EDGE_FILL_TIME_UNMEASURED,
            &labels([("cell", CELL), ("region", REGION)])
        ),
        Some(0.0),
        "the premise failed: the cell still reports venues it cannot judge"
    );
    let refused = refusals_under(&second, GATE_FILL_DISPERSION);
    assert_eq!(
        refused.len(),
        1,
        "a cycle across venues twenty-nine milliseconds apart was not refused under \
         `{GATE_FILL_DISPERSION}`: {:?}",
        second.refusals
    );
    assert!(
        refused[0].contains(FAR) && refused[0].contains(NEAR),
        "the refusal did not name the two venues whose spread caused it: {}",
        refused[0]
    );
    assert!(
        second.orders.is_empty(),
        "the refused cycle sent legs anyway: {:?}",
        second.orders
    );
    assert_eq!(
        gateway.placed.len(),
        2,
        "a leg reached a venue after the cycle was refused whole"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_FILL_DISPERSION)
            ])
        ),
        1,
        "the dispersion refusal did not reach the refusal series"
    );
    Ok(())
}

#[test]
fn two_venues_that_answer_together_keep_trading_and_their_fill_times_are_published() -> Result<()> {
    // The other half of the property above. Without it a gate that refused
    // every cycle after the first fill would pass the test that only checks
    // it refuses — and the cell would have stopped trading cycles entirely
    // while looking like it had a working control.
    let (mut cell, metrics) = cell_with(judged_on_one_fill(5)?)?;
    let mut gateway = SettlingGateway::taking(&[(NEAR, 1), (FAR, 2)]);

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the cycle did not go out: {:?}",
        first.refusals
    );

    let second = cell.work(t(20), &mut gateway)?;
    assert!(
        refusals_under(&second, GATE_FILL_DISPERSION).is_empty(),
        "two venues one millisecond apart were refused under a five-millisecond bound: {:?}",
        second.refusals
    );
    assert_eq!(
        second.orders.len(),
        2,
        "the cycle stopped going out although the spread was inside the bound: {:?}",
        second.refusals
    );

    let snapshot = metrics.snapshot();
    for (venue, millis) in [(NEAR, 1.0), (FAR, 2.0)] {
        let histogram = snapshot
            .histogram(
                EDGE_FILL_TIME_MILLIS,
                &labels([("cell", CELL), ("region", REGION), ("venue", venue)]),
            )
            .unwrap_or_else(|| panic!("{venue} recorded no fill time at all"));
        assert_eq!(
            histogram.count, 2,
            "{venue} filled twice and recorded {} observations",
            histogram.count
        );
        assert!(
            (histogram.max - millis).abs() < f64::EPSILON,
            "{venue}'s fill time was recorded as {} milliseconds rather than {millis}",
            histogram.max
        );
    }
    Ok(())
}

#[test]
fn a_cycle_is_refused_whole_once_the_message_budget_at_its_venues_is_spent() -> Result<()> {
    // §29.2's all-or-nothing rule at the one seam in this crate that sends
    // more than one order at once. A cycle short a leg is a position rather
    // than a smaller cycle, so a rate limit that funded the first leg and
    // refused the second would turn a control meant to keep the venue session
    // up into an open position nobody chose. The budget is therefore taken
    // for every leg or for none, and this is the call site that does it.
    //
    // One spendable placement per venue: the first pass takes it, the second
    // finds the same cycle and cannot fund it.
    let budget = RateLimits::new(3, 1, 2, 2, 4, 64)?;
    let (mut cell, metrics) = cell_under(judged_on_one_fill(500)?, budget)?;
    let mut gateway = SettlingGateway::taking(&[(NEAR, 1), (FAR, 2)]);

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the cycle did not go out while the budget held it: {:?}",
        first.refusals
    );

    // The same instant, so nothing refills and the bucket is the only thing
    // that has changed. The dispersion bound is half a second here, so the
    // gate under test is the budget and not §32.1's.
    let second = cell.work(t(10), &mut gateway)?;
    assert!(
        refusals_under(&second, GATE_FILL_DISPERSION).is_empty(),
        "the dispersion gate refused first, so this says nothing about the budget: {:?}",
        second.refusals
    );
    assert_eq!(
        refusals_under(&second, GATE_QUOTE_BUDGET).len(),
        1,
        "a cycle was admitted against a budget with no placement left in it: {:?}",
        second.refusals
    );
    assert!(
        second.orders.is_empty(),
        "a leg of the refused cycle reached a venue: {:?}",
        second.orders
    );
    assert_eq!(
        gateway.placed.len(),
        2,
        "the gateway was called after the cycle was refused whole"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_QUOTE_BUDGET)
            ])
        ),
        1,
        "the budget refusal did not reach the refusal series"
    );
    Ok(())
}
