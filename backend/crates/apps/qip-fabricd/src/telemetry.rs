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
            "fencing events: the broker lost its epoch and stopped accepting appends",
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
    // Offsets and lags are u64 and a gauge holds an f64: exact to 2^53, past
    // which the gauge rounds. This is the crossing point, stated once here for
    // high_watermark, archived_through and group_lag alike.
    pub fn high_watermark(&self, stream: &str, partition: u32, offset: u64) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_HIGH_WATERMARK, labels, offset as f64);
    }

    /// The highest archived offset for a stream and partition.
    pub fn archived_through(&self, stream: &str, partition: u32, offset: u64) {
        let mut labels = Labels::new();
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_ARCHIVED_THROUGH, labels, offset as f64);
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

    /// A fencing event: the broker lost its epoch and stopped accepting
    /// appends.
    ///
    /// A counter incremented by one per event, never a gauge holding the
    /// current boolean state — a gauge that a broker forgot to reset back to
    /// zero on recovery would read "fenced" forever, and a gauge nobody ever
    /// set back to zero would read "never fenced" through an outage a
    /// counter cannot un-ring. Unlabelled: there is only the one fact.
    pub fn fenced(&self) {
        let labels = Labels::new();
        self.metrics.count(names::EVENT_FABRIC_FENCED, labels);
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
    pub fn group_lag(&self, group: &str, stream: &str, partition: u32, lag: u64) {
        let mut labels = Labels::new();
        labels.insert("group".to_string(), group.to_string());
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_GROUP_LAG, labels, lag as f64);
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
    /// registers help text, never a series — so absence is asserted first,
    /// against the exact name a reader would otherwise have to trust blind.
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
    /// unbounded cardinality nobody reviewed (a key, an offset, an event id),
    /// and a missing one is a fact the constraint list promises the series
    /// carries and it does not.
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

        // append_total{stream,class,outcome} — a counter, bounded by the
        // stream catalogue and two enums. Never a key, offset or event id.
        assert_absent(&snapshot, names::EVENT_FABRIC_APPEND);
        recorder.append("orders", "submitted", "success");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_APPEND);
        assert_label_keys(found, &["stream", "class", "outcome"]);
        assert_eq!(
            found.labels.get("stream").map(String::as_str),
            Some("orders"),
            "stream label is bounded by catalogue"
        );
        assert_eq!(
            found.labels.get("class").map(String::as_str),
            Some("submitted"),
            "class label is bounded by enum"
        );
        assert_eq!(
            found.labels.get("outcome").map(String::as_str),
            Some("success"),
            "outcome label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for append_total, got {other:?}"),
        }

        // append_latency_ms{class} — a histogram of one observation.
        assert_absent(&snapshot, names::EVENT_FABRIC_APPEND_LATENCY_MS);
        recorder.append_latency_ms("submitted", 12.5);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_APPEND_LATENCY_MS);
        assert_label_keys(found, &["class"]);
        assert_eq!(
            found.labels.get("class").map(String::as_str),
            Some("submitted")
        );
        match &found.value {
            MetricValue::Histogram(h) => {
                assert_eq!(h.count, 1, "one latency observation recorded");
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(h.sum, 12.5, "the observation carries the recorded latency");
                }
            }
            other => panic!("expected Histogram for append_latency_ms, got {other:?}"),
        }

        // high_watermark{stream,partition} — a gauge, the current offset.
        assert_absent(&snapshot, names::EVENT_FABRIC_HIGH_WATERMARK);
        recorder.high_watermark("orders", 3, 1_234);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_HIGH_WATERMARK);
        assert_label_keys(found, &["stream", "partition"]);
        assert_eq!(
            found.labels.get("stream").map(String::as_str),
            Some("orders")
        );
        assert_eq!(found.labels.get("partition").map(String::as_str), Some("3"));
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 1_234.0, "high_watermark gauge holds the recorded value");
                }
            }
            other => panic!("expected Gauge for high_watermark, got {other:?}"),
        }

        // archived_through{stream,partition} — a gauge, the highest archived
        // offset. A distinct series from high_watermark, not a relabelling
        // of it: an archiver that fell behind the log's head must be able to
        // disagree with the head, and one series could never say so.
        assert_absent(&snapshot, names::EVENT_FABRIC_ARCHIVED_THROUGH);
        recorder.archived_through("orders", 3, 1_000);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_ARCHIVED_THROUGH);
        assert_label_keys(found, &["stream", "partition"]);
        assert_eq!(
            found.labels.get("stream").map(String::as_str),
            Some("orders")
        );
        assert_eq!(found.labels.get("partition").map(String::as_str), Some("3"));
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(
                        *v, 1_000.0,
                        "archived_through gauge holds the recorded value"
                    );
                }
            }
            other => panic!("expected Gauge for archived_through, got {other:?}"),
        }
        assert_ne!(
            names::EVENT_FABRIC_HIGH_WATERMARK,
            names::EVENT_FABRIC_ARCHIVED_THROUGH,
            "high_watermark and archived_through are distinct series"
        );

        // duplicates_total{stream} — a counter.
        assert_absent(&snapshot, names::EVENT_FABRIC_DUPLICATES);
        recorder.duplicate("orders");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_DUPLICATES);
        assert_label_keys(found, &["stream"]);
        assert_eq!(
            found.labels.get("stream").map(String::as_str),
            Some("orders")
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for duplicates_total, got {other:?}"),
        }

        // refusals_total{class,reason} — a counter.
        assert_absent(&snapshot, names::EVENT_FABRIC_REFUSALS);
        recorder.refusal("submitted", "fenced");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_REFUSALS);
        assert_label_keys(found, &["class", "reason"]);
        assert_eq!(
            found.labels.get("class").map(String::as_str),
            Some("submitted")
        );
        assert_eq!(
            found.labels.get("reason").map(String::as_str),
            Some("fenced")
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for refusals_total, got {other:?}"),
        }

        // shed_total{class} — a counter incremented by the shed count, not
        // always by one: a ring can shed a whole batch in one report.
        assert_absent(&snapshot, names::EVENT_FABRIC_SHED);
        recorder.shed("submitted", 3);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_SHED);
        assert_label_keys(found, &["class"]);
        assert_eq!(
            found.labels.get("class").map(String::as_str),
            Some("submitted")
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 3, "counter incremented by the shed count"),
            other => panic!("expected Counter for shed_total, got {other:?}"),
        }

        // fenced_total — a counter, unlabelled, incremented once per fencing
        // event (the broker lost its epoch and stopped accepting appends).
        // The first attempt at this packet recorded this as a 0/1 gauge
        // named without `_total`: a gauge a recovered broker never resets
        // reads as permanently fenced, and one that does get reset erases
        // the very outage the series exists to keep visible. A counter
        // cannot un-ring that bell, which is the point.
        assert_absent(&snapshot, names::EVENT_FABRIC_FENCED);
        recorder.fenced();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_FENCED);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "one fencing event recorded"),
            other => panic!(
                "expected Counter for fenced_total, got {other:?} — fenced() must not be a gauge"
            ),
        }

        // leader_epoch — a gauge, unlabelled: the current epoch this broker
        // holds.
        assert_absent(&snapshot, names::EVENT_FABRIC_LEADER_EPOCH);
        recorder.leader_epoch(7);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_LEADER_EPOCH);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 7.0, "leader_epoch gauge holds the recorded epoch");
                }
            }
            other => panic!("expected Gauge for leader_epoch, got {other:?}"),
        }

        // segments_sealed_total — a counter, unlabelled.
        assert_absent(&snapshot, names::EVENT_FABRIC_SEGMENTS_SEALED);
        recorder.segments_sealed();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_SEGMENTS_SEALED);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for segments_sealed_total, got {other:?}"),
        }

        // archive_lag_segments — a gauge, unlabelled.
        assert_absent(&snapshot, names::EVENT_FABRIC_ARCHIVE_LAG);
        recorder.archive_lag(4);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_ARCHIVE_LAG);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 4.0, "archive_lag gauge holds the recorded value");
                }
            }
            other => panic!("expected Gauge for archive_lag_segments, got {other:?}"),
        }

        // group_lag{group,stream,partition} — a gauge, distinct from
        // archive_lag_segments: a consumer group's offset lag and the
        // broker's own unarchived-segment count are two different facts, and
        // a recorder that wrote one under the other's name would silently
        // erase whichever fact lost the race to be read.
        assert_absent(&snapshot, names::EVENT_FABRIC_GROUP_LAG);
        recorder.group_lag("consumers", "orders", 3, 42);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_GROUP_LAG);
        assert_label_keys(found, &["group", "stream", "partition"]);
        assert_eq!(
            found.labels.get("group").map(String::as_str),
            Some("consumers")
        );
        assert_eq!(
            found.labels.get("stream").map(String::as_str),
            Some("orders")
        );
        assert_eq!(found.labels.get("partition").map(String::as_str), Some("3"));
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 42.0, "group_lag gauge holds the recorded value");
                }
            }
            other => panic!("expected Gauge for group_lag, got {other:?}"),
        }
    }
}
