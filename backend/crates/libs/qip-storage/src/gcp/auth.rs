//! The bearer-token port, and the two sources that need no cryptography.
//!
//! # Why this is a port and not a credential minter
//!
//! Google's APIs want `Authorization: Bearer <token>`. The canonical way to
//! obtain one is to sign a JWT assertion with a service account's private key
//! and exchange it at the OAuth2 token endpoint. The signature is RS256 —
//! RSASSA-PKCS1-v1_5 over SHA-256 — and this workspace cannot produce it.
//! ADR 0009 forbids in-tree cryptography, and the dependency policy permits
//! only `serde` and `serde_json`, so there is no RSA implementation to reach
//! for and writing one would be the exact mistake that ADR names.
//!
//! This is stated plainly rather than worked around because the alternative
//! is worse in a specific way: an adapter that *appeared* to authenticate —
//! by shipping a stub signer, or by treating an absent credential as "no
//! credential needed" — would fail at the first real request with a 401 that
//! looks like a rotated key rather than like a build that never had a signer.
//!
//! So minting is the deployment's job and presenting is this crate's. A
//! deployment supplies a [`TokenSource`]; the adapters ask it for a token
//! before every request and refuse, opening no connection, when there is none.
//!
//! # What a deployment can use
//!
//! * [`MetadataServerTokens`] — on GCE, GKE, Cloud Run or any environment with
//!   a metadata server, this is a complete answer that needs no key material
//!   at all. The instance's attached service account is exchanged for a token
//!   by the platform, and this source fetches it over plain HTTP to a
//!   link-local address. See its documentation for why plaintext is correct
//!   there and only there.
//! * [`TokenFile`] — a path some other process keeps fresh: a GKE projected
//!   service-account token, a sidecar that runs `gcloud auth print-access-token`,
//!   a secrets agent. This crate reads; it never writes and never refreshes.
//! * [`StaticToken`] — one token fixed at construction. Correct for a test and
//!   for a short-lived operator task, and wrong for a long-running process,
//!   because a GCP access token expires in about an hour and this source will
//!   still be handing out the dead one afterwards. It says so in its own
//!   documentation rather than leaving that to be discovered.
//!
//! # What this module does not promise
//!
//! It does not refresh, it does not retry, and it does not inspect a token. A
//! token is an opaque string here: nothing parses it, so nothing knows its
//! expiry, its scopes or its subject, and no error from this module can tell a
//! caller that a token was *expired* rather than *rejected*. The one exception
//! is [`MetadataServerTokens`], which is told an expiry by the metadata server
//! and caches against it — and which believes what it is told.
//!
//! It does not scope-check. Presenting a token minted for the wrong scope
//! produces a 403 from Google, which the adapters surface as
//! [`qip_core::Error::Denied`]; nothing here can catch it earlier.

use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_transport::{ClientLimits, HttpClient, HttpRequest, Method};
use std::sync::{Arc, Mutex};

/// The default address of the GCP metadata server.
///
/// A link-local address rather than the `metadata.google.internal` name: the
/// name needs DNS, and a DNS failure inside a token fetch is an outage that
/// looks like an authentication failure.
pub const DEFAULT_METADATA_BASE: &str = "http://169.254.169.254";

/// Path of the default service account's token on the metadata server.
pub const DEFAULT_METADATA_TOKEN_PATH: &str =
    "/computeMetadata/v1/instance/service-accounts/default/token";

/// The header the metadata server requires on every request.
///
/// Its presence is what proves the request came from code running on the
/// instance rather than from a browser tricked into issuing it, so the server
/// refuses any request without it. It is not a secret and carries no value.
const METADATA_FLAVOR_HEADER: &str = "metadata-flavor";

/// How long before a token's stated expiry [`MetadataServerTokens`] refetches.
///
/// A token that expires between the moment it is read and the moment the
/// request carrying it reaches Google is indistinguishable from a rejected
/// one. Sixty seconds is longer than any request this crate will wait for.
///
/// Public because it is observable behaviour rather than an implementation
/// detail: it is how much of a token's stated lifetime this crate declines to
/// use, which an operator reading a metadata-server request rate will want to
/// account for.
pub const REFRESH_MARGIN: qip_core::Duration = qip_core::Duration::from_secs(60);

/// An OAuth2 bearer token, kept out of logs.
///
/// A newtype rather than a `String` so that the only way to see the value is
/// [`AccessToken::expose`], which is greppable. Its [`std::fmt::Debug`] prints
/// the length and nothing else: a credential that reaches a log line, a crash
/// dump or a support ticket is a credential that has to be rotated, and the
/// usual way that happens is a struct derived `Debug` two refactors away from
/// where anyone was thinking about secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken(String);

impl AccessToken {
    /// Wrap a token, refusing one that cannot travel in a header.
    ///
    /// A blank token is refused rather than sent, because an empty
    /// `Authorization` header produces a 401 that reads as a bad credential
    /// instead of as an absent one. A control character is refused because in
    /// a header value it would end the header and let whatever followed be
    /// read as another — the request-splitting bug, with a credential as the
    /// injection point.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::invalid(
                "the access token is blank: an absent credential is a `None` token source, not an \
                 empty string, so that the adapter refuses before it opens a connection rather \
                 than sending an empty Authorization header",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::invalid(
                "the access token contains a control character: in a header value it would end \
                 the header and let the rest be read as another one",
            ));
        }
        Ok(Self(value))
    }

    /// The token itself. Named to be conspicuous at a call site and in a grep.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The `Authorization` header value this token becomes.
    pub fn header_value(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AccessToken(<redacted, {} bytes>)", self.0.len())
    }
}

/// Where a bearer token comes from.
///
/// Asked once per request rather than once per adapter, so that a source
/// backed by something that rotates — a file, a metadata server — is picked up
/// without restarting the process holding the adapter.
pub trait TokenSource: Send + Sync + std::fmt::Debug {
    /// The token to present on the next request.
    ///
    /// Returning `Err` refuses the request before a connection is opened. An
    /// implementation that cannot currently produce a token must do that
    /// rather than return a stale or empty one.
    fn token(&self) -> Result<AccessToken>;
}

/// One token, fixed at construction.
///
/// Correct for a test, for a one-off operator task, and for nothing that runs
/// for an hour: a GCP access token's lifetime is around 3600 seconds, and this
/// source has no way to notice that its own has passed. It will keep presenting
/// the dead token, and every request will fail with a 403 that names a rejected
/// credential rather than an expired one, because nothing here parses the token
/// well enough to tell the difference. A long-running deployment wants
/// [`MetadataServerTokens`] or [`TokenFile`].
#[derive(Clone, Debug)]
pub struct StaticToken(AccessToken);

impl StaticToken {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        Ok(Self(AccessToken::new(token)?))
    }
}

impl TokenSource for StaticToken {
    fn token(&self) -> Result<AccessToken> {
        Ok(self.0.clone())
    }
}

/// A token read from a file that something else keeps fresh.
///
/// The file is read on **every** request rather than cached, so a refresher
/// that rewrites it is picked up immediately and this crate never has to guess
/// when the previous token died. The cost is one small local read per request,
/// which does not register beside the network round trip that follows it.
///
/// Leading and trailing whitespace is trimmed, because every way of producing
/// such a file — a shell redirect, a projected volume, an editor — is liable to
/// leave a trailing newline, and a newline in a header value is refused by
/// [`AccessToken::new`]. Nothing else about the contents is interpreted.
///
/// A missing or unreadable file is an error, not an absent credential: the
/// deployment said where the token lives, so the file not being there is a
/// broken deployment rather than an unconfigured one.
#[derive(Clone, Debug)]
pub struct TokenFile {
    path: std::path::PathBuf,
}

impl TokenFile {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl TokenSource for TokenFile {
    fn token(&self) -> Result<AccessToken> {
        let raw = std::fs::read_to_string(&self.path).map_err(|error| {
            Error::io(format!(
                "cannot read the access token from {}: {error}. The deployment named this path, \
                 so a token that is not there is a broken deployment rather than an \
                 unconfigured one",
                self.path.display()
            ))
        })?;
        AccessToken::new(raw.trim())
    }
}

/// Tokens fetched from the GCP metadata server.
///
/// This is the source that needs no key material anywhere: the instance's
/// attached service account is exchanged for a token by the platform, and this
/// asks for the result. On GCE, GKE, Cloud Run and Cloud Functions it is the
/// whole of what a deployment has to arrange.
///
/// # Why plaintext HTTP is correct here and nowhere else
///
/// [`DEFAULT_METADATA_BASE`] is `169.254.169.254`, a link-local address that is
/// not routed off the host. The response never crosses a network anyone else
/// can reach, so the absence of TLS — which this build has no stack for — costs
/// nothing here. That reasoning is specific to this address: it does not
/// transfer to `storage.googleapis.com`, which is why the adapters in the sibling
/// modules require a TLS-terminating proxy and this source does not.
///
/// # Caching, and the clock it uses
///
/// A token is cached until [`REFRESH_MARGIN`] before the expiry the metadata
/// server stated, then refetched. The margin exists because a token that dies
/// in flight is indistinguishable from one that was rejected.
///
/// The expiry is compared against an injected [`Clock`], never
/// `SystemTime::now()`, so that a simulated or replayed run sees the same
/// refresh boundary it saw the first time.
///
/// # What it does not promise
///
/// It believes the metadata server about `expires_in`. It does not parse the
/// token to check. It does not retry: a failed fetch fails the request that
/// triggered it, and the next request tries again. And it caches per instance
/// of this type, not per process — two of these are two caches.
pub struct MetadataServerTokens {
    base_url: String,
    path: String,
    client: HttpClient,
    clock: Arc<dyn Clock>,
    /// The token and the instant it stops being usable.
    cached: Mutex<Option<(AccessToken, Timestamp)>>,
}

impl std::fmt::Debug for MetadataServerTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cached = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f.debug_struct("MetadataServerTokens")
            .field("base_url", &self.base_url)
            .field("path", &self.path)
            // Whether a token is held is worth knowing; the token never is.
            .field("cached", &cached.as_ref().map(|(_, until)| *until))
            .finish_non_exhaustive()
    }
}

impl MetadataServerTokens {
    /// A source pointed at the standard metadata address.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::at(DEFAULT_METADATA_BASE, DEFAULT_METADATA_TOKEN_PATH, clock)
    }

    /// A source pointed somewhere else — a test's loopback listener, or a
    /// deployment that reaches its metadata service through a known proxy.
    pub fn at(base_url: impl Into<String>, path: impl Into<String>, clock: Arc<dyn Clock>) -> Self {
        Self {
            base_url: base_url.into(),
            path: path.into(),
            // A metadata fetch is a few hundred bytes from a host on the same
            // machine. The limits are correspondingly tight: anything that
            // needs more than this is not the metadata server answering.
            client: HttpClient::new(ClientLimits {
                max_body: 64 * 1024,
                max_headers: 32,
                connect_timeout: std::time::Duration::from_secs(2),
                read_timeout: std::time::Duration::from_secs(5),
                write_timeout: std::time::Duration::from_secs(5),
                ..ClientLimits::default()
            }),
            clock,
            cached: Mutex::new(None),
        }
    }

    /// Drop the cached token, so the next call refetches.
    ///
    /// For an operator who has just changed the instance's service account and
    /// does not want to wait out the previous token.
    pub fn invalidate(&self) {
        *self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn fetch(&self) -> Result<(AccessToken, Timestamp)> {
        let target = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            if self.path.starts_with('/') {
                self.path.clone()
            } else {
                format!("/{}", self.path)
            }
        );
        let request = HttpRequest::new(Method::Get, &target)
            .map_err(Error::from)?
            .with_header("accept", "application/json")
            .with_header(METADATA_FLAVOR_HEADER, "Google");
        let response = self.client.send(&request).map_err(Error::from)?;
        if !response.is_success() {
            return Err(Error::denied(format!(
                "the metadata server at {target} answered HTTP {} when asked for an access \
                 token: {}. On an instance with no attached service account this is what that \
                 looks like",
                response.status,
                response.body_excerpt()
            )));
        }
        let body = response.body_as_str().map_err(Error::from)?;
        let wire: MetadataToken = serde_json::from_str(body).map_err(|error| {
            Error::schema(format!(
                "the metadata server sent a token document this decoder cannot read: {error}. \
                 The first bytes of it were: {}",
                response.body_excerpt()
            ))
        })?;
        let token = AccessToken::new(wire.access_token)?;
        // Saturating rather than wrapping: a metadata server claiming an
        // absurd lifetime should push the refresh far out, not wrap it into
        // the past and spin.
        // `Duration` counts in `i64` seconds and the wire field is unsigned:
        // a server claiming a lifetime past `i64::MAX` gets clamped rather
        // than wrapped into the past, which would make every call refetch.
        let lifetime =
            qip_core::Duration::from_secs(i64::try_from(wire.expires_in).unwrap_or(i64::MAX));
        let expires_at = self.clock.now().saturating_add(lifetime);
        Ok((token, expires_at.saturating_sub(REFRESH_MARGIN)))
    }
}

impl TokenSource for MetadataServerTokens {
    fn token(&self) -> Result<AccessToken> {
        let mut guard = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((token, usable_until)) = guard.as_ref()
            && self.clock.now() < *usable_until
        {
            return Ok(token.clone());
        }
        let (token, usable_until) = self.fetch()?;
        *guard = Some((token.clone(), usable_until));
        Ok(token)
    }
}

/// What the metadata server's token endpoint returns.
///
/// `token_type` is present in the response and deliberately not read: it is
/// always `Bearer`, and a decoder that refused an unexpected value there would
/// turn a harmless addition into an outage.
#[derive(Debug, serde::Deserialize)]
struct MetadataToken {
    access_token: String,
    /// Seconds from now, as the server counts them.
    expires_in: u64,
}
