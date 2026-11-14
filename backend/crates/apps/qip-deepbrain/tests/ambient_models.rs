//! R12-AMBIENT-1180: the always-on ambient models — a one-step forecast, a
//! path simulation and a weak-signal scan — run on every research sweep and
//! reach the attention router under a scheduled trigger naming the model,
//! with nobody asking and nothing else having fired first.
//!
//! Each test asserts its premise — that the platform holds the tape the model
//! reads — before it asserts what the model did, so an empty platform cannot
//! pass for a quiet one. Each names the mutation it was verified against.

use qip_contracts::ambient::{AttentionEvent, Disposition, Pathway, SignalClass, Trigger};
use qip_core::{Clock, Context, Decimal, Duration, ManualClock, ObjectId, Timestamp};
use qip_deepbrain::ambient_models::{
    self, AmbientModelConfig, FORECAST_MODEL, PATH_MODEL, WEAK_SIGNAL_MODEL,
};
use qip_deepbrain::config::DeepBrainConfig;
use qip_deepbrain::node::{self, Loops};
use qip_deepbrain::research::{
    AMBIENT_MODELS_VARIABLE, ResearchConfig, ResearchLoop, ResearchRound,
};
use qip_deepbrain::status::NodeStatus;
use qip_financial::quality::DataQuality;
use qip_financial::universe::Universe;
use qip_kernel::{Platform, PlatformConfig};
use qip_market::bar::{Bar, Interval};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_storage::{ChainArchive, MemoryKeyValueStore};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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

fn bar(symbol: &str, at: Timestamp, close: f64) -> SensedRecord {
    let price = |v: f64| Decimal::from_f64(v).unwrap_or_else(|| panic!("a price"));
    SensedRecord::Bar(Box::new(Bar {
        object_id: ObjectId::from_string(symbol.to_string()),
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

/// A calm daily return cycle: dispersed (so every model can measure it), a
/// few tenths of a percent a day, and no trend.
const CALM: [f64; 7] = [0.004, -0.003, 0.006, -0.005, 0.002, -0.004, 0.001];
/// A wild one: six to eight percent a day either way.
const WILD: [f64; 7] = [0.06, -0.07, 0.05, -0.08, 0.07, -0.06, 0.03];
/// Days of history before the newest return: past the default window.
const DAYS: usize = 40;

/// Forty days of `cycle` returns from 100, then one more day returning
/// `newest`, as daily bars on `symbol`.
fn tape(symbol: &str, cycle: &[f64], newest: f64) -> Vec<SensedRecord> {
    let mut price = 100.0;
    let mut out = vec![bar(symbol, start(), price)];
    for day in 1..=DAYS {
        let r = if day == DAYS {
            newest
        } else {
            cycle[day % cycle.len()]
        };
        price *= r.exp();
        out.push(bar(
            symbol,
            start().saturating_add(Duration::from_days(day as i64)),
            price,
        ));
    }
    out
}

fn now() -> Timestamp {
    start().saturating_add(Duration::from_days(DAYS as i64 + 1))
}

fn research(models: bool) -> ResearchLoop {
    let config = ResearchConfig {
        ambient_models: models,
        ..ResearchConfig::default()
    };
    ResearchLoop::new(config, BTreeMap::new())
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
}

fn sweep(records: Vec<SensedRecord>, subjects: usize, models: bool) -> ResearchRound {
    let mut platform = platform();
    platform.observe(records);
    assert_eq!(
        platform.price_history().len(),
        subjects,
        "premise: the platform holds every subject's tape"
    );
    assert!(
        platform
            .price_history()
            .values()
            .all(|p| p.len() == DAYS + 1),
        "premise: every tape is {} prices long",
        DAYS + 1
    );
    research(models)
        .sweep(&platform, now(), &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// The attention events `model` caused this sweep, with the signal each
/// names: the event must name the model as its origin and the signal must
/// carry the model's scheduled trigger.
fn woken_by<'a>(round: &'a ResearchRound, model: &str) -> Vec<(&'a AttentionEvent, String)> {
    round
        .attention_events
        .iter()
        .filter(|e| e.originating_models == vec![model.to_string()])
        .map(|e| {
            let signal = round
                .signals
                .iter()
                .find(|s| s.id() == e.signal_id)
                .unwrap_or_else(|| panic!("event {} names a signal the sweep routed", e.id));
            assert_eq!(
                signal.trigger(),
                &Trigger::Schedule(model.to_string()),
                "an ambient model's signal is scheduled, not an event"
            );
            assert_eq!(e.reason.trigger, Trigger::Schedule(model.to_string()));
            (e, signal.subject().to_string())
        })
        .collect()
}

/// Predict: a one-step forecast that misses by more than four residual
/// deviations is a surprise that wakes a specialist, named as the forecast
/// model's, and the same tape without the miss raises nothing of the kind.
///
/// The failure this prevents: a model that only ever described the past,
/// so a break in what it expected reached nobody until a detector called it
/// an anomaly. Mutation: compare `miss.abs() >= ...` against `f64::MAX`
/// in `ambient_models::run` — the activated-event assertion fails.
#[test]
fn a_forecast_that_misses_by_four_residual_deviations_wakes_a_specialist_and_a_calm_tape_does_not()
{
    let round = sweep(tape("JUMP", &CALM, 0.06), 1, true);
    let woken = woken_by(&round, FORECAST_MODEL);
    assert_eq!(woken.len(), 1, "{:?}", round.attention_events);
    let (event, subject) = &woken[0];
    assert_eq!(subject, "forecast-miss:JUMP");
    assert_eq!(event.pathway, Pathway::SpecialistActivation);
    assert_eq!(event.reason.class, SignalClass::Surprise);
    assert_eq!(event.disposition, Disposition::Activated);
    let report = round
        .ambient_models
        .as_ref()
        .unwrap_or_else(|| panic!("the models ran"));
    assert_eq!(report.forecasts.len(), 1, "the next step is forecast too");
    let next = &report.forecasts[0];
    assert!(next.lower < next.expected && next.expected < next.upper);

    let calm = sweep(tape("JUMP", &CALM, CALM[DAYS % CALM.len()]), 1, true);
    assert!(
        woken_by(&calm, FORECAST_MODEL).is_empty(),
        "a return the forecast expected is no surprise: {:?}",
        calm.attention_events
    );
    assert_eq!(
        calm.ambient_models.map(|r| r.forecasts.len()),
        Some(1),
        "premise: the calm tape was forecast, not skipped"
    );
}

/// Simulate paths: a subject whose own returns, walked ten days forward
/// over 256 seeded paths, reach a 15% drawdown on a quarter of them or more
/// is a risk review named as the path model's; a calm subject's paths never
/// do. The share is reproducible: the same seed draws the same paths.
///
/// Mutation: make the breach test `level < f64::MIN` in the path loop —
/// the wild subject's activated-event assertion fails.
#[test]
fn a_subject_whose_simulated_paths_breach_a_fifteen_percent_drawdown_raises_a_risk_review_and_a_calm_one_does_not()
 {
    let mut records = tape("WILD", &WILD, WILD[DAYS % WILD.len()]);
    records.extend(tape("CALM", &CALM, CALM[DAYS % CALM.len()]));
    let round = sweep(records, 2, true);
    let woken = woken_by(&round, PATH_MODEL);
    assert_eq!(woken.len(), 1, "{:?}", round.attention_events);
    let (event, subject) = &woken[0];
    assert_eq!(subject, "path-drawdown:WILD");
    assert_eq!(event.pathway, Pathway::RiskReview);
    assert_eq!(event.disposition, Disposition::Activated);
    let report = round
        .ambient_models
        .as_ref()
        .unwrap_or_else(|| panic!("the models ran"));
    let share = |s: &str| {
        report
            .paths
            .iter()
            .find(|p| p.subject == s)
            .map(|p| p.breach_bp)
            .unwrap_or_else(|| panic!("{s} was simulated"))
    };
    assert!(share("WILD") >= 2_500, "{:?}", report.paths);
    assert_eq!(share("CALM"), 0, "{:?}", report.paths);
    assert_eq!(
        event.priority_bp,
        share("WILD"),
        "severity is the breach share"
    );

    let again = sweep(
        {
            let mut r = tape("WILD", &WILD, WILD[DAYS % WILD.len()]);
            r.extend(tape("CALM", &CALM, CALM[DAYS % CALM.len()]));
            r
        },
        2,
        true,
    );
    assert_eq!(
        again.ambient_models.map(|r| r.paths),
        Some(report.paths.clone()),
        "a replay draws the same paths"
    );
}

/// `total` calm subjects; on the newest day the first `leaning` of them
/// move between one and a half and three of their own deviations up —
/// under the anomaly bar — and the rest move as usual.
fn leaning_book(leaning: usize, total: usize) -> Vec<SensedRecord> {
    let mut records = Vec::new();
    for (k, symbol) in ["AA", "BB", "CC", "DD", "EE", "FF", "GG", "HH"]
        .iter()
        .take(total)
        .enumerate()
    {
        let newest = if k < leaning {
            0.010
        } else {
            CALM[DAYS % CALM.len()]
        };
        records.extend(tape(symbol, &CALM, newest));
    }
    records
}

/// Scan weak signals: four of five subjects each a little more than two of
/// their own deviations up — none of them an anomaly — is the book leaning
/// one way, raised as research and named as the weak-signal scanner's.
/// Four of eight is not a majority of the book, and three of four is too
/// few subjects to be a breadth; neither raises anything.
///
/// Mutations: drop the `breadth < config.weak_breadth_bp` arm — the
/// four-of-eight half fails; drop the `members.len() < config.weak_min_subjects`
/// arm — the three-of-four half fails.
#[test]
fn four_of_five_subjects_leaning_the_same_way_below_the_anomaly_bar_is_a_weak_signal_and_half_a_book_or_three_subjects_are_not()
 {
    // Premise: the leaning return sits inside the weak band of the calm
    // cycle's own deviation, so it is a weak reading and not an anomaly.
    let fit: Vec<f64> = (DAYS - 32..DAYS).map(|d| CALM[d % CALM.len()]).collect();
    let mean = fit.iter().sum::<f64>() / fit.len() as f64;
    let sd =
        (fit.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (fit.len() as f64 - 1.0)).sqrt();
    let z = (0.010 - mean) / sd;
    assert!((1.5..3.0).contains(&z), "premise: z = {z}");

    let round = sweep(leaning_book(4, 5), 5, true);
    let woken = woken_by(&round, WEAK_SIGNAL_MODEL);
    assert_eq!(woken.len(), 1, "{:?}", round.attention_events);
    let (event, subject) = &woken[0];
    assert_eq!(subject, "weak-signal:breadth:up");
    assert_eq!(event.pathway, Pathway::ResearchTask);
    assert_eq!(event.disposition, Disposition::Activated);
    assert_eq!(event.priority_bp, 8_000, "four of five is 8000 bp");
    let signal = round
        .signals
        .iter()
        .find(|s| s.id() == event.signal_id)
        .unwrap_or_else(|| panic!("the signal"));
    assert_eq!(signal.affected_entities(), ["AA", "BB", "CC", "DD"]);

    let half = sweep(leaning_book(4, 8), 8, true);
    assert!(
        woken_by(&half, WEAK_SIGNAL_MODEL).is_empty(),
        "four of eight is not a majority: {:?}",
        half.attention_events
    );
    let few = sweep(leaning_book(3, 4), 4, true);
    assert!(
        woken_by(&few, WEAK_SIGNAL_MODEL).is_empty(),
        "three subjects are fewer than a breadth needs: {:?}",
        few.attention_events
    );
}

/// Refuse rather than guess: a tape too short for the window, or one with
/// a non-positive price, is reported unmeasurable by every model and raises
/// nothing; a book with fewer measurable subjects than a breadth is reported
/// as such. Nothing is clamped into a figure.
///
/// Mutation: push nothing to `unmeasured` for a short history in
/// `ambient_models::run` — the reason assertion fails.
#[test]
fn a_tape_the_models_cannot_measure_is_reported_by_reason_and_raises_nothing() {
    let short: BTreeMap<String, Vec<f64>> =
        BTreeMap::from([("SHORT".to_string(), vec![100.0, 101.0, 99.0])]);
    let mut issued = 0;
    let report = ambient_models::run(
        &AmbientModelConfig::default(),
        &short,
        now(),
        Duration::from_hours(1),
        &mut issued,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(report.signals.is_empty() && issued == 0);
    let reasons: BTreeSet<(&str, &str)> = report
        .unmeasured
        .iter()
        .map(|u| (u.model, u.reason))
        .collect();
    for model in ambient_models::MODELS {
        assert!(
            reasons.contains(&(model, "history_shorter_than_window")),
            "{model}: {:?}",
            report.unmeasured
        );
    }
    assert!(reasons.contains(&(
        WEAK_SIGNAL_MODEL,
        "fewer_measurable_subjects_than_a_breadth"
    )));

    let mut bad = vec![100.0; 40];
    bad[20] = 0.0;
    let report = ambient_models::run(
        &AmbientModelConfig::default(),
        &BTreeMap::from([("ZERO".to_string(), bad)]),
        now(),
        Duration::from_hours(1),
        &mut issued,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(report.signals.is_empty());
    assert!(
        report
            .unmeasured
            .iter()
            .any(|u| u.reason == "non_positive_or_non_finite_price"),
        "{:?}",
        report.unmeasured
    );
}

/// A threshold nobody could have meant is refused at assembly, not run.
/// Mutation: return `Ok(())` at the top of `validate` — every refusal fails.
#[test]
fn an_ambient_model_threshold_nobody_could_mean_is_refused() {
    let base = AmbientModelConfig::default();
    assert!(base.validate().is_ok(), "premise: the default is valid");
    let refused = [
        AmbientModelConfig {
            window: 3,
            ..base.clone()
        },
        AmbientModelConfig {
            forecast_miss_sigmas: f64::NAN,
            ..base.clone()
        },
        AmbientModelConfig {
            weak_low_sigmas: 3.0,
            weak_high_sigmas: 3.0,
            ..base.clone()
        },
        AmbientModelConfig {
            path_count: 0,
            ..base.clone()
        },
        AmbientModelConfig {
            path_drawdown: 1.0,
            ..base.clone()
        },
        AmbientModelConfig {
            path_alarm_bp: 0,
            ..base.clone()
        },
        AmbientModelConfig {
            weak_breadth_bp: 5_000,
            ..base.clone()
        },
        AmbientModelConfig {
            path_step: Duration::ZERO,
            ..base.clone()
        },
    ];
    for config in refused {
        assert!(config.validate().is_err(), "{config:?}");
        assert!(
            research(true).with_ambient_models(config.clone()).is_err(),
            "the loop refuses it too: {config:?}"
        );
    }
}

/// The switch: off, the sweep runs no model and says so with `None` rather
/// than an empty report; a value that is neither `on` nor `off` stops
/// start-up. Mutation: read `ambient_models` as always true in `sweep_over`
/// — the off half fails.
#[test]
fn with_the_models_switched_off_the_same_tape_raises_nothing_scheduled_and_a_typo_is_refused() {
    let round = sweep(tape("JUMP", &CALM, 0.06), 1, false);
    assert!(round.ambient_models.is_none());
    assert!(
        round
            .signals
            .iter()
            .all(|s| !matches!(s.trigger(), Trigger::Schedule(_))),
        "{:?}",
        round.signals
    );
    let lookup = |value: &'static str| {
        move |name: &str| (name == AMBIENT_MODELS_VARIABLE).then(|| value.to_string())
    };
    assert!(ResearchConfig::from_lookup(&lookup("off")).is_ok_and(|c| !c.ambient_models));
    assert!(ResearchConfig::from_lookup(&lookup("on")).is_ok_and(|c| c.ambient_models));
    assert!(ResearchConfig::from_lookup(&|_: &str| None).is_ok_and(|c| c.ambient_models));
    assert!(ResearchConfig::from_lookup(&lookup("yes")).is_err());
}

/// From the composition root's loop: the deep brain's `node::run_with`, the
/// loop `main.rs` runs, sweeps research on its cadence and the scheduled
/// models' signals reach the attention router on that sweep — no caller
/// asked for them.
///
/// Mutation: delete the `for signal in &report.signals` routing loop in
/// `sweep_over` — no event names a model and the assertion fails.
#[test]
fn the_deep_brains_run_loop_routes_every_scheduled_models_signal_without_a_prompt() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(now()));
    let config = PlatformConfig::default();
    let context = Context::new(clock.clone(), config.seed);
    let mut platform = Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut records = tape("JUMP", &CALM, 0.06);
    records.extend(tape("WILD", &WILD, WILD[DAYS % WILD.len()]));
    // Six leaning of eight measurable: 7500 bp, the default breadth.
    for symbol in ["AA", "BB", "CC", "DD", "EE", "FF"] {
        records.extend(tape(symbol, &CALM, 0.010));
    }
    platform.observe(records);
    assert_eq!(platform.price_history().len(), 8, "premise: eight tapes");
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(1),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(now()).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        now(),
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut research = research(true);
    let mut models: BTreeSet<String> = BTreeSet::new();
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops {
            desk: None,
            research: Some(&mut research),
            operations: None,
            tick: None,
            permissions: None,
            ambient: None,
            agency: None,
            warehouse: None,
            backlog: None,
        },
        |outcome| {
            let round = outcome
                .research
                .as_ref()
                .unwrap_or_else(|| panic!("the cycle swept research"));
            for event in &round.attention_events {
                if let Trigger::Schedule(model) = &event.reason.trigger {
                    models.extend(event.originating_models.iter().cloned());
                    assert_eq!(&event.originating_models, &vec![model.clone()]);
                }
            }
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        models,
        set(&[FORECAST_MODEL, PATH_MODEL, WEAK_SIGNAL_MODEL]),
        "every scheduled model woke something from the run loop"
    );
}
