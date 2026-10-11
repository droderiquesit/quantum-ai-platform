//! The NOW coordinator's three §11.2 contracts, produced by the cycle a
//! shipped binary runs (R12-WORLD-9016).
//!
//! `GlobalState` and `RegionalState` had constructors on the NOW Brain and no
//! test that read them off a platform that had run a cycle, so a brain whose
//! state was never promoted, or whose filters returned every variable, read
//! the same as a working one. The fixtures are `now_lattice.rs`'s, copied
//! rather than shared so this file does not move with that suite's edits.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Decimal, dec};
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::now_stage::NowReview;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::now::ScopeKind;

const SYMBOLS: [&str; 2] = ["AAA", "BBB"];

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
    for symbol in SYMBOLS {
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
        LimitSet::new("now-lattice-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        ),
    )
    .unwrap()
}

/// A daily bar for `symbol` closing at `closes_at`.
fn bar(symbol: &str, closes_at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap();
    SensedRecord::Bar(Box::new(Bar {
        object_id: object(symbol),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: closes_at.saturating_sub(Duration::from_days(1)),
        open: price(close),
        high: price(close * 1.002),
        low: price(close * 0.998),
        close: price(close),
        volume: dec!("1000000"),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

/// Thirty calm days for each symbol, the last closing at `start()`.
fn calm_history() -> Vec<SensedRecord> {
    let mut out = Vec::new();
    for symbol in SYMBOLS {
        for i in 0..30_i64 {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            out.push(bar(symbol, day(i - 29), close));
        }
    }
    out
}

fn reviews<T: EventBody>(platform: &Platform, prefix: &str) -> Vec<T> {
    platform
        .event_log()
        .by_topic(Topic::WorldModelUpdated)
        .into_iter()
        .filter(|e| {
            e.idempotency_key
                .as_deref()
                .is_some_and(|k| k.starts_with(prefix))
        })
        // `Platform::journal_record` appends a sealed `StreamEnvelope`'s
        // frame, so the log record's payload is the envelope wire and the
        // review is inside it. Decoding the frame as the review directly
        // read the wire's fields as the review's and failed on `cycle`,
        // which is how all seven tests here failed at their first read.
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<T>())
                .unwrap()
                .body
        })
        .collect()
}

fn last<T: EventBody>(platform: &Platform, prefix: &str) -> T {
    reviews::<T>(platform, prefix)
        .pop()
        .unwrap_or_else(|| panic!("no `{prefix}` review in the log"))
}

/// Fed a calm history then a 30% break in one instrument, the coordinator
/// produces a `GlobalState` holding only global-scope variables, a
/// `RegionalState` for the instruments' region holding only that region's,
/// and a `SurpriseEvent` for the broken instrument — whose residual is past
/// the configured bound — and none for the quiet one.
///
/// Mutations: flip the `ScopeKind::Global` filter in `NowBrain::global_state`
/// (the scope assertion fires); flip the region filter in `regional_state`
/// (the regional premise fires); raise the surprise test in the filter to
/// `>= surprise_sigmas * 1e6` (no surprise is found).
#[test]
fn the_cycle_produces_global_and_regional_state_and_a_surprise_only_where_a_residual_breaks_its_bound()
 {
    let mut platform = platform();
    platform.observe(calm_history());
    platform.run_cycle(start());
    platform.observe(vec![bar("AAA", day(1), 130.0), bar("BBB", day(1), 100.1)]);
    platform.run_cycle(day(1));
    let brain = platform.now_brain().brain();

    let global = brain.global_state(day(1));
    assert!(!global.variables.is_empty(), "no global state was promoted");
    assert!(
        global
            .variables
            .iter()
            .all(|v| v.key.scope.kind == ScopeKind::Global),
        "GlobalState carried a non-global variable: {:?}",
        global.variables.iter().map(|v| &v.key).collect::<Vec<_>>()
    );

    let regional = brain.regional_state("US", day(1));
    assert!(
        regional
            .variables
            .iter()
            .any(|v| v.key.scope.kind == ScopeKind::Instrument && v.key.scope.id == "obj-AAA"),
        "the US instrument is missing from the US RegionalState"
    );
    assert!(
        regional
            .variables
            .iter()
            .all(|v| v.partition.region == "US"),
        "RegionalState carried another region's variable"
    );
    assert!(
        brain.regional_state("nowhere", day(1)).variables.is_empty(),
        "a region nothing is observed in has state"
    );

    let review: NowReview = last(&platform, "now:");
    let instrument = |id: &str| {
        review
            .surprises
            .iter()
            .filter(|s| s.key.scope.kind == ScopeKind::Instrument && s.key.scope.id == id)
            .count()
    };
    assert!(
        instrument("obj-AAA") > 0,
        "the 30% break raised no surprise"
    );
    assert_eq!(
        instrument("obj-BBB"),
        0,
        "the quiet instrument was a surprise"
    );
    for surprise in &review.surprises {
        assert!(
            surprise.sigmas.abs() >= qip_world_model::now::DEFAULT_SURPRISE_SIGMAS,
            "a surprise inside its bound: {surprise:?}"
        );
    }
}
