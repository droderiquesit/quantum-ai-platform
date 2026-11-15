//! R12-OBS-9163 (ADR 0683): the six data-plane objectives are declared, and
//! an objective whose series nothing feeds reads unmeasured.
//!
//! Every test here: mutation not run (nothing was compiled).

use qip_observability::critical_paths::{NO_SOURCE_SERIES, SourceRule, evaluate};
use qip_observability::data_plane::{
    ARCHIVE_DELAY_OBJECTIVE, BIGTABLE_P99_OBJECTIVE, BQ_INGESTION_FRESHNESS_OBJECTIVE,
    DATA_PLANE_SERVICE, KNOWLEDGE_DELTA_FRESHNESS_OBJECTIVE, LINEAGE_COMPLETENESS_OBJECTIVE,
    REFETCH_HEALTH_OBJECTIVE, data_plane_objectives,
};
use qip_observability::metrics::{Metrics, labels, names};
use std::collections::BTreeSet;

const SIX: [&str; 6] = [
    ARCHIVE_DELAY_OBJECTIVE,
    REFETCH_HEALTH_OBJECTIVE,
    BIGTABLE_P99_OBJECTIVE,
    KNOWLEDGE_DELTA_FRESHNESS_OBJECTIVE,
    BQ_INGESTION_FRESHNESS_OBJECTIVE,
    LINEAGE_COMPLETENESS_OBJECTIVE,
];

fn status(metrics: &Metrics, name: &str) -> qip_observability::SloStatus {
    evaluate(&metrics.snapshot())
        .into_iter()
        .find(|s| s.slo.name == name)
        .unwrap_or_else(|| panic!("{name} is not evaluated"))
}

/// The requirement names six objectives and each has a target. Deleting one
/// from `data_plane_objectives` (or renaming one of the three it takes from
/// `default_slos`) drops it from the set and the first assertion fails.
#[test]
fn all_six_data_objectives_are_declared_under_the_data_plane_with_a_target() {
    let declared = data_plane_objectives();
    let names: BTreeSet<&str> = declared.iter().map(|p| p.slo.name.as_str()).collect();
    assert_eq!(names, BTreeSet::from(SIX));
    assert_eq!(declared.len(), 6, "each name once");
    for path in &declared {
        assert_eq!(path.slo.service, DATA_PLANE_SERVICE);
        assert!(path.slo.target > 0.0 && path.slo.target <= 1.0);
    }
}

/// An objective bound to no series says so and reads unmeasured however the
/// registry is fed: a series with the archive histogram's name is recorded
/// and the objective still reads unmeasured, because it is not bound to it.
#[test]
fn the_four_unbuilt_objectives_read_unmeasured_and_name_no_series() {
    let metrics = Metrics::new("test");
    metrics.observe_latency_ms(names::DATA_ARCHIVE_DELAY_MINUTES, labels([]), 5.0);
    let unbuilt: BTreeSet<&str> = [
        ARCHIVE_DELAY_OBJECTIVE,
        BIGTABLE_P99_OBJECTIVE,
        KNOWLEDGE_DELTA_FRESHNESS_OBJECTIVE,
        BQ_INGESTION_FRESHNESS_OBJECTIVE,
    ]
    .into();
    let declared = data_plane_objectives();
    for name in &unbuilt {
        let path = declared
            .iter()
            .find(|p| p.slo.name == *name)
            .expect("declared");
        assert_eq!(path.source_series, NO_SOURCE_SERIES);
        assert!(
            matches!(path.rule, SourceRule::Unbuilt { why } if !why.trim().is_empty()),
            "{name} must say what is missing"
        );
        assert!(!status(&metrics, name).is_observed(), "{name}");
    }
}

/// The two built objectives move with their series, and unmeasured without
/// them: deleting the lineage recording site leaves `before` and `after`
/// equal and the observation assertion fails.
#[test]
fn lineage_and_refetch_objectives_read_unmeasured_until_their_series_move() {
    let metrics = Metrics::new("test");
    assert!(!status(&metrics, LINEAGE_COMPLETENESS_OBJECTIVE).is_observed());
    assert!(!status(&metrics, REFETCH_HEALTH_OBJECTIVE).is_observed());

    metrics.increment(
        names::DATA_LINEAGE_RECORDS,
        labels([("outcome", "linked")]),
        19,
    );
    metrics.count(
        names::DATA_LINEAGE_RECORDS,
        labels([("outcome", "unlinked")]),
    );
    metrics.increment(
        names::DATA_REFETCH_CHECKS,
        labels([("outcome", "served")]),
        3,
    );
    metrics.count(
        names::DATA_REFETCH_CHECKS,
        labels([("outcome", "withdrawn")]),
    );

    let lineage = status(&metrics, LINEAGE_COMPLETENESS_OBJECTIVE);
    assert_eq!(lineage.observations, 20);
    assert!((lineage.achieved - 0.95).abs() < 1e-12, "{lineage:?}");
    let refetch = status(&metrics, REFETCH_HEALTH_OBJECTIVE);
    assert_eq!(refetch.observations, 4);
    assert!((refetch.achieved - 0.75).abs() < 1e-12, "{refetch:?}");
    assert!(!refetch.is_met, "a quarter withdrawn cannot meet 99%");
}
