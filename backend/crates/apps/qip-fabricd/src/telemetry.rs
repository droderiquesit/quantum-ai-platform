//! Observability and metrics emission for the event fabric broker.
//! See ADR 0100 § 1 for this module's role.

use qip_observability::critical_paths;
use qip_observability::metrics::{Labels, Metrics, labels, names};
use qip_observability::slo::FABRIC_ARCHIVE_BACKLOG;
use qip_streaming::event_fabric::broker::LeaseReport;
use qip_streaming::event_fabric::managed::CommitOutcome;
use std::sync::Arc;

/// Managed partitions by this instance's role in each (ADR 0111): `leading`,
/// `following` or `failed`. Named here rather than in `qip-observability`'s
/// registry because this binary is its only writer; the label is a source
/// literal from three.
pub const MANAGED_PARTITIONS: &str = "qip_fabricd_managed_partitions";
/// Segments whose local copy was deleted once archived and indexed, so their
/// history is now read back from the archive (FABRIC-8079).
pub const SEGMENTS_RETAINED: &str = "qip_fabricd_segments_retained_total";

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

/// The outcome label a managed commit's answer is timed under: one literal
/// per arm of `CommitOutcome`, and `error` for an unknown outcome, so the
/// series' cardinality is the port's own closed set.
pub const fn quorum_outcome(answer: &qip_core::error::Result<CommitOutcome>) -> &'static str {
    match answer {
        Ok(CommitOutcome::Committed) => "committed",
        Ok(CommitOutcome::Fenced { .. }) => "fenced",
        Ok(CommitOutcome::Conflict { .. }) => "conflict",
        Err(_) => "error",
    }
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
            names::EVENT_FABRIC_SERVING,
            "whether the protocol listener is accepting requests (1=yes, 0=no)",
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
        m.describe(
            MANAGED_PARTITIONS,
            "managed (P0/P1) partitions by this instance's role: leading, following, failed",
        );
        m.describe(
            SEGMENTS_RETAINED,
            "archived segments whose local copy was deleted; their history hydrates from the archive",
        );

        m.describe(
            names::EVENT_FABRIC_ARCHIVE_BACKLOG_CHECKS,
            "archive-lag readings judged against the fabric-archive-backlog ceiling, by outcome",
        );
        m.describe(
            names::EVENT_FABRIC_GROUP_LAG_CHECKS,
            "consumer-group lag readings judged against the fabric-consumer-lag ceiling, by outcome",
        );
        m.describe(
            names::EVENT_FABRIC_SERVING_CHECKS,
            "readings of the serving gauge taken at each objective evaluation: serving, not_serving",
        );
        m.describe(
            names::EVENT_FABRIC_QUORUM_ACK_MS,
            "milliseconds the managed log took to answer a P0/P1 commit (the quorum \
             acknowledgement), by class and outcome: committed, fenced, conflict, error",
        );

        Self { metrics }
    }

    /// The managed log answered one P0/P1 commit after `millis`: the quorum
    /// acknowledgement's latency (ADR 0111 Amendment 1), recorded at the seam
    /// where the answer arrives. `class` is the stream's QoS class from the
    /// catalogue and `outcome` one of [`quorum_outcome`]'s literals; never a
    /// stream's key, offset or epoch.
    pub fn quorum_ack_ms(&self, class: &str, outcome: &str, millis: f64) {
        self.metrics.observe_latency_ms(
            names::EVENT_FABRIC_QUORUM_ACK_MS,
            labels([("class", class), ("outcome", outcome)]),
            millis,
        );
    }

    /// Count one reading of a backlog gauge as `within` or `over` the
    /// ceiling `fabric_objectives` declares for `objective`. The ceiling is
    /// read from the declaration, not restated, so the recorder and the
    /// objective cannot carry two figures; an objective missing from it
    /// counts `over`, because a reading nobody can judge is not evidence of
    /// health.
    fn check(&self, series: &str, objective: &str, reading: f64) {
        let within = critical_paths::fabric_objectives()
            .iter()
            .find(|path| path.slo.name == objective)
            .and_then(|path| path.slo.backlog_ceiling)
            .is_some_and(|ceiling| reading <= ceiling);
        let outcome = if within { "within" } else { "over" };
        self.metrics.count(series, labels([("outcome", outcome)]));
    }

    /// Evaluate every fabric objective (R12-OBS-9160) against this
    /// process's registry and write each as the `qip_slo_*{objective}`
    /// gauges the other processes publish.
    ///
    /// First takes one reading of broker availability: the listener's
    /// `serving` gauge, as the listener last wrote it, counted `serving` or
    /// `not_serving`. A registry with no `serving` gauge — a listener that
    /// never recorded one — takes no reading, so the objective reads
    /// unmeasured rather than counting an absent gauge either way.
    pub fn evaluate_objectives(&self) {
        let serving = self
            .metrics
            .snapshot()
            .gauge(names::EVENT_FABRIC_SERVING, &Labels::new());
        if let Some(value) = serving {
            let outcome = if value > 0.0 {
                "serving"
            } else {
                "not_serving"
            };
            self.metrics.count(
                names::EVENT_FABRIC_SERVING_CHECKS,
                labels([("outcome", outcome)]),
            );
        }
        let snapshot = self.metrics.snapshot();
        let statuses: Vec<_> = critical_paths::fabric_objectives()
            .iter()
            .map(|path| {
                let (good, total) = path.counts(&snapshot);
                path.evaluate(good, total)
            })
            .collect();
        critical_paths::record(&self.metrics, &statuses);
    }

    /// One lease pass: how many managed partitions this instance leads,
    /// follows, and could not tend. Written every pass, so a partition that
    /// moves to another instance moves on the chart.
    pub fn leases(&self, report: &LeaseReport) {
        for (role, count) in [
            ("leading", report.leading.len()),
            ("following", report.following.len()),
            ("failed", report.failed.len()),
        ] {
            let mut labels = Labels::new();
            labels.insert("role".to_string(), role.to_string());
            self.metrics.gauge(MANAGED_PARTITIONS, labels, count as f64);
        }
    }

    /// `count` segments' local copies were retained away.
    pub fn segments_retained(&self, count: u64) {
        self.metrics
            .increment(SEGMENTS_RETAINED, Labels::new(), count);
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

    /// Whether the protocol listener is accepting requests.
    ///
    /// A gauge: 1 when serving, 0 when not. Updates whenever the protocol
    /// listener's state changes, so the metric is observable for liveness
    /// independent of fabric topics.
    pub fn serving(&self, accepting: bool) {
        let labels = Labels::new();
        self.metrics.gauge(
            names::EVENT_FABRIC_SERVING,
            labels,
            if accepting { 1.0 } else { 0.0 },
        );
    }

    /// Segments that were sealed.
    pub fn segments_sealed(&self) {
        let labels = Labels::new();
        self.metrics
            .count(names::EVENT_FABRIC_SEGMENTS_SEALED, labels);
    }

    /// Archive lag in segments, and every fabric objective evaluated over
    /// every reading so far.
    ///
    /// The archive-backlog objective was declared with a ceiling and a
    /// target and evaluated by nothing, so a backlog that grew past its
    /// ceiling moved a gauge and missed no objective anywhere. Each reading
    /// is now judged against the ceiling and counted. This is called once
    /// per archiver pass whatever the pass found, which makes it the
    /// fabric's evaluation clock: the other six objectives are evaluated
    /// here too (R12-OBS-9160), so each reaches `/metrics` on the default
    /// path without a loop of its own.
    pub fn archive_lag(&self, lag: u64) {
        self.metrics
            .gauge(names::EVENT_FABRIC_ARCHIVE_LAG, Labels::new(), lag as f64);
        // A segment count to the ceiling's `f64`: exact for any count a disk
        // can hold.
        self.check(
            names::EVENT_FABRIC_ARCHIVE_BACKLOG_CHECKS,
            FABRIC_ARCHIVE_BACKLOG,
            lag as f64,
        );
        self.evaluate_objectives();
    }

    /// Consumer group lag for a group, stream and partition, and the reading
    /// judged against the `fabric-consumer-lag` ceiling: the gauge keeps
    /// only the last value per partition, and an objective needs every one.
    pub fn group_lag(&self, group: &str, stream: &str, partition: u32, lag: u64) {
        let mut labels = Labels::new();
        labels.insert("group".to_string(), group.to_string());
        labels.insert("stream".to_string(), stream.to_string());
        labels.insert("partition".to_string(), partition.to_string());
        self.metrics
            .gauge(names::EVENT_FABRIC_GROUP_LAG, labels, lag as f64);
        self.check(
            names::EVENT_FABRIC_GROUP_LAG_CHECKS,
            critical_paths::FABRIC_CONSUMER_LAG_OBJECTIVE,
            lag as f64,
        );
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

        // serving — a gauge, unlabelled: 1 when the broker is accepting
        // requests, 0 when not. Not a counter (a 0/1 gauge that never gets
        // reset would read "not serving" forever after a recovery), but a
        // gauge that the broker sets at startup and when the listener stops.
        assert_absent(&snapshot, names::EVENT_FABRIC_SERVING);
        recorder.serving(true);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_SERVING);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 1.0, "serving=true gauge is 1.0");
                }
            }
            other => panic!("expected Gauge for serving, got {other:?}"),
        }
        recorder.serving(false);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_SERVING);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 0.0, "serving=false gauge is 0.0");
                }
            }
            other => panic!("expected Gauge for serving, got {other:?}"),
        }
    }
    /// R12-OBS-9157: the `fabric-archive-backlog` objective is evaluated
    /// where the lag is measured. Two readings within the ceiling of two and
    /// one of five is two good observations in three, below the 0.99 target,
    /// so the objective reads measured and missed: three observations and a
    /// budget consumed past one. Before any reading it is absent, not met.
    ///
    /// Mutation: in `archive_lag`, drop the count under
    /// `EVENT_FABRIC_ARCHIVE_BACKLOG_CHECKS` — the evaluation reads zero
    /// observations and the measured assertion fires.
    #[test]
    fn a_backlog_past_its_ceiling_marks_the_archive_backlog_objective_missed() {
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let recorder = FabricdTelemetry::new(metrics.clone());
        let objective = labels([("objective", "fabric-archive-backlog")]);
        assert_eq!(
            metrics
                .snapshot()
                .gauge(names::SLO_OBSERVATIONS, &objective),
            None,
            "premise: no reading, no evaluation"
        );

        recorder.archive_lag(0);
        recorder.archive_lag(2);
        let snapshot = metrics.snapshot();
        assert_eq!(
            snapshot.gauge(names::SLO_OBSERVATIONS, &objective),
            Some(2.0)
        );
        assert_eq!(
            snapshot.gauge(names::SLO_BUDGET_CONSUMED, &objective),
            Some(0.0),
            "two readings at or under the ceiling consumed budget"
        );

        recorder.archive_lag(5);
        let snapshot = metrics.snapshot();
        assert_eq!(
            snapshot.gauge(names::SLO_OBSERVATIONS, &objective),
            Some(3.0),
            "the lagging reading was not evaluated"
        );
        let consumed = snapshot
            .gauge(names::SLO_BUDGET_CONSUMED, &objective)
            .expect("a measured objective carries its budget");
        assert!(
            consumed > 1.0,
            "one reading in three over the ceiling should miss a 0.99 target: {consumed}"
        );
        assert_eq!(
            snapshot.counter(
                names::EVENT_FABRIC_ARCHIVE_BACKLOG_CHECKS,
                &labels([("outcome", "over")])
            ),
            1
        );
    }

    /// The observations and budget gauges `critical_paths::record` wrote for
    /// `objective`.
    fn published(metrics: &Metrics, objective: &str) -> (Option<f64>, Option<f64>) {
        let snapshot = metrics.snapshot();
        let objective = labels([("objective", objective)]);
        (
            snapshot.gauge(names::SLO_OBSERVATIONS, &objective),
            snapshot.gauge(names::SLO_BUDGET_CONSUMED, &objective),
        )
    }

    /// R12-OBS-9160 in the process that measures the fabric: one archiver
    /// pass publishes all seven objectives on this registry - the five
    /// measurable ones observed once their recorders have run, the two
    /// unbuilt ones with zero observations and no budget.
    ///
    /// Mutation: in `group_lag`, delete the `self.check(
    /// names::EVENT_FABRIC_GROUP_LAG_CHECKS, ..)` call - `fabric-consumer-lag`
    /// has no observation and its assertion fires. Mutation: in
    /// `evaluate_objectives`, delete the `EVENT_FABRIC_SERVING_CHECKS` count -
    /// `fabric-broker-availability` has none and its assertion fires.
    #[test]
    fn an_archive_pass_publishes_every_fabric_objective_from_the_series_this_process_records() {
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let recorder = FabricdTelemetry::new(metrics.clone());
        recorder.serving(true);
        recorder.group_lag("ledger", "outcomes", 0, 5);
        recorder.append("outcomes", "p1_outcomes", "success");
        recorder.append("outcomes", "p1_outcomes", "refused");
        recorder.refusal("p1_outcomes", "schema_refused");
        recorder.quorum_ack_ms("p1_outcomes", "committed", 3.0);
        assert_eq!(
            published(&metrics, critical_paths::FABRIC_CONSUMER_LAG_OBJECTIVE),
            (None, None),
            "premise: nothing is evaluated before an archiver pass"
        );

        recorder.archive_lag(0);
        for objective in [
            critical_paths::FABRIC_PUBLISH_QUORUM_OBJECTIVE,
            critical_paths::FABRIC_BROKER_AVAILABILITY_OBJECTIVE,
            critical_paths::FABRIC_CONSUMER_LAG_OBJECTIVE,
            FABRIC_ARCHIVE_BACKLOG,
            critical_paths::FABRIC_SCHEMA_REJECT_OBJECTIVE,
        ] {
            let (observations, budget) = published(&metrics, objective);
            assert!(
                observations.is_some_and(|n| n >= 1.0),
                "{objective} has no observation: {observations:?}"
            );
            assert!(budget.is_some(), "{objective} published no budget");
        }
        for objective in [
            critical_paths::FABRIC_UNDER_REPLICATION_OBJECTIVE,
            critical_paths::FABRIC_MIRROR_LAG_OBJECTIVE,
        ] {
            assert_eq!(
                published(&metrics, objective),
                (Some(0.0), None),
                "{objective} must publish as unmeasured"
            );
        }
        let schema = published(&metrics, critical_paths::FABRIC_SCHEMA_REJECT_OBJECTIVE);
        assert!(
            schema.1.is_some_and(|b| b > 1.0),
            "one schema refusal in two produces misses a 99.9% target: {schema:?}"
        );
    }

    /// The other half of the check: a registry whose recording sites did
    /// not run (no `serving` gauge, no group-lag reading, no managed commit,
    /// no produce) publishes those objectives unmeasured on the same pass,
    /// never with a budget a dashboard would read as healthy.
    ///
    /// Mutation: in `evaluate_objectives`, count a `serving` reading when
    /// the gauge is absent (`.or(Some(1.0))`) - availability is observed
    /// with no listener having written anything and the assertion fires.
    #[test]
    fn an_objective_whose_recording_site_never_ran_publishes_as_unmeasured() {
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let recorder = FabricdTelemetry::new(metrics.clone());
        recorder.archive_lag(0);
        assert!(
            published(&metrics, FABRIC_ARCHIVE_BACKLOG)
                .0
                .is_some_and(|n| n >= 1.0),
            "premise: the pass evaluated, and measured what it recorded"
        );
        for objective in [
            critical_paths::FABRIC_PUBLISH_QUORUM_OBJECTIVE,
            critical_paths::FABRIC_BROKER_AVAILABILITY_OBJECTIVE,
            critical_paths::FABRIC_CONSUMER_LAG_OBJECTIVE,
            critical_paths::FABRIC_SCHEMA_REJECT_OBJECTIVE,
        ] {
            assert_eq!(
                published(&metrics, objective),
                (Some(0.0), None),
                "{objective} reads measured with its recording site never run"
            );
        }
    }

    /// R12-OBS-9146's recorder: the quorum-acknowledgement histogram carries
    /// exactly `class` and `outcome`, never a stream, key or offset.
    ///
    /// Mutation: in `quorum_ack_ms`, add a third label - the label key set
    /// assertion fires.
    #[test]
    fn the_quorum_ack_series_is_a_histogram_labelled_by_class_and_outcome_only() {
        let metrics = Arc::new(Metrics::new("qip-fabricd"));
        let recorder = FabricdTelemetry::new(metrics.clone());
        assert_absent(&metrics.snapshot(), names::EVENT_FABRIC_QUORUM_ACK_MS);
        recorder.quorum_ack_ms("p0_control", "fenced", 42.0);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EVENT_FABRIC_QUORUM_ACK_MS);
        assert_label_keys(found, &["class", "outcome"]);
        assert_eq!(
            found.labels.get("outcome").map(String::as_str),
            Some("fenced")
        );
        match &found.value {
            MetricValue::Histogram(h) => {
                assert_eq!(h.count, 1);
                assert!((h.sum - 42.0).abs() < 1e-9, "{}", h.sum);
            }
            other => panic!("expected a histogram, got {other:?}"),
        }
    }
}
