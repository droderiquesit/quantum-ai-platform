//! The Cloud Monitoring encoding of a registry snapshot (ADR 0121,
//! R12-OBS-9143): the request bodies `projects.timeSeries.create` accepts.
//!
//! # Why this exists
//!
//! Nothing scrapes a Cloud Run service. The managed-Prometheus sidecar that
//! would have done it is refused by the platform's own vulnerability gate
//! (`.claude/rules/domains/observability.md`), and ADR 0121 replaced the wait
//! for a fixed image with a push from the process itself. This module is the
//! pure half of that push: it turns a [`crate::Snapshot`] into JSON and
//! touches no socket, no clock and no environment, so the library keeps the
//! "no I/O" rule `.claude/rules/architecture/00-boundaries.md` sets for it.
//! The thread, the token and the HTTP call live in `qip-telemetry-drain` and
//! the composition roots.
//!
//! # The shape, and the three places it differs from `/metrics`
//!
//! * **Counters are `CUMULATIVE` `INT64` series with a start time.** Cloud
//!   Monitoring computes a rate from consecutive cumulative points that share
//!   a start time, and treats a new start time as a reset. The exporter
//!   passes the instant it was started, which is the instant every counter in
//!   this process last stood at zero; a restart is a new process, a new start
//!   time and a new task, which is what a reset is.
//! * **Gauges are `GAUGE` `DOUBLE` series with an end time only.** A gauge
//!   point is an instant, and the API refuses a gauge interval whose start
//!   differs from its end.
//! * **Histograms are `CUMULATIVE` `DISTRIBUTION` series** with the
//!   registry's own bounds as `explicitBuckets` and its per-bucket counts —
//!   not the cumulative counts `to_prometheus` renders, because the API's
//!   `bucketCounts` is per bucket. There are `bounds.len() + 1` counts, the
//!   first the underflow bucket and the last the overflow bucket, which is
//!   the layout [`crate::Histogram`] already stores.
//!
//! **One boundary differs and cannot be reconciled from a snapshot.** The
//! registry files an observation equal to a bound in the bucket that bound
//! closes (`value <= bound`, Prometheus's `le`); Cloud Monitoring defines an
//! explicit bucket as closed below and open above (`bounds[i-1] <= x <
//! bounds[i]`). An observation landing exactly on a bound is therefore
//! charted one bucket lower in Cloud Monitoring than the API's own definition
//! says it should be. Counts, totals and the mean are exact; only a quantile
//! read from a distribution whose observations sit exactly on its bounds can
//! move by one bucket. Re-bucketing would need the raw observations, which
//! the registry does not keep.
//!
//! A distribution carries `count`, `mean` and the buckets, and no
//! `sumOfSquaredDeviation`: the registry does not track a second moment, and
//! the field is optional. Writing a zero would assert a variance of zero,
//! which is a number nobody computed.
//!
//! # What is refused rather than sent
//!
//! A series the API would reject is left out of the bodies and named in
//! [`TimeSeriesEncoding::unencodable`], for the exporter to count: a gauge
//! that is not finite (JSON has no NaN, and `serde_json` would write `null`),
//! a histogram whose bounds are not finite and strictly increasing, or whose
//! counts do not number one more than its bounds, or whose mean is not
//! finite, and any cumulative series whose start time does not precede the
//! end time at the millisecond precision the timestamps are written at.
//! One malformed series in a batch would otherwise fail every series beside
//! it.
//!
//! # Labels
//!
//! A series' labels become its metric labels unchanged. Their cardinality is
//! bounded where they are recorded (every label is an enum, a fixed
//! configuration value or a source literal), and that bound is what keeps
//! the per-series ingestion cost bounded; this module adds no label of its
//! own. What tells one process from another is the monitored resource
//! ([`MonitoredTask`]), not a metric label, because a resource label is where
//! Cloud Monitoring expects the writer's identity and a metric label carrying
//! a process id would be a runtime string on every series.

use crate::metrics::{MetricValue, SeriesSnapshot, Snapshot};
use qip_core::Timestamp;
use serde_json::{Value, json};

/// Every pushed metric type is this prefix followed by the registry name,
/// so `qip_cycles_total` is pushed as
/// `custom.googleapis.com/qip/qip_cycles_total`. The alert policies that
/// read a pushed series must name it this way (ADR 0121 decision 3); the
/// Prometheus-domain names the policies name today are written only by a
/// managed-Prometheus collector, which is the component that is refused.
pub const METRIC_TYPE_PREFIX: &str = "custom.googleapis.com/qip/";

/// The most time series one `projects.timeSeries.create` request may carry,
/// as the API documents it. A larger request is refused whole.
pub const MAX_SERIES_PER_REQUEST: usize = 200;

/// The monitored-resource type every series is written against: a task of a
/// job, which is what one replica of a Cloud Run service is. Chosen over
/// `global` because `global` has no label to tell two replicas apart, and two
/// processes writing one cumulative series with different start times is a
/// series the API refuses half of.
pub const RESOURCE_TYPE: &str = "generic_task";

/// The process a series came from, as `generic_task`'s five labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitoredTask {
    /// The project the series are written into.
    pub project_id: String,
    /// The region the process runs in.
    pub location: String,
    /// A grouping above the job; the platform writes one namespace.
    pub namespace: String,
    /// The binary: `qip-api`, `qip-fastbrain`, `qip-deepbrain`.
    pub job: String,
    /// This process, unique among the replicas of `job`.
    pub task_id: String,
}

impl MonitoredTask {
    fn to_json(&self) -> Value {
        json!({
            "type": RESOURCE_TYPE,
            "labels": {
                "project_id": self.project_id,
                "location": self.location,
                "namespace": self.namespace,
                "job": self.job,
                "task_id": self.task_id,
            }
        })
    }
}

/// What one snapshot became.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeSeriesEncoding {
    /// One `TimeSeries` object per encodable series, in the snapshot's order.
    pub time_series: Vec<Value>,
    /// The registry names of series left out because the API would refuse
    /// them, in the snapshot's order. See the module documentation.
    pub unencodable: Vec<String>,
}

impl TimeSeriesEncoding {
    /// The request bodies, each `{"timeSeries": [...]}` holding at most
    /// [`MAX_SERIES_PER_REQUEST`] series. Empty when there is nothing to
    /// send, so an exporter whose registry is empty posts nothing rather than
    /// a body the API refuses for being empty.
    pub fn request_bodies(&self) -> Vec<Value> {
        self.time_series
            .chunks(MAX_SERIES_PER_REQUEST)
            .map(|chunk| json!({ "timeSeries": chunk }))
            .collect()
    }
}

/// Encode a snapshot. The body of [`Snapshot::to_cloud_monitoring_time_series`].
pub(crate) fn encode(
    snapshot: &Snapshot,
    task: &MonitoredTask,
    start: Timestamp,
    now: Timestamp,
) -> TimeSeriesEncoding {
    let resource = task.to_json();
    // Compared at the precision the timestamps are written at: two instants
    // a microsecond apart render as one RFC 3339 string, and the API refuses
    // a cumulative interval whose start is not before its end.
    let interval_is_valid = start.as_millis() < now.as_millis();
    let start_text = start.to_rfc3339();
    let end_text = now.to_rfc3339();
    let mut time_series = Vec::new();
    let mut unencodable = Vec::new();
    for series in &snapshot.series {
        match encode_one(series, &resource, interval_is_valid, &start_text, &end_text) {
            Some(value) => time_series.push(value),
            None => unencodable.push(series.name.clone()),
        }
    }
    TimeSeriesEncoding {
        time_series,
        unencodable,
    }
}

fn encode_one(
    series: &SeriesSnapshot,
    resource: &Value,
    interval_is_valid: bool,
    start: &str,
    end: &str,
) -> Option<Value> {
    let (kind, value_type, interval, value) = match &series.value {
        MetricValue::Counter(total) => {
            if !interval_is_valid {
                return None;
            }
            (
                "CUMULATIVE",
                "INT64",
                json!({ "startTime": start, "endTime": end }),
                // A JSON string, as proto3's JSON mapping writes every int64:
                // a JSON number loses precision above 2^53.
                json!({ "int64Value": total.to_string() }),
            )
        }
        MetricValue::Gauge(level) => {
            if !level.is_finite() {
                return None;
            }
            (
                "GAUGE",
                "DOUBLE",
                json!({ "endTime": end }),
                json!({ "doubleValue": level }),
            )
        }
        MetricValue::Histogram(histogram) => {
            let increasing = histogram.bounds.iter().all(|bound| bound.is_finite())
                && histogram.bounds.windows(2).all(|pair| pair[0] < pair[1]);
            let mean = histogram.mean();
            if !interval_is_valid
                || !increasing
                || histogram.counts.len() != histogram.bounds.len() + 1
                || !mean.is_finite()
            {
                return None;
            }
            let bucket_counts: Vec<String> = histogram.counts.iter().map(u64::to_string).collect();
            (
                "CUMULATIVE",
                "DISTRIBUTION",
                json!({ "startTime": start, "endTime": end }),
                json!({
                    "distributionValue": {
                        "count": histogram.count.to_string(),
                        "mean": mean,
                        "bucketOptions": {
                            "explicitBuckets": { "bounds": histogram.bounds }
                        },
                        "bucketCounts": bucket_counts,
                    }
                }),
            )
        }
    };
    Some(json!({
        "metric": {
            "type": format!("{METRIC_TYPE_PREFIX}{}", series.name),
            "labels": series.labels,
        },
        "resource": resource,
        "metricKind": kind,
        "valueType": value_type,
        "points": [{ "interval": interval, "value": value }],
    }))
}
