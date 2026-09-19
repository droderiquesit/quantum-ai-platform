//! §56.2 rule 21, driven through the cell: "Reservation is settlement-aware.
//! Check availability at the time the leg needs it."
//!
//! Every test here runs a real pass over a real triangular dislocation, so
//! what is proven is the gate at the seam and not the calendar arithmetic
//! (`src/settlement.rs` holds that). The fixture is the arbitrage suite's:
//! one crypto venue, three books a percent out of line, a desk that finds
//! exactly one cycle. What varies is the venue's settlement terms, which is
//! the only input the gate reads.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_arbitrage::{
    ArbitrageGraph, EdgeAssumptions, Node, OpportunityScanner, PlanSettings, SearchSettings,
    SizePolicy, VenueFacts,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{
    BeliefPriors, CausalDigest, EpisodicDigest, HaltCommand, PolicyPayload, Slot,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::arbitrage::ArbitrageDesk;
use qip_edge::cell::{Cell, CellConfig, Placer, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::policy::{VerifiedHalt, VerifiedPolicy};
use qip_edge::settlement::{GATE_SETTLEMENT, SettlementConvention, SettlementTerms};
use qip_edge::telemetry::EDGE_SETTLEMENT_UNPROJECTED;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use std::collections::BTreeMap;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "CX";
const DESK: &str = "arb-desk";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-tests";

/// 2025-10-09T08:53:20Z plus `secs`: a Thursday morning, inside a 16:00
/// cut-off, so a T+2 fill lands on the Monday and the date asserted below
/// is a date a reader can re-derive from the calendar.
fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object(name: &str) -> ObjectId {
    ObjectId::from_string(name)
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn node(object_name: &str) -> Node {
    Node::new(object(object_name), venue())
}

fn book(market: &str, bid: (&str, &str), ask: (&str, &str)) -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(market), venue(), VenueStatus::Open);
    for (index, (side, (price, size))) in [(BookSide::Bid, bid), (BookSide::Ask, ask)]
        .into_iter()
        .enumerate()
    {
        let when = t(index as i64);
        state.apply(&MarketMessage::new(
            object(market),
            Origin::new(venue(), "feed-a", 0, index as u64),
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

/// A real triangular dislocation: ETH/BTC a percent away from what the two
/// dollar legs imply.
fn ethereum_books() -> Result<Vec<VenueState>> {
    Ok(vec![
        book("ETHUSDT", ("3000", "200"), ("3000.1", "200"))?,
        book("ETHBTC", ("0.0505", "200"), ("0.05051", "200"))?,
        book("BTCUSDT", ("60000", "10"), ("60001", "10"))?,
    ])
}

fn ethereum_graph() -> Result<ArbitrageGraph> {
    let mut graph = ArbitrageGraph::new();
    graph.register_venue(
        venue(),
        VenueFacts::new(VenueClass::CryptoExchange, VenueStatus::Open),
    );
    let unquoted = Decimal::ONE;
    let fee = d("0.0004");
    graph.add_trade(
        node("USDT"),
        node("ETH"),
        unquoted,
        fee,
        object("ETHUSDT"),
        BookSide::Ask,
        t(0),
        0,
    )?;
    graph.add_trade(
        node("ETH"),
        node("BTC"),
        unquoted,
        fee,
        object("ETHBTC"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    graph.add_trade(
        node("BTC"),
        node("USDT"),
        unquoted,
        fee,
        object("BTCUSDT"),
        BookSide::Bid,
        t(0),
        0,
    )?;
    Ok(graph)
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
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
        ethereum_graph()?,
        SizePolicy::uniform(d("10000"))
            .with(object("ETH"), d("3.3"))
            .with(object("BTC"), d("0.16")),
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

fn halt(issued_at: Timestamp) -> Result<VerifiedHalt> {
    let command = HaltCommand::new(CELL, issued_at, "operator halt").signed(POLICY_KEY)?;
    VerifiedHalt::verify(command, POLICY_KEY, CELL, issued_at)
}

/// A wired cell holding the dislocation, a fresh policy, the desk, and the
/// venue's settlement terms if any.
fn cell_with(terms: Option<SettlementTerms>) -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let mut config = CellConfig::new(CELL, REGION).with_venue(venue());
    if let Some(terms) = terms {
        config = config.with_settlement(&venue(), terms);
    }
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?
        .with_metrics(Arc::clone(&metrics))
        .with_arbitrage(desk()?)?;
    cell.apply_policy(fresh_policy(t(5))?, t(5))?;
    for state in ethereum_books()? {
        cell.track(state);
    }
    Ok((cell, metrics))
}

#[derive(Debug, Default)]
struct RecordingGateway {
    placed: usize,
}

impl Placer for RecordingGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed += 1;
        Ok(())
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

fn unprojected(metrics: &Metrics) -> Option<f64> {
    metrics.snapshot().gauge(
        EDGE_SETTLEMENT_UNPROJECTED,
        &labels([("cell", CELL), ("region", REGION)]),
    )
}

#[test]
fn a_cycle_whose_leg_spends_proceeds_that_settle_on_a_calendar_is_refused_whole_before_any_capital_is_held()
-> Result<()> {
    // The failure this closes: the cell held a cycle gross out of settled
    // capital and read that as the reservation being settled, while the
    // second leg of every conversion chain spends what the first delivered
    // at the same venue — an asset the venue's calendar says is in
    // settlement for two days. A cell with no bridge (§32.2) was sending
    // the chain anyway.
    //
    // Premise: at a venue that credits on the fill the same cycle goes out
    // whole, so what stops it below is the calendar and not the scanner.
    let (mut instant, _) = cell_with(Some(SettlementTerms::instant()))?;
    let mut gateway = RecordingGateway::default();
    let baseline = instant.work(t(10), &mut gateway)?;
    assert_eq!(
        baseline.orders.len(),
        3,
        "the premise failed: the cycle did not go out at an instant venue: {baseline:?}"
    );
    assert!(
        refusals_under(&baseline, GATE_SETTLEMENT).is_empty(),
        "an instant venue was refused under the settlement gate"
    );

    let (cell, metrics) = cell_with(Some(SettlementTerms::weekday(SettlementConvention::T2)?))?;
    let mut cell = cell.with_region_allocation(d("1000000"))?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;

    assert!(
        report.orders.is_empty(),
        "a cycle funded by T+2 proceeds still sent legs: {:?}",
        report.orders
    );
    assert_eq!(gateway.placed, 0, "a leg reached the venue");

    let refused = refusals_under(&report, GATE_SETTLEMENT);
    assert_eq!(
        refused.len(),
        1,
        "the settlement gate did not refuse exactly once: {:?}",
        report.refusals
    );
    // The reason is the evidence: the day the proceeds land, walked on
    // settlement days, and the wait from the leg's own fire instant. A
    // Thursday 08:53 fill on T+2 is Monday 09:00 — Saturday on a
    // calendar-day count — and that is four days on: 96 hours 6 minutes.
    let reason = refused[0];
    assert!(
        reason.contains(" CX settles T+2:"),
        "the refusal did not name the venue's terms: {reason}"
    );
    assert!(
        reason.contains("usable from 2025-10-13T09:00:00.000Z"),
        "the refusal did not land the proceeds on the Monday: {reason}"
    );
    assert!(
        reason.contains(", 96 h 6 min after the leg fires"),
        "the refusal did not state the wait from the fire instant: {reason}"
    );
    assert!(
        reason.contains("leg 2 (") && reason.contains("spends what leg 1 ("),
        "the refusal did not name the dependent leg and its source: {reason}"
    );
    let whole = refusals_under(&report, "arbitrage_cycle");
    assert!(
        whole
            .iter()
            .any(|reason| reason.contains("proceeds still in settlement when it fires")),
        "the cycle was not vetoed whole for settlement: {whole:?}"
    );
    // Before the hold: the projection runs ahead of the region reservation,
    // so a refused cycle never touched the region's capital.
    assert_eq!(
        cell.region_allocation_free(),
        Some(d("1000000")),
        "a cycle the calendar refused took region capital first"
    );
    assert_eq!(
        metrics.snapshot().counter(
            names::EDGE_REFUSALS,
            &labels([
                ("cell", CELL),
                ("region", REGION),
                ("gate", GATE_SETTLEMENT)
            ])
        ),
        1,
        "the refusal was not counted under its own gate"
    );
    Ok(())
}

#[test]
fn a_venue_with_no_terms_is_not_judged_and_the_gauge_counts_it_on_every_pass_including_a_halted_one()
-> Result<()> {
    // The stated arm: a venue the cell holds no terms for is neither read
    // as instant nor as T+2, and its dependent legs go out unjudged — which
    // is exactly the state a chart of refusals cannot show, so the count of
    // such venues is a gauge written on every pass.
    let (mut cell, metrics) = cell_with(None)?;
    assert_eq!(
        unprojected(&metrics),
        None,
        "the premise failed: the gauge was written before any pass"
    );
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(10), &mut gateway)?;
    assert_eq!(
        report.orders.len(),
        3,
        "a venue with no terms was judged rather than counted: {report:?}"
    );
    assert!(refusals_under(&report, GATE_SETTLEMENT).is_empty());
    assert_eq!(
        unprojected(&metrics),
        Some(1.0),
        "the one venue with no terms was not counted"
    );

    // With terms, the same cell reports nothing unprojected.
    let (mut stated, metrics) = cell_with(Some(SettlementTerms::instant()))?;
    stated.work(t(10), &mut RecordingGateway::default())?;
    assert_eq!(unprojected(&metrics), Some(0.0));

    // Halted before its first pass, so the pass is the only thing that could
    // have written the gauge: a halted cell that went dark on the number
    // would read exactly like a cell with terms for every venue.
    let (mut halted, metrics) = cell_with(None)?;
    halted.apply_halt(halt(t(8))?, t(8));
    let report = halted.work(t(10), &mut RecordingGateway::default())?;
    assert!(
        report.halted,
        "the premise failed: the pass did not see the halt"
    );
    assert_eq!(
        unprojected(&metrics),
        Some(1.0),
        "a halted pass did not publish how many venues the settlement gate cannot project"
    );
    Ok(())
}
