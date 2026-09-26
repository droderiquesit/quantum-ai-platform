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

        // Record a commit and verify the series appeared with the expected name
        // and bounded label: outcome (enum). Never a partition key, strategy id
        // or order id.
        recorder.commit("success");
        let snapshot = metrics.snapshot();
        assert!(!snapshot.series.is_empty(), "series moved after recording");

        let found = snapshot
            .series
            .iter()
            .find(|s| s.name == names::LEDGER_COMMITS)
            .expect("commits_total series was recorded");

        // The outcome label is bounded by the enum of possible outcomes,
        // not by a partition key or order id from the commit content. Only
        // one label, never unbounded cardinality from keys.
        assert_eq!(
            found.labels.len(),
            1,
            "only one label (outcome), never partition keys or ids"
        );
        assert_eq!(
            found.labels.get("outcome").map(|s| s.as_str()),
            Some("success"),
            "outcome label is bounded by enum"
        );

        // The value is a counter (monotonic).
        match &found.value {
            qip_observability::metrics::MetricValue::Counter(v) => {
                assert_eq!(*v, 1, "counter incremented by one");
            }
            other => panic!("expected Counter, got {:?}", other),
        }
    }
}
