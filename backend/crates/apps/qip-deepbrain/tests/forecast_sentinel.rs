//! The forecast sentinel (R12-AMBIENT-3066), reached from the deep brain's
//! research sweep: the kernel's forecast lattice holds an expected future
//! state per instrument at every central horizon, and a forecast that reality
//! resolves outside its own 80% interval is raised by the sweep as an
//! assumption signal and routed through the attention router — on by
//! default, and with the sentinel switched off, not raised at all.
//!
//! Each test asserts its premise first — that the lattice actually recorded
//! a miss — so a lattice that forecast nothing cannot pass for a quiet one.

use qip_contracts::ambient::{AmbientSignal, Pathway, SignalClass, Trigger};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_deepbrain::research::{
    FORECAST_SENTINEL_VARIABLE, ResearchConfig, ResearchLoop, forecast_miss_signal,
};
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::lattice_stage::ForecastMiss;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_world_model::lattice::LatticeHorizon;
use std::collections::{BTreeMap, BTreeSet};

const SUBJECT: &str = "obj-F";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("the platform assembles: {e}"))
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn bar(at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap_or_else(|| panic!("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(SUBJECT.to_string()),
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

/// A calm month, one cycle (the lattice issues its day-ahead forecast), then
/// a close 30% above it a day later and a second cycle, which resolves that
/// forecast far outside its interval. Returns the platform after the second
/// cycle and the instant it ran at.
fn platform_with_a_missed_forecast() -> (Platform, Timestamp) {
    let mut platform = platform();
    let calm: Vec<SensedRecord> = (0..30_i64)
        .map(|i| {
            let close = if i % 2 == 0 { 100.1 } else { 99.9 };
            bar(start().saturating_sub(Duration::from_days(30 - i)), close)
        })
        .collect();
    platform.observe(calm);
    platform.run_cycle(start());
    // A daily bar closes a day after it opens, so the bar opening at `start`
    // closes at `after`: the day-ahead forecast's due instant.
    let after = start().saturating_add(Duration::from_days(1));
    platform.observe(vec![bar(start(), 130.0)]);
    platform.run_cycle(after);
    (platform, after)
}

fn sentinel(enabled: bool) -> ResearchLoop {
    let config = ResearchConfig {
        forecast_sentinel: enabled,
        ..ResearchConfig::default()
    };
    ResearchLoop::new(config, BTreeMap::new())
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
}

fn is_forecast_miss(signal: &AmbientSignal) -> bool {
    matches!(signal.trigger(), Trigger::Event(name) if name == "forecast_miss")
}

/// R12-AMBIENT-3066: a day-ahead forecast the lattice held, resolved by a
/// close 30% away, reaches the sweep's attention router as an assumption
/// signal naming the instrument and the horizon, and the router activates it
/// on the reflex-model-update pathway. Read once: a second sweep with no new
/// cycle raises nothing again.
///
/// Mutation: drop the `misses_since` read from `ResearchLoop::sweep` (route
/// nothing from it) and no forecast-miss signal is routed — the second
/// assertion fails. Mutation: in `LatticeStage::run`, record a miss when
/// `record.covered` rather than when it is not — the premise fails, because
/// the 30% jump is the one resolution that is not covered.
#[test]
fn a_forecast_reality_resolves_outside_its_interval_reaches_the_attention_router() {
    let (platform, after) = platform_with_a_missed_forecast();
    let (misses, newest, lost) = platform.lattice().misses_since(0);
    assert!(
        misses
            .iter()
            .any(|m| m.subject.ends_with(SUBJECT) && m.horizon == LatticeHorizon::HoursDays),
        "premise: the lattice recorded the day-ahead miss: {misses:?}"
    );
    assert_eq!(lost, 0);
    assert_eq!(newest, misses.len() as u64);

    let mut loop_ = sentinel(true);
    let round = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(round.forecast_misses, misses.len());
    let raised: Vec<&AmbientSignal> = round
        .signals
        .iter()
        .filter(|s| is_forecast_miss(s))
        .collect();
    let day_ahead = raised
        .iter()
        .find(|s| s.subject().ends_with(":hours_days"))
        .unwrap_or_else(|| panic!("the day-ahead miss is routed: {raised:?}"));
    assert_eq!(day_ahead.class(), SignalClass::Assumption);
    assert_eq!(
        day_ahead.horizon(),
        LatticeHorizon::HoursDays.representative()
    );
    assert!(
        day_ahead.severity_bp() >= 1_000,
        "a 30% jump against a calm interval is material: {}",
        day_ahead.severity_bp()
    );
    let activated = round
        .attention_events
        .iter()
        .find(|e| e.signal_id == day_ahead.id())
        .unwrap_or_else(|| panic!("the router answered the miss: {:?}", round.attention_events));
    assert_eq!(activated.pathway, Pathway::ReflexModelUpdate);
    assert_eq!(
        activated.originating_models,
        vec!["forecast_miss".to_string()]
    );

    let again = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.forecast_misses, 0);
    assert!(!again.signals.iter().any(is_forecast_miss));
}

/// Disabling the sentinel removes its signals: the same platform, holding
/// the same miss, swept by a loop with the sentinel off, routes no
/// forecast-miss signal — while the rest of the sweep still runs, so the
/// quiet is the switch and not a sweep that failed.
///
/// Mutation: ignore `config.forecast_sentinel` in `ResearchLoop::sweep`
/// (raise regardless) and the forecast-miss assertion fails.
#[test]
fn with_the_sentinel_off_the_same_miss_raises_no_signal() {
    let (platform, after) = platform_with_a_missed_forecast();
    assert!(
        !platform.lattice().misses_since(0).0.is_empty(),
        "premise: there is a miss to raise"
    );
    let round = sentinel(false)
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        round.now_surprises >= 1 && !round.signals.is_empty(),
        "premise: the sweep ran and routed its other signals"
    );
    assert_eq!(round.forecast_misses, 0);
    assert!(
        !round.signals.iter().any(is_forecast_miss),
        "{:?}",
        round.signals
    );
}

/// The composition root's switch: unset is on (the production default),
/// `off` is off, and anything else stops start-up rather than guessing.
///
/// Mutation: default `forecast_sentinel` to `false` in `ResearchConfig` and
/// the first assertion fails.
#[test]
fn the_sentinel_is_on_by_default_off_only_when_said_and_a_typo_is_refused() {
    let unset = ResearchConfig::from_lookup(&|_| None).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        unset.forecast_sentinel,
        "on in production unless switched off"
    );
    let off = ResearchConfig::from_lookup(&|name| {
        (name == FORECAST_SENTINEL_VARIABLE).then(|| "off".to_string())
    })
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(!off.forecast_sentinel);
    let typo = ResearchConfig::from_lookup(&|name| {
        (name == FORECAST_SENTINEL_VARIABLE).then(|| "of".to_string())
    });
    assert!(
        typo.is_err(),
        "a value that is neither on nor off is refused"
    );
}

fn miss(quantiles: [f64; 3], outcome: f64) -> ForecastMiss {
    ForecastMiss {
        forecast_id: "fc-1".to_string(),
        subject: SUBJECT.to_string(),
        horizon: LatticeHorizon::WeeksMonths,
        issued_at: start(),
        observed_at: start().saturating_add(Duration::from_days(30)),
        quantiles,
        outcome,
    }
}

/// The size of a miss is how far outside the interval reality landed, in
/// widths of that interval: a tenth of a width is the router's materiality
/// line, a whole width or more is the top of the scale, the side does not
/// matter, and a degenerate interval or an outcome inside it is refused.
///
/// Mutation: size by the distance from the median instead of from the bound
/// and the first assertion reads 6000, not 1000.
#[test]
fn a_miss_is_sized_in_interval_widths_past_the_bound_and_a_degenerate_one_is_refused() {
    let now = start();
    let ttl = Duration::from_hours(1);
    let mut issued = 0;
    let size = |m: ForecastMiss, issued: &mut u64| {
        forecast_miss_signal(&m, now, ttl, issued).map(|s| s.severity_bp())
    };
    let interval = [90.0, 100.0, 110.0];
    assert_eq!(size(miss(interval, 112.0), &mut issued).ok(), Some(1_000));
    assert_eq!(size(miss(interval, 88.0), &mut issued).ok(), Some(1_000));
    assert_eq!(size(miss(interval, 200.0), &mut issued).ok(), Some(10_000));
    assert!(size(miss(interval, 105.0), &mut issued).is_err(), "inside");
    assert!(size(miss([100.0, 100.0, 100.0], 120.0), &mut issued).is_err());
    assert!(size(miss(interval, f64::NAN), &mut issued).is_err());
    assert_eq!(issued, 3, "only a signal that was raised takes an id");
}
