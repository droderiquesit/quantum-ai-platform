//! The port through which a connector touches the outside world.
//!
//! The same shape `qip_data_finder::probe::SourceProbe` uses, for the same
//! reason: one narrow trait carries every network fact, so the whole lifecycle
//! — backoff, rate limiting, staleness, dedup, quarantine — is exercised
//! against scripted answers with no socket open.
//!
//! Two implementations ship. [`HttpSourceTransport`] speaks HTTP/1.1 through
//! `qip_transport::HttpClient`, and [`crate::connector::emulator::SourceEmulator`]
//! replays recorded fixtures. There is deliberately no third that tries the
//! network and falls back to a fixture: a connector that quietly served a
//! recorded price would be indistinguishable downstream from one that worked.
//!
//! # The credential never enters a request
//!
//! [`SourceRequest`] has no field that can hold one. The transport resolves
//! the manifest's [`SecretRef`] once at construction and applies it as it
//! writes the request, so a request logged, serialised or compared in a test
//! cannot carry a credential — and neither can a connector, which never sees
//! one.

use super::manifest::{AuthScheme, SourceManifest};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_transport::{HttpClient, HttpRequest, Method, Url};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

/// Why a request is being made, which decides how a failure is read.
///
/// A health check that fails is a source to watch; a fetch that fails is data
/// not arriving. Collapsing the two puts both on the same runbook page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPurpose {
    #[default]
    Fetch,
    Health,
}

/// One request a connector wants made, with no credential in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRequest {
    /// Path under the manifest's `base_url`.
    pub path: String,
    /// Query parameters, ordered so the same fetch produces the same URL and
    /// therefore the same fixture key.
    pub query: BTreeMap<String, String>,
    pub purpose: RequestPurpose,
}

impl SourceRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            query: BTreeMap::new(),
            purpose: RequestPurpose::Fetch,
        }
    }

    pub fn for_health(mut self) -> Self {
        self.purpose = RequestPurpose::Health;
        self
    }

    /// Path and query as they go on the wire, which is also the key a
    /// recorded fixture is filed under.
    pub fn target(&self) -> String {
        if self.query.is_empty() {
            return self.path.clone();
        }
        let query: Vec<String> = self
            .query
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{}?{}", self.path, query.join("&"))
    }
}

/// What the source answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceResponse {
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub body: String,
    /// Measured by the transport rather than by the caller around the call: a
    /// caller timing it would be reading a clock, and the emulator would then
    /// report the test harness's speed instead of the source's.
    pub latency: Duration,
    /// What the source asked us to wait, from `retry-after`. Honoured over
    /// the computed backoff when it is longer — the source knows when it will
    /// serve again and the ladder is only a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<Duration>,
}

impl SourceResponse {
    pub fn json(status: u16, body: impl Into<String>, latency: Duration) -> Self {
        Self {
            status,
            media_type: Some("application/json".to_string()),
            body: body.into(),
            latency,
            retry_after: None,
        }
    }

    pub fn with_retry_after(mut self, wait: Duration) -> Self {
        self.retry_after = Some(wait);
        self
    }

    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Whether the source is asking for less traffic rather than reporting a
    /// fault. 429 and 408 both mean "come back", and neither is data loss.
    pub const fn is_rate_limited(&self) -> bool {
        matches!(self.status, 408 | 429)
    }

    /// Whether trying again could plausibly succeed. A 401 could not; a 503
    /// could.
    pub const fn is_transient(&self) -> bool {
        self.is_rate_limited() || (self.status >= 500 && self.status <= 599)
    }

    /// The first bytes of the body, for an error message. Bounded, because an
    /// error that quotes a megabyte of HTML is an error nobody reads.
    pub fn body_excerpt(&self) -> String {
        const LIMIT: usize = 240;
        let mut excerpt: String = self.body.chars().take(LIMIT).collect();
        if self.body.chars().nth(LIMIT).is_some() {
            excerpt.push('…');
        }
        excerpt
    }
}

/// The port. One question, no state the connector can see.
pub trait SourceTransport: std::fmt::Debug {
    /// What this transport is, for an error message that has to say whether a
    /// fixture or a socket produced it.
    fn describe(&self) -> String;

    /// Make one request. `at` is the caller's clock, used for the emulator's
    /// scripted latency and never read from the machine.
    fn request(&mut self, request: &SourceRequest, at: Timestamp) -> Result<SourceResponse>;
}

/// The transport a deployment uses: HTTP/1.1 over a plaintext socket.
///
/// Blocking, one connection per request, every wait and every buffer bounded
/// by `qip_transport::ClientLimits`. No async runtime, because a connector
/// polls a handful of endpoints a second and the machinery to do that without
/// blocking a thread costs more in unreviewable complexity than it saves.
pub struct HttpSourceTransport {
    endpoint: Url,
    client: HttpClient,
    /// Resolved once, held here and nowhere else. Never in a request, never
    /// in a log, never in `Debug`.
    credential: Option<(String, String)>,
    source_id: String,
}

impl std::fmt::Debug for HttpSourceTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSourceTransport")
            .field("source_id", &self.source_id)
            .field("endpoint", &self.endpoint.to_string())
            // Present or absent is worth knowing; the value never is.
            .field(
                "credential",
                &self.credential.as_ref().map(|(header, _)| header.as_str()),
            )
            .finish_non_exhaustive()
    }
}

impl HttpSourceTransport {
    /// Bounds chosen for a JSON document from a public API: a megabyte is a
    /// generous ceiling for a ticker or a rate table, and refusing one costs
    /// nothing. The timeouts are what stop a source that accepts a connection
    /// and then says nothing from holding the ingestion thread forever.
    pub const LIMITS: qip_transport::ClientLimits = qip_transport::ClientLimits {
        max_status_line: 8 * 1024,
        max_header_line: 8 * 1024,
        max_headers: 64,
        max_body: 1024 * 1024,
        max_chunk: 256 * 1024,
        connect_timeout: StdDuration::from_secs(3),
        read_timeout: StdDuration::from_secs(8),
        write_timeout: StdDuration::from_secs(5),
    };

    /// Build the transport a manifest describes, resolving its credential.
    ///
    /// Fails when the manifest is unconfigured rather than returning a
    /// transport that would fail on every request: a deployment missing a
    /// credential should fail while somebody is watching the rollout, not an
    /// hour later inside a poll loop.
    pub fn connect(manifest: &SourceManifest) -> Result<Self> {
        let missing = manifest.missing_configuration();
        if !missing.is_empty() {
            return Err(Error::unavailable(format!(
                "`{}` cannot open a socket and will not substitute recorded data: {}",
                manifest.source_id,
                missing.join("; ")
            )));
        }
        let base = manifest.endpoint.base_url.as_deref().unwrap_or_default();
        let endpoint = Url::parse(base).map_err(Error::from)?;
        let credential = match manifest.auth.scheme {
            AuthScheme::None => None,
            AuthScheme::Header => {
                let header = manifest.auth.header.clone().ok_or_else(|| {
                    Error::invalid("a header credential with no header survived validation")
                })?;
                Some((header, Self::require_secret(manifest)?))
            }
            AuthScheme::Bearer => Some((
                "authorization".to_string(),
                format!("Bearer {}", Self::require_secret(manifest)?),
            )),
        };
        Ok(Self {
            endpoint,
            client: HttpClient::new(Self::LIMITS),
            credential,
            source_id: manifest.source_id.clone(),
        })
    }

    fn require_secret(manifest: &SourceManifest) -> Result<String> {
        let reference = manifest.auth.secret.as_ref().ok_or_else(|| {
            Error::invalid("an authenticated manifest with no secret reference survived validation")
        })?;
        let value = reference.resolve()?.ok_or_else(|| {
            Error::unavailable(format!(
                "`{}` names the credential variable `{}`, which the deployment has not set",
                manifest.source_id,
                reference.variable()
            ))
        })?;
        if value.chars().any(|c| c.is_control()) {
            return Err(Error::invalid(format!(
                "the credential in `{}` contains a control character; sent as a header value it \
                 would end the header and let the rest be read as another one",
                reference.variable()
            )));
        }
        Ok(value)
    }
}

impl SourceTransport for HttpSourceTransport {
    fn describe(&self) -> String {
        format!("HTTP/1.1 to {}", self.endpoint)
    }

    fn request(&mut self, request: &SourceRequest, _at: Timestamp) -> Result<SourceResponse> {
        let target = format!(
            "{}{}",
            self.endpoint,
            request.target().trim_start_matches('/')
        );
        let mut http = HttpRequest::new(Method::Get, &target)
            .map_err(Error::from)?
            .with_header("accept", "application/json");
        if let Some((header, value)) = &self.credential {
            http = http.with_header(header, value);
        }
        // The socket's own elapsed time, not the platform clock: this measures
        // the peer, and the platform clock is deliberately not readable here.
        let began = std::time::Instant::now();
        let response = self.client.send(&http).map_err(Error::from)?;
        let latency =
            Duration::from_nanos(i64::try_from(began.elapsed().as_nanos()).unwrap_or(i64::MAX));
        let body = response
            .body_as_str()
            .map_err(Error::from)
            .map(str::to_string)
            .unwrap_or_else(|_| String::new());
        Ok(SourceResponse {
            status: response.status,
            media_type: response.header("content-type").map(str::to_string),
            body,
            latency,
            retry_after: response.header("retry-after").and_then(parse_retry_after),
        })
    }
}

/// `retry-after` as a delta-seconds count.
///
/// The HTTP-date form is deliberately not read: parsing it needs a wall clock
/// to subtract from, and this transport has none. An unparseable value yields
/// `None`, which falls back to the manifest's own ladder rather than to zero.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds = value.trim().parse::<i64>().ok()?;
    if seconds < 0 {
        return None;
    }
    Some(Duration::from_secs(seconds))
}
