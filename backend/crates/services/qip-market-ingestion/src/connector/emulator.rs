//! A source emulator that answers from recorded fixtures.
//!
//! The same [`SourceTransport`] port the HTTP client implements, so a
//! connector cannot tell the two apart and there is no branch inside it
//! deciding which — a branch that would eventually be the one that let a
//! recorded price reach production.
//!
//! # Why fixtures rather than a mock
//!
//! A mock proves a decoder was called. A recorded body proves the decoder
//! reads what the source actually sends, including the parts nobody would
//! think to mock: a trade id that is a JSON number rather than a string, a
//! rate table keyed by currency, a timestamp with six fractional digits. Each
//! fixture in `fixtures/` is a body captured from the live endpoint and
//! checked in, so the test suite runs with no network and still fails when a
//! decoder stops matching reality.
//!
//! # What the emulator can do that a live source cannot be asked to
//!
//! The answers worth testing are the ones a healthy source will not produce on
//! demand: `429` with a `retry-after`, a 500 that clears on the third try, a
//! body that is not JSON, a page missing a required field, the same page
//! served twice. Each is a recorded answer here, which is what makes backoff,
//! quarantine and deduplication testable at all.
//!
//! # Fixtures are data
//!
//! [`SourceEmulator::from_json`] loads a whole script from JSON, so a new
//! connector's fixtures are a file rather than a Rust literal, and re-recording
//! one is a diff a reviewer can read.

use super::transport::{SourceRequest, SourceResponse, SourceTransport};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// One recorded answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedAnswer {
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default)]
    pub body: String,
    /// What the round trip took when recorded, so a test asserting on latency
    /// reads the source's speed rather than the harness's.
    #[serde(default)]
    pub latency_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
    /// When set, the transport itself fails rather than answering — a refused
    /// connection, a read timeout. There is no HTTP status for "nothing came
    /// back", and a connector that survives a 500 but not a dropped socket is
    /// a connector that has not been tested against the more common failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_error: Option<String>,
}

impl RecordedAnswer {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            media_type: Some("application/json".to_string()),
            body: body.into(),
            latency_ms: 0,
            retry_after_secs: None,
            transport_error: None,
        }
    }

    pub fn rate_limited(retry_after_secs: i64) -> Self {
        Self {
            status: 429,
            media_type: Some("application/json".to_string()),
            body: r#"{"message":"too many requests"}"#.to_string(),
            latency_ms: 0,
            retry_after_secs: Some(retry_after_secs),
            transport_error: None,
        }
    }

    pub fn unreachable(detail: impl Into<String>) -> Self {
        Self {
            status: 0,
            media_type: None,
            body: String::new(),
            latency_ms: 0,
            retry_after_secs: None,
            transport_error: Some(detail.into()),
        }
    }

    pub const fn with_latency_ms(mut self, latency_ms: i64) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    fn as_response(&self) -> Result<SourceResponse> {
        if let Some(detail) = &self.transport_error {
            return Err(Error::unavailable(detail.clone()));
        }
        let mut response = SourceResponse {
            status: self.status,
            media_type: self.media_type.clone(),
            body: self.body.clone(),
            latency: Duration::from_millis(self.latency_ms),
            retry_after: None,
        };
        if let Some(seconds) = self.retry_after_secs {
            response = response.with_retry_after(Duration::from_secs(seconds));
        }
        Ok(response)
    }
}

/// The answers for one request target, in order.
///
/// The last answer repeats once the script runs out, so a fixture only writes
/// the answers a test cares about — and re-polling a healthy source keeps
/// serving the same page, which is exactly the redelivery deduplication has to
/// absorb.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedExchange {
    /// Matched as a substring of the request target, so a fixture does not
    /// have to spell out a query whose order this code decides.
    pub target: String,
    pub answers: Vec<RecordedAnswer>,
}

impl RecordedExchange {
    pub fn new(target: impl Into<String>, answers: Vec<RecordedAnswer>) -> Self {
        Self {
            target: target.into(),
            answers,
        }
    }

    /// One target that always answers the same way.
    pub fn always(target: impl Into<String>, answer: RecordedAnswer) -> Self {
        Self::new(target, vec![answer])
    }
}

/// The whole recorded script for one source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedSource {
    #[serde(default)]
    pub exchanges: Vec<RecordedExchange>,
}

/// A transport that answers from recorded fixtures.
#[derive(Clone, Debug)]
pub struct SourceEmulator {
    exchanges: Vec<RecordedExchange>,
    hits: Vec<usize>,
    calls: Vec<String>,
}

impl SourceEmulator {
    pub fn new(exchanges: Vec<RecordedExchange>) -> Self {
        let hits = vec![0; exchanges.len()];
        Self {
            exchanges,
            hits,
            calls: Vec::new(),
        }
    }

    /// One target, one answer, repeated.
    pub fn serving(target: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new(vec![RecordedExchange::always(
            target,
            RecordedAnswer::json(200, body),
        )])
    }

    /// Load a whole script from JSON.
    ///
    /// Unknown fields are refused, so a fixture written against an older shape
    /// fails as a fixture problem rather than quietly serving defaults.
    pub fn from_json(text: &str) -> Result<Self> {
        let recorded: RecordedSource = serde_json::from_str(text).map_err(|error| {
            Error::schema(format!("this is not a recorded source script: {error}"))
        })?;
        if recorded.exchanges.is_empty() {
            return Err(Error::invalid(
                "a recorded script with no exchanges answers 404 to everything, which reads as a \
                 broken connector rather than as an empty fixture",
            ));
        }
        Ok(Self::new(recorded.exchanges))
    }

    /// Every target asked for, in order. A test asserting that a health check
    /// did not consume a fetch reads this.
    pub fn calls(&self) -> &[String] {
        &self.calls
    }

    /// Rewind every script, keeping the fixtures. For a test that runs the
    /// same connector twice and needs the second run to see the first answer.
    pub fn rewind(&mut self) {
        for hit in &mut self.hits {
            *hit = 0;
        }
        self.calls.clear();
    }
}

impl SourceTransport for SourceEmulator {
    fn describe(&self) -> String {
        format!(
            "a source emulator with {} recorded target(s)",
            self.exchanges.len()
        )
    }

    fn request(&mut self, request: &SourceRequest, _at: Timestamp) -> Result<SourceResponse> {
        let target = request.target();
        self.calls.push(target.clone());
        for (index, exchange) in self.exchanges.iter().enumerate() {
            if !target.contains(exchange.target.as_str()) {
                continue;
            }
            let position = self.hits[index].min(exchange.answers.len().saturating_sub(1));
            self.hits[index] = self.hits[index].saturating_add(1);
            let Some(answer) = exchange.answers.get(position) else {
                return Err(Error::invalid(format!(
                    "the recorded exchange for {:?} has no answers",
                    exchange.target
                )));
            };
            return answer.as_response();
        }
        // A 404 naming the target, so a mis-specified fixture fails as a
        // fixture problem rather than as a mysterious refusal.
        Ok(SourceResponse::json(
            404,
            format!(
                r#"{{"error":"no recorded exchange matches","target":"{target}","recorded":{}}}"#,
                self.exchanges.len()
            ),
            Duration::ZERO,
        ))
    }
}
