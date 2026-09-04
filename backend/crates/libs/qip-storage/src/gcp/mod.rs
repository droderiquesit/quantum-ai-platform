//! Adapters for two Google Cloud services, over their JSON REST APIs.
//!
//! [`CloudStorageBlobStore`] satisfies [`crate::BlobStore`] against a Cloud
//! Storage bucket; [`BigQueryWarehouse`] streams rows into a BigQuery table and
//! runs queries against it. Both speak plain HTTP/1.1 through
//! [`qip_transport::HttpClient`] — the platform's one HTTP client, with bounded
//! bodies and explicit timeouts — and both refuse, opening no connection, when
//! they are not configured.
//!
//! These are the only two managed targets in [`crate::provider::StorageTarget`]
//! that are real. They are real because they are the two with a plain JSON REST
//! API. AlloyDB and Spanner speak PostgreSQL wire protocol and gRPC, Bigtable
//! speaks gRPC, Memorystore speaks RESP — each needs a protocol implementation
//! this workspace does not have and a dependency policy that permits only
//! `serde` will not acquire.
//!
//! # The two things a deployment must supply
//!
//! **A TLS-terminating proxy.** [`qip_transport::http`] has no TLS stack and
//! refuses the `https` scheme by name rather than downgrading it, so nothing
//! here can talk to `storage.googleapis.com` directly. The address these
//! adapters are given must be an `http://` endpoint inside the deployment's
//! own network that terminates TLS and forwards to Google — a sidecar, an
//! egress gateway, a service mesh. This is not a detail to discover in
//! production: a bearer token sent to a public endpoint in clear text is a
//! credential given away, and the adapter's refusal to construct an `https`
//! URL is what stops that happening by accident.
//!
//! **A bearer token.** See [`auth`] for why minting one is out of scope here
//! and what a deployment can use instead.
//!
//! Both are named in [`GcpAccess::missing_configuration`] one at a time, so an
//! operator who has supplied one of them learns which is left rather than
//! rechecking both.
//!
//! # The peer is untrusted, including this one
//!
//! Google is not hostile, but the thing at the end of the socket is whatever
//! the proxy forwards to, and a misconfigured proxy is the ordinary case. So
//! every response is bounded before it is buffered, every wait is bounded, and
//! a body that claims to be one thing and is another is refused rather than
//! guessed at. A response's `content-type` is not trusted to decide which code
//! path runs: trusting it would let the peer choose.
//!
//! # What these adapters do not do
//!
//! They do not retry. One request per operation, and a failure is returned to
//! the caller with which kind of failure it was — a rejected credential, a
//! missing bucket, a rate limit, a server error — because those go on different
//! runbook pages. Retrying belongs to a caller that knows whether the operation
//! is safe to repeat; a blob `put` is, a BigQuery insert is only because it
//! carries an insert id.
//!
//! They do not batch across calls, hold connections open, or run anything in
//! the background. One call, one connection, one answer, on the calling thread.
//!
//! They do not fall back. There is no path through this module that writes to
//! the local disk when the network is unavailable. That is deliberate and it is
//! the reason the module exists in this shape: a deployment configured for
//! Cloud Storage that quietly wrote to a container's filesystem would pass
//! every smoke test, report every write as successful, and lose the archive
//! when the pod was rescheduled.

pub mod auth;
pub mod bigquery;
pub mod storage;

pub use auth::{AccessToken, MetadataServerTokens, StaticToken, TokenFile, TokenSource};
pub use bigquery::{
    BigQueryConfig, BigQueryWarehouse, InsertOutcome, InsertRow, QueryPage, QueryParameter,
    QueryRequest, QueryRow, RowError,
};
pub use storage::{CloudStorageBlobStore, CloudStorageConfig};

/// Environment variable naming the TLS-terminating proxy.
pub const ENDPOINT_VARIABLE: &str = "QIP_GCP_ENDPOINT";
/// Environment variable opting into metadata-server tokens.
pub const METADATA_VARIABLE: &str = "QIP_GCP_METADATA_SERVER";
/// Environment variable naming a file something else keeps a token in.
pub const TOKEN_FILE_VARIABLE: &str = "QIP_GCP_TOKEN_FILE";
/// Environment variable carrying a literal bearer token.
pub const TOKEN_VARIABLE: &str = "QIP_GCP_ACCESS_TOKEN";
/// Environment variable naming the Cloud Storage bucket.
pub const BUCKET_VARIABLE: &str = "QIP_CLOUD_STORAGE_BUCKET";
/// Environment variable naming the BigQuery project a job is billed to.
pub const PROJECT_VARIABLE: &str = "QIP_BIGQUERY_PROJECT";
/// Environment variable naming the BigQuery dataset.
pub const DATASET_VARIABLE: &str = "QIP_BIGQUERY_DATASET";

use qip_core::error::{Error, Result};
use qip_transport::{ClientLimits, HttpRequest, Method, Url};
use std::sync::Arc;

/// Default limits for a request to a GCP JSON endpoint.
///
/// `max_body` is the one worth arguing about, and 32 MiB is chosen against a
/// specific failure: it is the size of the largest object
/// [`CloudStorageBlobStore`] will download in one piece, and a limit is a
/// promise about how much of this process's memory one peer can claim. A
/// deployment archiving larger objects raises it knowingly, having thought
/// about how many concurrent downloads that multiplies by.
pub fn default_limits() -> ClientLimits {
    ClientLimits {
        max_body: 32 * 1024 * 1024,
        max_headers: 64,
        connect_timeout: std::time::Duration::from_secs(5),
        read_timeout: std::time::Duration::from_secs(30),
        write_timeout: std::time::Duration::from_secs(30),
        ..ClientLimits::default()
    }
}

/// How to reach Google, and with what credential.
///
/// Shared by both adapters because both need exactly this and nothing more.
/// Not `Serialize`, and its [`std::fmt::Debug`] never reaches the token: the
/// token source's own `Debug` is what prints, and each of them redacts.
#[derive(Clone)]
pub struct GcpAccess {
    /// `http://host[:port]` of the TLS-terminating proxy. `None` means
    /// unconfigured, which is what makes an adapter report itself unavailable
    /// rather than guess at an address.
    base_url: Option<Url>,
    /// Where a bearer token comes from. `None` is unconfigured, not "this API
    /// needs no credential" — no Google API does.
    tokens: Option<Arc<dyn TokenSource>>,
    /// What this process will hold and how long it will wait. The peer decides
    /// how much to send; these decide how much of it matters.
    limits: ClientLimits,
}

impl std::fmt::Debug for GcpAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcpAccess")
            .field("base_url", &self.base_url.as_ref().map(Url::to_string))
            .field("tokens", &self.tokens)
            .field("limits", &self.limits)
            .finish()
    }
}

impl Default for GcpAccess {
    fn default() -> Self {
        Self::unconfigured()
    }
}

impl GcpAccess {
    /// Access that cannot reach anything, and says so.
    ///
    /// This exists so that an adapter can be constructed, listed and asked what
    /// it needs without a credential being present. A component that cannot
    /// work still has to exist in order to report why.
    pub fn unconfigured() -> Self {
        Self {
            base_url: None,
            tokens: None,
            limits: default_limits(),
        }
    }

    /// Point this access at a TLS-terminating proxy.
    ///
    /// Fails on an address that is present and wrong, so a typo is caught where
    /// it was configured rather than on the first request. An `https://` URL is
    /// refused here by [`qip_transport::Url`], with an error that explains the
    /// missing TLS stack — that refusal is the whole safety property, so it is
    /// deliberately not softened.
    pub fn with_endpoint(mut self, base_url: &str) -> Result<Self> {
        self.base_url = Some(Url::parse(base_url).map_err(Error::from)?);
        Ok(self)
    }

    /// Resolve the endpoint and credential from values a composition root
    /// read.
    ///
    /// Values, not variables: this crate does not read the process
    /// environment. A credential is a property of the deployment, never of
    /// the build, and the composition root is the one place that may read
    /// one — [`crate::managed::ManagedSettings::from_env`] is where the
    /// variables below are looked up, and where `QIP_GCP_ACCESS_TOKEN` goes
    /// through `qip_core::secret` so it may arrive as a mounted file. Taking
    /// values is also what lets the resolution rules be tested without a test
    /// mutating the process environment, which is shared by every test in the
    /// binary.
    ///
    /// * `endpoint` — [`ENDPOINT_VARIABLE`], `http://…` of the TLS-terminating
    ///   proxy.
    /// * exactly one of:
    ///   * `metadata` — [`METADATA_VARIABLE`] set to `1`: take tokens from the
    ///     instance metadata server, which is the whole answer on GCE, GKE and
    ///     Cloud Run and needs no key material anywhere.
    ///   * `token_file` — [`TOKEN_FILE_VARIABLE`], a path something else
    ///     keeps fresh, re-read before every request.
    ///   * `token` — [`TOKEN_VARIABLE`], a literal token. Correct for an
    ///     operator task and wrong for a long-running process, because it
    ///     expires in about an hour and nothing here will notice.
    ///
    /// Requiring exactly one is deliberate. Two credentials configured at once
    /// means somebody changed how this deployment authenticates and did not
    /// finish; silently preferring one of them would make which token is
    /// actually presented depend on the order of a `match`.
    ///
    /// `clock` is used only by the metadata source, to decide when a cached
    /// token is close enough to its expiry to refetch. It is injected rather
    /// than read from the host so a replayed run sees the same boundary.
    pub fn from_values(
        endpoint: Option<&str>,
        metadata: Option<&str>,
        token_file: Option<&str>,
        token: Option<&str>,
        clock: Arc<dyn qip_core::Clock>,
    ) -> Result<Self> {
        let mut access = Self::unconfigured();
        if let Some(endpoint) = endpoint.map(str::trim).filter(|v| !v.is_empty()) {
            access = access.with_endpoint(endpoint)?;
        }

        let use_metadata = metadata
            .map(str::trim)
            .is_some_and(|v| matches!(v, "1" | "true" | "TRUE" | "yes" | "on"));
        let token_file = token_file.map(str::trim).filter(|v| !v.is_empty());
        let token = token.map(str::trim).filter(|v| !v.is_empty());

        let chosen = usize::from(use_metadata)
            + usize::from(token_file.is_some())
            + usize::from(token.is_some());
        if chosen > 1 {
            return Err(Error::invalid(format!(
                "more than one GCP credential is configured ({METADATA_VARIABLE}, \
                 {TOKEN_FILE_VARIABLE}, {TOKEN_VARIABLE}): set exactly one. Preferring one of \
                 them silently would make which token is presented depend on the order of a \
                 match rather than on what the deployment meant"
            )));
        }
        if use_metadata {
            access = access.with_tokens(Arc::new(auth::MetadataServerTokens::new(clock)));
        } else if let Some(path) = token_file {
            access = access.with_tokens(Arc::new(auth::TokenFile::new(path)));
        } else if let Some(token) = token {
            access = access.with_tokens(Arc::new(auth::StaticToken::new(token)?));
        }
        Ok(access)
    }

    /// Supply the bearer token source. See [`auth`].
    pub fn with_tokens(mut self, tokens: Arc<dyn TokenSource>) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Override the transport limits.
    pub fn with_limits(mut self, limits: ClientLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> ClientLimits {
        self.limits
    }

    pub fn endpoint(&self) -> Option<&Url> {
        self.base_url.as_ref()
    }

    /// Whether both halves are present.
    pub fn is_configured(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// What has not been supplied, each named on its own.
    ///
    /// Separately rather than as one "not configured", so an operator with the
    /// endpoint but no credential is told which one is left.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.base_url.is_none() {
            missing.push(
                "no endpoint: set the `http://` address of a TLS-terminating proxy that forwards \
                 to the Google API. This build has no TLS stack and refuses `https` by name, so \
                 there is no address of Google's that can be used directly, and a bearer token \
                 sent to a public endpoint in clear text is a credential given away"
                    .to_string(),
            );
        }
        if self.tokens.is_none() {
            missing.push(
                "no credential: supply a `TokenSource`. Minting a token means RS256-signing a \
                 JWT with a service-account key, which this workspace cannot do — ADR 0009 \
                 forbids in-tree cryptography and the dependency policy permits only serde. Use \
                 `MetadataServerTokens` on GCE/GKE/Cloud Run, or `TokenFile` pointed at a path \
                 something else keeps fresh"
                    .to_string(),
            );
        }
        missing
    }

    /// The bearer token for the next request, or the reason there is none.
    fn token(&self) -> Result<AccessToken> {
        let Some(source) = &self.tokens else {
            return Err(Error::unavailable(
                "no token source is configured, so no request will be made: see \
                 `GcpAccess::with_tokens`",
            ));
        };
        source.token()
    }

    /// Build a request against `path_and_query`, carrying the credential.
    ///
    /// The token goes in the `Authorization` header and never in the URL. A URL
    /// is written to every access log on the path — the proxy's, Google's, and
    /// whatever is in between — and a credential in one is a credential in all
    /// of them, recoverable long after it should have been rotated.
    fn request(&self, method: Method, path_and_query: &str) -> Result<HttpRequest> {
        let Some(base) = &self.base_url else {
            return Err(Error::unavailable(
                "no endpoint is configured, so no connection will be opened: see \
                 `GcpAccess::with_endpoint`",
            ));
        };
        // Ask for the credential before building anything: a request that
        // cannot be authenticated must not reach the point of being sent.
        let token = self.token()?;
        let url = base.with_path(path_and_query).map_err(Error::from)?;
        Ok(HttpRequest::new(method, &url.to_string())
            .map_err(Error::from)?
            .with_header("authorization", &token.header_value())
            .with_header("accept", "application/json"))
    }
}

/// What a non-2xx status from a Google API means.
///
/// Separated by class because the operator action differs and one error type
/// for all of them would put every case on the same runbook page: a rejected
/// credential is a deployment to fix, a 404 is a name to fix, a 429 is a quota
/// to wait for or raise, a 5xx is Google to wait for.
///
/// `service` names which adapter is speaking, so a message in a log says
/// whether it was the bucket or the warehouse that refused.
pub(crate) fn status_refusal(service: &str, status: u16, excerpt: &str) -> Error {
    match status {
        400 => Error::invalid(format!(
            "{service} rejected the request as malformed (HTTP 400): {excerpt}"
        )),
        401 => Error::denied(format!(
            "{service} did not accept this deployment's bearer token (HTTP 401). The token is \
             expired, absent or not a token; it is not quoted here and is not written to any log \
             by this crate"
        )),
        403 => Error::denied(format!(
            "{service} accepted the token and refused the operation (HTTP 403): the service \
             account is missing the IAM role for it, or the token was minted for the wrong \
             scope. {excerpt}"
        )),
        404 => Error::not_found(format!(
            "{service} has no such resource (HTTP 404): {excerpt}"
        )),
        409 => Error::invalid(format!(
            "{service} reports a conflict (HTTP 409): {excerpt}"
        )),
        408 | 429 => Error::unavailable(format!(
            "{service} is rate-limiting or timing out this deployment (HTTP {status}): {excerpt}"
        )),
        500..=599 => Error::unavailable(format!(
            "{service} failed to serve the request (HTTP {status}): {excerpt}"
        )),
        other => Error::invalid(format!(
            "{service} answered HTTP {other}, which this adapter does not know how to read: \
             {excerpt}"
        )),
    }
}

/// Percent-encode one path or query component.
///
/// Written here rather than taken as a dependency, which the policy would not
/// permit anyway. The rule is the conservative one: everything that is not an
/// RFC 3986 *unreserved* character is escaped. That escapes more than it
/// strictly must — `~` and `-` need no escaping and are left alone, while `!`
/// and `*` are escaped although some encoders leave them — and encoding too
/// much is safe where encoding too little is not.
///
/// `/` is escaped along with the rest, which is the point for Cloud Storage: an
/// object called `archive/2026/day.json` is one object whose *name* contains
/// slashes, not three path segments, and Google's API requires the name to
/// arrive as `archive%2F2026%2Fday.json`. Sending the raw slashes addresses a
/// resource that does not exist, and the 404 that comes back looks like a
/// missing object rather than like an encoding bug.
pub(crate) fn percent_encode(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for byte in component.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
