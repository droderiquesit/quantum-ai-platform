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
//! one. A source that reads two headers gets both, in the order the manifest
//! names them: the primary header first, its companion second, after the
//! transport's own `accept`.
//!
//! # And never leaves in a response
//!
//! A vendor's error page, a debugging endpoint or a misrouted proxy can echo
//! the request's headers back in the body. `SourceResponse::body_excerpt` is
//! quoted into health details and failure reports by design — an operator
//! needs the vendor's words — so a body that carried the credential would
//! carry it into every line those reach. The transport replaces each
//! credential value in a body with a marker before the body leaves it,
//! because it is the only component that holds the values to look for.
//!
//! [`SecretRef`]: super::manifest::SecretRef

use super::manifest::{AuthScheme, SecretRef, SecretResolver, SourceManifest};
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
    /// in a log, never in `Debug`. In the order they are written to the
    /// wire: the manifest's `header`, then its `companion`.
    credentials: Vec<(String, String)>,
    /// The raw secret values, for scrubbing a body that echoes one. Distinct
    /// from the header values because a bearer header carries `Bearer <x>`
    /// and the body would echo `<x>`.
    secrets: Vec<String>,
    source_id: String,
}

/// What a credential becomes in a response body that echoed it.
pub const REDACTED: &str = "[credential redacted]";

impl std::fmt::Debug for HttpSourceTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSourceTransport")
            .field("source_id", &self.source_id)
            .field("endpoint", &self.endpoint.to_string())
            // Which headers are sent is worth knowing; their values never are.
            .field(
                "credential_headers",
                &self
                    .credentials
                    .iter()
                    .map(|(header, _)| header.as_str())
                    .collect::<Vec<_>>(),
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
        Self::connect_with(manifest, &SecretRef::resolve)
    }

    /// [`Self::connect`] with the credentials read through `resolve` instead
    /// of the deployment's environment.
    ///
    /// The production path passes [`SecretRef::resolve`]; a test passes a
    /// closure over a map and two files, which is the only way the `_FILE`
    /// indirection and the two-header emission can be proven in a build that
    /// cannot write its own environment.
    pub fn connect_with(manifest: &SourceManifest, resolve: SecretResolver<'_>) -> Result<Self> {
        let missing = manifest.missing_configuration_with(resolve);
        if !missing.is_empty() {
            return Err(Error::unavailable(format!(
                "`{}` cannot open a socket and will not substitute recorded data: {}",
                manifest.source_id,
                missing.join("; ")
            )));
        }
        let base = manifest.endpoint.base_url.as_deref().unwrap_or_default();
        let endpoint = Url::parse(base).map_err(Error::from)?;
        let mut credentials = Vec::new();
        let mut secrets = Vec::new();
        match manifest.auth.scheme {
            AuthScheme::None => {}
            AuthScheme::Header => {
                let header = manifest.auth.header.clone().ok_or_else(|| {
                    Error::invalid("a header credential with no header survived validation")
                })?;
                let primary = manifest.auth.secret.as_ref().ok_or_else(|| {
                    Error::invalid(
                        "an authenticated manifest with no secret reference survived validation",
                    )
                })?;
                let value = Self::require_secret(manifest, primary, resolve)?;
                secrets.push(value.clone());
                credentials.push((header, value));
                // Second on the wire, as in the manifest. The order is fixed
                // here rather than left to a map so a packet capture reads
                // the same way the manifest does.
                if let Some(companion) = &manifest.auth.companion {
                    let value = Self::require_secret(manifest, &companion.secret, resolve)?;
                    secrets.push(value.clone());
                    credentials.push((companion.header.clone(), value));
                }
            }
            AuthScheme::Bearer => {
                let primary = manifest.auth.secret.as_ref().ok_or_else(|| {
                    Error::invalid(
                        "an authenticated manifest with no secret reference survived validation",
                    )
                })?;
                let value = Self::require_secret(manifest, primary, resolve)?;
                credentials.push(("authorization".to_string(), format!("Bearer {value}")));
                secrets.push(value);
            }
        }
        Ok(Self {
            endpoint,
            client: HttpClient::new(Self::LIMITS),
            credentials,
            secrets,
            source_id: manifest.source_id.clone(),
        })
    }

    fn require_secret(
        manifest: &SourceManifest,
        reference: &SecretRef,
        resolve: SecretResolver<'_>,
    ) -> Result<String> {
        let value = resolve(reference)?.ok_or_else(|| {
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
        if value.trim().is_empty() {
            // Beyond being no credential, an empty value would make the body
            // scrub below match everywhere.
            return Err(Error::invalid(format!(
                "the credential in `{}` is blank; an empty header authenticates nothing and \
                 would be sent as if it did",
                reference.variable()
            )));
        }
        Ok(value)
    }

    /// The credential headers this transport writes, in the order it writes
    /// them: the manifest's `header`, then its `companion`. Names only.
    ///
    /// The same vector [`SourceTransport::request`] iterates, so what this
    /// reports is the wire order and not a description of it.
    pub fn credential_headers(&self) -> Vec<&str> {
        self.credentials
            .iter()
            .map(|(header, _)| header.as_str())
            .collect()
    }

    /// The body with every credential value replaced by [`REDACTED`].
    ///
    /// Applied before the body leaves the transport, so no caller — the
    /// runtime's excerpt, a health detail, a test's assertion message — can
    /// write a credential the vendor echoed.
    fn scrub(&self, body: String) -> String {
        self.secrets
            .iter()
            .fold(body, |body, secret| body.replace(secret, REDACTED))
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
        for (header, value) in &self.credentials {
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
        let body = self.scrub(body);
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
