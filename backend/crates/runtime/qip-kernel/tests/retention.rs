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

/// The 513th instrument's daily bars are refused by the fallback series —
/// the bound is on instruments, and a new one is not admitted by evicting
/// another's insurance — and the refusal reaches the cycle report *once*,
/// as the fact that this instrument is uninsured, not once per bar per
/// cycle for the life of the process.
///
/// Mutated by dropping the `self.fallback_refused.insert(key.clone())`
/// condition in `Platform::observe` — confirmed the second cycle then
/// carries the refusal again and this fails, then restored.
#[test]
fn an_instrument_past_the_fallback_bound_is_reported_once_and_not_per_bar_per_cycle() -> Result<()>
{
    let mut platform = platform()?;
    let bound = qip_data_finder::retention::FALLBACK_INSTRUMENTS;
    // Fill the series to its bound, one daily bar each.
    let absorbed = platform.observe(
        (0..bound)
            .map(|index| bar(&format!("obj-{index:04}"), Interval::Day, 0))
            .collect(),
    );
    assert_eq!(
        absorbed, bound,
        "premise: the series was filled to its bound"
    );
    assert_eq!(platform.fallback_series().instruments(), bound);

    // One more instrument, two bars of it, then two cycles.
    let absorbed = platform.observe(vec![
        bar("obj-uninsured", Interval::Day, 0),
        bar("obj-uninsured", Interval::Day, 1),
    ]);
    assert_eq!(
        absorbed, 2,
        "premise: the bars were absorbed into the working series"
    );
    assert!(
        platform.fallback_bars("obj-uninsured").is_empty(),
        "premise: the instrument past the bound is not insured"
    );
    let fallback_problems = |report: &qip_kernel::CycleReport| -> Vec<String> {
        report
            .problems()
            .into_iter()
            .filter(|(_, problem)| problem.contains("fallback series"))
            .map(|(_, problem)| problem.to_string())
            .collect()
    };
    let first = platform.run_cycle(start().saturating_add(Duration::from_days(2)));
    let problems = fallback_problems(&first);
    assert_eq!(
        problems.len(),
        1,
        "two bars of one uninsured instrument are one fact, not two: {problems:?}"
    );
    assert!(
        problems[0].contains("obj-uninsured"),
        "the problem does not name the instrument: {}",
        problems[0]
    );

    // The next cycle brings another bar of it; the fact is already on the
    // record and is not repeated.
    platform.observe(vec![bar("obj-uninsured", Interval::Day, 2)]);
    let second = platform.run_cycle(start().saturating_add(Duration::from_days(3)));
    assert!(
        fallback_problems(&second).is_empty(),
        "the refusal was reported again for the same instrument: {:?}",
        fallback_problems(&second)
    );
    Ok(())
}
