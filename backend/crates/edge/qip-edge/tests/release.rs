//! §32.1's latency-equalised dispatch, driven through the cell (ADR 0084).
//!
//! The subject is the same two-venue spatial cycle `dispersion.rs` drives,
//! and the property is the blueprint's own diagram: `EQUALISED send at
//! (max - own) -> all arrive at 8 ms; dispersion collapses to jitter`. The
//! cell learns each venue's fill time from its first cycle, computes a
//! release schedule from the medians, and the gateway here fills each leg a
//! fixed latency after the instant it was told to release it — so the
//! second cycle's legs arrive together if and only if the schedule is right
//! and the fill time is measured from the release instant.
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
    Cell, CellConfig, ExecutionReport, GATE_RELEASE_LATE, Placer, UnreleasedOrder, WorkReport,
};
use qip_edge::dispersion::DispersionPolicy;
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_edge::policy::VerifiedPolicy;
use qip_edge::quoting::RateLimits;
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
/// A venue that fills everything it is given, a fixed time after the instant
/// the cell told it to release the order — so a leg held back on the
/// schedule arrives later by exactly its hold, which is what the equaliser
/// is counting on.
#[derive(Debug, Default)]
struct ReleasingGateway {
    /// `(order id, venue, the instant the cell said to release no earlier than)`.
    placed: Vec<(String, VenueId, Timestamp)>,
    pending: Vec<ExecutionReport>,
    /// Milliseconds between release and the venue's report, by venue.
    latency_millis: BTreeMap<String, i64>,
    /// Whether the venue answers at all. A venue that does not leaves every
    /// order open, which is the state the withdrawal test needs.
    answers: bool,
    /// What the gateway will say it withdrew unreleased, on the next drain.
    withdrawn: Vec<UnreleasedOrder>,
}

impl ReleasingGateway {
    fn taking(latencies: &[(&str, i64)]) -> Self {
        Self {
            latency_millis: latencies
                .iter()
                .map(|(venue, millis)| ((*venue).to_string(), *millis))
                .collect(),
            answers: true,
            ..Self::default()
        }
    }

    fn silent() -> Self {
        Self::default()
    }

    fn release_of(&self, venue: &str) -> Vec<Timestamp> {
        self.placed
            .iter()
            .filter(|(_, at, _)| at.as_str() == venue)
            .map(|(_, _, release)| *release)
            .collect()
    }
}

impl Placer for ReleasingGateway {
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
        self.placed.push((order_id.to_string(), venue.clone(), at));
        if !self.answers {
            return Ok(());
        }
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
        Ok(())
    }

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        std::mem::take(&mut self.pending)
    }

    fn unreleased(&mut self) -> Vec<UnreleasedOrder> {
        std::mem::take(&mut self.withdrawn)
    }
}

/// A dispersion policy that judges a venue on its first fill, so one cycle is
/// enough to measure both, under a bound wide enough that the cycle is never
/// refused — the subject is the schedule, not the gate.
fn judged_on_one_fill(bound_millis: i64) -> Result<DispersionPolicy> {
    DispersionPolicy::new(Duration::from_millis(bound_millis), 1, 8)
}

fn ms(at: Timestamp, millis: i64) -> Timestamp {
    at.saturating_add(Duration::from_millis(millis))
}

/// Every `order_sent` entry the journal holds, as `(venue, release_at, equalised)`.
fn sent_entries(cell: &Cell) -> Vec<(String, Option<Timestamp>, bool)> {
    cell.journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::OrderSent {
                venue,
                release_at,
                equalised,
                ..
            } => Some((venue.clone(), *release_at, *equalised)),
            _ => None,
        })
        .collect()
}

fn median_of(cell: &Cell, venue: &str) -> Option<Duration> {
    cell.fill_times()
        .into_iter()
        .find(|state| state.venue == venue)
        .and_then(|state| state.median)
}

fn refusals_under<'a>(report: &'a WorkReport, gate: &str) -> Vec<&'a str> {
    report
        .refusals
        .iter()
        .filter(|(g, _)| g == gate)
        .map(|(_, reason)| reason.as_str())
        .collect()
}

#[test]
fn the_slowest_venue_is_released_first_and_every_leg_arrives_together() -> Result<()> {
    // The failure this prevents is the one §32.1 calls the dominant risk on
    // any multi-venue execution: both legs released at once into venues that
    // answer twenty-nine milliseconds apart, so the cycle is a position for
    // twenty-nine milliseconds on every pass. The first pass is the premise
    // and the measurement: the cell has no fill times, sends unequalised and
    // says so, and learns both venues from its own legs. The second pass is
    // the property, and the third is the detail ADR 0084 says decides
    // whether the mechanism works at all — the fill time is measured from
    // the release instant, so a held leg's median does not grow by its own
    // hold and the schedule does not chase its tail.
    let (mut cell, _metrics) = cell_with(judged_on_one_fill(100)?)?;
    let mut gateway = ReleasingGateway::taking(&[(NEAR, 1), (FAR, 30)]);

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the spatial cycle did not go out as two legs: {:?}",
        first.refusals
    );
    let unequalised = sent_entries(&cell);
    assert_eq!(unequalised.len(), 2);
    for (venue, release_at, equalised) in &unequalised {
        assert_eq!(
            *release_at,
            Some(t(10)),
            "{venue} was held on a schedule nobody had measured"
        );
        assert!(
            !equalised,
            "{venue}'s entry claims an equalised send with no median on either venue"
        );
    }
    assert_eq!(
        median_of(&cell, NEAR),
        Some(Duration::from_millis(1)),
        "the premise failed: the near venue was not measured from its first leg"
    );
    assert_eq!(
        median_of(&cell, FAR),
        Some(Duration::from_millis(30)),
        "the premise failed: the far venue was not measured from its first leg"
    );

    let second = cell.work(t(20), &mut gateway)?;
    assert_eq!(
        second.orders.len(),
        2,
        "the second cycle did not go out: {:?}",
        second.refusals
    );
    assert_eq!(
        gateway.release_of(NEAR),
        vec![t(10), ms(t(20), 29)],
        "the near venue's leg was not held back by the difference in medians"
    );
    assert_eq!(
        gateway.release_of(FAR),
        vec![t(10), t(20)],
        "the far venue's leg was not released first"
    );
    let equalised: Vec<_> = sent_entries(&cell).into_iter().skip(2).collect();
    assert_eq!(equalised.len(), 2);
    for (venue, release_at, was_equalised) in &equalised {
        let expected = if venue == NEAR { ms(t(20), 29) } else { t(20) };
        assert_eq!(
            *release_at,
            Some(expected),
            "{venue}'s journal entry does not carry the instant it was released at"
        );
        assert!(
            was_equalised,
            "{venue}'s entry says the cycle was sent unequalised with both venues measured"
        );
    }
    // The blueprint's diagram: both arrive together.
    let arrivals: Vec<Timestamp> = second.fills.iter().map(|fill| fill.at).collect();
    assert_eq!(
        arrivals,
        vec![ms(t(20), 30), ms(t(20), 30)],
        "the legs did not arrive together"
    );

    // Measured from the release instant: the near venue still takes one
    // millisecond, not thirty.
    let third = cell.work(t(30), &mut gateway)?;
    assert_eq!(third.orders.len(), 2, "{:?}", third.refusals);
    assert_eq!(
        median_of(&cell, NEAR),
        Some(Duration::from_millis(1)),
        "the near venue's fill time grew by its own hold, so the equaliser is chasing its tail"
    );
    assert_eq!(
        gateway.release_of(NEAR).last().copied(),
        Some(ms(t(30), 29)),
        "the third schedule shrank the hold the second one computed"
    );
    Ok(())
}

#[test]
fn two_cells_driven_through_identical_passes_journal_identical_release_instants() -> Result<()> {
    // ADR 0084 §5, stated so it can be checked: the schedule is a function of
    // the pass and of nothing else. Two cells, the same inputs, the same
    // `order_sent` entries down to the release instant. A mutation that
    // reads a clock anywhere on the path from fill time to release instant
    // fails this, which is the property the whole design exists for.
    let mut journals = Vec::new();
    for _ in 0..2 {
        let (mut cell, _metrics) = cell_with(judged_on_one_fill(100)?)?;
        let mut gateway = ReleasingGateway::taking(&[(NEAR, 1), (FAR, 30)]);
        for pass in [t(10), t(20), t(30)] {
            let report = cell.work(pass, &mut gateway)?;
            assert_eq!(report.orders.len(), 2, "{:?}", report.refusals);
        }
        journals.push(sent_entries(&cell));
    }
    let (first, second) = (&journals[0], &journals[1]);
    assert_eq!(
        first.len(),
        6,
        "the premise failed: three cycles of two legs did not journal six sends"
    );
    // A range rather than the exact instant, on purpose: the exact instant
    // is the other test's property, and a premise that pinned it here would
    // fire before the identity assertion under a mutation that perturbs the
    // instant — proving the premise, not the property.
    assert!(
        first.iter().any(|(venue, release_at, _)| {
            venue == NEAR && release_at.is_some_and(|at| at > t(20) && at < t(30))
        }),
        "the premise failed: no leg was held on a non-zero offset, so identity here proves \
         nothing about the schedule: {first:?}"
    );
    assert_eq!(
        first, second,
        "two cells handed the same passes computed different release instants"
    );
    Ok(())
}

#[test]
fn an_order_the_gateway_withdrew_unreleased_is_refused_under_its_gate_closed_and_stops_the_cell()
-> Result<()> {
    // ADR 0084 §4: a leg whose release instant is already past is withdrawn,
    // not sent late. The cell's side of that: the chain says the order was
    // sent and the venue never saw it, which is a disagreement between the
    // cell's record and a venue channel — the class every other break is —
    // and for a cycle leg it is also a position, because the legs released
    // on time are out against one that is not.
    let (mut cell, metrics) = cell_with(judged_on_one_fill(100)?)?;
    let mut gateway = ReleasingGateway::silent();

    let first = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        first.orders.len(),
        2,
        "the premise failed: the cycle did not go out: {:?}",
        first.refusals
    );
    let leg = first.orders[0].order_id.clone();
    let venue = first.orders[0].venue.clone();
    assert!(
        cell.open_orders()
            .iter()
            .any(|order| order.order_id == leg && order.closed.is_none()),
        "the premise failed: the leg is not held open on the cell's record"
    );
    assert!(
        !cell.is_halted(),
        "the premise failed: the cell is already halted"
    );

    gateway.withdrawn.push(UnreleasedOrder {
        order_id: leg.clone(),
        venue: venue.clone(),
        scheduled: t(10),
        lag: Duration::from_secs(10),
        tolerance: Duration::from_millis(250),
    });
    let second = cell.work(t(20), &mut gateway)?;
    assert!(
        cell.is_halted(),
        "a leg the venue never received left the cell trading: {:?}",
        second.refusals
    );
    let refused: Vec<&str> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_RELEASE_LATE => {
                Some(reason.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        refused.len(),
        1,
        "the withdrawal was not journaled under `{GATE_RELEASE_LATE}`"
    );
    assert!(
        refused[0].contains(&leg) && refused[0].contains("withdrawn rather than sent late"),
        "the refusal does not name the order and what happened to it: {}",
        refused[0]
    );
    assert_eq!(
        cell.open_orders()
            .iter()
            .find(|order| order.order_id == leg)
            .and_then(|order| order.closed.clone())
            .as_deref(),
        Some("unreleased"),
        "the order the venue never received is still open on the cell's record"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_RELEASE_LATE)
            ])
        ),
        1,
        "the withdrawal did not reach the refusal series"
    );
    assert!(
        refusals_under(&second, GATE_RELEASE_LATE).is_empty(),
        "the withdrawal is not a pass-time refusal and must not be pushed onto the report"
    );
    Ok(())
}
