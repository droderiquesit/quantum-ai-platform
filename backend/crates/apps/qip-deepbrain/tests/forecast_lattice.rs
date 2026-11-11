//! The forecast lattice, reached from the deep brain's run loop
//! (R12-AMBIENT-1139): with nobody asking, every cycle the node runs journals
//! a `kernel/forecast-lattice` review, and each review re-issues a forecast
//! at every central horizon whose last forecast went stale.
//!
//! Every lattice test in `qip-kernel/tests/now_lattice.rs` calls `run_cycle`
//! directly, so until this suite nothing showed the schedule doing the
//! asking. This one drives `node::run_with` and makes no request itself.

use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, Context, Decimal, ManualClock, ObjectId, dec};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::status::NodeStatus;
use qip_events::{EventBody, Topic};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::{DataQuality, Provenance};
use qip_financial::universe::Universe;
use qip_kernel::lattice_stage::LatticeReview;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use qip_streaming::envelope::StreamEnvelope;
use qip_world_model::lattice::LatticeHorizon;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const SYMBOLS: [&str; 2] = ["AAA", "BBB"];
/// How far the clock moves between cycles: long enough that the short
/// horizons' forecasts expire, so a review that re-issued nothing would be
/// visible.
const STEP: Duration = Duration::from_days(2);
const CYCLES: u64 = 3;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn day(n: i64) -> Timestamp {
    start().saturating_add(Duration::from_days(n))
}

fn bar(symbol: &str, at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap_or_else(|| panic!("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(format!("obj-{symbol}")),
        venue: "XNYS".to_string(),
        interval: Interval::Day,
        open_time: at,
        open: price(close),
        high: price(close * 1.001),
        low: price(close * 0.999),
        close: price(close),
        volume: Decimal::from_int(1_000_000),
        trade_count: 5_000,
        vwap: None,
        quality: DataQuality::default(),
    }))
}

fn platform(clock: Arc<dyn Clock>) -> Platform {
    let mut universe = Universe::new();
    for symbol in SYMBOLS {
        universe
            .insert(
                FinancialObject::builder(
                    ObjectId::from_string(format!("obj-{symbol}")),
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
                .unwrap_or_else(|e| panic!("{e}")),
            )
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let config = PlatformConfig::default();
    let context = Context::new(clock, config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        universe,
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"));
    let mut history = Vec::new();
    for symbol in SYMBOLS {
        for i in 0..30_i64 {
            let close = if i % 2 == 0 { 100.2 } else { 99.8 };
            history.push(bar(symbol, day(i - 29), close));
        }
    }
    platform.observe(history);
    platform
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
        // frame; the review is inside it.
        .map(|e| {
            StreamEnvelope::from_frame(e)
                .and_then(|envelope| envelope.decode::<T>())
                .unwrap_or_else(|e| panic!("a journaled review decodes: {e}"))
                .body
        })
        .collect()
}

/// R12-AMBIENT-1139: the node's own loop, with no request from anyone,
/// journals one forecast-lattice review per cycle, and every forecast a
/// review left to go stale by the next cycle is re-issued in that cycle's
/// review, at the same target and horizon. Mutation: replace the
/// `journal_record(lattice, "kernel/forecast-lattice", now)` call in
/// `Platform` with `Ok(())`, and no review is journaled.
#[test]
fn the_run_loop_journals_a_lattice_review_every_cycle_and_reissues_each_stale_horizon_unasked() {
    let manual = Arc::new(ManualClock::new(start()));
    let clock: Arc<dyn Clock> = manual.clone();
    let mut platform = platform(clock.clone());
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(CYCLES),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let summary = node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops::default(),
        // Time passes between cycles; this is the only thing the test does
        // while the loop runs, and it asks the loop for nothing.
        |_| manual.advance(STEP),
    )
    .unwrap_or_else(|e| panic!("the node runs: {e}"));
    assert_eq!(
        summary.cycles, CYCLES,
        "premise: the node ran {CYCLES} cycles"
    );

    let journaled: Vec<LatticeReview> = reviews(&platform, "lattice:");
    assert_eq!(
        journaled.len() as u64,
        CYCLES,
        "one forecast-lattice review per cycle the node ran"
    );
    let cycles: BTreeSet<u64> = journaled.iter().map(|r| r.cycle).collect();
    assert_eq!(
        cycles.len() as u64,
        CYCLES,
        "each review is its own cycle's"
    );
    assert!(
        !journaled[0].issued.is_empty(),
        "premise: the first review issued forecasts"
    );

    let mut stale_reissued = 0usize;
    for pair in journaled.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        for forecast in &before.issued {
            if forecast.expires_at > after.at {
                continue;
            }
            let horizon = LatticeHorizon::of(forecast.horizon);
            assert!(
                after.issued.iter().any(
                    |f| f.target == forecast.target && LatticeHorizon::of(f.horizon) == horizon
                ),
                "{} at {} went stale before cycle {} and was not re-issued",
                forecast.target,
                horizon.as_str(),
                after.cycle
            );
            stale_reissued += 1;
        }
    }
    assert!(
        stale_reissued > 0,
        "premise: some forecast went stale between cycles, so the re-issue check is not vacuous"
    );
}
