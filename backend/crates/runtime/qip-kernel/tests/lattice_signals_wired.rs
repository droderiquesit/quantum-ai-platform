//! The forecast lattice's row signals as the cycle sets them (R12-WORLD-3085,
//! R12-WORLD-3088, ADR 0632). Every test drives `Platform::run_cycle` and
//! reads the lattice review the cycle journaled.
//!
//! The failure this prevents: `LatticeStage::set_signals` had no caller, so
//! on every deployed path the flow member stated the level and the
//! structural member read no claims, and each member's own unit test passed
//! regardless.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_contracts::forecast::OutcomeDistribution;
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_events::Topic;
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::lattice_stage::LatticeReview;
use qip_kernel::platform::Platform;
use qip_market::Side;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::{Trade, TradeCondition};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::lattice::LatticeHorizon;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn day(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_days(n))
}

fn object(symbol: &str) -> qip_core::ObjectId {
    qip_core::ObjectId::from_string(format!("obj-{symbol}"))
}

fn platform() -> Platform {
    let mut universe = Universe::new();
    for symbol in ["AAA", "BBB"] {
        universe
            .insert(
                FinancialObject::builder(
                    object(symbol),
                    symbol,
                    InstrumentType::CommonStock,
                    qip_financial::costs::LiquidityProfile::listed(
                        Decimal::from_int(5_000_000),
                        3.0,
                    ),
                )
                .venue("XNYS")
                .geography("US")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .unwrap(),
            )
            .unwrap();
    }
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::new("lattice-signals").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
    .unwrap()
}

fn calm_history() -> Vec<SensedRecord> {
    let mut out = Vec::new();
    for symbol in ["AAA", "BBB"] {
        for i in 0..30_i64 {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            let at = day(i - 29);
            let price = |v: f64| Decimal::from_f64(v).unwrap();
            out.push(SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at.saturating_sub(Duration::from_days(1)),
                open: price(close),
                high: price(close * 1.002),
                low: price(close * 0.998),
                close: price(close),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: None,
                quality: DataQuality::default(),
            })));
        }
    }
    out
}

fn print(symbol: &str, n: i64, size: i64, side: Side) -> SensedRecord {
    let at = start().saturating_sub(Duration::from_mins(60 - n));
    SensedRecord::Trade(Trade {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        at,
        capture_time: at,
        sequence: Some(n as u64),
        price: dec!("100"),
        size: Decimal::from_int(size),
        aggressor: Some(side),
        condition: TradeCondition::Regular,
        trade_id: Some(format!("{symbol}-{n}")),
        quality: DataQuality::default(),
    })
}

/// The median the first pass issued for `subject` at `horizon`.
fn median(platform: &Platform, subject: &str, horizon: LatticeHorizon) -> f64 {
    let review: LatticeReview = platform
        .event_log()
        .by_topic(Topic::WorldModelUpdated)
        .into_iter()
        .filter(|e| {
            e.idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with("lattice:"))
        })
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<LatticeReview>())
                .unwrap()
                .body
        })
        .next_back()
        .expect("a lattice review in the log");
    let forecast = review
        .issued
        .iter()
        .find(|f| {
            f.target == format!("obj-{subject}/close") && LatticeHorizon::of(f.horizon) == horizon
        })
        .unwrap_or_else(|| panic!("no {horizon:?} forecast for {subject}"));
    let OutcomeDistribution::Quantiles { points } = &forecast.distribution else {
        panic!("premise: a quantile distribution");
    };
    points
        .iter()
        .find(|p| p.quantile_bp == 5_000)
        .expect("a median point")
        .value
        .to_f64()
}

fn run_with(trades: Vec<SensedRecord>) -> Platform {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.observe(trades);
    platform.run_cycle(start());
    platform
}

/// WORLD-3085: the paper feed's signed aggressor flow reaches the
/// seconds-minutes flow member through the cycle. Buy-dominated flow lifts
/// AAA's median above the no-flow control and sell-dominated flow drops it
/// below; BBB, which printed nothing, is identical in all three runs.
/// Mutation: delete the `self.lattice.set_signals(signals)` call in
/// `understand_now`, or stop storing `lattice_flow_imbalance` in
/// `understand_federation`: all three AAA medians are equal.
#[test]
fn the_paper_feeds_signed_flow_moves_the_seconds_row_of_the_instrument_that_printed_it() {
    let none = run_with(Vec::new());
    let buying = run_with(vec![
        print("AAA", 1, 300, Side::Buy),
        print("AAA", 2, 100, Side::Sell),
    ]);
    let selling = run_with(vec![
        print("AAA", 1, 100, Side::Buy),
        print("AAA", 2, 300, Side::Sell),
    ]);
    let row = LatticeHorizon::SecondsMinutes;
    let (n, b, s) = (
        median(&none, "AAA", row),
        median(&buying, "AAA", row),
        median(&selling, "AAA", row),
    );
    assert!(
        b > n,
        "premise and property: buy flow lifts the row: {b} vs {n}"
    );
    assert!(s < n, "sell flow lowers the row: {s} vs {n}");
    assert!(
        (median(&buying, "BBB", row) - median(&none, "BBB", row)).abs() < 1e-12,
        "an instrument that printed nothing is read as having no flow"
    );
}

/// The flow is one cycle's, not an accumulation: a second cycle with no new
/// prints states the level again. Mutation: stop replacing
/// `lattice_flow_imbalance` in `understand_federation` (keep the old map).
#[test]
fn flow_seen_in_one_cycle_is_not_read_again_in_the_next() {
    let mut platform = run_with(vec![print("AAA", 1, 300, Side::Buy)]);
    let fed = median(&platform, "AAA", LatticeHorizon::SecondsMinutes);
    platform.run_cycle(start().saturating_add(Duration::from_days(2)));
    let quiet = median(&platform, "AAA", LatticeHorizon::SecondsMinutes);
    let control = run_with(Vec::new());
    let level = median(&control, "AAA", LatticeHorizon::SecondsMinutes);
    assert!(fed > level, "premise: the first cycle read the flow");
    assert!(
        (quiet - level).abs() < (fed - level).abs() / 2.0,
        "the quiet cycle still leans with the old flow: {quiet} vs level {level}, fed {fed}"
    );
}

/// Log-return bars for `symbol` following `returns`, one per day, ending at
/// `start()`.
fn bars_from_returns(symbol: &str, returns: &[f64]) -> Vec<SensedRecord> {
    let mut price = 100.0_f64;
    let count = returns.len();
    returns
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let open = price;
            price *= r.exp();
            let at = start().saturating_sub(Duration::from_days((count - 1 - i) as i64));
            let money = |v: f64| Decimal::from_f64(v).unwrap();
            SensedRecord::Bar(Box::new(Bar {
                object_id: object(symbol),
                venue: "XNYS".to_string(),
                interval: Interval::Day,
                open_time: at.saturating_sub(Duration::from_days(1)),
                open: money(open),
                high: money(open.max(price) * 1.002),
                low: money(open.min(price) * 0.998),
                close: money(price),
                volume: dec!("1000000"),
                trade_count: 5_000,
                vwap: None,
                quality: DataQuality::default(),
            }))
        })
        .collect()
}

fn noise(seed: u64, count: usize, scale: f64) -> Vec<f64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 1_000_000) as f64 / 1_000_000.0 - 0.5) * 2.0 * scale
        })
        .collect()
}

fn last_review(platform: &Platform) -> LatticeReview {
    platform
        .event_log()
        .by_topic(Topic::WorldModelUpdated)
        .into_iter()
        .filter(|e| {
            e.idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with("lattice:"))
        })
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<LatticeReview>())
                .unwrap()
                .body
        })
        .next_back()
        .expect("a lattice review in the log")
}

/// WORLD-3088: a causal claim the cycle itself discovered (a real lagged
/// pair) is read by the pass that issues the effect's quarters row, and the
/// cause's own row, which no claim bears on, reads none. The claim count is
/// on the journaled review, so the log alone says what the row was handed.
/// Mutation: clear `structural_claims` before `set_signals` in
/// `understand_now`; the review then reads no claim for obj-BBB.
#[test]
fn a_live_causal_claim_the_cycle_found_is_handed_to_the_effects_row() {
    let count = 120;
    let cause = noise(11, count, 0.02);
    let mut effect = vec![0.0; count];
    let jitter = noise(22, count, 0.004);
    for t in 1..count {
        effect[t] = 0.8 * cause[t - 1] + jitter[t];
    }
    let mut platform = platform();
    platform.observe(bars_from_returns("AAA", &cause));
    platform.observe(bars_from_returns("BBB", &effect));
    platform.run_cycle(start());
    assert!(
        platform
            .world()
            .causal()
            .edges()
            .iter()
            .any(|e| e.cause == "obj-AAA" && e.effect == "obj-BBB"),
        "premise: the cycle found AAA -> BBB"
    );
    let read = last_review(&platform).signals_read;
    assert!(
        read.structural_claims.get("obj-BBB").copied().unwrap_or(0) >= 1,
        "the effect's row was handed no claim: {read:?}"
    );
    assert!(
        !read.structural_claims.contains_key("obj-AAA"),
        "a claim bearing on the cause was invented: {read:?}"
    );
}

/// The flow shows on the journaled review too, for exactly the subject that
/// printed it. Mutation: key the flow by venue instead of subject.
#[test]
fn the_review_records_the_flow_only_for_the_subject_that_printed_it() {
    let platform = run_with(vec![
        print("AAA", 1, 300, Side::Buy),
        print("AAA", 2, 100, Side::Sell),
    ]);
    let read = last_review(&platform).signals_read;
    assert_eq!(read.flow_imbalance.len(), 1, "{read:?}");
    let ratio = read.flow_imbalance["obj-AAA"];
    assert!(
        (ratio - 0.5).abs() < 1e-12,
        "(300-100)/(300+100) = 0.5, got {ratio}"
    );
}
