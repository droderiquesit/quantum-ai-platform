//! The curiosity sentinel (R12-AMBIENT-3073), reached from the deep brain's
//! research sweep: the kernel measures what the platform does not know — a
//! component whose record is too thin to estimate, a model no better than a
//! coin, an unscored capacity, an unsettled regime boundary — and the sweep
//! raises the most uncertain of them as research-task signals through the
//! attention router. On by default, and with the sentinel off, not raised.
//!
//! The exploration desk ranks probes from the same measure, but only under a
//! capital budget, and the budget is zero unless a mandate sets a share
//! aside. Every test here runs on `PlatformConfig::default` and asserts that
//! the desk holds nothing, so what is proven is the path a default
//! deployment actually runs.
//!
//! Each test asserts its premise first — that the kernel measured an unknown
//! — so a platform that knew everything cannot pass for a quiet sentinel.

use qip_contracts::ambient::{AmbientSignal, Pathway, SignalClass, Trigger};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_deepbrain::research::{
    ResearchConfig, ResearchLoop, UNKNOWN_TRIGGER, rank_unknowns, unknown_signal,
};
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::exploration::{MAXIMUM_OPEN_UNKNOWNS, ProbeKind};
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
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
/// a close a day later and a second cycle, which resolves that forecast and
/// so gives the self model a forecasting component with a record far too
/// thin to estimate from: a measured unknown, produced by the cycle on
/// `PlatformConfig::default` and by nothing a test wrote into the model.
/// Returns the platform after the second cycle and the instant it ran at.
fn platform_with_resolved_forecasts() -> (Platform, Timestamp) {
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
        curiosity_sentinel: enabled,
        ..ResearchConfig::default()
    };
    ResearchLoop::new(config, BTreeMap::new())
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
}

fn is_unknown(signal: &AmbientSignal) -> bool {
    matches!(signal.trigger(), Trigger::Event(name) if name == UNKNOWN_TRIGGER)
}

/// The unknowns the kernel measures, most uncertain first with ties by
/// subject — the order the sentinel owes — restricted to those above zero.
fn measured(platform: &Platform, now: Timestamp) -> Vec<(String, ProbeKind, f64)> {
    let mut unknowns: Vec<(String, ProbeKind, f64)> = platform
        .unknowns(now)
        .into_iter()
        .filter(|(_, (_, uncertainty))| *uncertainty > 0.0)
        .map(|(subject, (kind, uncertainty))| (subject, kind, uncertainty))
        .collect();
    unknowns.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    unknowns
}

/// R12-AMBIENT-3073: on a default platform, whose exploration desk holds no
/// budget and so opens no probe, an unknown the kernel measured still
/// reaches the sweep's attention router as a research-task signal naming the
/// subject and sized at its uncertainty; the router answers it on the
/// research-task pathway. At most a probe book's worth are raised, the most
/// uncertain first, and a subject whose signal is live is not raised again.
///
/// Mutation: route nothing from `Platform::unknowns` in
/// `ResearchLoop::sweep_over` and no curiosity signal is routed — the
/// raised-subjects assertion fails. Mutation: drop the live check from
/// `rank_unknowns` and the second sweep raises the same subjects again.
/// Mutation: make `Platform::unknowns` return an empty map and the premise
/// fails.
#[test]
fn an_unknown_the_kernel_measures_reaches_the_attention_router_without_an_exploration_budget() {
    let (platform, after) = platform_with_resolved_forecasts();
    assert_eq!(
        platform.exploration().held(),
        Decimal::ZERO,
        "premise: the default desk holds no exploration budget"
    );
    let unknowns = measured(&platform, after);
    assert!(
        !unknowns.is_empty(),
        "premise: the cycle left the kernel something it measurably does not know"
    );
    let expected: Vec<&(String, ProbeKind, f64)> =
        unknowns.iter().take(MAXIMUM_OPEN_UNKNOWNS).collect();

    let mut loop_ = sentinel(true);
    let round = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let raised: Vec<&AmbientSignal> = round.signals.iter().filter(|s| is_unknown(s)).collect();
    let subjects: Vec<String> = raised.iter().map(|s| s.subject().to_string()).collect();
    let owed: Vec<String> = expected
        .iter()
        .map(|(subject, kind, _)| format!("unknown:{}:{subject}", kind.as_str()))
        .collect();
    assert_eq!(
        subjects, owed,
        "the most uncertain unknowns, in order, and no more than the desk has slots"
    );
    assert!(
        round.unknowns_refused.is_empty(),
        "{:?}",
        round.unknowns_refused
    );
    for (signal, (subject, _, uncertainty)) in raised.iter().zip(&expected) {
        assert_eq!(signal.class(), SignalClass::Opportunity);
        // Statistic to integer, as the sentinel sizes it.
        assert_eq!(
            signal.severity_bp(),
            (uncertainty * 10_000.0).round() as u32
        );
        assert_eq!(round.unknowns_raised.get(subject), Some(uncertainty));
    }
    let first = raised
        .first()
        .unwrap_or_else(|| panic!("at least one unknown is routed"));
    let answered = round
        .attention_events
        .iter()
        .find(|e| e.signal_id == first.id())
        .unwrap_or_else(|| panic!("the router answered: {:?}", round.attention_events));
    assert_eq!(answered.pathway, Pathway::ResearchTask);
    assert_eq!(
        answered.originating_models,
        vec![UNKNOWN_TRIGGER.to_string()]
    );

    let again = loop_
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let repeated: Vec<&str> = again
        .signals
        .iter()
        .filter(|s| is_unknown(s))
        .map(|s| s.subject())
        .filter(|s| subjects.iter().any(|raised| raised == s))
        .collect();
    assert!(
        repeated.is_empty(),
        "a live unknown is not raised a second time: {repeated:?}"
    );
}

/// Disabling the sentinel removes its signals: the same platform, holding
/// the same unknowns, swept by a loop with the sentinel off, routes no
/// curiosity signal — while the rest of the sweep still runs, so the quiet
/// is the switch and not a sweep that failed.
///
/// Mutation: ignore `config.curiosity_sentinel` in `sweep_over` (raise
/// regardless) and the last assertion fails.
#[test]
fn with_the_sentinel_off_the_same_unknowns_raise_no_signal() {
    let (platform, after) = platform_with_resolved_forecasts();
    assert!(
        !measured(&platform, after).is_empty(),
        "premise: there is an unknown to raise"
    );
    let round = sentinel(false)
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !round.signals.is_empty(),
        "premise: the sweep ran and routed its other signals"
    );
    assert!(round.unknowns_raised.is_empty());
    assert!(!round.signals.iter().any(is_unknown), "{:?}", round.signals);
}

/// The production default is on: the deep brain's composition root builds
/// its research loop from `ResearchConfig::from_lookup`, and with nothing
/// set the sentinel runs.
///
/// Mutation: default `curiosity_sentinel` to `false` in `ResearchConfig`
/// and this fails.
#[test]
fn the_sentinel_is_on_when_the_composition_root_sets_nothing() {
    let unset = ResearchConfig::from_lookup(&|_| None).unwrap_or_else(|e| panic!("{e}"));
    assert!(unset.curiosity_sentinel, "on in production");
}

/// An unknown is sized at its uncertainty and carries its probe kind in the
/// subject; an uncertainty outside `[0, 1]` is refused, not clamped, and a
/// refused one takes no id.
///
/// Mutation: clamp the uncertainty into `[0, 1]` instead of refusing it and
/// the out-of-range assertions fail.
#[test]
fn an_unknown_is_sized_at_its_uncertainty_and_one_outside_the_unit_interval_is_refused() {
    let now = start();
    let ttl = Duration::from_hours(1);
    let mut issued = 0;
    let signal = unknown_signal(
        "model:x",
        ProbeKind::StaleEstimate,
        0.75,
        now,
        ttl,
        &mut issued,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(signal.severity_bp(), 7_500);
    assert_eq!(signal.subject(), "unknown:stale_estimate:model:x");
    assert_eq!(signal.expiry(), now.saturating_add(ttl));
    assert!(
        unknown_signal(
            "model:x",
            ProbeKind::StaleEstimate,
            1.5,
            now,
            ttl,
            &mut issued
        )
        .is_err()
    );
    assert!(
        unknown_signal(
            "model:x",
            ProbeKind::StaleEstimate,
            -0.1,
            now,
            ttl,
            &mut issued
        )
        .is_err()
    );
    assert!(
        unknown_signal(
            "model:x",
            ProbeKind::StaleEstimate,
            f64::NAN,
            now,
            ttl,
            &mut issued
        )
        .is_err()
    );
    assert_eq!(issued, 1, "only a signal that was raised takes an id");
}

/// The selection the sweep makes, over more unknowns than the desk has
/// slots — the default platform measures exactly as many as there are
/// slots, so the cap cannot be shown through it: the most uncertain come
/// first whatever their names, a certain subject and a live one are left
/// out, and no more than the slot count is taken.
///
/// Mutation: drop the `truncate` and the length assertion fails. Mutation:
/// sort by subject only and the first-subject assertion fails.
#[test]
fn a_sweep_takes_the_most_uncertain_unknowns_first_and_no_more_than_the_desk_has_slots() {
    let total = MAXIMUM_OPEN_UNKNOWNS + 4;
    // Named so that alphabetical order is the reverse of uncertainty order.
    let mut unknowns: BTreeMap<String, (ProbeKind, f64)> = (0..total)
        .map(|i| {
            (
                format!("subject-{i:02}"),
                (
                    ProbeKind::UncertainModel,
                    (i + 1) as f64 / (total + 1) as f64,
                ),
            )
        })
        .collect();
    unknowns.insert("certain".to_string(), (ProbeKind::StaleEstimate, 0.0));
    let most = format!("subject-{:02}", total - 1);
    let next = format!("subject-{:02}", total - 2);
    assert!(
        unknowns.len() > MAXIMUM_OPEN_UNKNOWNS + 1,
        "premise: more unknowns than slots even with one live"
    );

    let ranked = rank_unknowns(unknowns.clone(), &BTreeMap::new());
    assert_eq!(ranked.len(), MAXIMUM_OPEN_UNKNOWNS);
    assert_eq!(ranked.first().map(|r| r.0.as_str()), Some(most.as_str()));
    assert!(
        ranked.windows(2).all(|pair| pair[0].2 > pair[1].2),
        "strictly descending uncertainty: {ranked:?}"
    );
    assert!(ranked.iter().all(|r| r.0 != "certain"));

    let live: BTreeMap<String, Timestamp> = [(most.clone(), start())].into_iter().collect();
    let without = rank_unknowns(unknowns, &live);
    assert_eq!(without.len(), MAXIMUM_OPEN_UNKNOWNS);
    assert_eq!(without.first().map(|r| r.0.as_str()), Some(next.as_str()));
    assert!(without.iter().all(|r| r.0 != most));
}
