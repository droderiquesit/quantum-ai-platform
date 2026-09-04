//! Metrics, tracing, logging and SLO evaluation.

// Exact float comparison is deliberate here: these assertions cover degenerate
// cases and identities where the function must return an exact sentinel value,
// not merely something close to it.
#![allow(clippy::float_cmp)]

use qip_core::testing::approx_eq;
use qip_core::{Clock, Duration, ManualClock, Timestamp, TraceId};
use qip_observability::logs::Severity;
use qip_observability::metrics::{Histogram, MetricValue, labels, names};
use qip_observability::slo::{Slo, SloWindow, default_slos};
use qip_observability::trace::SpanKind;
use qip_observability::{Metrics, Telemetry, Tracer};
use std::sync::Arc;

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 8, 22)))
}

// --- metrics ----------------------------------------------------------------

#[test]
fn counters_accumulate_per_label_set() {
    let metrics = Metrics::new("test");
    metrics.count(names::ORDERS_FILLED, labels([("venue", "XNYS")]));
    metrics.count(names::ORDERS_FILLED, labels([("venue", "XNYS")]));
    metrics.increment(names::ORDERS_FILLED, labels([("venue", "XLON")]), 5);

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::ORDERS_FILLED, &labels([("venue", "XNYS")])),
        2
    );
    assert_eq!(
        snapshot.counter(names::ORDERS_FILLED, &labels([("venue", "XLON")])),
        5
    );
    assert_eq!(snapshot.counter_total(names::ORDERS_FILLED), 7);
    assert_eq!(snapshot.counter("never_recorded", &labels([])), 0);
}

#[test]
fn a_counter_at_the_maximum_saturates_instead_of_wrapping_to_a_small_number() {
    // A counter that wraps past u64::MAX reads on a dashboard as a process
    // restart — the exact failure a monotonic counter exists to distinguish
    // from an actual restart. Saturating keeps it pinned at the ceiling and
    // truthful about having lost count, rather than fabricating history.
    let metrics = Metrics::new("test");
    metrics.increment(names::ORDERS_FILLED, labels([]), u64::MAX);
    metrics.increment(names::ORDERS_FILLED, labels([]), 5);

    let snapshot = metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::ORDERS_FILLED, &labels([])),
        u64::MAX,
        "a counter must saturate at its ceiling, not wrap past it"
    );
}

#[test]
fn gauges_hold_the_latest_value() {
    let metrics = Metrics::new("test");
    metrics.gauge(names::PORTFOLIO_LEVERAGE, labels([]), 1.4);
    metrics.gauge(names::PORTFOLIO_LEVERAGE, labels([]), 1.8);
    assert_eq!(
        metrics
            .snapshot()
            .gauge(names::PORTFOLIO_LEVERAGE, &labels([])),
        Some(1.8)
    );
}

#[test]
fn histograms_bucket_observations_and_report_quantiles() {
    let metrics = Metrics::new("test");
    for millis in [0.4, 0.6, 1.2, 2.0, 4.0, 8.0, 60.0, 200.0] {
        metrics.observe_latency_ms(names::EXECUTION_LATENCY_MS, labels([]), millis);
    }
    let snapshot = metrics.snapshot();
    let histogram = snapshot
        .histogram(names::EXECUTION_LATENCY_MS, &labels([]))
        .unwrap();
    assert_eq!(histogram.count, 8);
    assert!(approx_eq(histogram.sum, 276.2, 1e-9));
    assert!(approx_eq(histogram.min, 0.4, 1e-12));
    assert!(approx_eq(histogram.max, 200.0, 1e-12));
    assert!(histogram.mean() > 30.0);

    // The median sits in the low-millisecond buckets; the tail does not.
    let median = histogram.quantile(0.5);
    assert!((1.0..=5.0).contains(&median), "median {median}");
    assert!(
        histogram.quantile(0.99) > 50.0,
        "p99 {}",
        histogram.quantile(0.99)
    );
    assert!(histogram.quantile(0.0) <= median);
}

#[test]
fn histograms_ignore_non_finite_observations() {
    let mut histogram = Histogram::latency_ms();
    histogram.observe(1.0);
    histogram.observe(f64::NAN);
    histogram.observe(f64::INFINITY);
    assert_eq!(histogram.count, 1);
}

#[test]
fn a_histogram_bucket_at_the_maximum_saturates_instead_of_wrapping() {
    // Same failure class as the counter above, one layer down: the bucket
    // and total counts inside a Histogram are u64 and would silently wrap
    // rather than staying pinned, understating the busiest bucket in the
    // series.
    let mut histogram = Histogram::with_bounds(vec![1.0]);
    histogram.counts[0] = u64::MAX;
    histogram.count = u64::MAX;
    histogram.observe(0.5);
    assert_eq!(
        histogram.counts[0],
        u64::MAX,
        "a bucket count must saturate, not wrap past its ceiling"
    );
    assert_eq!(
        histogram.count,
        u64::MAX,
        "the total observation count must saturate, not wrap past its ceiling"
    );
}

#[test]
fn an_empty_histogram_does_not_divide_by_zero() {
    let histogram = Histogram::unit_interval();
    assert_eq!(histogram.mean(), 0.0);
    assert_eq!(histogram.quantile(0.5), 0.0);
}

#[test]
fn prometheus_export_is_well_formed() {
    let metrics = Metrics::new("test");
    metrics.count(names::ORDERS_FILLED, labels([("venue", "XNYS")]));
    metrics.describe(names::ORDERS_FILLED, "orders filled");
    metrics.gauge(names::PORTFOLIO_VALUE, labels([]), 1_000_000.0);
    metrics.observe_latency_ms(names::EXECUTION_LATENCY_MS, labels([]), 3.5);

    let text = metrics.snapshot().to_prometheus();
    assert!(text.contains("# TYPE qip_orders_filled_total counter"));
    assert!(text.contains("qip_orders_filled_total{venue=\"XNYS\"} 1"));
    assert!(text.contains("qip_portfolio_value 1000000"));
    assert!(text.contains("qip_execution_latency_milliseconds_count 1"));
    assert!(
        text.contains("_bucket{"),
        "histogram buckets must be exported"
    );
}

#[test]
fn otlp_metrics_export_carries_a_counter_a_gauge_and_a_histogram_in_schema_correct_json() {
    // ADR 0028: a sibling encoder to `to_prometheus`, not a replacement, so
    // this proves the new shape rather than re-proving the old one.
    let metrics = Metrics::new("qip-api");
    metrics.count(names::ORDERS_FILLED, labels([("venue", "XNYS")]));
    metrics.increment(names::ORDERS_FILLED, labels([("venue", "XNYS")]), 4);
    metrics.describe(names::ORDERS_FILLED, "orders filled");
    metrics.gauge(names::PORTFOLIO_VALUE, labels([]), 1_500_000.5);
    for millis in [1.0, 2.0, 60.0] {
        metrics.observe_latency_ms(names::EXECUTION_LATENCY_MS, labels([]), millis);
    }

    let export = metrics
        .snapshot()
        .to_otlp_metrics(1_700_000_000_000_000_000);

    // Top-level shape: one resource, naming the service, one scope carrying
    // every metric.
    assert_eq!(
        export["resourceMetrics"][0]["resource"]["attributes"][0]["value"]["stringValue"],
        "qip-api"
    );
    let scope_metrics = export["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
        .as_array()
        .expect("scopeMetrics.metrics must be an array");
    assert_eq!(
        scope_metrics.len(),
        3,
        "three distinct series were recorded, so three OTLP metrics must appear"
    );

    let find = |name: &str| {
        scope_metrics
            .iter()
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("{name} is missing from the OTLP export"))
    };

    // The counter: a monotonic cumulative sum, its value a JSON *string* per
    // OTLP's protobuf-JSON mapping for a 64-bit integer, and the accumulated
    // total (1 + 4 = 5), not either increment alone.
    let counter = find(names::ORDERS_FILLED);
    assert_eq!(counter["description"], "orders filled");
    assert_eq!(counter["sum"]["isMonotonic"], true);
    assert_eq!(
        counter["sum"]["aggregationTemporality"],
        "AGGREGATION_TEMPORALITY_CUMULATIVE"
    );
    let counter_point = &counter["sum"]["dataPoints"][0];
    assert_eq!(counter_point["asInt"], "5", "the two increments must sum");
    assert_eq!(counter_point["timeUnixNano"], "1700000000000000000");
    assert_eq!(counter_point["attributes"][0]["key"], "venue");
    assert_eq!(
        counter_point["attributes"][0]["value"]["stringValue"],
        "XNYS"
    );
    assert!(
        counter_point["asInt"].is_string(),
        "a 64-bit counter value must be a JSON string, not a number that can lose precision"
    );

    // The gauge: a bare instantaneous double, no cumulative wrapper.
    let gauge = find(names::PORTFOLIO_VALUE);
    assert_eq!(gauge["gauge"]["dataPoints"][0]["asDouble"], 1_500_000.5);
    assert!(
        gauge["gauge"]["dataPoints"][0]["asDouble"].is_number(),
        "a gauge value is a double and stays a JSON number"
    );
    assert!(
        gauge.get("sum").is_none(),
        "a gauge must not also be encoded as a sum"
    );

    // The histogram: per-bucket (not cumulative) counts, count and sum both
    // present, and as many bucket counts as the fixed latency boundaries plus
    // the overflow bucket.
    let histogram = find(names::EXECUTION_LATENCY_MS);
    let point = &histogram["histogram"]["dataPoints"][0];
    assert_eq!(point["count"], "3");
    let bucket_counts: Vec<u64> = point["bucketCounts"]
        .as_array()
        .expect("bucketCounts must be an array")
        .iter()
        .map(|v| {
            v.as_str()
                .expect("a bucket count must be a JSON string")
                .parse()
                .expect("a bucket count string must parse as u64")
        })
        .collect();
    assert_eq!(
        bucket_counts.iter().sum::<u64>(),
        3,
        "the per-bucket counts must sum to the total observation count, proving they are \
         per-bucket rather than the cumulative counts `to_prometheus` renders"
    );
    let bounds = point["explicitBounds"]
        .as_array()
        .expect("explicitBounds must be an array");
    assert_eq!(
        bucket_counts.len(),
        bounds.len() + 1,
        "there must be one more bucket than boundary — the overflow bucket"
    );

    // The whole document must actually be JSON, not merely `serde_json::Value`
    // in memory: round-trip it the way the drain thread's POST body will be
    // built.
    let text = serde_json::to_string(&export).expect("the OTLP export must serialise");
    let back: serde_json::Value =
        serde_json::from_str(&text).expect("the serialised OTLP export must parse back");
    assert_eq!(back, export);
}

#[test]
fn an_empty_snapshot_produces_an_otlp_document_with_no_metrics() {
    // The premise a reader needs before trusting the populated test above:
    // an empty registry does not fabricate a metric, it produces an empty
    // list inside the same envelope.
    let metrics = Metrics::new("empty-service");
    let export = metrics.snapshot().to_otlp_metrics(0);
    assert_eq!(
        export["resourceMetrics"][0]["resource"]["attributes"][0]["value"]["stringValue"],
        "empty-service"
    );
    assert_eq!(
        export["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .expect("metrics must be an array even when empty")
            .len(),
        0
    );
}

// --- tracing ----------------------------------------------------------------

#[test]
fn spans_form_a_tree_within_one_trace() {
    let clock = clock();
    let tracer = Arc::new(Tracer::new("reasoning-engine", clock.clone()));

    let mut root = tracer.start("investigate", SpanKind::Internal);
    root.set_attribute("opportunity.id", "opp-1");
    let trace_id = root.trace_id().clone();

    clock.advance(Duration::from_millis(5));
    let child = root.child("retrieve-evidence", SpanKind::Client);
    clock.advance(Duration::from_millis(12));
    child.finish();

    clock.advance(Duration::from_millis(3));
    root.finish();

    let spans = tracer.trace(&trace_id);
    assert_eq!(spans.len(), 2);
    let parent = spans.iter().find(|s| s.name == "investigate").unwrap();
    let child = spans
        .iter()
        .find(|s| s.name == "retrieve-evidence")
        .unwrap();
    assert_eq!(
        child.parent_span_id.as_deref(),
        Some(parent.span_id.as_str())
    );
    assert_eq!(child.trace_id.as_str(), parent.trace_id.as_str());
    assert_eq!(
        parent.attributes.get("opportunity.id").map(String::as_str),
        Some("opp-1")
    );
    assert_eq!(child.duration(), Some(Duration::from_millis(12)));
    assert_eq!(parent.duration(), Some(Duration::from_millis(20)));
}

#[test]
fn a_failed_span_records_its_error() {
    let tracer = Arc::new(Tracer::new("execution-engine", clock()));
    let span = tracer.start("submit", SpanKind::Client);
    span.finish_with_error("broker rejected the order");

    let spans = tracer.spans();
    assert_eq!(spans.len(), 1);
    assert!(spans[0].is_error());
    match &spans[0].status {
        qip_observability::trace::SpanStatus::Error { message } => {
            assert!(message.contains("broker rejected"));
        }
        other => panic!("expected an error status, got {other:?}"),
    }
}

#[test]
fn span_ids_are_deterministic_so_a_replay_reproduces_the_trace() {
    let make = || {
        let tracer = Arc::new(Tracer::new("svc", clock()));
        for i in 0..5 {
            let span = tracer.start(format!("op-{i}"), SpanKind::Internal);
            span.finish();
        }
        tracer
            .spans()
            .into_iter()
            .map(|s| (s.trace_id.as_str().to_string(), s.span_id))
            .collect::<Vec<_>>()
    };
    assert_eq!(make(), make());
}

#[test]
fn span_ids_have_the_w3c_lengths() {
    let tracer = Arc::new(Tracer::new("svc", clock()));
    let span = tracer.start("op", SpanKind::Server);
    assert_eq!(span.trace_id().as_str().len(), 32);
    assert_eq!(span.span_id().len(), 16);
    span.finish();
}

#[test]
fn trace_export_names_every_leaf_span_field_the_way_otlp_does() {
    // The failure this prevents: the drain thread POSTs this document to a
    // collector that validates it. The envelope was OTLP from the start, but
    // each leaf carried this crate's own names — `trace_id`, `start` as an
    // RFC 3339 string, `kind` as `"server"`, attributes as a map, and a
    // `status` nested inside a `status` with no `code` — so the batch would be
    // rejected whole and the only symptom would be a failure counter climbing.
    // The previous version of this test asserted the envelope only and passed
    // over every one of those leaves, which is why the gap survived it.
    let clock = clock();
    let tracer = Arc::new(Tracer::new("api", clock.clone()));
    let mut root = tracer.start("GET /portfolios", SpanKind::Server);
    root.set_attribute("http.route", "/portfolios");
    let child = root.child("load-positions", SpanKind::Client);
    clock.advance(Duration::from_millis(7));
    child.finish();
    root.finish();

    let export = tracer.export();
    let spans = export["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .expect("scopeSpans.spans must be an array");
    // The premise: `is_array()` is true of `[]`, so prove there is something
    // to inspect before inspecting it.
    assert_eq!(spans.len(), 2, "two spans were finished: {export}");

    let parent = spans
        .iter()
        .find(|s| s["name"] == "GET /portfolios")
        .expect("the root span is missing from the export");
    let child = spans
        .iter()
        .find(|s| s["name"] == "load-positions")
        .expect("the child span is missing from the export");

    assert_eq!(
        parent["traceId"].as_str().map(str::len),
        Some(32),
        "traceId must be the 32-character hex OTLP asks for"
    );
    assert_eq!(parent["spanId"].as_str().map(str::len), Some(16));
    assert_eq!(
        child["parentSpanId"], parent["spanId"],
        "the child must name its parent under OTLP's key"
    );
    assert!(
        parent.get("parentSpanId").is_none(),
        "a root span has no parent to name"
    );

    // Enums are integers: OTLP/JSON's protobuf mapping does not accept the
    // name string this crate's own derive produces.
    assert_eq!(parent["kind"], 2, "SpanKind::Server is SPAN_KIND_SERVER, 2");
    assert_eq!(child["kind"], 3, "SpanKind::Client is SPAN_KIND_CLIENT, 3");
    assert_eq!(parent["status"], serde_json::json!({"code": 1}));

    // Instants are nanoseconds as decimal strings, not RFC 3339 and not JSON
    // numbers, which lose precision above 2^53.
    let start: i64 = parent["startTimeUnixNano"]
        .as_str()
        .expect("startTimeUnixNano must be a JSON string")
        .parse()
        .expect("startTimeUnixNano must parse as nanoseconds");
    let end: i64 = parent["endTimeUnixNano"]
        .as_str()
        .expect("endTimeUnixNano must be a JSON string")
        .parse()
        .expect("endTimeUnixNano must parse as nanoseconds");
    assert_eq!(
        end - start,
        7_000_000,
        "the span must cover the seven milliseconds the clock advanced"
    );

    // Attributes are a KeyValue array, not a map.
    let attributes = parent["attributes"]
        .as_array()
        .expect("attributes must be an OTLP KeyValue array, not a map");
    let route = attributes
        .iter()
        .find(|kv| kv["key"] == "http.route")
        .expect("the attribute set on the span is missing from the export");
    assert_eq!(route["value"]["stringValue"], "/portfolios");

    // None of this crate's own leaf names may survive into the wire form.
    for span in spans {
        for internal in ["trace_id", "span_id", "parent_span_id", "start", "end"] {
            assert!(
                span.get(internal).is_none(),
                "the leaf still carries this crate's own `{internal}`: {span}"
            );
        }
    }

    // The envelope, which was already correct and must stay so.
    assert_eq!(
        export["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
        "api"
    );
}

#[test]
fn a_failed_span_exports_otlps_error_code_and_its_message() {
    // `SpanStatus`'s derive tags the enum with the field name `status`, so a
    // failed span serialised `"status": {"status": "error"}` — no `code` at
    // all, which an OTLP reader shows as unset, that is, as having succeeded.
    // That is the one thing an error span exists to deny.
    let tracer = Arc::new(Tracer::new("execution-engine", clock()));
    tracer
        .start("submit", SpanKind::Client)
        .finish_with_error("broker rejected the order");

    let export = tracer.export();
    let span = &export["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
    assert_eq!(
        span["name"], "submit",
        "the premise: the failed span is the one being inspected"
    );
    assert_eq!(span["status"]["code"], 2, "STATUS_CODE_ERROR is 2");
    assert_eq!(span["status"]["message"], "broker rejected the order");
}

#[test]
fn traces_are_isolated_from_one_another() {
    let tracer = Arc::new(Tracer::new("svc", clock()));
    let a = tracer.start("a", SpanKind::Internal);
    let a_trace = a.trace_id().clone();
    a.finish();
    let b = tracer.start("b", SpanKind::Internal);
    b.finish();

    assert_eq!(tracer.trace(&a_trace).len(), 1);
    assert_eq!(tracer.spans().len(), 2);
    assert!(tracer.trace(&TraceId::new("deadbeef")).is_empty());
}

// --- logging ----------------------------------------------------------------

#[test]
fn logs_carry_structured_fields() {
    let telemetry = Telemetry::new("risk-engine", clock());
    telemetry.logger.with(
        Severity::Warn,
        "leverage limit approached",
        [
            ("limit", "max_leverage"),
            ("value", "1.9"),
            ("threshold", "2.0"),
        ],
    );
    let records = telemetry.logger.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].severity, Severity::Warn);
    assert_eq!(
        records[0].fields.get("limit").map(String::as_str),
        Some("max_leverage")
    );
    assert!(records[0].to_line().contains("limit=max_leverage"));
}

#[test]
fn severity_filtering_suppresses_lower_levels() {
    let telemetry = Telemetry::new("svc", clock());
    telemetry.logger.set_minimum_severity(Severity::Warn);
    telemetry.logger.debug("noisy");
    telemetry.logger.info("routine");
    telemetry.logger.warn("notable");
    telemetry.logger.error("bad");

    let records = telemetry.logger.records();
    assert_eq!(records.len(), 2);
    assert_eq!(telemetry.logger.at_least(Severity::Error).len(), 1);
}

#[test]
fn log_timestamps_come_from_the_injected_clock() {
    let clock = clock();
    let telemetry = Telemetry::new("svc", clock.clone());
    telemetry.logger.info("first");
    clock.advance(Duration::from_hours(1));
    telemetry.logger.info("second");

    let records = telemetry.logger.records();
    assert_eq!(records[1].at.since(records[0].at), Duration::from_hours(1));
}

// --- SLOs -------------------------------------------------------------------

#[test]
fn slo_evaluation_reports_budget_consumption() {
    let slo = Slo::availability("api", "api", 0.99, SloWindow::Day);
    assert!(approx_eq(slo.error_budget(), 0.01, 1e-12));

    let met = slo.evaluate(9_990, 10_000);
    assert!(met.is_met, "99.9% achieved against a 99% target");
    assert!(
        met.budget_consumed < 0.2,
        "consumed {}",
        met.budget_consumed
    );

    let missed = slo.evaluate(9_800, 10_000);
    assert!(!missed.is_met);
    assert!(
        missed.budget_consumed > 1.0,
        "consumed {}",
        missed.budget_consumed
    );
    assert!(missed.is_page_worthy());
}

#[test]
fn an_slo_with_no_observations_is_vacuously_met() {
    let slo = Slo::latency("exec", "execution-engine", 0.999, 100.0, SloWindow::Day);
    let status = slo.evaluate(0, 0);
    assert!(status.is_met);
    assert_eq!(status.budget_consumed, 0.0);
    assert!(!status.is_page_worthy(), "no data must not page");
}

#[test]
fn a_handful_of_failures_does_not_page() {
    let slo = Slo::availability("api", "api", 0.99, SloWindow::Day);
    let status = slo.evaluate(3, 5);
    assert!(
        !status.is_page_worthy(),
        "too few observations to be meaningful"
    );
}

#[test]
fn the_shipped_slos_cover_the_critical_paths() {
    let slos = default_slos();
    let services: Vec<&str> = slos.iter().map(|s| s.service.as_str()).collect();
    for required in ["market-ingestion", "risk-engine", "execution-engine", "api"] {
        assert!(services.contains(&required), "{required} needs an SLO");
    }
    // Latency-critical services must have a latency objective, not just uptime.
    for slo in &slos {
        if matches!(
            slo.service.as_str(),
            "risk-engine" | "execution-engine" | "market-ingestion"
        ) {
            assert!(
                slo.latency_threshold_ms.is_some(),
                "{} needs a latency target",
                slo.name
            );
        }
        assert!(
            (0.0..1.0).contains(&slo.target) || slo.target < 1.0,
            "{} target",
            slo.name
        );
    }
}

#[test]
fn the_silent_telemetry_surface_stays_quiet() {
    let telemetry = Telemetry::silent();
    telemetry.logger.info("should not be retained");
    assert!(telemetry.logger.records().is_empty());
    // Metrics still record, so tests can assert on them.
    telemetry.metrics.count("x", labels([]));
    assert_eq!(telemetry.metrics.snapshot().counter_total("x"), 1);
}

#[test]
fn every_metric_shape_survives_a_json_round_trip() {
    // The API serves snapshots over HTTP, so all three shapes must serialise.
    let metrics = Metrics::new("api");
    metrics.count("c", labels([("a", "b")]));
    metrics.gauge("g", labels([]), 1.5);
    metrics.observe_latency_ms("h", labels([]), 4.0);

    let snapshot = metrics.snapshot();
    let text = serde_json::to_string(&snapshot).expect("snapshot must serialise");
    let back: qip_observability::Snapshot =
        serde_json::from_str(&text).expect("snapshot must deserialise");

    assert_eq!(back.counter("c", &labels([("a", "b")])), 1);
    assert_eq!(back.gauge("g", &labels([])), Some(1.5));
    assert_eq!(back.histogram("h", &labels([])).unwrap().count, 1);

    let value = MetricValue::Counter(42);
    assert!(serde_json::to_string(&value).unwrap().contains("counter"));

    let clock: Arc<dyn Clock> = clock();
    assert!(clock.now() > Timestamp::EPOCH);
}

#[test]
fn a_metric_described_before_it_is_first_recorded_keeps_its_documentation() {
    // Describing used to walk the series that already existed, so a component
    // describing its metrics where it is assembled — the one place that knows
    // what they mean and the one place guaranteed to run once — described
    // nothing at all, and every `# HELP` line afterwards read as a bare metric
    // name. The order must not matter.
    let metrics = Metrics::new("test");
    metrics.describe(names::ORDERS_REFUSED, "orders a control refused");

    // The premise: describing creates no series, so the assertion below is
    // about help surviving until a record arrives rather than about a series
    // that was there all along.
    assert!(
        metrics.snapshot().series.is_empty(),
        "describing a metric must not bring it into existence; a name nothing records \
         must stay absent from the export"
    );

    metrics.count(
        names::ORDERS_REFUSED,
        labels([("control", "pre-trade-risk")]),
    );
    let text = metrics.snapshot().to_prometheus();
    assert!(
        text.contains("# HELP qip_orders_refused_total orders a control refused"),
        "the description registered before the first record was lost: {text}"
    );
}

#[test]
fn describing_a_metric_after_it_is_recorded_still_reaches_the_series_already_there() {
    // The direction that already worked, kept as a test because the fix for the
    // other direction is where it would be broken.
    let metrics = Metrics::new("test");
    metrics.gauge(names::PORTFOLIO_VALUE, labels([]), 1.0);
    assert!(
        metrics
            .snapshot()
            .to_prometheus()
            .contains("# HELP qip_portfolio_value \n"),
        "the premise: the series starts undocumented"
    );

    metrics.describe(names::PORTFOLIO_VALUE, "the book, marked");
    assert!(
        metrics
            .snapshot()
            .to_prometheus()
            .contains("# HELP qip_portfolio_value the book, marked")
    );
}

#[test]
fn resetting_the_registry_discards_the_series_and_keeps_the_documentation() {
    // A description is registered once, where a component is assembled. A reset
    // that forgot it would leave every series recorded afterwards undocumented
    // for the life of the process, with no second call to put it back.
    let metrics = Metrics::new("test");
    metrics.describe(names::ORDERS_FILLED, "fills received");
    metrics.count(names::ORDERS_FILLED, labels([]));
    assert_eq!(
        metrics.snapshot().counter_total(names::ORDERS_FILLED),
        1,
        "the premise: there is a series to discard"
    );

    metrics.reset();
    assert!(
        metrics.snapshot().series.is_empty(),
        "reset must discard what was recorded"
    );

    metrics.count(names::ORDERS_FILLED, labels([]));
    assert!(
        metrics
            .snapshot()
            .to_prometheus()
            .contains("# HELP qip_orders_filled_total fills received"),
        "the documentation did not survive the reset"
    );
}

#[test]
fn a_label_value_from_configuration_cannot_forge_or_break_an_exposition_line() {
    // `cell` and `region` are the edge node's configuration strings as given,
    // and `venue` is whatever the venue list said — the same ConfigMap the
    // composition roots refuse a live ceiling from. The export used to escape
    // only `"` on sample lines and nothing on `_bucket` lines, so a cell id
    // carrying `"} 1\n…` closed the label set early and forged a
    // `qip_edge_halted … 0` line for a halted cell, and a trailing `\` broke
    // the quoting so the collector dropped the target whole. The exposition
    // must be well-formed whatever upstream passed.
    let hostile = "eu-1\\\"} 1\nqip_edge_halted{cell=\"eu-1\"} 0";
    assert!(
        hostile.contains('\\') && hostile.contains('"') && hostile.contains('\n'),
        "the premise: the value carries all three characters the format escapes"
    );

    let metrics = Metrics::new("edge");
    metrics.gauge(
        names::EDGE_HALTED,
        labels([("cell", hostile), ("source", "kill_switch")]),
        1.0,
    );
    metrics.observe_unit(names::EDGE_NETTING_RATIO, labels([("cell", hostile)]), 0.05);
    let text = metrics.snapshot().to_prometheus();

    let expected_gauge = r#"qip_edge_halted{cell="eu-1\\\"} 1\nqip_edge_halted{cell=\"eu-1\"} 0",source="kill_switch"} 1"#;
    assert!(
        text.lines().any(|line| line == expected_gauge),
        "the sample line is not the escaped form:\n{text}"
    );
    // Exactly one line names the halt series — the forged `… 0` sample never
    // becomes a line of its own.
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("qip_edge_halted"))
            .count(),
        1,
        "a label value forged a second sample line:\n{text}"
    );

    let expected_bucket = r#"qip_edge_netting_ratio_bucket{cell="eu-1\\\"} 1\nqip_edge_halted{cell=\"eu-1\"} 0",le="0.1"} 1"#;
    assert!(
        text.lines().any(|line| line == expected_bucket),
        "the bucket line is not the escaped form:\n{text}"
    );
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("qip_edge_netting_ratio_bucket"))
            .count(),
        Histogram::unit_interval().bounds.len(),
        "a bucket label forged or split a line:\n{text}"
    );
}
