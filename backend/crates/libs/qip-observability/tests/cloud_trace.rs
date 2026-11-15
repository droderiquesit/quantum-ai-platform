//! The Cloud Trace `batchWrite` encoding (ADR 0664, R12-OBS-10059).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_observability::Tracer;
use qip_observability::cloud_trace::{MAX_SPANS_PER_REQUEST, batch_write_body};
use qip_observability::trace::{Span, SpanKind};
use std::sync::Arc;

fn finished_span(name: &str) -> Span {
    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 11, 15)));
    let tracer = Arc::new(Tracer::new("qip-api", clock.clone() as Arc<dyn Clock>));
    let mut span = tracer.start(name, SpanKind::Server);
    span.set_attribute("http.route", "/v1/status");
    clock.advance(Duration::from_millis(7));
    span.finish();
    tracer.spans().pop().expect("a finished span")
}

/// A request span the tracer recorded is the span `batchWrite` receives: its
/// ids, name, kind and both instants, under the project-qualified resource
/// name the API routes on. Mutation: in `encode_span`, write `INTERNAL`
/// for every kind — the kind assertion fails.
#[test]
fn a_recorded_request_span_becomes_a_batch_write_span() {
    let span = finished_span("GET /v1/status");
    assert!(span.end.is_some(), "premise: the span is finished");
    let out = batch_write_body("peos-dev", std::slice::from_ref(&span)).unwrap();
    assert!(out.unencodable.is_empty());
    assert_eq!(out.bodies.len(), 1);
    let sent = &out.bodies[0]["spans"][0];
    assert_eq!(
        sent["name"],
        format!(
            "projects/peos-dev/traces/{}/spans/{}",
            span.trace_id.as_str(),
            span.span_id
        )
    );
    assert_eq!(sent["spanKind"], "SERVER");
    assert_eq!(sent["displayName"]["value"], "GET /v1/status");
    assert_eq!(sent["startTime"], span.start.to_rfc3339());
    assert_eq!(sent["endTime"], span.end.unwrap().to_rfc3339());
    assert_ne!(
        sent["startTime"], sent["endTime"],
        "premise: a measured duration"
    );
    assert_eq!(
        sent["attributes"]["attributeMap"]["http.route"]["stringValue"]["value"],
        "/v1/status"
    );
}

/// One bad span is left out and named; it does not fail its neighbours.
/// Mutation: delete the `span.end?` early return — the unfinished span is sent
/// and the `unencodable` assertion fails.
#[test]
fn a_span_the_api_would_refuse_is_named_and_left_out_of_the_batch() {
    let good = finished_span("ok");
    let mut unfinished = finished_span("unfinished");
    unfinished.end = None;
    let mut bad_trace = finished_span("bad-trace");
    bad_trace.trace_id = qip_core::TraceId::new("NOT-HEX");
    let mut backwards = finished_span("backwards");
    backwards.end = Some(Timestamp::from_secs(0));
    let spans = [
        good.clone(),
        unfinished.clone(),
        bad_trace.clone(),
        backwards.clone(),
    ];
    let out = batch_write_body("peos-dev", &spans).unwrap();
    assert_eq!(out.bodies[0]["spans"].as_array().unwrap().len(), 1);
    assert_eq!(out.bodies[0]["spans"][0]["spanId"], good.span_id.as_str());
    assert_eq!(
        out.unencodable,
        vec![unfinished.span_id, bad_trace.span_id, backwards.span_id]
    );
}

/// Text over the API's caps is cut at a character boundary and says how much;
/// extra attributes are counted as dropped. Mutation: return `(text, 0)`
/// unconditionally from `truncated` — the 128-byte name assertion fails.
#[test]
fn over_long_text_is_cut_on_a_character_boundary_and_the_cut_is_stated() {
    let mut span = finished_span("x");
    span.name = format!("{}é", "a".repeat(127)); // 129 bytes; é straddles the cap
    for n in 0..40 {
        span.attributes.insert(format!("k{n:02}"), "v".into());
    }
    let held = span.attributes.len();
    assert!(held > 32, "premise: more attributes than the API takes");
    let out = batch_write_body("peos-dev", &[span]).unwrap();
    let sent = &out.bodies[0]["spans"][0];
    assert_eq!(sent["displayName"]["value"].as_str().unwrap().len(), 127);
    assert_eq!(sent["displayName"]["truncatedByteCount"], 2);
    assert_eq!(sent["attributes"]["droppedAttributesCount"], held - 32);
}

/// More than a request's worth splits into several bodies, none over the cap.
/// Mutation: change `chunks(MAX_SPANS_PER_REQUEST)` to `chunks(usize::MAX)` —
/// a single body holds all of them and this fails.
#[test]
fn a_large_batch_is_split_into_bounded_requests() {
    let span = finished_span("s");
    let spans = vec![span; MAX_SPANS_PER_REQUEST * 2 + 1];
    let out = batch_write_body("peos-dev", &spans).unwrap();
    let sizes: Vec<usize> = out
        .bodies
        .iter()
        .map(|b| b["spans"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [MAX_SPANS_PER_REQUEST, MAX_SPANS_PER_REQUEST, 1]);
}

/// A project id that could alter the request path is refused.
#[test]
fn a_project_id_that_could_alter_the_path_is_refused() {
    let span = finished_span("s");
    assert!(
        batch_write_body("peos-dev", std::slice::from_ref(&span)).is_ok(),
        "premise"
    );
    for bad in ["", "a/b", "../x", "Peos", "a b"] {
        assert!(
            batch_write_body(bad, std::slice::from_ref(&span)).is_err(),
            "{bad}"
        );
    }
}
