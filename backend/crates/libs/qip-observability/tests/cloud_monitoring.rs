//! The Cloud Monitoring encoding of a registry snapshot (ADR 0121,
//! R12-OBS-9143), asserted as whole JSON objects.
//!
//! Whole objects rather than a field here and there, because the failure
//! this encoding can have is a series the API accepts as the wrong kind: a
//! counter written as a `GAUGE` is ingested without complaint, charts as a
//! line that only rises, and turns every `rate()` an alert policy takes of it
//! into nonsense. Nothing downstream refuses that; only an assertion on
//! `metricKind` does.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::Timestamp;
use qip_observability::cloud_monitoring::{MAX_SERIES_PER_REQUEST, MonitoredTask};
use qip_observability::metrics::{Histogram, labels, names};
use qip_observability::{Metrics, Snapshot};
use serde_json::{Value, json};

fn task() -> MonitoredTask {
    MonitoredTask {
        project_id: "qip-dev".to_string(),
        location: "europe-west2".to_string(),
        namespace: "qip".to_string(),
        job: "qip-api".to_string(),
        task_id: "4242-1790000000000".to_string(),
    }
}

fn resource() -> Value {
    json!({
        "type": "generic_task",
        "labels": {
            "project_id": "qip-dev",
            "location": "europe-west2",
            "namespace": "qip",
            "job": "qip-api",
            "task_id": "4242-1790000000000",
        }
    })
}

const START: Timestamp = Timestamp::from_secs(1_790_000_000);
const NOW: Timestamp = Timestamp::from_secs(1_790_000_060);

fn two_bounds() -> Histogram {
    Histogram::with_bounds(vec![1.0, 10.0])
}

fn find<'a>(series: &'a [Value], metric_type: &str) -> &'a Value {
    series
        .iter()
        .find(|s| s["metric"]["type"] == metric_type)
        .unwrap_or_else(|| panic!("{metric_type} is missing from {series:#?}"))
}

#[test]
fn a_pushed_snapshot_encodes_counters_as_cumulative_series_and_histograms_as_distributions() {
    let metrics = Metrics::new("qip-api");
    let venue = labels([("venue", "XNYS")]);
    metrics.increment(names::ORDERS_FILLED, venue.clone(), 1);
    metrics.increment(names::ORDERS_FILLED, venue, 4);
    metrics.gauge(names::KILL_SWITCH_TRIPPED, labels([]), 1.0);
    for value in [0.5, 5.0, 50.0] {
        metrics.observe_with(
            names::EXECUTION_LATENCY_MS,
            labels([("venue", "XNYS")]),
            value,
            two_bounds,
        );
    }
    let encoding = metrics
        .snapshot()
        .to_cloud_monitoring_time_series(&task(), START, NOW);

    // Premise: three series were recorded and none was refused, so a missing
    // object below is the encoder's fault and not an empty registry's.
    assert_eq!(encoding.unencodable, Vec::<String>::new());
    assert_eq!(encoding.time_series.len(), 3);

    // The counter: CUMULATIVE, INT64, the accumulated total (1 + 4) as a
    // string, over an interval that starts at the exporter's start.
    assert_eq!(
        find(
            &encoding.time_series,
            "custom.googleapis.com/qip/qip_orders_filled_total"
        ),
        &json!({
            "metric": {
                "type": "custom.googleapis.com/qip/qip_orders_filled_total",
                "labels": { "venue": "XNYS" },
            },
            "resource": resource(),
            "metricKind": "CUMULATIVE",
            "valueType": "INT64",
            "points": [{
                "interval": {
                    "startTime": "2026-09-21T14:13:20.000Z",
                    "endTime": "2026-09-21T14:14:20.000Z",
                },
                "value": { "int64Value": "5" },
            }],
        })
    );

    // The gauge: an instant, so no start time — the API refuses a gauge
    // interval whose start differs from its end.
    assert_eq!(
        find(
            &encoding.time_series,
            "custom.googleapis.com/qip/qip_kill_switch_tripped"
        ),
        &json!({
            "metric": {
                "type": "custom.googleapis.com/qip/qip_kill_switch_tripped",
                "labels": {},
            },
            "resource": resource(),
            "metricKind": "GAUGE",
            "valueType": "DOUBLE",
            "points": [{
                "interval": { "endTime": "2026-09-21T14:14:20.000Z" },
                "value": { "doubleValue": 1.0 },
            }],
        })
    );

    // The histogram: a CUMULATIVE distribution with the registry's own
    // bounds and per-bucket (not cumulative) counts — underflow, one inner
    // bucket, overflow — and the mean (0.5 + 5 + 50) / 3.
    assert_eq!(
        find(
            &encoding.time_series,
            "custom.googleapis.com/qip/qip_execution_latency_milliseconds"
        ),
        &json!({
            "metric": {
                "type": "custom.googleapis.com/qip/qip_execution_latency_milliseconds",
                "labels": { "venue": "XNYS" },
            },
            "resource": resource(),
            "metricKind": "CUMULATIVE",
            "valueType": "DISTRIBUTION",
            "points": [{
                "interval": {
                    "startTime": "2026-09-21T14:13:20.000Z",
                    "endTime": "2026-09-21T14:14:20.000Z",
                },
                "value": {
                    "distributionValue": {
                        "count": "3",
                        "mean": 18.5,
                        "bucketOptions": { "explicitBuckets": { "bounds": [1.0, 10.0] } },
                        "bucketCounts": ["1", "1", "1"],
                    }
                },
            }],
        })
    );

    // And the one request body carries all three.
    assert_eq!(
        encoding.request_bodies(),
        vec![json!({ "timeSeries": encoding.time_series })]
    );
}

/// The API refuses a request carrying more than two hundred series whole, so
/// a registry that grew past that would stop being pushed at all.
#[test]
fn a_snapshot_larger_than_one_request_is_split_into_bodies_of_at_most_two_hundred_series() {
    let metrics = Metrics::new("qip-api");
    let gates: Vec<String> = (0..450).map(|i| format!("gate_{i:03}")).collect();
    for gate in &gates {
        metrics.count(names::ORDERS_REFUSED, labels([("gate", gate.as_str())]));
    }
    let encoding = metrics
        .snapshot()
        .to_cloud_monitoring_time_series(&task(), START, NOW);
    assert_eq!(encoding.time_series.len(), 450, "premise: 450 series");
    let sizes: Vec<usize> = encoding
        .request_bodies()
        .iter()
        .map(|body| body["timeSeries"].as_array().unwrap().len())
        .collect();
    assert_eq!(
        sizes,
        vec![MAX_SERIES_PER_REQUEST, MAX_SERIES_PER_REQUEST, 50]
    );

    let empty = Snapshot {
        service: "qip-api".to_string(),
        series: Vec::new(),
    };
    assert!(
        empty
            .to_cloud_monitoring_time_series(&task(), START, NOW)
            .request_bodies()
            .is_empty(),
        "an empty registry posts nothing rather than a body the API refuses"
    );
}

/// One malformed series in a batch fails every series beside it, so a value
/// the API would refuse is named and left out rather than sent.
#[test]
fn a_series_the_api_would_refuse_is_named_and_left_out_rather_than_sent() {
    let metrics = Metrics::new("qip-api");
    metrics.gauge(names::PORTFOLIO_LEVERAGE, labels([]), f64::NAN);
    metrics.gauge(names::PORTFOLIO_VALUE, labels([]), 2.0);
    metrics.count(names::CYCLES_RUN, labels([]));
    let snapshot = metrics.snapshot();

    let encoding = snapshot.to_cloud_monitoring_time_series(&task(), START, NOW);
    assert_eq!(
        encoding.unencodable,
        vec![names::PORTFOLIO_LEVERAGE.to_string()]
    );
    assert_eq!(encoding.time_series.len(), 2);

    // A cumulative interval must start before it ends at the millisecond
    // precision it is written at; the gauge has no start and still goes.
    let same_millisecond = Timestamp::from_nanos(START.as_nanos() + 999);
    let encoding = snapshot.to_cloud_monitoring_time_series(&task(), START, same_millisecond);
    assert_eq!(
        encoding.unencodable,
        vec![
            names::CYCLES_RUN.to_string(),
            names::PORTFOLIO_LEVERAGE.to_string()
        ]
    );
    assert_eq!(encoding.time_series.len(), 1);
    assert_eq!(encoding.time_series[0]["metricKind"], "GAUGE");
}
