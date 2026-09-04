//! Distributed tracing.
//!
//! Span and trace identity follow the W3C trace-context format, so spans
//! exported from here join traces produced by anything else in the deployment.

use crate::metrics::otlp_attributes;
use qip_core::{Clock, Duration, Timestamp, TraceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SpanStatus {
    Unset,
    Ok,
    Error { message: String },
}

/// A completed unit of work.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Span {
    pub trace_id: TraceId,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub kind: SpanKind,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
    pub status: SpanStatus,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<SpanEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpanEvent {
    pub name: String,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

impl Span {
    pub fn duration(&self) -> Option<Duration> {
        self.end.map(|end| end.since(self.start))
    }

    pub fn is_error(&self) -> bool {
        matches!(self.status, SpanStatus::Error { .. })
    }
}

/// An in-progress span. Finishing it records it on the tracer.
#[derive(Debug)]
pub struct ActiveSpan {
    span: Span,
    tracer: Arc<Tracer>,
}

impl ActiveSpan {
    pub fn set_attribute(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.span.attributes.insert(key.into(), value.into());
    }

    pub fn trace_id(&self) -> &TraceId {
        &self.span.trace_id
    }

    pub fn span_id(&self) -> &str {
        &self.span.span_id
    }

    /// Start a child of this span.
    pub fn child(&self, name: impl Into<String>, kind: SpanKind) -> ActiveSpan {
        self.tracer.start_child(
            name,
            kind,
            self.span.trace_id.clone(),
            Some(self.span.span_id.clone()),
        )
    }

    /// Finish the span, recording it as successful unless an error was set.
    pub fn finish(mut self) {
        self.span.end = Some(self.tracer.clock.now());
        if matches!(self.span.status, SpanStatus::Unset) {
            self.span.status = SpanStatus::Ok;
        }
        self.tracer.record(self.span.clone());
    }

    /// Finish the span as failed.
    pub fn finish_with_error(mut self, message: impl Into<String>) {
        self.span.status = SpanStatus::Error {
            message: message.into(),
        };
        self.span.end = Some(self.tracer.clock.now());
        self.tracer.record(self.span.clone());
    }
}

/// Creates and collects spans.
#[derive(Debug)]
pub struct Tracer {
    service: String,
    clock: Arc<dyn Clock>,
    counter: AtomicU64,
    finished: Mutex<Vec<Span>>,
    /// Cap on retained spans, so a long-running process cannot grow unbounded.
    capacity: usize,
}

impl Tracer {
    pub fn new(service: impl Into<String>, clock: Arc<dyn Clock>) -> Self {
        Self {
            service: service.into(),
            clock,
            counter: AtomicU64::new(0),
            finished: Mutex::new(Vec::new()),
            capacity: 10_000,
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Start a root span, beginning a new trace.
    pub fn start(self: &Arc<Self>, name: impl Into<String>, kind: SpanKind) -> ActiveSpan {
        let trace_id = TraceId::new(self.next_id(32));
        self.start_child(name, kind, trace_id, None)
    }

    /// Start a span inside an existing trace.
    pub fn start_child(
        self: &Arc<Self>,
        name: impl Into<String>,
        kind: SpanKind,
        trace_id: TraceId,
        parent_span_id: Option<String>,
    ) -> ActiveSpan {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), self.service.clone());
        ActiveSpan {
            span: Span {
                trace_id,
                span_id: self.next_id(16),
                parent_span_id,
                name: name.into(),
                kind,
                start: self.clock.now(),
                end: None,
                status: SpanStatus::Unset,
                attributes,
                events: Vec::new(),
            },
            tracer: self.clone(),
        }
    }

    /// Deterministic hexadecimal id of `digits` characters.
    ///
    /// Derived from a counter rather than randomness so a replayed run produces
    /// the same trace ids as the original.
    fn next_id(&self, digits: usize) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        let mut hasher = qip_core::Hasher256::new();
        hasher.update(self.service.as_bytes());
        hasher.update(&n.to_le_bytes());
        let digest = hasher.finish();
        qip_core::hash::to_hex(&digest)[..digits].to_string()
    }

    fn record(&self, span: Span) {
        let mut guard = self.finished.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.capacity {
            guard.remove(0);
        }
        guard.push(span);
    }

    /// Every recorded span.
    pub fn spans(&self) -> Vec<Span> {
        self.finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Spans belonging to one trace, in start order.
    pub fn trace(&self, trace_id: &TraceId) -> Vec<Span> {
        let mut spans: Vec<Span> = self
            .spans()
            .into_iter()
            .filter(|s| s.trace_id.as_str() == trace_id.as_str())
            .collect();
        spans.sort_by_key(|s| s.start.as_nanos());
        spans
    }

    pub fn clear(&self) {
        self.finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Export as an OTLP/JSON `ResourceSpans` document.
    ///
    /// Every leaf is encoded by [`otlp_span`] rather than by `Span`'s own
    /// `Serialize`. The envelope was OTLP from the start; the leaves were not,
    /// and a document that is OTLP down to the second level and this crate's
    /// own record below it is rejected exactly as thoroughly as one that was
    /// never OTLP at all.
    pub fn export(&self) -> serde_json::Value {
        let spans: Vec<serde_json::Value> = self.spans().iter().map(otlp_span).collect();
        serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": self.service}}
                    ]
                },
                "scopeSpans": [{
                    "scope": {"name": "qip-observability"},
                    "spans": spans,
                }]
            }]
        })
    }
}

/// One span as an OTLP/JSON `Span`.
///
/// Deliberately separate from `Span`'s own `Serialize` derive. That derive is
/// this crate's internal record — snake_case names, RFC 3339 instants, an
/// attribute map — and it is what every in-tree reader of a stored span
/// expects. OTLP is a different document naming the same facts differently,
/// and producing it by mangling the derive would change every in-tree reader
/// to fix a wire format no in-tree reader consumes.
///
/// The failure this prevents: the drain thread POSTs this body to a collector
/// that validates it against OTLP's schema. A leaf carrying `trace_id` where
/// `traceId` is required fails the whole batch, so every span in it is
/// dropped and the only symptom is a failure counter climbing with no field
/// named in the response to point at.
///
/// Follows OTLP's protobuf-JSON mapping, the same one
/// [`crate::Snapshot::to_otlp_metrics`] documents: `fixed64` instants are
/// decimal *strings*, because a JSON number loses precision above 2^53, and
/// enum fields — `kind`, `status.code` — are integers, because that mapping
/// forbids the enum's name string. The ids need no conversion at all:
/// [`Tracer::next_id`] already produces the lowercase hex OTLP asks for, 32
/// characters for a trace and 16 for a span.
fn otlp_span(span: &Span) -> serde_json::Value {
    let mut out = serde_json::json!({
        "traceId": span.trace_id.as_str(),
        "spanId": span.span_id,
        "name": span.name,
        "kind": otlp_span_kind(span.kind),
        "startTimeUnixNano": span.start.as_nanos().to_string(),
        "attributes": otlp_attributes(&span.attributes),
        "status": otlp_status(&span.status),
    });
    if let Some(parent) = &span.parent_span_id {
        out["parentSpanId"] = serde_json::json!(parent);
    }
    // An unfinished span has no end instant and this encoder will not invent
    // one: `"endTimeUnixNano": "0"` renders in a trace viewer as a span that
    // ended in 1970 — a fabricated fifty-six-year duration that looks like a
    // measurement rather than like the absence it is. Nothing a `Tracer`
    // records can be in that state, because `ActiveSpan::finish` and
    // `finish_with_error` both set `end` before recording, but `Span` and its
    // fields are public and `export` encodes whatever is in the ring.
    if let Some(end) = span.end {
        out["endTimeUnixNano"] = serde_json::json!(end.as_nanos().to_string());
    }
    if !span.events.is_empty() {
        out["events"] =
            serde_json::json!(span.events.iter().map(otlp_span_event).collect::<Vec<_>>());
    }
    out
}

/// One span event as OTLP's `Span.Event`.
///
/// Encoded rather than dropped even though nothing in the tree records one
/// yet: a field an encoder silently omits is a gap discovered by whoever
/// first sets it, in a collector's rejection rather than here.
fn otlp_span_event(event: &SpanEvent) -> serde_json::Value {
    serde_json::json!({
        "timeUnixNano": event.at.as_nanos().to_string(),
        "name": event.name,
        "attributes": otlp_attributes(&event.attributes),
    })
}

/// OTLP's `SpanKind` as the integer its JSON mapping requires.
///
/// The name and the integer are not interchangeable: OTLP/JSON encodes enum
/// values as integers, so the `"server"` this crate's own derive produces is
/// neither the integer nor the `SPAN_KIND_SERVER` some decoders tolerate.
/// `SPAN_KIND_UNSPECIFIED` (0) has no arm because every span names its kind
/// at [`Tracer::start`].
const fn otlp_span_kind(kind: SpanKind) -> i32 {
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
        SpanKind::Producer => 4,
        SpanKind::Consumer => 5,
    }
}

/// OTLP's `Status`: an integer `code`, and a `message` only where one exists.
///
/// `SpanStatus`'s derive tags the enum with the field name `status`, so a span
/// serialises `"status": {"status": "ok"}` — a doubly nested key and no `code`
/// at all, which an OTLP reader sees as unset, that is, as having succeeded.
/// The message belongs to the error arm alone: an empty `message` beside a
/// successful span reads in a viewer as an error whose text was lost.
fn otlp_status(status: &SpanStatus) -> serde_json::Value {
    match status {
        SpanStatus::Unset => serde_json::json!({"code": 0}),
        SpanStatus::Ok => serde_json::json!({"code": 1}),
        SpanStatus::Error { message } => serde_json::json!({"code": 2, "message": message}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unfinished() -> Span {
        Span {
            trace_id: TraceId::new("a".repeat(32)),
            span_id: "b".repeat(16),
            parent_span_id: None,
            name: "unfinished".into(),
            kind: SpanKind::Internal,
            start: Timestamp::from_secs(1),
            end: None,
            status: SpanStatus::Unset,
            attributes: BTreeMap::new(),
            events: Vec::new(),
        }
    }

    #[test]
    fn a_span_with_no_end_instant_omits_the_field_rather_than_claiming_it_ended_at_the_epoch() {
        let encoded = otlp_span(&unfinished());
        // The premise: the instant that does exist is encoded, so the absence
        // asserted below is about `end` alone and not about a dead encoder.
        assert_eq!(
            encoded["startTimeUnixNano"], "1000000000",
            "the start instant must be nanoseconds as a decimal string: {encoded}"
        );
        assert!(
            encoded.get("endTimeUnixNano").is_none(),
            "an absent end instant must stay absent, not become a 1970 timestamp: {encoded}"
        );
        assert_eq!(encoded["status"], serde_json::json!({"code": 0}));
        assert!(
            encoded["status"].get("message").is_none(),
            "only the error arm carries a message"
        );
        assert!(
            encoded.get("events").is_none(),
            "a span with no events must not carry an empty array"
        );
    }

    #[test]
    fn a_span_event_carries_otlps_time_name_and_key_value_attributes() {
        // Nothing in the tree records a span event yet, so this is the field
        // most likely to be encoded once and never read back.
        let mut attributes = BTreeMap::new();
        attributes.insert("venue".to_string(), "XNYS".to_string());
        let mut span = unfinished();
        span.events.push(SpanEvent {
            name: "order.acknowledged".into(),
            at: Timestamp::from_secs(2),
            attributes,
        });

        let encoded = otlp_span(&span);
        let events = encoded["events"]
            .as_array()
            .unwrap_or_else(|| panic!("events must be an array: {encoded}"));
        assert_eq!(events.len(), 1, "the premise: one event was recorded");
        assert_eq!(events[0]["name"], "order.acknowledged");
        assert_eq!(events[0]["timeUnixNano"], "2000000000");
        assert_eq!(events[0]["attributes"][0]["key"], "venue");
        assert_eq!(events[0]["attributes"][0]["value"]["stringValue"], "XNYS");
        assert!(
            events[0].get("at").is_none(),
            "`at` is this crate's name for the instant, not OTLP's: {encoded}"
        );
    }
}
