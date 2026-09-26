//! Observability and metrics emission for the event fabric broker.
//! See ADR 0100 § 1 for this module's role.

use qip_observability::metrics::{Labels, Metrics, names};
use std::sync::Arc;

/// Records metrics from the event fabric broker.
///
/// Takes an Arc<Metrics> at construction — never reached for one itself —
/// and holds it for the life of the service. Every method returns `()` and
/// performs no I/O or allocation of unbounded size. Labels are bounded by
/// the stream catalogue, enums, or source literals: never a key, offset,
/// event id or order id.
#[derive(Debug)]
pub struct FabricdTelemetry {
    metrics: Arc<Metrics>,
}

impl FabricdTelemetry {
    /// Construct a recorder and describe every metric it records.
    ///
    /// Describing metrics where the recorder is assembled — the one place
    /// guaranteed to run exactly once — ensures that documentation is
    /// registered before the first series is recorded, and `Metrics::describe`
    /// keeps text by name so it is not lost.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        let m = &metrics;

        m.describe(
            names::EVENT_FABRIC_APPEND,
            "appends to the event fabric log, by stream and outcome (success, duplicate, refused)",
        );
        m.describe(
            names::EVENT_FABRIC_APPEND_LATENCY_MS,
            "milliseconds from append request to completion, by outcome class",
        );
        m.describe(
            names::EVENT_FABRIC_HIGH_WATERMARK,
            "current high-water offset for a stream and partition",
        );
        m.describe(
            names::EVENT_FABRIC_ARCHIVED_THROUGH,
            "highest archived offset for a stream and partition",
        );
        m.describe(
            names::EVENT_FABRIC_DUPLICATES,
            "duplicate append requests detected, by stream",
        );
        m.describe(
            names::EVENT_FABRIC_REFUSALS,
            "appends the broker refused, by class and reason",
        );
        m.describe(
            names::EVENT_FABRIC_SHED,
            "events shed because the ring was full, by class",
        );
        m.describe(
            names::EVENT_FABRIC_FENCED,
            "whether the broker is fenced (1) or accepting appends (0)",
        );
        m.describe(
            names::EVENT_FABRIC_LEADER_EPOCH,
            "the current leader epoch this broker holds",
        );
        m.describe(
            names::EVENT_FABRIC_SEGMENTS_SEALED,
            "log segments the broker has sealed",
        );
        m.describe(
            names::EVENT_FABRIC_ARCHIVE_LAG,
            "segments on disk that have not been archived",
        );
        m.describe(
            names::EVENT_FABRIC_GROUP_LAG,
            "offset lag for a consumer group, stream and partition",
        );

        Self { metrics }
    }

    /// An append was attempted.
    ///
    /// Recorded for every append with `stream`, `class` (the event class),
    /// and `outcome` (success, duplicate, or a refusal reason).
    /// Labels are bounded by the stream catalogue and the enum of outcome
    /// values, never by the content key.
    pub fn append(&self, stream: &str, class: &str, outcome: &str) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("class".to_string(), class.to_string());
        labels.insert("outcome".to_string(), outcome.to_string());
        self.metrics.count(names::EVENT_FABRIC_APPEND, labels);
    }

    /// An append completed or was refused.
    ///
    /// Recorded for every append with the outcome class and the latency
    /// in milliseconds.
    pub fn append_latency_ms(&self, class: &str, millis: f64) {
        let mut labels = Labels::new();
        labels.insert("class".to_string(), class.to_string());
        self.metrics
            .observe_latency_ms(names::EVENT_FABRIC_APPEND_LATENCY_MS, labels, millis);
    }

    /// The high-water offset for a stream and partition.
    ///
    /// A gauge, because it is the current position, not a rate. The stream
    /// is bounded by the catalogue; the partition is a number.
    pub fn high_watermark(&self, stream: &str, partition: u32) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_HIGH_WATERMARK, labels, 0.0);
    }

    /// The highest archived offset for a stream and partition.
    pub fn archived_through(&self, stream: &str, partition: u32) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_ARCHIVED_THROUGH, labels, 0.0);
    }

    /// A duplicate append was detected.
    pub fn duplicate(&self, stream: &str) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        self.metrics.count(names::EVENT_FABRIC_DUPLICATES, labels);
    }

    /// An append was refused.
    ///
    /// Recorded with `class` and `reason` labels, both bounded by enums.
    pub fn refusal(&self, class: &str, reason: &str) {
        let mut labels = Labels::new();
        labels.insert("class".to_string(), class.to_string());
        labels.insert("reason".to_string(), reason.to_string());
        self.metrics.count(names::EVENT_FABRIC_REFUSALS, labels);
    }

    /// Events were shed because the ring was full.
    pub fn shed(&self, class: &str, count: u64) {
        let mut labels = Labels::new();
        labels.insert("class".to_string(), class.to_string());
        self.metrics
            .increment(names::EVENT_FABRIC_SHED, labels, count);
    }

    /// The fenced state of the broker.
    pub fn fenced(&self, is_fenced: bool) {
        let labels = Labels::new();
        self.metrics.gauge(
            names::EVENT_FABRIC_FENCED,
            labels,
            f64::from(u8::from(is_fenced)),
        );
    }

    /// The current leader epoch.
    pub fn leader_epoch(&self, epoch: u64) {
        let labels = Labels::new();
        self.metrics
            .gauge(names::EVENT_FABRIC_LEADER_EPOCH, labels, epoch as f64);
    }

    /// Segments that were sealed.
    pub fn segments_sealed(&self) {
        let labels = Labels::new();
        self.metrics
            .count(names::EVENT_FABRIC_SEGMENTS_SEALED, labels);
    }

    /// Archive lag in segments.
    pub fn archive_lag(&self, lag: u64) {
        let labels = Labels::new();
        self.metrics
            .gauge(names::EVENT_FABRIC_ARCHIVE_LAG, labels, lag as f64);
    }

    /// Consumer group lag for a group, stream and partition.
    pub fn group_lag(&self, group: &str, stream: &str, partition: u32) {
        let mut labels = Labels::new();
        labels.insert("group".to_string(), group.to_string());
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_GROUP_LAG, labels, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_fabricd_recorder_writes_its_named_series_with_bounded_labels() {
        // Assert the premise: the recorder was assembled and holds a metrics
        // registry that can record, not an empty stub.
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let recorder = FabricdTelemetry::new(metrics.clone());
        let snapshot = metrics.snapshot();
        assert!(
            snapshot.series.is_empty(),
            "premise: registry starts empty before any recording"
        );

        // Record an append outcome and verify the series appeared with
        // the expected name and bounded labels: stream (catalogue), class
        // and outcome (enums). Never a key, offset or event id.
        recorder.append("orders", "submitted", "success");
        let snapshot = metrics.snapshot();
        assert!(!snapshot.series.is_empty(), "series moved after recording");

        let found = snapshot
            .series
            .iter()
            .find(|s| s.name == names::EVENT_FABRIC_APPEND)
            .expect("append_total series was recorded");

        // Labels are bounded: stream is from the catalogue, class and outcome
        // are from the enums.
        assert_eq!(
            found.labels.get("stream").map(|s| s.as_str()),
            Some("orders"),
            "stream label is bounded by catalogue"
        );
        assert_eq!(
            found.labels.get("class").map(|s| s.as_str()),
            Some("submitted"),
            "class label is bounded by enum"
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
