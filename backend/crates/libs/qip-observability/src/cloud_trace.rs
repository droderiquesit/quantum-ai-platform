//! The Cloud Trace v2 encoding of finished spans (ADR 0664, R12-OBS-10059):
//! the request bodies `projects.traces.batchWrite` accepts.
//!
//! Pure on purpose, like [`crate::cloud_monitoring`]: spans in, JSON out, no
//! socket, token or clock. The call lives in the composition roots.
//!
//! # What is refused rather than sent
//!
//! One malformed span fails the whole `batchWrite`, taking every span beside
//! it. A span the API would reject is therefore left out and named in
//! [`TraceEncoding::unencodable`]: a trace id that is not 32 lowercase hex
//! characters, a span id or parent id that is not 16, an unfinished span (the
//! API requires an end time and this encoder will not invent one), or an end
//! before the start.
//!
//! # What is truncated, and said so
//!
//! The API caps a display name at 128 bytes, an attribute key at 128, an
//! attribute value at 256 and a span at 32 attributes. Text over a cap is cut
//! at a character boundary and carries `truncatedByteCount`, and attributes
//! over the count are reported in `droppedAttributesCount`, so a cut is a
//! stated fact and not a silent loss.

use crate::trace::{Span, SpanKind, SpanStatus};
use serde_json::{Value, json};

/// Spans per request body (ADR 0664 decision 2).
pub const MAX_SPANS_PER_REQUEST: usize = 200;
const MAX_NAME_BYTES: usize = 128;
const MAX_KEY_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 256;
const MAX_ATTRIBUTES: usize = 32;
/// `google.rpc.Code.UNKNOWN`: the status code for a span that failed with a
/// message and no finer classification.
const RPC_UNKNOWN: i32 = 2;

/// The bodies for one batch of spans, and the spans that could not be encoded.
#[derive(Debug)]
pub struct TraceEncoding {
    /// One `batchWrite` body per [`MAX_SPANS_PER_REQUEST`] encodable spans.
    pub bodies: Vec<Value>,
    /// `span_id` of every span left out, for the exporter to count.
    pub unencodable: Vec<String>,
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `text` cut to at most `max` bytes at a character boundary, and how many
/// bytes were cut.
fn truncated(text: &str, max: usize) -> (&str, usize) {
    if text.len() <= max {
        return (text, 0);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], text.len() - end)
}

fn truncatable(text: &str, max: usize) -> Value {
    let (kept, cut) = truncated(text, max);
    json!({"value": kept, "truncatedByteCount": cut})
}

const fn span_kind(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "INTERNAL",
        SpanKind::Server => "SERVER",
        SpanKind::Client => "CLIENT",
        SpanKind::Producer => "PRODUCER",
        SpanKind::Consumer => "CONSUMER",
    }
}

fn encode_span(project: &str, span: &Span) -> Option<Value> {
    let end = span.end?;
    if end < span.start
        || !is_lower_hex(span.trace_id.as_str(), 32)
        || !is_lower_hex(&span.span_id, 16)
        || span
            .parent_span_id
            .as_deref()
            .is_some_and(|p| !is_lower_hex(p, 16))
    {
        return None;
    }
    let mut attribute_map = serde_json::Map::new();
    for (key, value) in span.attributes.iter().take(MAX_ATTRIBUTES) {
        let (key, _) = truncated(key, MAX_KEY_BYTES);
        attribute_map.insert(
            key.to_string(),
            json!({"stringValue": truncatable(value, MAX_VALUE_BYTES)}),
        );
    }
    let mut out = json!({
        "name": format!("projects/{project}/traces/{}/spans/{}", span.trace_id.as_str(), span.span_id),
        "spanId": span.span_id,
        "displayName": truncatable(&span.name, MAX_NAME_BYTES),
        "startTime": span.start.to_rfc3339(),
        "endTime": end.to_rfc3339(),
        "spanKind": span_kind(span.kind),
        "attributes": {
            "attributeMap": attribute_map,
            "droppedAttributesCount": span.attributes.len().saturating_sub(MAX_ATTRIBUTES),
        },
    });
    if let Some(parent) = &span.parent_span_id {
        out["parentSpanId"] = json!(parent);
    }
    if let SpanStatus::Error { message } = &span.status {
        out["status"] =
            json!({"code": RPC_UNKNOWN, "message": truncated(message, MAX_VALUE_BYTES).0});
    }
    Some(out)
}

/// Encode `spans` for `project` (a project id, not a number-prefixed path).
/// Refuses a project id that could change the request path.
pub fn batch_write_body(project: &str, spans: &[Span]) -> qip_core::error::Result<TraceEncoding> {
    if project.is_empty()
        || !project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(qip_core::error::Error::invalid(
            "a Cloud Trace project id is lowercase letters, digits and hyphens; refusing one that could alter the request path",
        ));
    }
    let mut encoded = Vec::new();
    let mut unencodable = Vec::new();
    for span in spans {
        match encode_span(project, span) {
            Some(value) => encoded.push(value),
            None => unencodable.push(span.span_id.clone()),
        }
    }
    let bodies = encoded
        .chunks(MAX_SPANS_PER_REQUEST)
        .map(|chunk| json!({"spans": chunk}))
        .collect();
    Ok(TraceEncoding {
        bodies,
        unencodable,
    })
}
