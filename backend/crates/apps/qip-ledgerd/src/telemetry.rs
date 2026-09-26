//! Observability and metrics emission for the ledger.
//! See ADR 0100 § 1 for this module's role.

use qip_observability::metrics::{Labels, Metrics, names};
use std::sync::Arc;

/// Records metrics from the ledger service.
///
/// Takes an Arc<Metrics> at construction — never reached for one itself —
/// and holds it for the life of the service. Every method returns `()` and
/// performs no I/O or allocation of unbounded size. Labels are bounded by
/// enums or source literals: never a partition key, strategy id, order id
/// or other runtime data.
#[derive(Debug)]
pub struct LedgerTelemetry {
    metrics: Arc<Metrics>,
}

impl LedgerTelemetry {
    /// Construct a recorder and describe every metric it records.
    ///
    /// Describing metrics where the recorder is assembled — the one place
    /// guaranteed to run exactly once — ensures that documentation is
    /// registered before the first series is recorded, and `Metrics::describe`
    /// keeps text by name so it is not lost.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        let m = &metrics;

        m.describe(
            names::LEDGER_COMMITS,
            "commits applied to the ledger, by outcome (success, conflict, duplicate, refused)",
        );
        m.describe(
            names::LEDGER_COMMIT_LATENCY_MS,
            "milliseconds from commit request to completion",
        );
        m.describe(
            names::LEDGER_DUPLICATES,
            "duplicate commit requests detected, by kind of duplicate",
        );
        m.describe(
            names::LEDGER_PARKED_KEYS,
            "partition keys waiting for contention to clear",
        );
        m.describe(
            names::LEDGER_LIVE_FILL_REFUSED,
            "commits refused because they would record a live fill",
        );
        m.describe(
            names::LEDGER_UNBALANCED_REFUSED,
            "commits refused because they would unbalance a ledger partition",
        );
        m.describe(
            names::LEDGER_LAG,
            "unapplied records in a partition, by partition",
        );
        m.describe(
            names::LEDGER_STORE_RETRIES,
            "store operations that had to be retried",
        );

        Self { metrics }
    }

    /// A commit was attempted.
    ///
    /// Recorded for every commit with the outcome. Labels are bounded by
    /// the enum of outcome values: success, conflict, duplicate, or refused.
    pub fn commit(&self, outcome: &str) {
        let mut labels = Labels::new();
        labels.insert("outcome".to_string(), outcome.to_string());
        self.metrics.count(names::LEDGER_COMMITS, labels);
    }

    /// A commit completed or was refused.
    ///
    /// Recorded for every commit with the latency in milliseconds.
    pub fn commit_latency_ms(&self, millis: f64) {
        let labels = Labels::new();
        self.metrics
            .observe_latency_ms(names::LEDGER_COMMIT_LATENCY_MS, labels, millis);
    }

    /// A duplicate commit was detected.
    ///
    /// Recorded with `kind` labelling the type of duplication detected.
    pub fn duplicate(&self, kind: &str) {
        let mut labels = Labels::new();
        labels.insert("kind".to_string(), kind.to_string());
        self.metrics.count(names::LEDGER_DUPLICATES, labels);
    }

    /// Partition keys currently waiting for contention to clear.
    ///
    /// A gauge, because it is the current count, not a rate of changes.
    pub fn parked_keys(&self, count: u64) {
        let labels = Labels::new();
        self.metrics
            .gauge(names::LEDGER_PARKED_KEYS, labels, count as f64);
    }

    /// A commit was refused because it would record a live fill.
    pub fn live_fill_refused(&self) {
        let labels = Labels::new();
        self.metrics.count(names::LEDGER_LIVE_FILL_REFUSED, labels);
    }

    /// A commit was refused because it would unbalance a ledger partition.
    pub fn unbalanced_refused(&self) {
        let labels = Labels::new();
        self.metrics.count(names::LEDGER_UNBALANCED_REFUSED, labels);
    }

    /// Records in a partition waiting to be applied.
    ///
    /// Recorded with the partition label. The partition is a bounded identifier.
    pub fn lag(&self, partition: &str) {
        let mut labels = Labels::new();
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics.gauge(names::LEDGER_LAG, labels, 0.0);
    }

    /// A store operation had to be retried.
    pub fn store_retry(&self) {
        let labels = Labels::new();
        self.metrics.count(names::LEDGER_STORE_RETRIES, labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_observability::metrics::{MetricValue, SeriesSnapshot, Snapshot};
    use std::collections::BTreeSet;

    /// A series must not exist before its recorder has been called. Checking
    /// only presence after recording would pass identically if the series
    /// had somehow been there from construction — `Metrics::describe`
    /// registers help text, never a series.
    fn assert_absent(snapshot: &Snapshot, name: &str) {
        assert!(
            !snapshot.series.iter().any(|s| s.name == name),
            "premise: {name} must be absent before its recorder is called"
        );
    }

    fn find<'a>(snapshot: &'a Snapshot, name: &str) -> &'a SeriesSnapshot {
        snapshot
            .series
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} series was not recorded"))
    }

    /// Set equality on label keys, not subset or superset: an extra label is
    /// unbounded cardinality from a partition key, strategy id or order id
    /// nobody reviewed, and a missing one is a fact the series claims to
    /// carry and does not.
    fn assert_label_keys(found: &SeriesSnapshot, expected: &[&str]) {
        let actual: BTreeSet<&str> = found.labels.keys().map(String::as_str).collect();
        let expected: BTreeSet<&str> = expected.iter().copied().collect();
        assert_eq!(
            actual, expected,
            "{}'s label key set must match exactly",
            found.name
        );
    }

    #[test]
    fn each_ledgerd_recorder_writes_its_named_series_with_bounded_labels() {
        // Assert the premise: the recorder was assembled and holds a metrics
        // registry that can record, not an empty stub.
        let metrics = Arc::new(Metrics::new("qip-ledgerd"));
        let recorder = LedgerTelemetry::new(metrics.clone());
        let snapshot = metrics.snapshot();
        assert!(
            snapshot.series.is_empty(),
            "premise: registry starts empty before any recording"
        );

        // commits_total{outcome} — a counter. The outcome label is bounded by
        // the enum of possible outcomes, never a partition key, strategy id
        // or order id from the commit content.
        assert_absent(&snapshot, names::LEDGER_COMMITS);
        recorder.commit("success");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_COMMITS);
        assert_label_keys(found, &["outcome"]);
        assert_eq!(
            found.labels.get("outcome").map(String::as_str),
            Some("success"),
            "outcome label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for commits_total, got {other:?}"),
        }

        // commit_latency_ms — a histogram, unlabelled, one observation.
        assert_absent(&snapshot, names::LEDGER_COMMIT_LATENCY_MS);
        recorder.commit_latency_ms(8.25);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_COMMIT_LATENCY_MS);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Histogram(h) => {
                assert_eq!(h.count, 1, "one latency observation recorded");
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(h.sum, 8.25, "the observation carries the recorded latency");
                }
            }
            other => panic!("expected Histogram for commit_latency_ms, got {other:?}"),
        }

        // duplicates_total{kind} — a counter.
        assert_absent(&snapshot, names::LEDGER_DUPLICATES);
        recorder.duplicate("replay");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_DUPLICATES);
        assert_label_keys(found, &["kind"]);
        assert_eq!(
            found.labels.get("kind").map(String::as_str),
            Some("replay"),
            "kind label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for duplicates_total, got {other:?}"),
        }

        // parked_keys — a gauge, unlabelled: the current count, not a rate.
        assert_absent(&snapshot, names::LEDGER_PARKED_KEYS);
        recorder.parked_keys(5);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_PARKED_KEYS);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 5.0, "parked_keys gauge holds the recorded count");
                }
            }
            other => panic!("expected Gauge for parked_keys, got {other:?}"),
        }

        // live_fill_refused_total — a counter, unlabelled: the paper-trading
        // boundary refusing a commit that would record a live fill.
        assert_absent(&snapshot, names::LEDGER_LIVE_FILL_REFUSED);
        recorder.live_fill_refused();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_LIVE_FILL_REFUSED);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for live_fill_refused_total, got {other:?}"),
        }

        // unbalanced_refused_total — a counter, unlabelled, distinct from
        // live_fill_refused_total: the two refuse for different reasons and
        // an operator investigating one must not be reading the other's
        // count.
        assert_absent(&snapshot, names::LEDGER_UNBALANCED_REFUSED);
        recorder.unbalanced_refused();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_UNBALANCED_REFUSED);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for unbalanced_refused_total, got {other:?}"),
        }
        assert_ne!(
            names::LEDGER_LIVE_FILL_REFUSED,
            names::LEDGER_UNBALANCED_REFUSED,
            "live_fill_refused_total and unbalanced_refused_total are distinct series"
        );

        // lag_records{partition} — a gauge. Mutation: swapping this
        // recorder to write LEDGER_PARKED_KEYS's name (its neighbour above)
        // must fail this find(), since parked_keys carries no partition
        // label and this series does.
        assert_absent(&snapshot, names::LEDGER_LAG);
        recorder.lag("p3");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_LAG);
        assert_label_keys(found, &["partition"]);
        assert_eq!(
            found.labels.get("partition").map(String::as_str),
            Some("p3"),
            "partition is a bounded identifier"
        );
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 0.0, "lag_records gauge holds the recorded value");
                }
            }
            other => panic!("expected Gauge for lag_records, got {other:?}"),
        }

        // store_retries_total — a counter, unlabelled.
        assert_absent(&snapshot, names::LEDGER_STORE_RETRIES);
        recorder.store_retry();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::LEDGER_STORE_RETRIES);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for store_retries_total, got {other:?}"),
        }
    }
}
