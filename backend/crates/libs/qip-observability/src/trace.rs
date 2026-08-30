//! Distributed tracing.
//!
//! Span and trace identity follow the W3C trace-context format, so spans
//! exported from here join traces produced by anything else in the deployment.

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

    pub fn add_event(&mut self, name: impl Into<String>) {
        let at = self.tracer.clock.now();
        self.span.events.push(SpanEvent {
            name: name.into(),
            at,
            attributes: BTreeMap::new(),
        });
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.span.status = SpanStatus::Error {
            message: message.into(),
        };
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

    /// Export as OTLP-shaped JSON.
    pub fn export(&self) -> serde_json::Value {
        serde_json::json!({
            "resourceSpans": [{
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": self.service}}
                    ]
                },
                "scopeSpans": [{
                    "scope": {"name": "qip-observability"},
                    "spans": self.spans(),
                }]
            }]
        })
    }
}
