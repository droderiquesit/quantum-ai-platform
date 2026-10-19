//! The critical-path objectives read from a snapshot and recorded as
//! fixed-label series (OBS-7115/7121/7122's SLO half), and the entity
//! graph's bound.
//!
//! The failure prevented: an objective that names a series and is evaluated
//! by nothing. Before this, `critical_path_slos` was read by one test that
//! checked its length, and a quantum job failing every time moved no number.

use qip_core::Timestamp;
use qip_observability::Metrics;
use qip_observability::aiops::{EntityGraph, EntityKey, Signal, SignalClass};
use qip_observability::critical_paths::{
    critical_path_slos, evaluate, record, reflex_objectives, service_objectives,
};
use qip_observability::metrics::{labels, names};

fn status_of<'a>(
    statuses: &'a [qip_observability::SloStatus],
    name: &str,
) -> &'a qip_observability::SloStatus {
    statuses
        .iter()
        .find(|s| s.slo.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the catalogue"))
}

#[test]
fn quantum_and_model_package_outcomes_become_measured_objectives_and_an_absent_series_stays_unmeasured()
 {
    let metrics = Metrics::new("test");
    // Premise: before anything is recorded, both read unmeasured.
    let before = evaluate(&metrics.snapshot());
    assert!(!status_of(&before, "quantum-job-success").is_observed());
    assert!(!status_of(&before, "model-serving-package").is_observed());

    metrics.increment(names::QUANTUM_JOBS, labels([("outcome", "succeeded")]), 3);
    metrics.count(names::QUANTUM_JOBS, labels([("outcome", "failed")]));
    metrics.count(names::MODEL_PACKAGES, labels([("outcome", "written")]));

    let after = evaluate(&metrics.snapshot());
    let quantum = status_of(&after, "quantum-job-success");
    assert_eq!(quantum.observations, 4);
    assert!((quantum.achieved - 0.75).abs() < 1e-12);
    assert!(!quantum.is_met, "0.75 is below the 0.95 target");
    assert!(status_of(&after, "model-serving-package").is_met);
    assert!(!status_of(&after, "reconciliation").is_observed());
}

#[test]
fn a_ledger_commit_in_a_bucket_straddling_the_threshold_counts_against_the_objective() {
    let metrics = Metrics::new("test");
    // 100 ms is a bucket bound: at or under it is good.
    metrics.observe_latency_ms(names::LEDGER_COMMIT_LATENCY_MS, labels([]), 90.0);
    // 120 ms lands in the (100, 250] bucket, bad.
    metrics.observe_latency_ms(names::LEDGER_COMMIT_LATENCY_MS, labels([]), 120.0);
    let statuses = evaluate(&metrics.snapshot());
    let ledger = status_of(&statuses, "ledger-commit");
    assert_eq!(ledger.observations, 2);
    assert!((ledger.achieved - 0.5).abs() < 1e-12, "{ledger:?}");
}

#[test]
fn recording_writes_observations_for_every_objective_and_a_budget_only_for_a_measured_one() {
    let source = Metrics::new("source");
    source.count(names::QUANTUM_JOBS, labels([("outcome", "failed")]));
    let statuses = evaluate(&source.snapshot());
    // The eight critical paths, the service objectives and the reflex
    // objectives beside them.
    assert_eq!(
        statuses.len(),
        critical_path_slos().len() + service_objectives().len() + reflex_objectives().len()
    );

    let sink = Metrics::new("sink");
    record(&sink, &statuses);
    let snapshot = sink.snapshot();
    for path in critical_path_slos()
        .into_iter()
        .chain(service_objectives())
        .chain(reflex_objectives())
    {
        assert!(
            snapshot
                .gauge(
                    names::SLO_OBSERVATIONS,
                    &labels([("objective", path.slo.name.as_str())])
                )
                .is_some(),
            "{} has no observations gauge",
            path.slo.name
        );
    }
    assert!(
        snapshot
            .gauge(
                names::SLO_BUDGET_CONSUMED,
                &labels([("objective", "quantum-job-success")])
            )
            .is_some_and(|b| b >= 1.0)
    );
    assert_eq!(
        snapshot.gauge(
            names::SLO_BUDGET_CONSUMED,
            &labels([("objective", "ledger-commit")])
        ),
        None,
        "an unmeasured objective has no budget to report"
    );
}

#[test]
fn an_entity_graph_with_retention_keeps_the_newest_signals_and_refuses_zero() {
    let mut graph = EntityGraph::with_retention(2).unwrap_or_else(|e| panic!("{e}"));
    let entity = EntityKey::new("deployment", "qip-deepbrain");
    for i in 0..5 {
        let signal = Signal {
            class: SignalClass::Infrastructure,
            name: "cycle_ms".into(),
            value: f64::from(i),
            at: Timestamp::from_secs(i64::from(i)),
        };
        graph
            .record(&signal, std::slice::from_ref(&entity))
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let values: Vec<f64> = graph.signals_for(&entity).iter().map(|s| s.value).collect();
    assert_eq!(values, vec![3.0, 4.0]);
    assert!(EntityGraph::with_retention(0).is_err());
}

#[test]
fn the_catalogue_holds_a_model_serving_objective_on_the_serving_series_at_the_package_target() {
    // R12-OBS-7128: "removing it fails the check". Both targets are asserted
    // because the objective beside it is the package-written one, and the two
    // must not collapse into one name or drift to different targets.
    let slos = critical_path_slos();
    let serving = slos
        .iter()
        .find(|path| path.slo.name == "model-serving")
        .expect("the catalogue has a model-serving objective");
    let package = slos
        .iter()
        .find(|path| path.slo.name == "model-serving-package")
        .expect("the catalogue keeps the package objective");
    assert_eq!(serving.source_series, names::MODEL_SERVES);
    assert_ne!(serving.source_series, package.source_series);
    assert!((serving.slo.target - package.slo.target).abs() < 1e-12);
    assert!((serving.slo.target - 0.99).abs() < 1e-12);
}

/// R12-OBS-9159: every objective `default_slos` declares for the reflex is
/// bound to a series, evaluated where the node records it, and carries the
/// declaration's own threshold and target rather than a restated one.
///
/// The failure this prevents: six objectives declared and read by nothing, so
/// the reflex was held to a target no code evaluated.
///
/// Mutation: rename the `risk-gate-latency-p99` binding in
/// `reflex_objectives` - the declared name has no path and the first
/// assertion fires.
#[test]
fn every_declared_reflex_objective_is_bound_to_a_series_the_node_records() {
    use qip_observability::critical_paths::{EDGE_NODE_SERVICE, REFLEX_SERVICE};
    use qip_observability::slo::default_slos;

    let declared: Vec<_> = default_slos()
        .into_iter()
        .filter(|slo| slo.service == REFLEX_SERVICE)
        .collect();
    assert_eq!(
        declared.len(),
        6,
        "premise: six reflex objectives are declared"
    );
    let bound = reflex_objectives();
    for slo in &declared {
        let path = bound
            .iter()
            .find(|path| path.slo.name == slo.name)
            .unwrap_or_else(|| panic!("{} is declared and bound to no series", slo.name));
        assert_eq!(path.slo.service, EDGE_NODE_SERVICE, "{}", slo.name);
        assert!(
            (path.slo.target - slo.target).abs() < f64::EPSILON,
            "{} target moved: {} against {}",
            slo.name,
            path.slo.target,
            slo.target
        );
        assert_eq!(
            path.slo.latency_threshold_ms, slo.latency_threshold_ms,
            "{}",
            slo.name
        );
    }
    assert_eq!(
        bound.len(),
        declared.len(),
        "a binding names no declaration"
    );
}
