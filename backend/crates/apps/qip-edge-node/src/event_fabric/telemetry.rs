//! ADR 0100 §8: the event fabric's own `/metrics` surface, named beside every
//! other process's in the vertical-slice diagram.

use qip_observability::metrics::{Labels, Metrics, names};
use std::sync::Arc;

/// Records metrics for the edge's journal and event fabric connections.
///
/// Takes an Arc<Metrics> at construction — never reached for one itself —
/// and holds it for the life of the node. Every method returns `()` and
/// performs no I/O or allocation of unbounded size. Labels are bounded by
/// enums or source literals: never a key, spool id, or other runtime data.
#[derive(Debug)]
pub struct OutboxTelemetry {
    metrics: Arc<Metrics>,
}

impl OutboxTelemetry {
    /// Construct a recorder and describe every metric it records.
    ///
    /// Describing metrics where the recorder is assembled — the one place
    /// guaranteed to run exactly once — ensures that documentation is
    /// registered before the first series is recorded, and `Metrics::describe`
    /// keeps text by name so it is not lost.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        let m = &metrics;

        m.describe(
            names::EDGE_JOURNAL_RING_DEPTH,
            "depth of the event journal's ring buffer in events",
        );
        m.describe(
            names::EDGE_JOURNAL_SPOOL_BYTES,
            "bytes the event journal has buffered in memory",
        );
        m.describe(
            names::EDGE_JOURNAL_SPOOL_UNARCHIVED_BYTES,
            "bytes the journal has buffered but not yet archived",
        );
        m.describe(
            names::EDGE_JOURNAL_PRESSURE,
            "pressure on the journal buffer, by state: low, medium, high",
        );

        m.describe(
            names::EDGE_EVENT_FABRIC_DRAIN,
            "event batches drained from the fabric, by outcome (success, lost, refused)",
        );
        m.describe(
            names::EDGE_EVENT_FABRIC_CONNECTED,
            "whether the node is connected to the event fabric (1) or disconnected (0)",
        );
        m.describe(
            names::EDGE_EVENT_FABRIC_INPUT_GAPS,
            "detected gaps in the input sequence from the fabric",
        );
        m.describe(
            names::EDGE_EVENT_FABRIC_CONTROL_APPLIED,
            "control events applied from the fabric, by kind",
        );
        m.describe(
            names::EDGE_EVENT_FABRIC_CONTROL_REFUSED,
            "control events refused because they could not be applied, by reason",
        );
        m.describe(
            names::EDGE_EVENT_FABRIC_PASS_DURATION_MS,
            "milliseconds for one pass to drain and apply events",
        );

        Self { metrics }
    }

    /// A batch was drained from the event fabric.
    ///
    /// Recorded with `outcome` bounded by the enum of possible outcomes.
    pub fn drain(&self, outcome: &str) {
        let mut labels = Labels::new();
        labels.insert("outcome".to_string(), outcome.to_string());
        self.metrics.count(names::EDGE_EVENT_FABRIC_DRAIN, labels);
    }

    /// Connection state to the event fabric.
    ///
    /// A gauge written on every pass: `1` when connected, `0` when disconnected.
    pub fn connected(&self, is_connected: bool) {
        let labels = Labels::new();
        self.metrics.gauge(
            names::EDGE_EVENT_FABRIC_CONNECTED,
            labels,
            f64::from(u8::from(is_connected)),
        );
    }

    /// An input gap was detected in the event sequence.
    pub fn input_gap(&self) {
        let labels = Labels::new();
        self.metrics
            .count(names::EDGE_EVENT_FABRIC_INPUT_GAPS, labels);
    }

    /// A control event was applied.
    ///
    /// Recorded with `kind` labelling the type of control applied.
    pub fn control_applied(&self, kind: &str) {
        let mut labels = Labels::new();
        labels.insert("kind".to_string(), kind.to_string());
        self.metrics
            .count(names::EDGE_EVENT_FABRIC_CONTROL_APPLIED, labels);
    }

    /// A control event was refused.
    ///
    /// Recorded with `reason` labelling why it could not be applied.
    pub fn control_refused(&self, reason: &str) {
        let mut labels = Labels::new();
        labels.insert("reason".to_string(), reason.to_string());
        self.metrics
            .count(names::EDGE_EVENT_FABRIC_CONTROL_REFUSED, labels);
    }

    /// A pass completed, draining and applying events.
    ///
    /// Recorded with the duration in milliseconds.
    pub fn pass_duration_ms(&self, millis: f64) {
        let labels = Labels::new();
        self.metrics
            .observe_latency_ms(names::EDGE_EVENT_FABRIC_PASS_DURATION_MS, labels, millis);
    }

    /// Journal ring buffer depth.
    pub fn ring_depth(&self, depth: u64) {
        let labels = Labels::new();
        self.metrics
            .gauge(names::EDGE_JOURNAL_RING_DEPTH, labels, depth as f64);
    }

    /// Journal spool memory usage.
    pub fn spool_bytes(&self, bytes: u64) {
        let labels = Labels::new();
        self.metrics
            .gauge(names::EDGE_JOURNAL_SPOOL_BYTES, labels, bytes as f64);
    }

    /// Journal spool memory that has not been archived.
    pub fn spool_unarchived_bytes(&self, bytes: u64) {
        let labels = Labels::new();
        self.metrics.gauge(
            names::EDGE_JOURNAL_SPOOL_UNARCHIVED_BYTES,
            labels,
            bytes as f64,
        );
    }

    /// Pressure on the journal buffer.
    ///
    /// Recorded with `state` bounded by the enum: low, medium, high.
    pub fn pressure(&self, state: &str) {
        let mut labels = Labels::new();
        labels.insert("state".to_string(), state.to_string());
        self.metrics
            .gauge(names::EDGE_JOURNAL_PRESSURE, labels, 1.0);
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
    /// unbounded cardinality from a key or spool id nobody reviewed, and a
    /// missing one is a fact the series claims to carry and does not.
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
    fn each_outbox_recorder_writes_its_named_series_with_bounded_labels() {
        // Assert the premise: the recorder was assembled and holds a metrics
        // registry that can record, not an empty stub.
        let metrics = Arc::new(Metrics::new("qip-edge-node"));
        let recorder = OutboxTelemetry::new(metrics.clone());
        let snapshot = metrics.snapshot();
        assert!(
            snapshot.series.is_empty(),
            "premise: registry starts empty before any recording"
        );

        // drain_total{outcome} — a counter.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_DRAIN);
        recorder.drain("success");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_DRAIN);
        assert_label_keys(found, &["outcome"]);
        assert_eq!(
            found.labels.get("outcome").map(String::as_str),
            Some("success"),
            "outcome label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for drain_total, got {other:?}"),
        }

        // connected — a gauge, unlabelled: 1 connected, 0 disconnected.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_CONNECTED);
        recorder.connected(true);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_CONNECTED);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 1.0, "connected gauge reads 1 when connected");
                }
            }
            other => panic!("expected Gauge for connected, got {other:?}"),
        }

        // input_gaps_total — a counter, unlabelled.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_INPUT_GAPS);
        recorder.input_gap();
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_INPUT_GAPS);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for input_gaps_total, got {other:?}"),
        }

        // control_applied_total{kind} — a counter.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_CONTROL_APPLIED);
        recorder.control_applied("halt");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_CONTROL_APPLIED);
        assert_label_keys(found, &["kind"]);
        assert_eq!(
            found.labels.get("kind").map(String::as_str),
            Some("halt"),
            "kind label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for control_applied_total, got {other:?}"),
        }

        // control_refused_total{reason} — a counter, distinct from
        // control_applied_total: a control that was refused and one that
        // was applied are opposite facts about the same kind of event.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_CONTROL_REFUSED);
        recorder.control_refused("stale_sequence");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_CONTROL_REFUSED);
        assert_label_keys(found, &["reason"]);
        assert_eq!(
            found.labels.get("reason").map(String::as_str),
            Some("stale_sequence"),
            "reason label is bounded by enum"
        );
        match &found.value {
            MetricValue::Counter(v) => assert_eq!(*v, 1, "counter incremented by one"),
            other => panic!("expected Counter for control_refused_total, got {other:?}"),
        }
        assert_ne!(
            names::EDGE_EVENT_FABRIC_CONTROL_APPLIED,
            names::EDGE_EVENT_FABRIC_CONTROL_REFUSED,
            "control_applied_total and control_refused_total are distinct series"
        );

        // pass_duration_ms — a histogram, unlabelled, one observation.
        assert_absent(&snapshot, names::EDGE_EVENT_FABRIC_PASS_DURATION_MS);
        recorder.pass_duration_ms(3.5);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_EVENT_FABRIC_PASS_DURATION_MS);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Histogram(h) => {
                assert_eq!(h.count, 1, "one duration observation recorded");
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(h.sum, 3.5, "the observation carries the recorded duration");
                }
            }
            other => panic!("expected Histogram for pass_duration_ms, got {other:?}"),
        }

        // ring_depth — a gauge, unlabelled.
        assert_absent(&snapshot, names::EDGE_JOURNAL_RING_DEPTH);
        recorder.ring_depth(42);
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_JOURNAL_RING_DEPTH);
        assert_label_keys(found, &[]);
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 42.0, "ring_depth gauge holds the recorded depth");
                }
            }
            other => panic!("expected Gauge for ring_depth, got {other:?}"),
        }

        // spool_bytes and spool_unarchived_bytes — two distinct gauges
        // tracking different facts, not a mutation of one name.
        assert_absent(&snapshot, names::EDGE_JOURNAL_SPOOL_BYTES);
        recorder.spool_bytes(1024);
        let snapshot = metrics.snapshot();
        let spool_bytes_found = find(&snapshot, names::EDGE_JOURNAL_SPOOL_BYTES);
        assert_label_keys(spool_bytes_found, &[]);
        match &spool_bytes_found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 1024.0, "spool_bytes gauge set to recorded value");
                }
            }
            other => panic!("expected Gauge for spool_bytes, got {other:?}"),
        }

        assert_absent(&snapshot, names::EDGE_JOURNAL_SPOOL_UNARCHIVED_BYTES);
        recorder.spool_unarchived_bytes(512);
        let snapshot = metrics.snapshot();
        let spool_unarchived_found = find(&snapshot, names::EDGE_JOURNAL_SPOOL_UNARCHIVED_BYTES);
        assert_label_keys(spool_unarchived_found, &[]);
        match &spool_unarchived_found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(
                        *v, 512.0,
                        "spool_unarchived_bytes gauge set to recorded value"
                    );
                }
            }
            other => panic!("expected Gauge for spool_unarchived_bytes, got {other:?}"),
        }
        assert_ne!(
            names::EDGE_JOURNAL_SPOOL_BYTES,
            names::EDGE_JOURNAL_SPOOL_UNARCHIVED_BYTES,
            "spool_bytes and spool_unarchived_bytes are distinct series"
        );

        // pressure{state} — a gauge. Mutation: swapping this recorder to
        // write EDGE_EVENT_FABRIC_CONNECTED's name must fail this find(),
        // since connected carries no state label and this series does.
        assert_absent(&snapshot, names::EDGE_JOURNAL_PRESSURE);
        recorder.pressure("high");
        let snapshot = metrics.snapshot();
        let found = find(&snapshot, names::EDGE_JOURNAL_PRESSURE);
        assert_label_keys(found, &["state"]);
        assert_eq!(
            found.labels.get("state").map(String::as_str),
            Some("high"),
            "state label is bounded by enum: low, medium, high"
        );
        match &found.value {
            MetricValue::Gauge(v) => {
                #[allow(clippy::float_cmp)]
                {
                    assert_eq!(*v, 1.0, "pressure gauge holds the recorded value");
                }
            }
            other => panic!("expected Gauge for pressure, got {other:?}"),
        }
    }
}
