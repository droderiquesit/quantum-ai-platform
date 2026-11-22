//! The curiosity sentinel (R12-AMBIENT-3073), reached from the deep brain's
//! research sweep: the kernel measures what the platform does not know — a
//! component whose record is too thin to estimate, a model no better than a
//! coin, an unscored capacity, an unsettled regime boundary — and the sweep
//! raises those an answer is worth most on as research-task signals through
//! the attention router, each with the gap behind it, so that an activation
//! queues a research question on the desk and launches a scout job the node
//! hands the discovery desk. On by default; `QIP_DEEPBRAIN_CURIOSITY_SENTINEL`
//! set to `off` at the composition root removes the signals and their work.
//!
//! The exploration desk ranks probes from the same measure, but only under a
//! capital budget, and the budget is zero unless a mandate sets a share
//! aside. Every test here runs on `PlatformConfig::default` and asserts that
//! the desk holds nothing, so what is proven is the path a default
//! deployment actually runs.
//!
//! Each test asserts its premise first — that the kernel measured an unknown
//! — so a platform that knew everything cannot pass for a quiet sentinel.

use qip_contracts::ambient::AcquisitionKind;
use qip_contracts::ambient::{AmbientSignal, Disposition, Pathway, SignalClass, Trigger};
use qip_core::{Context, Decimal, Duration, ObjectId, Timestamp};
use qip_deepbrain::acquisition_dispatch::{Handler, Routing, dispatch};
use qip_deepbrain::research::{
    CURIOSITY_SENTINEL_VARIABLE, ResearchConfig, ResearchLoop, ResearchRound, UNKNOWN_TRIGGER,
    rank_unknowns, unknown_signal,
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

/// The loop the composition root builds: its configuration comes from
/// `ResearchConfig::from_lookup`, the function `qip-deepbrain`'s `main.rs`
/// calls, with the sentinel's variable unset (`None`) or set to `value`. No
/// test here writes the switch as a struct field, because a field only a
/// test can write is a switch no deployment holds.
fn sentinel(variable: Option<&str>) -> ResearchLoop {
    let config = ResearchConfig::from_lookup(&|name| {
        (name == CURIOSITY_SENTINEL_VARIABLE)
            .then(|| variable.map(str::to_string))
            .flatten()
    })
    .unwrap_or_else(|e| panic!("the configuration is read: {e}"));
    ResearchLoop::new(config, BTreeMap::new())
        .unwrap_or_else(|e| panic!("the research loop assembles: {e}"))
}

/// A platform whose unknowns differ in uncertainty, produced by the cycle
/// alone: each cycle adds one observation to the world-model components it
/// scores, and a component first scored a cycle later is one sample thinner
/// — so it is the most uncertain while sorting neither first nor last by
/// name. Returns the platform and the instant of its last cycle.
fn platform_with_unequal_unknowns() -> (Platform, Timestamp) {
    let mut platform = platform();
    let mut at = start();
    for i in 0..3_i64 {
        let close = 100.0 + (i as f64) * if i % 2 == 0 { 9.0 } else { -7.0 };
        platform.observe(vec![bar(at, close)]);
        at = at.saturating_add(Duration::from_days(1));
        platform.run_cycle(at);
    }
    (platform, at)
}

/// The research question the desk queues for one unknown, as the round
/// reports it — `None` where the unknown started nothing.
fn task<'a>(round: &'a ResearchRound, subject: &str) -> Option<&'a str> {
    round.unknown_tasks.get(subject).map(String::as_str)
}

fn is_unknown(signal: &AmbientSignal) -> bool {
    matches!(signal.trigger(), Trigger::Event(name) if name == UNKNOWN_TRIGGER)
}

/// The unknowns the kernel measures, in the order the sentinel owes: what an
/// answer is worth to the book first (`Platform::resolution_value`, the
/// exploration desk's own valuation), ties by uncertainty and then by
/// subject — restricted to those an answer is worth something on.
fn measured(platform: &Platform, now: Timestamp) -> Vec<(String, ProbeKind, f64)> {
    let worth = |uncertainty: f64| {
        platform
            .resolution_value(uncertainty)
            .unwrap_or_else(|| panic!("an uncertainty of {uncertainty} has a value"))
    };
    let mut unknowns: Vec<(String, ProbeKind, f64)> = platform
        .unknowns(now)
        .into_iter()
        .filter(|(_, (_, uncertainty))| *uncertainty > 0.0 && worth(*uncertainty).is_positive())
        .map(|(subject, (kind, uncertainty))| (subject, kind, uncertainty))
        .collect();
    unknowns.sort_by(|a, b| {
        worth(b.2)
            .cmp(&worth(a.2))
            .then_with(|| b.2.total_cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
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

    let mut loop_ = sentinel(None);
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

    // What the activation set in motion — the half this test did not hold
    // until 2026-11-22, when the signal was routed with no gap behind it and
    // an activation started nothing while every assertion above passed.
    // Every unknown the router activated is a research question queued on
    // the desk and a scout job the node hands the discovery desk.
    let activated: Vec<&(String, ProbeKind, f64)> = raised
        .iter()
        .zip(&expected)
        .filter(|(signal, _)| {
            round
                .attention_events
                .iter()
                .any(|e| e.signal_id == signal.id() && e.disposition == Disposition::Activated)
        })
        .map(|(_, unknown)| *unknown)
        .collect();
    assert_eq!(
        activated.len(),
        expected.len(),
        "premise: the router activated every unknown it was handed: {:?}",
        round.attention_events
    );
    let (to_desk, routed) = dispatch(&round.scouted);
    for (subject, kind, _) in &activated {
        let question = format!("Memory gap: {subject} is not known ({})", kind.as_str());
        assert_eq!(
            task(&round, subject),
            Some(question.as_str()),
            "the unknown is a question queued on the desk: {:?}",
            round.unknown_tasks
        );
        let scouts: Vec<_> = round
            .scouted
            .iter()
            .filter(|s| s.question == question)
            .collect();
        assert_eq!(scouts.len(), 1, "one scout job per unknown: {scouts:?}");
        let scout = scouts[0];
        assert_eq!(scout.entities, vec![subject.clone()]);
        assert_eq!(scout.kind, AcquisitionKind::NewSourceQuery);
        assert_eq!(
            to_desk
                .iter()
                .filter(|s| s.request_id == scout.request_id)
                .count(),
            1,
            "the node hands it to the discovery desk"
        );
        let dispatched = routed
            .iter()
            .find(|d| d.request_id == scout.request_id)
            .unwrap_or_else(|| panic!("the scout was routed: {routed:?}"));
        assert_eq!(dispatched.routing, Routing::Handled(Handler::DiscoveryDesk));
    }
    assert_eq!(
        round.unknown_tasks.len(),
        activated.len(),
        "and no task for an unknown that was not activated"
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
    assert!(
        again.unknown_tasks.is_empty(),
        "and starts no second task: {:?}",
        again.unknown_tasks
    );
}

/// Disabling the sentinel removes its signals and the work they start: the
/// same platform, holding the same unknowns, swept once by a loop configured
/// as the composition root configures it with nothing set and once with the
/// sentinel's variable `off`. The first raises the unknowns, queues their
/// questions and launches their scouts; the second routes no curiosity
/// signal, queues no such question and launches no such scout — while the
/// rest of its sweep still runs, so the quiet is the switch and not a sweep
/// that failed.
///
/// Mutation: ignore `config.curiosity_sentinel` in `sweep_over` (raise
/// regardless) and the off-side assertions fail. Mutation: delete the
/// `CURIOSITY_SENTINEL_VARIABLE` read in `ResearchConfig::from_lookup` and
/// `off` leaves the sentinel on — the same assertions fail.
#[test]
fn turning_the_sentinel_off_at_the_composition_root_removes_its_signals_and_their_work() {
    let (platform, after) = platform_with_resolved_forecasts();
    assert!(
        !measured(&platform, after).is_empty(),
        "premise: there is an unknown to raise"
    );
    let on = sentinel(None)
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let questions: BTreeSet<&str> = on.unknown_tasks.values().map(String::as_str).collect();
    assert!(
        on.signals.iter().any(is_unknown) && !questions.is_empty(),
        "premise: with nothing set the sentinel raises unknowns and they start work"
    );
    assert!(
        on.scouted
            .iter()
            .any(|s| questions.contains(s.question.as_str())),
        "premise: and a scout is launched on one: {:?}",
        on.scouted
    );

    let off = sentinel(Some("off"))
        .sweep(&platform, after, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !off.signals.is_empty(),
        "premise: the sweep ran and routed its other signals"
    );
    assert!(off.unknowns_raised.is_empty(), "{:?}", off.unknowns_raised);
    assert!(!off.signals.iter().any(is_unknown), "{:?}", off.signals);
    assert!(off.unknown_tasks.is_empty(), "{:?}", off.unknown_tasks);
    let scouted: Vec<&str> = off
        .scouted
        .iter()
        .map(|s| s.question.as_str())
        .filter(|q| questions.contains(q))
        .collect();
    assert!(scouted.is_empty(), "no scout on an unknown: {scouted:?}");
    // The other signals are the same set either way: the switch removes the
    // sentinel's signals and nothing else.
    let others = |round: &ResearchRound| -> Vec<String> {
        round
            .signals
            .iter()
            .filter(|s| !is_unknown(s))
            .map(|s| s.subject().to_string())
            .collect()
    };
    assert_eq!(others(&off), others(&on));
}

/// The switch a deployed binary holds: `qip-deepbrain`'s `main.rs` builds
/// its research configuration from `ResearchConfig::from_lookup`, and the
/// sentinel is on with nothing set, on at `on`, off at `off`, and anything
/// else stops start-up rather than being read as either.
///
/// Mutation: default `curiosity_sentinel` to `false` in `ResearchConfig` and
/// the unset assertion fails. Mutation: read any unrecognised value as `on`
/// and the refusal assertion fails.
#[test]
fn the_sentinel_is_on_unless_the_composition_root_is_told_off_and_a_typo_is_refused() {
    let read = |value: Option<&str>| {
        ResearchConfig::from_lookup(&|name| {
            (name == CURIOSITY_SENTINEL_VARIABLE)
                .then(|| value.map(str::to_string))
                .flatten()
        })
    };
    let sentinel_of = |value: Option<&str>| {
        read(value)
            .unwrap_or_else(|e| panic!("{value:?} is read: {e}"))
            .curiosity_sentinel
    };
    assert!(sentinel_of(None), "on in production with nothing set");
    assert!(sentinel_of(Some("on")));
    assert!(!sentinel_of(Some("off")));
    for typo in ["OFF", "false", "0", ""] {
        let refused = read(Some(typo))
            .err()
            .unwrap_or_else(|| panic!("{typo:?} is refused, not read as on or off"));
        assert!(
            refused.message().starts_with(CURIOSITY_SENTINEL_VARIABLE),
            "the refusal names the variable: {}",
            refused.message()
        );
    }
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

/// The order the sweep raises unknowns in, through the sweep: on a
/// platform whose unknowns differ, the one an answer is worth most on is
/// routed first although it sorts in the middle by name, and each queued
/// question follows in the same order.
///
/// Mutation: sort `rank_unknowns` by subject only and the order assertion
/// fails.
#[test]
fn a_sweep_routes_the_unknown_worth_most_first_whatever_its_name() {
    let (platform, at) = platform_with_unequal_unknowns();
    let unknowns = measured(&platform, at);
    let all = platform.unknowns(at);
    let by_name: Vec<&str> = all
        .keys()
        .map(String::as_str)
        .filter(|name| unknowns.iter().any(|(subject, _, _)| subject == name))
        .collect();
    let by_value: Vec<&str> = unknowns
        .iter()
        .map(|(subject, _, _)| subject.as_str())
        .collect();
    assert!(
        by_value.len() > 1 && by_value.len() <= MAXIMUM_OPEN_UNKNOWNS,
        "premise: several unknowns, all within the slots: {by_value:?}"
    );
    assert_ne!(
        by_value, by_name,
        "premise: value order is not name order on this platform"
    );
    let first_value = platform.resolution_value(unknowns[0].2);
    let last_value = platform.resolution_value(unknowns[unknowns.len() - 1].2);
    assert!(
        first_value > last_value,
        "premise: the first is worth strictly more than the last: {first_value:?} {last_value:?}"
    );

    let round = sentinel(None)
        .sweep(&platform, at, &set(&[]), &set(&["US"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let routed: Vec<String> = round
        .signals
        .iter()
        .filter(|s| is_unknown(s))
        .map(|s| s.subject().to_string())
        .collect();
    let owed: Vec<String> = unknowns
        .iter()
        .map(|(subject, kind, _)| format!("unknown:{}:{subject}", kind.as_str()))
        .collect();
    assert_eq!(routed, owed);
}

/// The selection the sweep makes, over more unknowns than the desk has
/// slots — no platform a test here can build through the cycle measures
/// more than the slot count, so the cap is shown on `rank_unknowns`, the
/// function the sweep calls: the unknown an answer is worth most on comes
/// first whatever its name or its uncertainty, one an answer is worth
/// nothing on, a certain subject and a live one are left out, and no more
/// than the slot count is taken.
///
/// Mutation: drop the `truncate` and the length assertion fails. Mutation:
/// sort by uncertainty before value and the first-subject assertion fails.
/// Mutation: drop the positive-value filter and the worthless assertion
/// fails.
#[test]
fn the_selection_takes_the_unknowns_worth_most_first_and_no_more_than_the_desk_has_slots() {
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
    // The least uncertain of all, and the one an answer is worth most on.
    unknowns.insert("prized".to_string(), (ProbeKind::CapacityAtSize, 0.01));
    // Uncertain, and worth nothing.
    unknowns.insert("worthless".to_string(), (ProbeKind::RegimeBoundary, 0.5));
    let most = format!("subject-{:02}", total - 1);
    assert!(
        unknowns.len() > MAXIMUM_OPEN_UNKNOWNS + 2,
        "premise: more unknowns than slots even with one live"
    );
    // Statistic to money: a stated price per unit of uncertainty, with two
    // exceptions the uncertainty alone could not produce.
    let value = |uncertainty: f64| -> Option<Decimal> {
        if uncertainty < 0.02 {
            Some(Decimal::from_int(1_000_000))
        } else if (0.49..0.51).contains(&uncertainty) {
            Some(Decimal::ZERO)
        } else {
            Decimal::from_f64(uncertainty * 100.0)
        }
    };

    let ranked = rank_unknowns(unknowns.clone(), &BTreeMap::new(), &value);
    assert_eq!(ranked.len(), MAXIMUM_OPEN_UNKNOWNS);
    let subjects: Vec<&str> = ranked.iter().map(|u| u.subject.as_str()).collect();
    assert_eq!(subjects.first().copied(), Some("prized"));
    assert_eq!(subjects.get(1).copied(), Some(most.as_str()));
    assert!(
        ranked.windows(2).all(|pair| pair[0].value > pair[1].value),
        "strictly descending value: {ranked:?}"
    );
    assert!(!subjects.contains(&"certain"));
    assert!(!subjects.contains(&"worthless"));

    let live: BTreeMap<String, Timestamp> = [("prized".to_string(), start())].into_iter().collect();
    let without = rank_unknowns(unknowns, &live, &value);
    assert_eq!(without.len(), MAXIMUM_OPEN_UNKNOWNS);
    assert_eq!(
        without.first().map(|u| u.subject.as_str()),
        Some(most.as_str())
    );
    assert!(without.iter().all(|u| u.subject != "prized"));

    // With slots to spare, so the cap cannot be what leaves it out: an
    // unknown an answer is worth nothing on is not raised at all.
    let few: BTreeMap<String, (ProbeKind, f64)> = [
        ("worthless".to_string(), (ProbeKind::RegimeBoundary, 0.5)),
        (
            "worth-a-little".to_string(),
            (ProbeKind::StaleEstimate, 0.25),
        ),
    ]
    .into_iter()
    .collect();
    let raised: Vec<String> = rank_unknowns(few, &BTreeMap::new(), &value)
        .into_iter()
        .map(|u| u.subject)
        .collect();
    assert_eq!(raised, ["worth-a-little"]);
}
