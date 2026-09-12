//! §22.1's fallback series as the platform fills it: the SENSE stage's own
//! `observe` is the seam, daily bars are what it keeps, and everything finer
//! is the transient class the section says is not kept.

// See the note in `absorption.rs`: in a test the assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_data_finder::retention::{FALLBACK_BARS_PER_INSTRUMENT, FALLBACK_RETENTION};
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::platform::Platform;
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Result<Platform> {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

fn bar(subject: &str, interval: Interval, index: i64) -> SensedRecord {
    let close = 100 + index;
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(subject),
        venue: "XSIM".to_string(),
        interval,
        open_time: start().saturating_add(Duration::from_millis(
            interval.duration().as_millis() * index,
        )),
        open: Decimal::from_int(close),
        high: Decimal::from_int(close + 1),
        low: Decimal::from_int(close - 1),
        close: Decimal::from_int(close),
        volume: Decimal::from_int(1_000),
        vwap: None,
        trade_count: 10,
        quality: DataQuality::default(),
    }))
}

/// Daily bars the platform observes are retained in the fallback series;
/// minute bars for the same subject are absorbed into the working series
/// and retained in the fallback nowhere, because §22.1 files them as
/// transient.
///
/// Mutated by dropping the `bar.interval == Interval::Day` guard in
/// `Platform::observe` — confirmed the minute-bar half then fails because
/// `FallbackSeries::retain` refuses the interval and the refusal lands in
/// the capture problems, then restored.
#[test]
fn daily_bars_the_platform_observes_are_retained_and_minute_bars_are_not() -> Result<()> {
    let mut platform = platform()?;
    assert_eq!(
        platform.fallback_series().instruments(),
        0,
        "premise: nothing is retained before anything is observed"
    );
    assert_eq!(platform.fallback_series().retention(), FALLBACK_RETENTION);
    assert_eq!(
        platform.fallback_series().per_instrument(),
        FALLBACK_BARS_PER_INSTRUMENT
    );

    let absorbed = platform.observe(
        (0..5)
            .map(|day| bar("obj-AAA", Interval::Day, day))
            .collect(),
    );
    assert_eq!(absorbed, 5, "premise: every daily bar was absorbed");
    assert_eq!(
        platform.fallback_bars("obj-AAA").len(),
        5,
        "daily bars must be retained as insurance"
    );
    assert_eq!(
        platform.fallback_bars("obj-AAA")[0].open_time,
        start(),
        "the series is oldest first"
    );

    let absorbed = platform.observe(
        (0..5)
            .map(|minute| bar("obj-BBB", Interval::Minute, minute))
            .collect(),
    );
    assert_eq!(absorbed, 5, "premise: every minute bar was absorbed");
    assert!(
        platform.fallback_bars("obj-BBB").is_empty(),
        "minute bars are the transient class and must not reach the fallback series"
    );
    assert_eq!(platform.fallback_series().instruments(), 1);
    assert_eq!(platform.fallback_series().evicted(), 0);

    // And they were passed over, not offered and refused: the series refuses
    // a minute bar by itself, so the only thing the kernel's own guard
    // changes is whether every minute bar of every cycle becomes a capture
    // problem in the report. It must not.
    let report = platform.run_cycle(start().saturating_add(Duration::from_days(6)));
    let fallback_problems: Vec<&str> = report
        .problems()
        .into_iter()
        .filter(|(_, problem)| problem.contains("fallback series"))
        .map(|(_, problem)| problem)
        .collect();
    assert!(
        fallback_problems.is_empty(),
        "minute bars were offered to the fallback series and refused: {fallback_problems:?}"
    );
    Ok(())
}
