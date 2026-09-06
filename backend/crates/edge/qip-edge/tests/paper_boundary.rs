//! The edge half of the paper-trading boundary, at the two seams that hold it.
//!
//! The safety rules name `qip-edge`'s `Cell` as the third of three independent
//! layers: "no constructor taking a ceiling other than paper trading". Two
//! things were true of that layer at the same time and neither was written
//! down. The cell read `Placer::is_simulated` — the bit `qip-routing`'s own
//! documentation calls "the bit that decides", and the bit
//! `qip-execution-engine`'s order manager refuses on — and used it *only* to
//! stamp a journal entry, so the one process that places orders without
//! asking the central plane (ADR 0008) never compared its posture against the
//! class of venue it was sending to. And `Cell::new` returned `Result` over a
//! body that was a single `Ok(Self { .. })`, so a signature that read as
//! "assembly refuses a bad configuration" refused nothing.
//!
//! These tests hold both closed from outside the crate, through the public
//! order path. They are about refusals: every assertion here is that the
//! platform did *not* do something, so each one asserts its own premise first
//! — a cell that would have sent, a configuration that is otherwise good.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{
    Cell, CellConfig, CrossingInterval, GATE_LIVE_VENUE, Placer, PricingPolicy, WorkReport,
};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::journal::Decision;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::{Labels, Metrics, labels, names};
use qip_orderbook::venue::VenueState;
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec};
use qip_strategy::program::Program;
use std::sync::Arc;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-boundary-tests";
const STRATEGY: &str = "boundary-probe";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

fn by_gate(gate: &str) -> Labels {
    labels([("cell", CELL), ("region", REGION), ("gate", gate)])
}

fn level(sequence: u64, side: BookSide, price: &str, size: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: d(price),
            quantity: d(size),
            order_count: None,
        },
        when,
        when,
    )
}

/// A two-sided book with a mid of 100, so a marketable order can be priced.
fn book() -> Result<VenueState> {
    let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    for (index, (side, price, size)) in
        [(BookSide::Bid, "99", "500"), (BookSide::Ask, "101", "400")]
            .iter()
            .enumerate()
    {
        state.apply(&level(index as u64, *side, price, size, t(index as i64)))?;
    }
    Ok(state)
}

fn firing_strategy() -> Result<(CompiledStrategy, Program)> {
    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new(STRATEGY), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(d("10")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

fn signed_envelope() -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
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

/// A cell that will send exactly one order on its first pass, recording into
/// a registry the test can read.
fn armed_cell() -> Result<(Cell, Arc<Metrics>)> {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?.with_metrics(Arc::clone(&metrics));
    cell.track(book()?);
    let (compiled, program) = firing_strategy()?;
    cell.deploy_with_pricing(
        compiled,
        program,
        signed_envelope()?,
        PricingPolicy::Marketable,
    )?;
    Ok((cell, metrics))
}

/// A gateway whose class the test chooses, recording every order it is asked
/// to place.
///
/// `is_simulated` is a trait method, and both of `qip-edge-node`'s gateways
/// answer it from the adapter's own `Broker` rather than from configuration —
/// so a `false` here is the same shape as a `false` in a deployment, and the
/// refusal it triggers is a control that can fire rather than one held shut
/// by construction.
#[derive(Debug)]
struct ClassedGateway {
    simulated: bool,
    placed: Vec<String>,
}

impl ClassedGateway {
    const fn new(simulated: bool) -> Self {
        Self {
            simulated,
            placed: Vec::new(),
        }
    }
}

impl Placer for ClassedGateway {
    fn is_simulated(&self) -> bool {
        self.simulated
    }

    fn place(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        self.placed.push(order_id.to_string());
        Ok(())
    }
}

fn refused_under(report: &WorkReport, gate: &str) -> bool {
    // Whole-token comparison, not `contains`: a gate named `live_venue_probe`
    // would satisfy a substring match and mean something else entirely.
    report.refusals.iter().any(|(named, _)| named == gate)
}

#[test]
fn a_cell_handed_a_live_class_gateway_places_nothing_and_names_the_gate() -> Result<()> {
    // The premise, first and separately: this cell, this book, this strategy
    // and a *simulated* gateway put exactly one order at the venue. Without
    // it every assertion below would hold for a cell that was never going to
    // trade, which is the shape of a refusal test that guards nothing.
    let (mut cell, _) = armed_cell()?;
    let mut simulated = ClassedGateway::new(true);
    let report = cell.work(t(10), &mut simulated)?;
    assert_eq!(
        report.orders.len(),
        1,
        "the premise failed: a simulated gateway saw {:?} and the pass refused {:?}",
        simulated.placed,
        report.refusals
    );
    assert_eq!(
        simulated.placed.len(),
        1,
        "the premise failed: the simulated venue was not called"
    );

    // The same cell shape, handed a gateway that reports itself live.
    let (mut cell, metrics) = armed_cell()?;
    let mut live = ClassedGateway::new(false);
    let before = metrics
        .snapshot()
        .counter(names::EDGE_REFUSALS, &by_gate(GATE_LIVE_VENUE));
    let report = cell.work(t(10), &mut live)?;

    assert!(
        live.placed.is_empty(),
        "an order reached a live-class venue: {:?}",
        live.placed
    );
    assert!(
        report.orders.is_empty(),
        "the cell reported placing orders it must not have placed: {:?}",
        report.orders
    );
    assert!(
        refused_under(&report, GATE_LIVE_VENUE),
        "the pass did not refuse under the {GATE_LIVE_VENUE} gate: {:?}",
        report.refusals
    );
    // The refusal has to be attributable after the fact, from the chain
    // alone, and it has to name the posture that was in force — an operator
    // reading "refused" with no ceiling beside it cannot tell a live gateway
    // from a cell somebody had already stood down.
    let reasons: Vec<String> = cell
        .journal()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.decision {
            Decision::Refused { gate, reason } if gate == GATE_LIVE_VENUE => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("paper_trading")),
        "the chain does not name the ceiling in force at the refusal: {reasons:?}"
    );
    let after = metrics
        .snapshot()
        .counter(names::EDGE_REFUSALS, &by_gate(GATE_LIVE_VENUE));
    assert_eq!(
        (before, after),
        (0, 1),
        "the refusal is invisible: qip_edge_refusals_total{{gate=\"{GATE_LIVE_VENUE}\"}} went \
         {before} -> {after}"
    );
    Ok(())
}

#[test]
fn a_live_class_gateway_is_refused_on_every_pass_and_not_only_the_first() -> Result<()> {
    // A misconfiguration an operator has not yet found must keep saying so.
    // A gate that fired once and then went quiet would read on a dashboard
    // as a problem that had resolved itself.
    let (mut cell, metrics) = armed_cell()?;
    let mut live = ClassedGateway::new(false);
    for pass in 0..3 {
        let report = cell.work(t(10 + pass), &mut live)?;
        assert!(
            refused_under(&report, GATE_LIVE_VENUE),
            "pass {pass} did not refuse under the {GATE_LIVE_VENUE} gate: {:?}",
            report.refusals
        );
    }
    assert!(
        live.placed.is_empty(),
        "an order reached a live-class venue across repeated passes: {:?}",
        live.placed
    );
    assert_eq!(
        metrics
            .snapshot()
            .counter(names::EDGE_REFUSALS, &by_gate(GATE_LIVE_VENUE)),
        3,
        "the series does not count one refusal per pass"
    );
    Ok(())
}

#[test]
fn a_configuration_the_cell_could_not_work_under_is_refused_at_assembly() -> Result<()> {
    // `Cell::new` advertised this in its signature from the day it was
    // written and performed none of it: the body was one `Ok(Self { .. })`
    // and the configuration was moved in unexamined. Each case below is a
    // cell that would have started, reported healthy, and been unable to do
    // the job it was deployed for.
    let features = || FeatureEngine::new(MarketState::default(), Duration::from_secs(5));

    // The premise: the configuration these cases spoil is otherwise good, so
    // a refusal below is about the field the case changed and nothing else.
    let good = CellConfig::new(CELL, REGION).with_venue(venue());
    assert!(
        Cell::new(good.clone(), features()).is_ok(),
        "the premise failed: the unspoiled configuration was refused, so every case below \
         would pass for the wrong reason"
    );

    // No cell id: orders would be numbered `-1`, `-2`, and two such cells
    // sharing a regional allocation would key their holds identically.
    let mut nameless = good.clone();
    nameless.cell_id = "   ".to_string();
    assert!(
        Cell::new(nameless, features()).is_err(),
        "a cell with no id was assembled"
    );

    // No region: the state delta reaches the centre attributed to nowhere and
    // the metric registry gets an empty `region` label.
    let mut regionless = good.clone();
    regionless.region = String::new();
    assert!(
        Cell::new(regionless, features()).is_err(),
        "a cell with no region was assembled"
    );

    // No venue: `venue_for` searches the configured list, so every signal the
    // cell raises dies at venue selection. It would look deployed and be inert.
    let mut venueless = good.clone();
    venueless.venues.clear();
    assert!(
        Cell::new(venueless, features()).is_err(),
        "a cell with no venue was assembled"
    );

    // A crossing interval of zero passes measures §27.1's cap against
    // nothing, so every cross is admitted. `CellConfig::with_crossing_interval`
    // has always refused it — but every field of `CellConfig` is `pub`, so
    // writing the field directly walked straight past that check until
    // assembly ran it again.
    let mut past_the_builder = good.clone();
    past_the_builder.crossing_interval = Some(CrossingInterval::Passes(0));
    assert!(
        good.clone()
            .with_crossing_interval(CrossingInterval::Passes(0))
            .is_err(),
        "the premise failed: the builder no longer refuses a zero-pass interval, so the case \
         below is not the bypass it was written for"
    );
    assert!(
        Cell::new(past_the_builder, features()).is_err(),
        "a zero-pass crossing interval reached a cell by skipping the builder"
    );

    // And the other half of a working gate: a configuration that names a real
    // interval is still admitted. A check that refused everything would pass
    // every assertion above and be worthless.
    let admitted = good.with_crossing_interval(CrossingInterval::Passes(4))?;
    assert!(
        Cell::new(admitted, features()).is_ok(),
        "assembly refuses a configuration that is entirely valid"
    );
    Ok(())
}
