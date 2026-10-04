//! Entity graph, anomaly detection, timeline, runbook remediation, release gate.
#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]

use qip_core::Timestamp;
use qip_observability::aiops::{
    Effector, EntityGraph, EntityKey, ReleaseDecision, Remediator, Runbook, Signal, SignalClass,
    TimelineEvent, TimelineSource, build_timeline, detect_level_shift, release_gate,
};
use qip_observability::critical_paths::critical_path_slos;
use qip_observability::slo::{Slo, SloWindow};
use std::collections::BTreeSet;

fn t(s: i64) -> Timestamp {
    Timestamp::from_secs(s)
}

// --- OBS-001 ---

#[test]
fn five_signal_classes_about_one_instrument_are_reachable_from_its_single_node() {
    let instrument = EntityKey::new("instrument", "XNYS:ACME");
    let deployment = EntityKey::new("deployment", "fastbrain-eu");
    let mut g = EntityGraph::new();
    let classes = [
        (
            SignalClass::Infrastructure,
            vec![deployment.clone(), instrument.clone()],
        ),
        (
            SignalClass::Application,
            vec![deployment.clone(), instrument.clone()],
        ),
        (SignalClass::DataQuality, vec![instrument.clone()]),
        (SignalClass::ModelHealth, vec![instrument.clone()]),
        (SignalClass::TradingBehaviour, vec![instrument.clone()]),
    ];
    for (class, entities) in &classes {
        let s = Signal {
            class: *class,
            name: "x".into(),
            value: 1.0,
            at: t(1),
        };
        g.record(&s, entities).unwrap();
    }
    // premise: the fixture really spans five classes
    assert_eq!(
        classes.iter().map(|c| c.0).collect::<BTreeSet<_>>().len(),
        5
    );
    assert_eq!(g.classes_for(&instrument).len(), 5);
    assert_eq!(g.signals_for(&instrument).len(), 5);
    // one node per entity, not per class
    assert_eq!(g.node_count(), 2);
}

#[test]
fn a_signal_about_no_entity_is_refused() {
    let mut g = EntityGraph::new();
    let s = Signal {
        class: SignalClass::Application,
        name: "x".into(),
        value: 0.0,
        at: t(0),
    };
    assert!(g.record(&s, &[]).is_err());
    assert_eq!(g.node_count(), 0);
}

// --- OBS-003 ---

fn noisy(n: usize, step_at: Option<usize>) -> Vec<(Timestamp, f64)> {
    (0..n)
        .map(|i| {
            let noise = [0.0, 0.3, -0.2, 0.1, -0.3, 0.2][i % 6];
            let lift = if step_at.is_some_and(|s| i >= s) {
                5.0
            } else {
                0.0
            };
            (t(i64::try_from(i).unwrap()), 100.0 + noise + lift)
        })
        .collect()
}

#[test]
fn a_step_change_crossing_no_threshold_is_named_by_series_and_window_and_the_same_series_without_it_is_silent()
 {
    let flat = detect_level_shift("qip_x", &noisy(40, None), 8).unwrap();
    assert!(flat.is_none(), "premise: the unstepped series is quiet");
    let a = detect_level_shift("qip_x", &noisy(40, Some(20)), 8)
        .unwrap()
        .expect("step must be found");
    assert_eq!(a.series, "qip_x");
    assert!(a.window_start <= t(20) && a.window_end >= t(20));
}

#[test]
fn a_series_too_short_to_judge_is_refused_not_called_quiet() {
    assert!(detect_level_shift("qip_x", &noisy(5, None), 8).is_err());
}

// --- OBS-004 ---

#[test]
fn the_timeline_holds_exactly_the_five_in_window_events_in_order_each_attributed() {
    let ev = |source, at, d: &str| TimelineEvent {
        source,
        at: t(at),
        detail: d.into(),
    };
    let events = vec![
        ev(TimelineSource::ModelDrift, 50, "drift"),
        ev(TimelineSource::Telemetry, 20, "shift"),
        ev(TimelineSource::VenueBehaviour, 40, "venue"),
        ev(TimelineSource::Deployment, 10, "diff"),
        ev(TimelineSource::DataAnomaly, 30, "data"),
        ev(TimelineSource::Deployment, 500, "outside"),
    ];
    let tl = build_timeline(t(0), t(100), &events).unwrap();
    let got: Vec<_> = tl.iter().map(|e| (e.at, e.source)).collect();
    assert_eq!(
        got,
        vec![
            (t(10), TimelineSource::Deployment),
            (t(20), TimelineSource::Telemetry),
            (t(30), TimelineSource::DataAnomaly),
            (t(40), TimelineSource::VenueBehaviour),
            (t(50), TimelineSource::ModelDrift),
        ]
    );
    assert!(build_timeline(t(5), t(1), &events).is_err());
}

// --- OBS-005 / OBS-030 ---

#[derive(Debug, Default)]
struct Recorder(Vec<(String, String)>);
impl Effector for Recorder {
    fn apply(&mut self, action: &str, target: &str) -> qip_core::Result<String> {
        self.0.push((action.into(), target.into()));
        Ok("done".into())
    }
}

fn runbooks() -> Vec<Runbook> {
    vec![Runbook {
        name: "restart-api".into(),
        action: "restart".into(),
        targets: ["qip-api".to_string()].into(),
    }]
}

#[test]
fn an_action_in_no_runbook_is_refused_and_recorded_while_a_named_one_executes() {
    let mut r = Remediator::new(runbooks(), Recorder::default()).unwrap();
    assert!(r.remediate("INC-1", "delete_topic", "orders").is_err());
    assert_eq!(
        r.audit().records().len(),
        1,
        "the refusal is itself audited"
    );
    assert!(
        r.audit().records()[0]
            .result
            .contains("no runbook names action delete_topic")
    );
    // a permitted verb on a target outside the runbook is refused too
    assert!(r.remediate("INC-1", "restart", "qip-ledgerd").is_err());
    assert_eq!(r.remediate("INC-1", "restart", "qip-api").unwrap(), "done");
}

#[test]
fn the_audit_record_names_runbook_incident_target_and_result() {
    let mut r = Remediator::new(runbooks(), Recorder::default()).unwrap();
    r.remediate("INC-9", "restart", "qip-api").unwrap();
    let rec = &r.audit().records()[0];
    assert_eq!(
        (
            rec.runbook.as_str(),
            rec.incident.as_str(),
            rec.target.as_str(),
            rec.result.as_str()
        ),
        ("restart-api", "INC-9", "qip-api", "done")
    );
    assert!(r.audit().verify());
}

#[test]
fn a_runbook_naming_an_action_outside_the_permitted_set_cannot_be_registered() {
    let bad = Runbook {
        name: "x".into(),
        action: "delete_topic".into(),
        targets: BTreeSet::new(),
    };
    assert!(Remediator::new(vec![bad], Recorder::default()).is_err());
}

// --- OBS-028 ---

#[test]
fn an_exhausted_budget_refuses_a_promotion_to_its_plane_and_remaining_budget_lets_it_through() {
    let slo = Slo::availability("a", "ledger", 0.99, SloWindow::Day);
    let exhausted = slo.evaluate(90, 100);
    let healthy = slo.evaluate(100, 100);
    assert!(exhausted.budget_consumed >= 1.0, "premise");
    match release_gate("ledger", std::slice::from_ref(&exhausted)) {
        ReleaseDecision::Refused { reason } => assert!(reason.contains("error budget exhausted")),
        ReleaseDecision::Proceed => panic!("must refuse"),
    }
    assert_eq!(release_gate("ledger", &[healthy]), ReleaseDecision::Proceed);
    // another plane's exhausted budget is not this plane's reason
    assert_eq!(release_gate("api", &[exhausted]), ReleaseDecision::Proceed);
    // nothing measured never blocks
    assert_eq!(
        release_gate("ledger", &[slo.evaluate(0, 0)]),
        ReleaseDecision::Proceed
    );
}

// --- OBS-002 ---

#[test]
fn the_catalogue_holds_one_objective_per_critical_path_and_an_unfed_one_reads_unmeasured() {
    let c = critical_path_slos();
    assert_eq!(c.len(), 8);
    let names: BTreeSet<_> = c.iter().map(|p| p.slo.name.clone()).collect();
    assert_eq!(names.len(), 8);
    for p in &c {
        assert!(!p.source_series.is_empty());
        assert!(p.slo.target > 0.0 && p.slo.target <= 1.0);
        assert!(
            !p.evaluate(0, 0).is_observed(),
            "{} must read unmeasured",
            p.slo.name
        );
    }
    assert!(c[0].evaluate(10, 10).is_observed());
}
