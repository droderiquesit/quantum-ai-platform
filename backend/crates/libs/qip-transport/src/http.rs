//! A minimal HTTP/1.1 client.
//!
//! The other half of `qip_api::http`, written against `std::net` for the same
//! reason that server is: what the platform needs of HTTP is a request line,
//! some headers, a body and a response, and a dependency has to earn its place.
//!
//! # The limits are the interesting part, again
//!
//! The server's limits exist because an unauthenticated caller can consume its
//! resources. A client's limits exist because **the peer is the untrusted
//! party here**: a peer that answers with a header list that never ends, a
//! `content-length` of four gigabytes, a chunked body that never sends its
//! terminating chunk, or a connection that accepts the request and then says
//! nothing, is a peer that kills this process rather than its own. Every limit
//! below is one of those, and each is enforced *while reading* rather than
//! after, so an oversized response is refused before it has been buffered.
//!
//! # One connection per request
//!
//! No keep-alive, no pooling. `qip_api::http` writes `connection: close` on
//! every response, so a client that held the socket open would be waiting for
//! bytes the server has already decided not to send. Connection reuse is worth
//! having when a peer supports it; adding it before the peer does would be
//! untested code on the path that moves money.
//!
//! # There is no TLS here
//!
//! [`Url::parse`] refuses `https` by name rather than silently downgrading it.
//! See the crate documentation for the deployment boundary that makes plaintext
//! acceptable and for what production has to add.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration as StdDuration;

/// Limits applied to every response this client reads.
///
/// Defaults chosen for mesh traffic inside one VPC: small documents, a peer a
/// few milliseconds away, and a failure that should be visible in seconds
/// rather than minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientLimits {
    /// Largest status line accepted.
    pub max_status_line: usize,
    /// Largest single header line.
    pub max_header_line: usize,
    /// Most response headers accepted. An unbounded header list is an
    /// unbounded allocation, and it is the peer that chooses how many to send.
    pub max_headers: usize,
    /// Largest response body accepted, however it is framed.
    pub max_body: usize,
    /// Largest single chunk a chunked response may declare. A chunk header is
    /// a hexadecimal number the peer picks, so it is refused before the
    /// allocation rather than after it.
    pub max_chunk: usize,
    /// How long the TCP handshake may take.
    pub connect_timeout: StdDuration,
    /// How long the peer may go silent mid-response before the read fails.
    pub read_timeout: StdDuration,
    /// How long writing the request may take.
    pub write_timeout: StdDuration,
}

impl Default for ClientLimits {
    fn default() -> Self {
        Self {
            max_status_line: 8 * 1024,
            max_header_line: 8 * 1024,
            max_headers: 64,
            max_body: 4 * 1024 * 1024,
            max_chunk: 1024 * 1024,
            connect_timeout: StdDuration::from_secs(2),
            read_timeout: StdDuration::from_secs(15),
            write_timeout: StdDuration::from_secs(30),
        }
    }
}

/// Where in a response a failure happened.
///
/// Carried on the error rather than folded into its message because "the peer
/// went quiet" means something different at each of these points: during the
/// status line it is a peer that accepted a connection and never answered,
/// during the body it is a peer that answered and then died.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    StatusLine,
    Headers,
    Body,
    ChunkHeader,
    Trailers,
}

impl Phase {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::StatusLine => "the status line",
            Self::Headers => "the headers",
            Self::Body => "the body",
            Self::ChunkHeader => "a chunk header",
            Self::Trailers => "the trailers",
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every way a request can fail, named.
///
/// A single stringly-typed error would make the retry decision impossible:
/// [`Self::is_transient`] is what separates "the peer is restarting, try again"
/// from "the peer answered with something this client cannot parse, and trying
/// again will produce the same thing".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// The URL could not be parsed.
    InvalidUrl { url: String, detail: String },
    /// A scheme this build cannot speak. `https` is the one that matters.
    ///
    /// `scheme` can only ever be a token matching the RFC 3986 scheme
    /// grammar — `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`, so never `@`,
    /// `:` or `/` — because [`split_scheme`] is the only function that
    /// produces the `Some` this variant is built from, and it stops
    /// advancing at the first byte outside that grammar. Until 2026-09-13
    /// this was not true: the unbounded `raw.split_once("://")` it replaced
    /// could hand this variant an entire scheme-less credential-bearing
    /// string as its "scheme" — see [`split_scheme`]'s doc comment — and
    /// this variant's `Display` prints `scheme` outright, with no call to
    /// [`redact_userinfo`], because a value that can only be a clean scheme
    /// token needs none. That was the second, structurally separate finding
    /// of this round: fixing [`redact_userinfo`] alone would have left this
    /// arm printing the same credential by a different path.
    UnsupportedScheme { scheme: String },
    /// DNS said no.
    Resolve { authority: String, detail: String },
    /// DNS said nothing: a name that resolves to an empty set.
    NoAddress { authority: String },
    /// The connection was refused, unreachable or reset during the handshake.
    ConnectFailed { address: String, detail: String },
    /// The handshake did not complete inside [`ClientLimits::connect_timeout`].
    ConnectTimeout {
        authority: String,
        after: StdDuration,
    },
    /// The request could not be written.
    WriteFailed { detail: String },
    /// The peer went silent for longer than [`ClientLimits::read_timeout`].
    ReadTimeout { phase: Phase, after: StdDuration },
    /// The peer closed the connection part-way through its own response.
    ClosedEarly { phase: Phase },
    /// The socket failed for a reason that is neither a timeout nor a clean
    /// close.
    ReadFailed { phase: Phase, detail: String },
    /// The bytes arrived and are not HTTP this client can parse.
    Malformed { phase: Phase, detail: String },
    /// The response body exceeded [`ClientLimits::max_body`]. `at_least`
    /// records what was known when the refusal happened — the declared length
    /// where one was declared, otherwise how much had been read.
    BodyTooLarge { limit: usize, at_least: usize },
    /// A header line exceeded [`ClientLimits::max_header_line`], or the status
    /// line exceeded [`ClientLimits::max_status_line`].
    LineTooLong { phase: Phase, limit: usize },
    /// More headers than [`ClientLimits::max_headers`].
    TooManyHeaders { limit: usize },
}

impl HttpError {
    /// Whether repeating the identical request could plausibly succeed.
    ///
    /// The distinction is not cosmetic. A transient error goes back on the
    /// retry ladder; a permanent one goes straight to the dead-letter path,
    /// because spending five attempts and thirty seconds of backoff on a
    /// response this client will never be able to parse delays every message
    /// behind it for nothing.
    pub const fn is_transient(&self) -> bool {
        match self {
            Self::Resolve { .. }
            | Self::NoAddress { .. }
            | Self::ConnectFailed { .. }
            | Self::ConnectTimeout { .. }
            | Self::WriteFailed { .. }
            | Self::ReadTimeout { .. }
            | Self::ClosedEarly { .. }
            | Self::ReadFailed { .. } => true,
            Self::InvalidUrl { .. }
            | Self::UnsupportedScheme { .. }
            | Self::Malformed { .. }
            | Self::BodyTooLarge { .. }
            | Self::LineTooLong { .. }
            | Self::TooManyHeaders { .. } => false,
        }
    }

    /// A stable machine-readable code, for metrics and for tests that assert
    /// which failure mode was reached rather than matching on prose.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl { .. } => "invalid_url",
            Self::UnsupportedScheme { .. } => "unsupported_scheme",
            Self::Resolve { .. } => "resolve",
            Self::NoAddress { .. } => "no_address",
            Self::ConnectFailed { .. } => "connect_failed",
            Self::ConnectTimeout { .. } => "connect_timeout",
            Self::WriteFailed { .. } => "write_failed",
            Self::ReadTimeout { .. } => "read_timeout",
            Self::ClosedEarly { .. } => "closed_early",
            Self::ReadFailed { .. } => "read_failed",
            Self::Malformed { .. } => "malformed",
            Self::BodyTooLarge { .. } => "body_too_large",
            Self::LineTooLong { .. } => "line_too_long",
            Self::TooManyHeaders { .. } => "too_many_headers",
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl { url, detail } => write!(f, "{url} is not a usable URL: {detail}"),
            Self::UnsupportedScheme { scheme } => write!(
                f,
                "the {scheme} scheme is not supported: this build has no TLS stack, and a client \
                 that quietly downgraded to plaintext would be worse than one that refuses"
            ),
            Self::Resolve { authority, detail } => {
                write!(f, "cannot resolve {authority}: {detail}")
            }
            Self::NoAddress { authority } => {
                write!(f, "{authority} resolved to no addresses")
            }
            Self::ConnectFailed { address, detail } => {
                write!(f, "cannot connect to {address}: {detail}")
            }
            Self::ConnectTimeout { authority, after } => write!(
                f,
                "connecting to {authority} did not complete within {after:?}"
            ),
            Self::WriteFailed { detail } => write!(f, "cannot write the request: {detail}"),
            Self::ReadTimeout { phase, after } => {
                write!(
                    f,
                    "the peer sent nothing for {after:?} while reading {phase}"
                )
            }
            Self::ClosedEarly { phase } => {
                write!(f, "the peer closed the connection while sending {phase}")
            }
            Self::ReadFailed { phase, detail } => {
                write!(f, "the connection failed while reading {phase}: {detail}")
            }
            Self::Malformed { phase, detail } => {
                write!(f, "{phase} is not valid HTTP/1.1: {detail}")
            }
            Self::BodyTooLarge { limit, at_least } => write!(
                f,
                "the response body is at least {at_least} bytes and the limit is {limit}: a peer \
                 that streams without end must not be able to exhaust this process"
            ),
            Self::LineTooLong { phase, limit } => {
                write!(f, "{phase} exceeded the {limit} byte limit")
            }
            Self::TooManyHeaders { limit } => {
                write!(f, "the response carried more than {limit} headers")
            }
        }
    }
}

impl std::error::Error for HttpError {}

/// The platform error an `HttpError` becomes when it crosses into code that
/// speaks `qip_core::Result`.
///
/// The mapping is deliberate: a tripped limit is a [`qip_core::Error::Guard`]
/// rather than an I/O failure, because the peer did nothing wrong at the socket
/// level — this client refused what it sent.
impl From<HttpError> for qip_core::Error {
    fn from(error: HttpError) -> Self {
        let message = error.to_string();
        match error {
            HttpError::InvalidUrl { .. } | HttpError::UnsupportedScheme { .. } => {
                Self::invalid(message)
            }
            HttpError::ConnectTimeout { .. } | HttpError::ReadTimeout { .. } => {
                Self::timeout(message)
            }
            HttpError::Malformed { .. } => Self::schema(message),
            HttpError::BodyTooLarge { .. }
            | HttpError::LineTooLong { .. }
            | HttpError::TooManyHeaders { .. } => Self::guard(message),
            HttpError::Resolve { .. }
            | HttpError::NoAddress { .. }
            | HttpError::ConnectFailed { .. }
            | HttpError::WriteFailed { .. }
            | HttpError::ClosedEarly { .. }
            | HttpError::ReadFailed { .. } => Self::io(message),
        }
    }
}

type HttpResult<T> = std::result::Result<T, HttpError>;

/// An HTTP method. The same closed set the server parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            _ => None,
        }
    }

    /// Whether a response to this method may carry a body at all.
    pub const fn expects_response_body(&self) -> bool {
        !matches!(self, Self::Head)
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The scheme prefix of `raw`, if RFC 3986 syntax puts one there, and the
/// remainder after the `"://"` that ends it — or `None` and `raw` unchanged
/// when no such prefix exists.
///
/// A URI scheme is `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` and, by that
/// grammar, can only ever be the literal prefix of the string — RFC 3986
/// does not admit a scheme found by scanning the tail. Both
/// [`Url::parse`] and [`redact_userinfo`] used to run their own
/// `raw.split_once("://")` to find it, and `split_once` finds the *first*
/// occurrence anywhere in the string, not the one at the start. Two
/// independent detectors that each searched the whole string is the exact
/// shape of the defect this closes: a fix landing in one of them — as it did
/// on 2026-09-12, in `redact_userinfo` alone — leaves the other to
/// misparse the identical string its own way. A scheme-less credential
/// whose path or query happens to contain the ordinary substring `"://"`
/// (`?redirect=http://…`, `?callback=http://…` — any parameter naming
/// another URL) supplies exactly such a later occurrence: `split_once`
/// matched on it, so almost the whole string up to that point — credential
/// included — was misread as "the scheme", which [`redact_userinfo`] then
/// had no reason to look inside for an `'@'`, and which
/// [`HttpError::UnsupportedScheme`] carried and printed outright once the
/// real parser failed the same way. Anchoring the check to the start, and
/// giving both callers this one function to anchor it in, removes the
/// "two detectors that can disagree" shape rather than repairing one more
/// instance of it.
fn split_scheme(raw: &str) -> (Option<&str>, &str) {
    let is_scheme_char = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.');
    let bytes = raw.as_bytes();
    let scheme_len = if bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        bytes.iter().position(|&b| !is_scheme_char(b))
    } else {
        // RFC 3986 requires a scheme to start with a letter. A string
        // starting with anything else — a digit, a colon, an `@` — has no
        // scheme at all, however much of it happens to precede a `"://"`
        // later on.
        None
    };
    match scheme_len {
        Some(len) if raw[len..].starts_with("://") => (Some(&raw[..len]), &raw[len + 3..]),
        _ => (None, raw),
    }
}

/// `raw` with any userinfo in its authority replaced by `…@`, for echoing
/// an address in a refusal.
///
/// A URL carrying userinfo is refused by [`Url::parse`] precisely because a
/// credential in the URL lands in every line that records it — and until
/// 2026-09-12 the refusal itself echoed the whole URL, so
/// `http://svc:TOKEN@127.0.0.1:9105` set in a deployment put `TOKEN` on
/// stderr and into Cloud Logging at start-up, from the one check that
/// existed to keep it out of there. Everything through the last `@` of the
/// authority goes, so no fragment of a credential that itself contains an
/// `@` survives; the host after it is kept, because that is the part an
/// operator needs to see to find the mistake. A URL with no userinfo is
/// returned unchanged. Pure text, no parse: it must work on exactly the
/// addresses the parser refuses.
///
/// **A `"://"` is not required to find the authority.** Until 2026-09-12
/// this function returned `raw` verbatim whenever `"://"` was absent, on the
/// premise that every credential-bearing string this process handles has a
/// scheme. That premise was wrong: an operator who sets
/// `QIP_LANGUAGE_MODEL_BASE_URL=svc:TOKEN@127.0.0.1:9106` (the `http://`
/// dropped by mistake) produces exactly the shape this function existed to
/// catch, and both call sites in [`require_loopback_egress`] and both
/// invocations inside [`Url::parse`]'s own `invalid` closure fed it straight
/// back to the caller — the fix that was supposed to keep `TOKEN` out of the
/// log printed it twice. So: treat everything up to the first `/`, `?` or
/// `#` as the candidate authority whether or not a scheme was found, and
/// redact it the same way either way. A string with no such delimiter is
/// entirely a candidate authority (matching `require_loopback_egress`'s
/// `"svc:TOKEN@127.0.0.1:9106"` scenario, which has none of the three).
///
/// **The scheme is found by [`split_scheme`], not by searching for `"://"`
/// in the whole string.** Until 2026-09-13 this function ran its own
/// `raw.split_once("://")`, which finds the first `"://"` *anywhere* —
/// including one an adversarial or merely ordinary query parameter puts
/// well past the authority, `?redirect=http://…` being the unremarkable
/// case. On `svc:TOKEN@127.0.0.1:9106/callback?redirect=http://evil.example/x`
/// that matched the query's `"://"`, not the (absent) scheme boundary, so
/// this function read everything up to it — `svc:TOKEN@127.0.0.1:9106/`
/// `callback?redirect=http` — as a "scheme", handed the rest to the
/// authority search, and found no `'@'` there because the real one was
/// buried inside the misread "scheme". `TOKEN` came back unredacted. See
/// [`split_scheme`] for why anchoring to the start closes this rather than
/// only patching the one input that was reproduced.
///
/// **An authority-candidate that could never be a real host does not prove
/// there is no credential.** Until 2026-09-13 this function trusted "no `@`
/// before the first `/`, `?` or `#`" unconditionally: when no scheme is
/// found, that first delimiter can be the *very first character* of the
/// whole string — an operator's `://` typo'd down to `//`, `/`, `?` or `#`
/// with the scheme dropped entirely, e.g. `//svc:TOKEN@127.0.0.1:9106` or
/// `/TOKEN@127.0.0.1:9106` — which makes the "authority" an empty string,
/// or (`://svc:TOKEN@…`, where the candidate is just `:`) a string that
/// cannot be a host at all. Neither is a real authority under any URL
/// grammar, so "no `@` in it" said nothing about the credential sitting
/// one character later, past the delimiter, and this function returned
/// `raw` untouched — six such inputs, all reachable through
/// `QIP_LANGUAGE_MODEL_BASE_URL`, all still leaking their token.
///
/// The fix is narrower than "search the whole remainder for `@`": that
/// would also catch `127.0.0.1:9105/path@notacredential`, whose authority
/// candidate (`127.0.0.1:9105`) *is* a real, complete host — a case
/// [`redact_userinfo_handles_the_full_adversarial_matrix`] pins as
/// intentional, because a scheme-less host that already looks like a
/// legitimate `host[:port]` followed by a `/path` containing an ordinary
/// `@` is not a credential and must not be over-redacted. `authority` is
/// widened past only when [`split_authority`] cannot read it as a
/// non-empty host at all — which is true of `""` and `":"`, and false of
/// both `127.0.0.1:9105` and, deliberately, the bare word `svc` (a
/// single-label hostname is legal, so `svc/TOKEN@127.0.0.1:9106` is
/// indistinguishable, by this function's own grammar, from a host named
/// `svc` followed by a path — this is a documented residual, not a silent
/// one; see [`redact_userinfo_handles_the_full_adversarial_matrix`]'s own
/// case for it).
pub fn redact_userinfo(raw: &str) -> String {
    let (scheme, rest) = split_scheme(raw);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, remainder) = rest.split_at(end);
    match authority.rsplit_once('@') {
        Some((_, host)) => match scheme {
            Some(scheme) => format!("{scheme}://…@{host}{remainder}"),
            None => format!("…@{host}{remainder}"),
        },
        None if authority_could_be_a_host(authority) => raw.to_string(),
        None => match rest.rfind('@') {
            None => raw.to_string(),
            Some(at) => {
                let after = &rest[at + 1..];
                let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
                let (host, tail) = after.split_at(host_end);
                match scheme {
                    Some(scheme) => format!("{scheme}://…@{host}{tail}"),
                    None => format!("…@{host}{tail}"),
                }
            }
        },
    }
}

/// Whether `candidate` could be a real authority's host on its own — the
/// same `host[:port]` grammar [`Url::parse`] parses with, via
/// [`split_authority`], requiring a non-empty host. Not a judgement about
/// how *likely* a string is to be a scheme typo: `"svc"` passes, same as
/// `"127.0.0.1:9105"`, because a bare single-label hostname is a legal
/// authority and this function is a display helper, not the parser — it
/// exists only to tell an authority-candidate that cannot be a host at all
/// (`""`, or anything [`split_authority`] refuses, such as `":"`) from one
/// that already is a complete host, so [`redact_userinfo`] knows which of
/// the two it is looking at before deciding whether to search further.
fn authority_could_be_a_host(candidate: &str) -> bool {
    matches!(split_authority(candidate), Some((host, _)) if !host.is_empty())
}

/// The one host an egress address may name: the proxy's loopback listener,
/// as a literal address.
///
/// The literal and not `localhost`, which the connector gate admitted until
/// 2026-09-12. `localhost` is a name the resolver answers — from
/// `/etc/hosts`, or whatever `nsswitch.conf` names — and nothing in this
/// process verifies that the answer is loopback; one hosts-file line,
/// writable by whoever can write the image, would send a plaintext request
/// carrying a credential off the instance under a name that reads as safe.
/// `127.0.0.1` is an address this client connects to without a lookup. It
/// is also the only spelling `infrastructure/terraform/variables.tf`
/// admits, so the gate in the process and the gate at plan time agree on
/// what loopback is.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// Refuse a base URL that is not the egress proxy on loopback, with a port.
///
/// This client speaks plaintext HTTP/1.1 and has no TLS stack (ADR 0009),
/// so the only address a request carrying a credential may be pointed at is
/// a loopback listener of the egress proxy that terminates TLS to the
/// vendor (ADR 0024): `http://127.0.0.1:<port>`, and nothing else — see
/// [`LOOPBACK_HOST`] for why not `localhost`. Three refusals, in order:
///
/// * **`https://`**, by name, because the deployment mistake it signals —
///   the vendor's own address — deserves a message naming the proxy rather
///   than the parser's "unsupported scheme".
/// * **Anything [`Url::parse`] refuses**, and the parse is this client's own:
///   the first version of this gate lived in `qip-market-ingestion`, split
///   the authority on its last colon and read
///   `http://127.0.0.1:9105@evil.example/` as loopback; the parser refused
///   that address later, on its userinfo, so no socket was opened — but a
///   gate that disagrees with the client it guards about what an address
///   *is* will one day admit what the client refuses, or refuse what it
///   would have opened. Now the host this gate checks is the host the
///   client would connect to, by construction, which is why the gate lives
///   beside the parser.
/// * **A host other than the literal, or no explicit port.** The proxy's
///   listeners are one per vendor on distinct ports; an address naming no
///   port would reach whatever answers on loopback port 80, and Terraform's
///   validation (`startswith("http://127.0.0.1:")`) has never admitted one,
///   so until 2026-09-12 the process was again the wider of the two gates.
///
/// Every echo of the address goes through [`redact_userinfo`], so a refusal
/// of `http://svc:TOKEN@…` does not print `TOKEN`. Only the first two arms
/// can meet a credential — the parser refuses userinfo before the host and
/// port arms run — but all four echo the redacted form, so a reordering
/// cannot reopen the leak. Called at the seams that
/// carry a credential — the connector pair in the API's, the deep brain's
/// and the fast brain's parsers and at `ConnectorFeed::open`; the deep
/// brain's hosted language-model listener; the fast brain's market-data
/// vendor — so the refusal names the variable there and the type here.
/// Refuses rather than rewriting: an address that is nearly right is a
/// deployment mistake somebody should see.
pub fn require_loopback_egress(base_url: &str) -> qip_core::Result<()> {
    let shown = redact_userinfo(base_url);
    if base_url.starts_with("https://") {
        return Err(qip_core::Error::invalid(format!(
            "the egress address is {shown}. This transport speaks plaintext HTTP/1.1 and has \
             no TLS stack: point it at the egress proxy that terminates TLS to the vendor, \
             never at the vendor itself"
        )));
    }
    let url = Url::parse(base_url).map_err(|error| {
        qip_core::Error::invalid(format!(
            "the egress address {shown:?} is not an absolute http:// URL this transport would \
             open — {error}. The egress proxy is reached at http://127.0.0.1:<port>"
        ))
    })?;
    let host = url.host();
    if host != LOOPBACK_HOST {
        return Err(qip_core::Error::invalid(format!(
            "the egress address is {shown}, whose host is `{host}`. A vendor is reached only \
             through the egress proxy on loopback — http://127.0.0.1:<port>, the literal \
             address and not a name a resolver answers — which terminates TLS to the vendor \
             (ADR 0024) and reaches only the hosts its bootstrap names; this transport has no \
             TLS stack (ADR 0009), so a plaintext address off the instance would carry a \
             request in the clear to whatever answers there"
        )));
    }
    if !url.port_is_explicit() {
        return Err(qip_core::Error::invalid(format!(
            "the egress address is {shown}, which names no port. The egress proxy's listeners \
             are one per vendor on distinct ports, http://127.0.0.1:<port>; an address with no \
             port would reach whatever answers on loopback port 80, and it is not the address \
             Terraform admits"
        )));
    }
    Ok(())
}

/// An absolute `http://` URL, parsed once so a malformed peer address fails at
/// configuration time rather than on the first publish.
#[derive(Clone, Debug)]
pub struct Url {
    host: String,
    port: u16,
    /// Whether the port was written in the address rather than defaulted.
    /// Not part of what the URL *is* — `http://h` and `http://h:80` open
    /// the same socket — so [`PartialEq`] ignores it; see the manual impl.
    port_explicit: bool,
    target: String,
}

impl PartialEq for Url {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port && self.target == other.target
    }
}

impl Eq for Url {}

impl Url {
    /// Parse `http://host[:port][/path][?query]`.
    ///
    /// Refuses `https` by name, refuses userinfo (`http://user:pass@host` puts
    /// a credential in every log line that records the URL), and refuses a
    /// fragment (it is a client-side construct that never goes on the wire).
    /// The address the error carries has its userinfo redacted — see
    /// [`redact_userinfo`] — so the refusal of a credential-bearing URL is
    /// not itself the leak.
    ///
    /// The scheme boundary is found by [`split_scheme`] — the same function
    /// [`redact_userinfo`] uses — rather than by this function running its
    /// own `"://"` search, which is what let a scheme-less credential
    /// string be misread as carrying a scheme at all until 2026-09-13. A
    /// string [`split_scheme`] does not anchor a scheme onto now falls
    /// through to the "no scheme" arm below, whatever it contains further
    /// in, and that arm already redacts through [`redact_userinfo`].
    pub fn parse(raw: &str) -> HttpResult<Self> {
        let invalid = |detail: &str| HttpError::InvalidUrl {
            url: redact_userinfo(raw),
            detail: detail.to_string(),
        };

        let (scheme, rest) = split_scheme(raw);
        let Some(scheme) = scheme else {
            return Err(invalid("no scheme; an absolute http:// URL is required"));
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" {
            // `scheme` is a byte-for-byte slice of `raw` that `split_scheme`
            // stopped advancing through at the first character outside the
            // RFC 3986 scheme grammar, so it can hold neither `@`, `:` nor
            // `/` — see `HttpError::UnsupportedScheme`'s doc comment for why
            // that is exactly what makes printing it unredacted safe. This
            // is the structural guarantee rather than a trusted one: if a
            // future edit to `split_scheme` ever let a non-conforming byte
            // through, this is where it would be caught, in every debug and
            // test build, before it reached a caller that prints the field.
            debug_assert!(
                scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')),
                "split_scheme returned a scheme token outside the RFC 3986 grammar: {scheme:?}"
            );
            return Err(HttpError::UnsupportedScheme { scheme });
        }
        if rest.is_empty() {
            return Err(invalid("no host"));
        }

        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, remainder) = rest.split_at(end);
        if authority.contains('@') {
            return Err(invalid(
                "userinfo in a URL puts a credential into every log line that records it",
            ));
        }
        if remainder.contains('#') {
            return Err(invalid("a fragment is never sent on the wire"));
        }

        let (host, port) = split_authority(authority).ok_or_else(|| invalid("malformed host"))?;
        let port_explicit = port.is_some();
        let port = port.unwrap_or(80);
        if host.is_empty() {
            return Err(invalid("empty host"));
        }
        if host.chars().any(char::is_control) || host.contains(' ') {
            return Err(invalid("the host contains a control character or a space"));
        }

        let target = if remainder.is_empty() {
            "/".to_string()
        } else if remainder.starts_with('/') {
            remainder.to_string()
        } else {
            format!("/{remainder}")
        };
        if target.chars().any(char::is_control) || target.contains(' ') {
            return Err(invalid(
                "the path contains a control character or a space, which would split the request \
                 line",
            ));
        }

        Ok(Self {
            host,
            port,
            port_explicit,
            target,
        })
    }

    /// This URL's authority with `path` appended, for a base address plus an
    /// endpoint path. Any path or query on the base is replaced, not merged:
    /// merging two paths is where a peer address silently starts pointing
    /// somewhere else.
    pub fn with_path(&self, path: &str) -> HttpResult<Self> {
        let separator = if path.starts_with('/') { "" } else { "/" };
        Self::parse(&format!(
            "http://{}{separator}{path}",
            self.authority_for_url()
        ))
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Whether the address wrote its port rather than leaving it to the
    /// scheme's default. `http://127.0.0.1` and `http://127.0.0.1:80` open
    /// the same socket, but only the second names a listener on purpose,
    /// and [`require_loopback_egress`] holds an egress address to that.
    pub fn port_is_explicit(&self) -> bool {
        self.port_explicit
    }

    /// Path and query, as it goes on the request line.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// `host:port`, as it goes in the `host` header and into DNS.
    pub fn authority(&self) -> String {
        self.authority_for_url()
    }

    fn authority_for_url(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "http://{}{}", self.authority_for_url(), self.target)
    }
}

/// Split `host:port`, `host`, `[v6]:port` or `[v6]`. The port is `None`
/// when the authority did not write one, so the caller can tell a default
/// from a choice; the default itself is the caller's.
fn split_authority(authority: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            _ => Some(tail.strip_prefix(':')?.parse().ok()?),
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), Some(port.parse().ok()?))),
        None => Some((authority.to_string(), None)),
    }
}

/// A request, built before anything is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    method: Method,
    url: Url,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Headers this client always writes itself. A caller-supplied copy of any of
/// them is dropped rather than merged: two `content-length` headers, or one
/// that disagrees with the body actually written, is the request-smuggling bug
/// in its original form.
const RESERVED_HEADERS: [&str; 4] = ["host", "content-length", "connection", "transfer-encoding"];

impl HttpRequest {
    pub fn new(method: Method, url: &str) -> HttpResult<Self> {
        Ok(Self {
            method,
            url: Url::parse(url)?,
            headers: Vec::new(),
            body: Vec::new(),
        })
    }

    /// A request whose body is a JSON document.
    pub fn json(method: Method, url: &str, body: Vec<u8>) -> HttpResult<Self> {
        Ok(Self::new(method, url)?
            .with_header("content-type", "application/json; charset=utf-8")
            .with_header("accept", "application/json")
            .with_body(body))
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&name.as_str()) {
            return self;
        }
        // A CR or an LF in a value would end the header and let the rest be
        // read as another header, or as a second request.
        let sanitised: String = value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        self.headers.push((name, sanitised));
        self
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn method(&self) -> Method {
        self.method
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The bytes that go on the wire.
    fn encode(&self) -> Vec<u8> {
        let mut out = format!(
            "{} {} HTTP/1.1\r\n",
            self.method.as_str(),
            self.url.target()
        )
        .into_bytes();
        out.extend_from_slice(format!("host: {}\r\n", self.url.authority()).as_bytes());
        out.extend_from_slice(b"connection: close\r\n");
        out.extend_from_slice(b"user-agent: qip-transport/1.1\r\n");
        for (name, value) in &self.headers {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        out.extend_from_slice(format!("content-length: {}\r\n\r\n", self.body.len()).as_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// A response, fully read and bounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    /// Header names lower-cased, so a lookup does not depend on what the peer
    /// chose to capitalise.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// The body as text, or the malformed error naming why it is not.
    pub fn body_as_str(&self) -> HttpResult<&str> {
        std::str::from_utf8(&self.body).map_err(|error| HttpError::Malformed {
            phase: Phase::Body,
            detail: format!("the body is not valid UTF-8: {error}"),
        })
    }

    /// The first 200 bytes of the body, for an error message. Bounded because
    /// an error message that embeds a megabyte of a peer's response is how a
    /// log becomes unreadable at exactly the moment it is needed.
    pub fn body_excerpt(&self) -> String {
        let cut = self.body.len().min(200);
        String::from_utf8_lossy(&self.body[..cut]).replace(['\r', '\n'], " ")
    }
}

/// The client.
///
/// Holds no connection and no mutable state, so one can be shared by value
/// across every caller that needs the same limits.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpClient {
    limits: ClientLimits,
}

impl HttpClient {
    pub const fn new(limits: ClientLimits) -> Self {
        Self { limits }
    }

    pub const fn limits(&self) -> ClientLimits {
        self.limits
    }

    /// Connect, write the request, read the whole response, close.
    ///
    /// A non-2xx status is **not** an error: it is a response, and what to do
    /// about a 503 as opposed to a 400 is the caller's decision. The errors
    /// here are the ones where there is no response to hand back.
    pub fn send(&self, request: &HttpRequest) -> HttpResult<HttpResponse> {
        let stream = self.connect(request.url())?;
        self.write_request(&stream, request)?;
        self.read_response(stream, request.method())
    }

    pub fn get(&self, url: &str) -> HttpResult<HttpResponse> {
        self.send(&HttpRequest::new(Method::Get, url)?)
    }

    /// Resolve and connect, with the handshake bounded by
    /// [`ClientLimits::connect_timeout`].
    ///
    /// Resolution itself is `std`'s blocking `getaddrinfo` and carries no
    /// timeout of its own — the one unbounded wait in this client, and it is
    /// named here rather than left to be discovered. In the deployment this is
    /// written for, peers are cluster DNS names with a local resolver; a
    /// deployment where that is not true wants a resolver this build does not
    /// have.
    fn connect(&self, url: &Url) -> HttpResult<TcpStream> {
        let authority = url.authority();
        let addresses: Vec<SocketAddr> = (url.host(), url.port())
            .to_socket_addrs()
            .map_err(|error| HttpError::Resolve {
                authority: authority.clone(),
                detail: error.to_string(),
            })?
            .collect();
        if addresses.is_empty() {
            return Err(HttpError::NoAddress { authority });
        }

        let mut last = None;
        for address in &addresses {
            match TcpStream::connect_timeout(address, self.limits.connect_timeout) {
                Ok(stream) => {
                    // Mesh messages are small and latency-sensitive; waiting
                    // 40ms for a second segment that is never coming is the
                    // whole of Nagle's downside here.
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(self.limits.read_timeout));
                    let _ = stream.set_write_timeout(Some(self.limits.write_timeout));
                    return Ok(stream);
                }
                Err(error) if is_timeout(&error) => {
                    last = Some(HttpError::ConnectTimeout {
                        authority: authority.clone(),
                        after: self.limits.connect_timeout,
                    });
                }
                Err(error) => {
                    last = Some(HttpError::ConnectFailed {
                        address: address.to_string(),
                        detail: error.to_string(),
                    });
                }
            }
        }
        Err(last.unwrap_or(HttpError::NoAddress { authority }))
    }

    fn write_request(&self, mut stream: &TcpStream, request: &HttpRequest) -> HttpResult<()> {
        let bytes = request.encode();
        stream
            .write_all(&bytes)
            .and_then(|()| stream.flush())
            .map_err(|error| HttpError::WriteFailed {
                detail: error.to_string(),
            })
    }

    fn read_response(&self, stream: TcpStream, method: Method) -> HttpResult<HttpResponse> {
        let limits = self.limits;
        let mut reader = BufReader::new(stream);

        // --- status line ---
        let line = read_line(
            &mut reader,
            limits.max_status_line,
            Phase::StatusLine,
            limits.read_timeout,
        )?;
        let (status, reason) = parse_status_line(&line)?;

        // --- headers ---
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        loop {
            if headers.len() >= limits.max_headers {
                return Err(HttpError::TooManyHeaders {
                    limit: limits.max_headers,
                });
            }
            let line = read_line(
                &mut reader,
                limits.max_header_line,
                Phase::Headers,
                limits.read_timeout,
            )?;
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                break;
            }
            let Some((name, value)) = line.split_once(':') else {
                return Err(HttpError::Malformed {
                    phase: Phase::Headers,
                    detail: format!("a header line has no colon: {line:?}"),
                });
            };
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            // A repeated `content-length` with two different values is the
            // classic desync: two parsers disagree about where the body ends.
            if name == "content-length"
                && let Some(existing) = headers.get(&name)
                && existing != &value
            {
                return Err(HttpError::Malformed {
                    phase: Phase::Headers,
                    detail: format!("two content-length headers disagree: {existing} and {value}"),
                });
            }
            headers.insert(name, value);
        }

        let chunked = headers
            .get("transfer-encoding")
            .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
        if chunked && headers.contains_key("content-length") {
            return Err(HttpError::Malformed {
                phase: Phase::Headers,
                detail: "the response declares both content-length and chunked transfer-encoding, \
                         which is how two parsers end up disagreeing about where the body ends"
                    .to_string(),
            });
        }

        // --- body ---
        let carries_body = method.expects_response_body() && status_allows_body(status);
        let body = if !carries_body {
            Vec::new()
        } else if chunked {
            read_chunked(&mut reader, &limits)?
        } else if let Some(declared) = headers.get("content-length") {
            let declared: usize = declared.parse().map_err(|_| HttpError::Malformed {
                phase: Phase::Headers,
                detail: format!("content-length {declared:?} is not a number"),
            })?;
            // Refused before the allocation, so a peer declaring four
            // gigabytes costs nothing but the connection.
            if declared > limits.max_body {
                return Err(HttpError::BodyTooLarge {
                    limit: limits.max_body,
                    at_least: declared,
                });
            }
            let mut body = vec![0u8; declared];
            reader
                .read_exact(&mut body)
                .map_err(|error| classify(&error, Phase::Body, limits.read_timeout))?;
            body
        } else {
            // No framing at all: the body ends when the connection does. Read
            // one byte past the limit so exceeding it is detectable rather
            // than silently truncated.
            let mut body = Vec::new();
            let read = reader
                .by_ref()
                .take(limits.max_body as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|error| classify(&error, Phase::Body, limits.read_timeout))?;
            if read > limits.max_body {
                return Err(HttpError::BodyTooLarge {
                    limit: limits.max_body,
                    at_least: read,
                });
            }
            body
        };

        Ok(HttpResponse {
            status,
            reason,
            headers,
            body,
        })
    }
}

/// Whether a status code may carry a body at all.
const fn status_allows_body(status: u16) -> bool {
    !matches!(status, 204 | 304) && (status < 100 || status >= 200)
}

fn parse_status_line(line: &str) -> HttpResult<(u16, String)> {
    let malformed = |detail: String| HttpError::Malformed {
        phase: Phase::StatusLine,
        detail,
    };
    let line = line.trim_end_matches(['\r', '\n']);
    let mut parts = line.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| malformed("the status line is empty".to_string()))?;
    if !version.starts_with("HTTP/1.") {
        return Err(malformed(format!(
            "{version:?} is not an HTTP/1.x version token"
        )));
    }
    let code = parts
        .next()
        .ok_or_else(|| malformed(format!("no status code in {line:?}")))?;
    let status: u16 = code
        .parse()
        .map_err(|_| malformed(format!("{code:?} is not a status code")))?;
    if !(100..600).contains(&status) {
        return Err(malformed(format!("{status} is not a status code")));
    }
    Ok((status, parts.next().unwrap_or("").trim().to_string()))
}

/// Read one line, refusing at the limit rather than after it.
fn read_line(
    reader: &mut BufReader<TcpStream>,
    limit: usize,
    phase: Phase,
    timeout: StdDuration,
) -> HttpResult<String> {
    let mut line = String::new();
    let read = reader
        .by_ref()
        .take(limit as u64)
        .read_line(&mut line)
        .map_err(|error| classify(&error, phase, timeout))?;
    if read == 0 {
        return Err(HttpError::ClosedEarly { phase });
    }
    if read >= limit {
        return Err(HttpError::LineTooLong { phase, limit });
    }
    Ok(line)
}

/// Decode a chunked body, bounded per chunk and in total.
fn read_chunked(reader: &mut BufReader<TcpStream>, limits: &ClientLimits) -> HttpResult<Vec<u8>> {
    let mut body: Vec<u8> = Vec::new();
    loop {
        let line = read_line(
            reader,
            limits.max_header_line,
            Phase::ChunkHeader,
            limits.read_timeout,
        )?;
        let header = line.trim_end_matches(['\r', '\n']);
        // A chunk extension after `;` is legal and this client has no use for
        // one, so it is discarded rather than parsed.
        let size_token = header.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_token, 16).map_err(|_| HttpError::Malformed {
            phase: Phase::ChunkHeader,
            detail: format!("{size_token:?} is not a hexadecimal chunk size"),
        })?;

        if size > limits.max_chunk {
            return Err(HttpError::BodyTooLarge {
                limit: limits.max_chunk,
                at_least: size,
            });
        }
        if body.len().saturating_add(size) > limits.max_body {
            return Err(HttpError::BodyTooLarge {
                limit: limits.max_body,
                at_least: body.len().saturating_add(size),
            });
        }

        if size == 0 {
            // Trailers, bounded exactly as the headers were, then the blank
            // line that ends the message.
            let mut trailers = 0usize;
            loop {
                if trailers > limits.max_headers {
                    return Err(HttpError::TooManyHeaders {
                        limit: limits.max_headers,
                    });
                }
                let line = read_line(
                    reader,
                    limits.max_header_line,
                    Phase::Trailers,
                    limits.read_timeout,
                )?;
                if line.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
                trailers += 1;
            }
            return Ok(body);
        }

        let start = body.len();
        body.resize(start + size, 0);
        reader
            .read_exact(&mut body[start..])
            .map_err(|error| classify(&error, Phase::Body, limits.read_timeout))?;

        let mut terminator = [0u8; 2];
        reader
            .read_exact(&mut terminator)
            .map_err(|error| classify(&error, Phase::Body, limits.read_timeout))?;
        if &terminator != b"\r\n" {
            return Err(HttpError::Malformed {
                phase: Phase::Body,
                detail: "a chunk did not end with CRLF".to_string(),
            });
        }
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Turn a socket failure into the error that says what actually happened.
fn classify(error: &std::io::Error, phase: Phase, timeout: StdDuration) -> HttpError {
    if is_timeout(error) {
        return HttpError::ReadTimeout {
            phase,
            after: timeout,
        };
    }
    match error.kind() {
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset => {
            HttpError::ClosedEarly { phase }
        }
        std::io::ErrorKind::InvalidData => HttpError::Malformed {
            phase,
            detail: error.to_string(),
        },
        _ => HttpError::ReadFailed {
            phase,
            detail: error.to_string(),
        },
    }
}
