//! A signal a cell refused reaches the centre's LEARN stage and is priced
//! (R12-MODEL-7210, ADR 0448).
//!
//! The seam crossed, end to end:
//!
//!   Cell::work (a decision gate refuses a fully reasoned intent)
//!   -> Cell::state_delta (the refusal carries the leg it proposed)
//!   -> CellStateDelta::to_frame -> decode_cell_delta (the mesh wire)
//!   -> CellReport::with_refusals (as `qip-api`'s ingest sink builds it)
//!   -> Platform::ingest_cell_report (queued for the twin)
//!   -> Platform::run_cycle, LEARN (priced once its horizon has passed)
//!
//! Each half was tested alone before this suite: the cell's side table in
//! `qip-edge`, the centre's queue in `qip-kernel/tests/learning.rs`. Only a
//! suite that sees both sides can show the wire carries what the centre
//! reads; a field the cell set and the decoder dropped would pass both.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, ExecutionReport, Placer, PricingPolicy};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::CellReport;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::Stage;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_orderbook::venue::VenueState;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_risk_engine::autonomy::{AutonomyLevel, OperatorIdentity};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::StrategyCompiler;
use qip_strategy::ir::{Expr, Rule, StrategySpec};

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const VENUE: &str = "XLON";
const SYMBOL: &str = "ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-tests";

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(format!("obj-{SYMBOL}"))
}

fn venue() -> VenueId {
    VenueId::new(VENUE)
}

// --- the cell ---------------------------------------------------------------

fn level(sequence: u64, side: BookSide, price: &str, when: Timestamp) -> MarketMessage {
    MarketMessage::new(
        object(),
        Origin::new(venue(), "feed-a", 0, sequence),
        MessageBody::LevelSet {
            side,
            price: Decimal::parse(price).expect("a decimal literal"),
            quantity: dec!("500"),
            order_count: None,
        },
        when,
        when,
    )
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

    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }
}

/// A cell with one strategy that always asks to buy 100, put at
/// observation by a verified operator: every pass reasons a whole intent and
/// the autonomy gate refuses it.
fn observing_cell() -> Result<Cell> {
    let config = CellConfig::new(CELL, REGION).with_venue(venue());
    let mut cell = Cell::new(
        config,
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )?;
    let mut book = VenueState::aggregated(object(), venue(), VenueStatus::Open);
    book.apply(&level(0, BookSide::Bid, "99", t(0)))?;
    book.apply(&level(1, BookSide::Ask, "101", t(1)))?;
    cell.track(book);

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(StrategyId::new("alpha"), object(), Duration::from_secs(30))
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(dec!("100")),
            Expr::Statistic(0.5),
            10,
        ));
    let compiled = compiler.compile(&spec)?;
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new("alpha"),
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
    let signature = sign_payload(ENVELOPE_KEY, &build("unsigned")?.signing_payload());
    let envelope = VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))?;
    cell.deploy_with_pricing(
        compiled,
        compiler.into_program(),
        envelope,
        PricingPolicy::Marketable,
    )?;

    let operator = OperatorIdentity::verified("alice@example.com", "oidc", t(40));
    cell.autonomy_mut().request_change(
        AutonomyLevel::Observation,
        &operator,
        "a drill that proves an observed refusal is learned from",
        None,
        t(41),
    )?;
    Ok(cell)
}

// --- the centre -------------------------------------------------------------

fn platform() -> Result<Platform> {
    let mut universe = Universe::new();
    universe.insert(
        FinancialObject::builder(
            object(),
            SYMBOL,
            InstrumentType::CommonStock,
            qip_financial::costs::LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue(VENUE)
        .sector(Sector::InformationTechnology)
        .price(dec!("100"))
        .provenance(Provenance::synthetic("test", t(0)))
        .build(t(0))?,
    )?;
    let limits = LimitSet::new("acceptance").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    );
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(t(0), config.seed);
    Platform::new(config, context, Telemetry::silent(), universe, limits)
}

fn bar(at: Timestamp, open: f64, close: f64) -> SensedRecord {
    let price = |value: f64| Decimal::from_f64(value).expect("a price");
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(),
        venue: VENUE.to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price(open),
        high: price(open.max(close) * 1.002),
        low: price(open.min(close) * 0.998),
        close: price(close),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Some(price((open + close) / 2.0)),
        quality: DataQuality::default(),
    }))
}

/// Flat at 100 for ninety days up to the instant of the refusal — the last
/// bar closes on it, because the twin enters at the open of the bar that
/// contains the decision, and a tape ending earlier puts the entry on the
/// first bar after it and the horizon's mark on that same open — then a rise both from day
/// to day and within each day — the twin enters at a bar's open, so a tape
/// flat inside each bar leaves only costs to score — so the buy the cell
/// declined would have earned, and the sign of the score is checkable rather
/// than merely present.
fn tape_before() -> Vec<SensedRecord> {
    (1..=90)
        .map(|day| bar(t(50).saturating_sub(Duration::from_days(day)), 100.0, 100.0))
        .collect()
}

fn tape_after() -> Vec<SensedRecord> {
    (1..=5)
        .map(|day| {
            bar(
                t(50).saturating_add(Duration::from_days(day)),
                100.0 + day as f64,
                100.5 + day as f64,
            )
        })
        .collect()
}

/// The failure this guards: until ADR 0448 a directional signal a cell
/// refused at a decision gate reached the centre as a gate and a sentence,
/// so LEARN priced every veto the desk made and none a cell made — the loop
/// learned only from what the cells were allowed to do.
///
/// Mutation: drop the `refused_legs.push` in `Cell::refuse_proposed` — the
/// delta carries no leg and the centre queues nothing ("queued nothing").
#[test]
fn a_signal_a_cell_refused_at_a_decision_gate_is_priced_by_the_centres_learn_stage() -> Result<()> {
    let mut cell = observing_cell()?;
    let mut gateway = RecordingGateway::default();
    let report = cell.work(t(50), &mut gateway)?;
    assert_eq!(
        gateway.placed, 0,
        "premise: an observing cell sent an order"
    );
    assert!(
        report.refusals.iter().any(|(gate, _)| gate == "autonomy"),
        "premise: the autonomy gate did not refuse: {:?}",
        report.refusals
    );

    // Over the wire, as the node publishes and the API decodes.
    let frame = cell.state_delta(&report, t(50)).to_frame()?;
    let decoded = qip_mesh::delta::decode_cell_delta(&frame)?;
    let cell_report = CellReport::new(decoded.standing.cell.clone(), decoded.standing.at)
        .with_region(decoded.standing.region.clone())
        .with_refusals(decoded.interval.refusals.clone());

    let mut platform = platform()?;
    platform.observe(tape_before());
    platform.ingest_cell_report(cell_report, t(50))?;
    assert_eq!(
        platform.cell_declined_awaiting_score(),
        1,
        "the centre queued nothing for the refused signal"
    );

    // Nothing is priced before the twin's horizon has passed.
    platform.run_cycle(t(50));
    assert!(platform.cell_declined_scores().is_empty());

    platform.observe(tape_after());
    let later = platform.run_cycle(t(50).saturating_add(Duration::from_days(3)));
    let learn = later.stage(Stage::Learn).expect("learn ran");
    assert!(
        learn.detail.contains("refused cycles priced"),
        "LEARN did not report pricing the cell's refusal: {}",
        learn.detail
    );
    let scores = platform.cell_declined_scores();
    assert_eq!(
        scores.len(),
        1,
        "one score for one refused signal: {scores:?}"
    );
    let score = &scores[0];
    assert_eq!(score.gate, "autonomy");
    assert_eq!(score.cell, CELL);
    assert_eq!(score.object_id, object());
    assert_eq!(score.venue, VENUE);
    assert!(
        score.would_have_earned.is_positive(),
        "the declined buy on a rising tape earned {}",
        score.would_have_earned
    );
    assert_eq!(platform.cell_declined_awaiting_score(), 0);
    Ok(())
}
