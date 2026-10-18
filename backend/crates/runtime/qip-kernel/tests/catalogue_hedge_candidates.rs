//! The Hedge Brain plans over the candidates a composition root declares from
//! the committed catalogue (blueprint §14.1, R12-RISK-4106, R12-RISK-4109,
//! R12-CONTRACT-6114).
//!
//! Until `paper_hedge_candidates` existed every binary handed the Hedge Brain
//! an empty candidate list. The search still ran every DECIDE and reported
//! each failing world as unprotectable, which reads like a working control
//! and could never plan a hedge. These tests assemble the platform the way
//! the three central roots now do — over `data/datasets/universe.json`, with
//! `PlatformConfig::with_hedge_candidates(paper_hedge_candidates(..))` and the
//! shipped survival policy — and hold that a book failing a library world is
//! planned a hedge from that catalogue, with every measure §14.1 names
//! recorded on the journalled plan.

#![allow(clippy::panic_in_result_fn)]
// The equalities below compare a figure copied from the catalogue, or a
// measure that is exactly zero by construction, not a computed float.
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, ObjectId, dec};
use qip_events::Topic;
use qip_execution_engine::order::Side;
use qip_financial::asset_class::InstrumentType;
use qip_financial::universe::Universe;
use qip_kernel::config::{PlatformConfig, paper_hedge_candidates};
use qip_kernel::cycle::Stage;
use qip_kernel::hedge::HedgePlanned;
use qip_kernel::platform::Platform;
use qip_kernel::survival::SurvivalPolicy;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::{Trade, TradeCondition};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_reasoning_engine::redteam::ReviewPolicy;
use qip_risk::hedge_search::InstrumentClass;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;

/// The file every central root reads, relative to this crate so the test
/// reads the committed artefact and not a copy of it.
const COMMITTED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../data/datasets/universe.json"
);

/// The name the book is long, and the one the tape gives a thesis on.
const EXPOSED: &str = "OBJ00000000000000000NWSC";
const SUBJECT: &str = "OBJ00000000000000000VNTG";

fn start() -> Timestamp {
    Timestamp::from_civil(2026, 10, 1)
}

fn catalogue() -> Universe {
    let text = std::fs::read_to_string(COMMITTED).expect("the committed catalogue is readable");
    qip_financial::catalogue::load(&text, start())
        .expect("the committed catalogue loads")
        .universe
}

fn leverage_only() -> LimitSet {
    LimitSet::new("hedge-candidates-test").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 3.0 })
            .with_rationale("gross exposure is capped at 3x equity"),
    )
}

fn platform(universe: Universe) -> Result<Platform> {
    let candidates = paper_hedge_candidates(&universe);
    let config = PlatformConfig {
        initial_equity: dec!("1000000"),
        review: ReviewPolicy {
            minimum_surviving_confidence: 0.10,
            ..ReviewPolicy::default()
        },
        ..PlatformConfig::default()
    }
    .with_hedge_candidates(candidates);
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        leverage_only(),
    )?;
    platform.set_survival_policy(SurvivalPolicy::shipped());
    Ok(platform)
}

fn bar(object: &str, at: Timestamp, open: f64, close: f64) -> SensedRecord {
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(object),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: Decimal::from_f64(open).expect("a price"),
        high: Decimal::from_f64(open.max(close) * 1.002).expect("a price"),
        low: Decimal::from_f64(open.min(close) * 0.998).expect("a price"),
        close: Decimal::from_f64(close).expect("a price"),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: Decimal::from_f64((open + close) / 2.0),
        quality: Default::default(),
    }))
}

/// A quiet series ending in `drop` on its last bar.
fn tape(object: &str, phase: f64, drop: f64) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    (0..120)
        .map(|i| {
            let noise = ((i as f64 * 0.7548776662 + phase) % 1.0 - 0.5) * 0.008;
            let last = if i == 119 { drop } else { 0.0 };
            let open = price;
            price *= 1.0 + noise + last;
            let at = start().saturating_sub(Duration::from_days(120 - i));
            bar(object, at, open, price)
        })
        .collect()
}

/// A print in every catalogue name, which is the only thing that gives a
/// candidate a reference price to be sized against.
fn print_every_name(platform: &mut Platform, universe: &Universe, at: Timestamp) {
    let prints: Vec<SensedRecord> = universe
        .iter()
        .map(|record| {
            SensedRecord::Trade(Trade {
                object_id: record.object_id.clone(),
                venue: record.venue.clone(),
                at,
                capture_time: at,
                sequence: None,
                price: dec!("100"),
                size: Decimal::from_int(100),
                aggressor: None,
                condition: TradeCondition::Regular,
                trade_id: None,
                quality: Default::default(),
            })
        })
        .collect();
    let count = prints.len();
    assert_eq!(
        platform.observe(prints),
        count,
        "every print must reach the snapshot"
    );
}

fn logged_plans(platform: &Platform) -> Vec<HedgePlanned> {
    platform
        .event_log()
        .by_topic(Topic::RiskEvaluated)
        .into_iter()
        .filter_map(|record| StreamEnvelope::from_frame(record).ok())
        .filter_map(|envelope| envelope.decode::<HedgePlanned>().ok())
        .map(|decoded| decoded.body)
        .collect()
}

#[test]
fn a_book_failing_a_library_world_is_planned_a_hedge_from_the_committed_catalogue() -> Result<()> {
    let universe = catalogue();
    let declared = paper_hedge_candidates(&universe);
    assert!(
        !declared.is_empty(),
        "premise: the committed catalogue declares at least one hedge candidate"
    );
    let mut platform = platform(catalogue())?;
    // Two names on one tape, so each loads fully on the equity factor the
    // library shocks; the final drop gives the cycle a thesis to construct,
    // and a construction is what withholds the hedge capacity the plan is
    // budgeted from.
    platform.observe(tape(EXPOSED, 0.0, -0.09));
    platform.observe(tape(SUBJECT, 0.0, -0.09));
    // 720,000 long on a 1,000,000 book: the 2008 world takes about 22% of
    // equity, past the 20% floor, and the shortfall is small enough to be
    // bought back inside the shipped 5% hedge capacity.
    let order = platform.order_from(
        ObjectId::from_string(EXPOSED),
        Side::Buy,
        dec!("7200"),
        dec!("100"),
        "prop-exposed",
        vec!["hyp-exposed".to_string()],
        start(),
    );
    platform.submit_order(order, start())?;
    print_every_name(&mut platform, &universe, start());

    let report = platform.run_cycle(start().saturating_add(Duration::from_days(1)));
    let decide = report.stage(Stage::Decide).expect("the cycle runs DECIDE");

    let plans = logged_plans(&platform);
    assert_eq!(plans.len(), 1, "one cycle journals one hedge-brain record");
    let record = &plans[0];
    assert!(
        record
            .protected
            .iter()
            .any(|world| world == "credit-crisis-2008"),
        "premise: the book fails the 2008 world: {:?}",
        record.protected
    );
    let plan = record
        .plan
        .as_ref()
        .unwrap_or_else(|| panic!("no hedge was planned: {}", decide.detail));
    assert!(!plan.instruments.is_empty());
    for leg in &plan.instruments {
        assert!(
            declared
                .iter()
                .any(|candidate| candidate.instrument.as_str() == leg.instrument),
            "the plan names {}, which no catalogue declaration offered",
            leg.instrument
        );
    }
    assert_eq!(
        plan.scenarios_protected, record.protected,
        "the plan protects every world that failed"
    );

    // R12-RISK-4109: the six measures the choice was made on reach the log.
    let measures = record
        .measures
        .as_ref()
        .expect("a chosen plan journals its measures");
    assert!(measures.cost.is_positive(), "cost: {}", measures.cost);
    assert!(
        measures.basis_risk.is_finite() && measures.basis_risk > 0.0,
        "basis risk: {}",
        measures.basis_risk
    );
    // Linear legs: measured at zero, not unmeasured.
    assert_eq!(measures.convexity, 0.0);
    assert!(
        measures.liquidity_decay_days.is_finite() && measures.liquidity_decay_days > 0.0,
        "liquidity decay: {}",
        measures.liquidity_decay_days
    );
    assert_eq!(measures.counterparty_risk, 0.0, "exchange-cleared legs");
    assert!(measures.unwind_feasible);
    Ok(())
}

/// The declaration is the catalogue's common stock and nothing else: a record
/// of another type has no stated equity loading, and a declared one would be
/// a guessed beta. Every declaration carries the record's own venue, spread
/// and daily volume rather than a restatement of them.
#[test]
fn every_common_stock_in_the_committed_catalogue_is_a_declared_hedge_candidate() {
    let universe = catalogue();
    let stocks: Vec<_> = universe
        .iter()
        .filter(|record| record.instrument_type == InstrumentType::CommonStock)
        .collect();
    assert!(
        !stocks.is_empty(),
        "premise: the catalogue lists common stock"
    );
    let declared = paper_hedge_candidates(&universe);
    assert_eq!(declared.len(), stocks.len());
    for record in stocks {
        let candidate = declared
            .iter()
            .find(|candidate| candidate.instrument == record.object_id)
            .unwrap_or_else(|| panic!("{} is not declared", record.symbol));
        assert_eq!(candidate.class, InstrumentClass::CorrelatedAsset);
        assert_eq!(candidate.venue, record.venue);
        assert_eq!(candidate.spread_bps, record.liquidity.typical_spread_bps);
        assert_eq!(
            candidate.average_daily_volume,
            record.liquidity.average_daily_volume
        );
        assert_eq!(candidate.factor_loadings.get("equity"), Some(&1.0));
        assert!(candidate.carry_bps_per_day > 0.0, "a short is never free");
    }
}
