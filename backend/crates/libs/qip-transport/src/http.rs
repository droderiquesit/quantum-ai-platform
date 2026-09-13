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
    /// [`redact_for_echo`], because a value that can only be a clean scheme
    /// token needs none. That was the second, structurally separate finding
    /// of this round: fixing [`redact_for_echo`] alone would have left this
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
/// [`Url::parse`] and [`redact_for_echo`] used to run their own
/// `raw.split_once("://")` to find it, and `split_once` finds the *first*
/// occurrence anywhere in the string, not the one at the start. Two
/// independent detectors that each searched the whole string is the exact
/// shape of the defect this closes: a fix landing in one of them — as it did
/// on 2026-09-12, in `redact_for_echo` alone — leaves the other to
/// misparse the identical string its own way. A scheme-less credential
/// whose path or query happens to contain the ordinary substring `"://"`
/// (`?redirect=http://…`, `?callback=http://…` — any parameter naming
/// another URL) supplies exactly such a later occurrence: `split_once`
/// matched on it, so almost the whole string up to that point — credential
/// included — was misread as "the scheme", which [`redact_for_echo`] then
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

/// `raw` rendered safe to print in a refusal: userinfo masked, the query
/// and fragment masked, control characters escaped.
///
/// **Use this for an address a caller is *refusing*, never for one it
/// accepted** — it deliberately destroys detail, and on an accepted address
/// that detail is the answer rather than the risk.
///
/// Three masks, and what each one does and does not promise:
///
/// * **Userinfo — guaranteed.** Everything through the last `@` *that falls
///   before the parameter cut* is replaced by `…@`; if the last `@` falls at
///   or after that cut, nothing between the scheme and the cut is shown at
///   all. Say the mechanism precisely, because getting this sentence wrong
///   is how round 6 shipped a leak: "everything through the last `@`
///   anywhere" was true of round 5's code, was carried forward verbatim over
///   a restructure that made it false, and the false version described an
///   implementation that printed half of any password containing a `?`.
///   What holds either way is the guarantee: userinfo precedes an `@` by
///   definition, [`split_scheme`] cannot have discarded one (its grammar
///   `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` has no `@` in it, and its
///   fallthrough returns the whole string), and every byte before the
///   surviving region is dropped rather than inspected. So **no
///   `@`-delimited credential can survive this function** — a proof about
///   the code rather than a summary of the cases that were tried, and
///   `no_byte_before_a_credentials_terminating_at_ever_survives_redaction`
///   executes it over every input in a small alphabet rather than leaving it
///   to be re-derived by the next reader. The six rounds it took are in ADR
///   0057.
/// * **Query and fragment — masked, and the reason is that they are *not*
///   covered by the proof above.** A credential with no `@` in it —
///   `?api_key=hf_REALSECRET`, the shape a vendor console hands an operator
///   to copy — is invisible to an `@` search, and every arm of
///   [`require_loopback_egress`] printed it in full until 2026-09-13.
///   Everything from the first `?` or `#` is therefore replaced by `?…` or
///   `#…`. This is structural, like the `@` rule: a region boundary, not a
///   judgement about which parameter looks secret.
/// * **An address with no `@` before the cut and no `?` or `#` at all — NOT
///   covered, deliberately.** It is returned whole. `hf_LIVEKEY_SECRET`,
///   pasted into the base-URL variable instead of the key variable next to
///   it, prints in full, and so does `http://host/v1/hunter2/chat`. This
///   said "a credential inside a *path* segment" until 2026-09-13 and that
///   was narrower than the code: the surviving region is every region, not
///   the path. Masking it would mean guessing which part of an unparseable
///   string is a secret, and that guess is the exact activity that produced
///   five consecutive leaks here. What survives is also what an operator
///   needs to identify which of several configured addresses was refused.
///   Named as a known limit rather than papered over: if it must close, the
///   answer is to stop putting the address in the message at all, not to
///   add a heuristic.
///
/// Anything outside printable ASCII is escaped rather than copied through —
/// see [`escape_controls`], which is a whitelist for the same reason this
/// function's regions are boundaries rather than judgements. A `\r\n` in a
/// rejected address otherwise ends the log line and begins one the operator
/// did not write, which is a forged record in whatever collects stderr;
/// whoever can set a config variable can otherwise write an arbitrary line
/// into the log.
///
/// Pure text, no parse: it has to work on exactly the addresses
/// [`Url::parse`] refuses, which are by definition the ones it cannot
/// parse.
///
/// # History
///
/// This function leaked a credential five times, in five different shapes,
/// across five fix rounds between 2026-09-12 and 2026-09-13: it required a
/// `"://"` before redacting anything; it found that `"://"` with
/// `split_once`, which matches a query's `://` as readily as a scheme's; it
/// read the text before the first `/`, `?` or `#` as "the authority" when
/// that text was empty or a bare `:`; and it then trusted a
/// `split_authority` check that accepted `abc:80`, `123` and `svc` as real
/// hosts. Each round closed the reported shape and left an adjacent one.
/// The pattern was the finding: **nothing in the string says whether a
/// host-shaped prefix is a host or a scheme-typo'd credential**, so every
/// predicate over the string's shape was eventually wrong about some
/// string. The fix was to delete the predicate, not narrow it again. ADR
/// 0057 records the decision and what it costs.
pub fn redact_for_echo(raw: &str) -> String {
    redact_parts(raw).0
}

/// [`redact_for_echo`]'s rendering, plus whether the *parsed* host is
/// visible in it.
///
/// One function answering both questions, because the alternative is
/// [`require_loopback_egress`] re-deriving the second from the string — and
/// two pieces of code deciding independently where a credential ends is the
/// shape that produced rounds 3 and 4. The second value exists because the
/// gate names `Url::parse`'s host beside this rendering, and a host is not
/// safe to print merely because the parser accepted it: the parser refuses
/// *userinfo*, so `http://hf_SECRET?x@127.0.0.1:9105` parses with the
/// secret as its **host**, and naming it re-prints the exact bytes this
/// function had just decided were credential. The two claims then
/// contradict each other inside one refusal.
///
/// The flag is `!masked_userinfo` and that is exact rather than
/// approximate. An address reaching the host arm is one [`Url::parse`]
/// accepted, so its authority holds no `@`; any `@` this function finds is
/// therefore past the host, and masking through it always takes the host
/// with it.
fn redact_parts(raw: &str) -> (String, bool) {
    let (scheme, rest) = split_scheme(raw);
    // The parameter region is cut off **first**, and is never searched for
    // anything — it is dropped whole. Order matters here and getting it
    // backwards reopens the leak: masking the query only after the `@`
    // search means an earlier `@` inside the query ends the search there and
    // prints everything after it, so `http://h/x?a=1@2&key=SECRET` would
    // come back carrying `SECRET`. Cutting first makes that unreachable,
    // because no byte at or past the first `?`/`#` reaches the output at all.
    // **Both boundaries are measured over the whole of `rest`, and only then
    // intersected.** Measuring the second one inside the first is what round
    // 6 did, and it leaked: a credential may itself contain a `?` or a `#`
    // — they are ordinary password characters, and they are *not* legal
    // unencoded in userinfo, which is exactly why such a string arrives here
    // rather than parsing — so the parameter cut can land in the middle of
    // the credential. The `@` search over that truncated prefix then finds
    // nothing, the code concludes "no userinfo", and prints the prefix,
    // which is the first half of the password:
    // `http://svc:SECRET?x@127.0.0.1:9105` came back as `http://svc:SECRET?…`.
    // 131,040 of 640,000 enumerated inputs leaked that way.
    let cut = rest.find(['?', '#']).unwrap_or(rest.len());
    let has_params = cut < rest.len();
    // `at >= cut` rather than `at > cut`: they are equivalent today, because
    // `rest[cut]` is a `?` or `#` and `rest[at]` is an `@`, so the two
    // indices cannot coincide. Written as `>=` anyway, because the slice in
    // the arm below would panic with start past end if they ever could, and
    // an edit that added `@` to the `find` set — a plausible future
    // tightening — would make that reachable. The guard costs nothing and
    // removes an invariant nothing states.
    let (kept, masked_userinfo, delimiter_is_credential) = match rest.rfind('@') {
        // The last `@` is past the cut, so the `?`/`#` that set the cut sits
        // *inside* the credential. Nothing between the scheme and the cut
        // can be shown: it is credential, not authority — and that includes
        // the delimiter byte itself, which is why the marker below is a
        // constant here rather than the byte that was found. Printing the
        // real one leaks which of `?` or `#` the password contained, one
        // character of it, from strictly before the terminating `@`.
        Some(at) if at >= cut => ("", true, true),
        Some(at) => (&rest[at + 1..cut], true, false),
        None => (&rest[..cut], false, false),
    };
    if !masked_userinfo && !has_params {
        // Nothing to mask. Returning `raw` rather than a reassembly keeps an
        // address that needed no redaction byte-identical to what was set,
        // which is what an operator compares against their configuration.
        return (escape_controls(raw), true);
    }
    let scheme = match scheme {
        Some(scheme) => format!("{}://", escape_controls(scheme)),
        None => String::new(),
    };
    let userinfo = if masked_userinfo { "…@" } else { "" };
    let params = if !has_params {
        String::new()
    } else if delimiter_is_credential {
        "?…".to_string()
    } else {
        format!("{}…", escape_controls(&rest[cut..cut + 1]))
    };
    // Every piece carrying caller text is escaped **before** the marker is
    // spliced in, never after. Escaping the assembled string would leave the
    // one non-ASCII character the whitelist has to admit — `…`, the marker
    // itself — indistinguishable from a `…` an operator typed, so an address
    // could render as though it had been redacted when it had not.
    (
        format!("{scheme}{userinfo}{}{params}", escape_controls(kept)),
        !masked_userinfo,
    )
}

/// `text` with everything outside printable ASCII replaced by an escape.
///
/// Not cosmetic. The output of [`redact_for_echo`], and the host
/// [`require_loopback_egress`] names beside it, go into a refusal that a
/// composition root prints to stderr at start-up. A `\r\n` inside a
/// rejected address would end that line and start another — one whose
/// contents the person who set the address chose. A log a reader cannot
/// trust to have one record per line is a log that can be made to say
/// anything.
///
/// **The test is "is this byte provably safe to print", not "is this byte
/// known to be dangerous".** An earlier version escaped `char::is_control`,
/// which is the Unicode `Cc` category and therefore covers C0, DEL and C1
/// but *not* U+2028 and U+2029 — which Python's `str.splitlines`, several
/// log viewers, and pre-ES2019 JSON parsing all treat as line boundaries,
/// leaving the forged-record hole open for exactly the consumers most
/// likely to be reading these logs. Nor did it cover U+202E, which reverses
/// the rendering of everything after it, or U+FEFF. Enumerating dangerous
/// characters is the same losing shape as enumerating dangerous URL
/// regions, and this file has already lost that argument five times: the
/// rule is now a whitelist, so a character that has not been considered is
/// escaped rather than passed through. The backslash is escaped too, so a
/// literal `\u{000a}` typed into an address cannot round-trip to something
/// a consumer that unescapes would turn back into a newline.
fn escape_controls(text: &str) -> String {
    let safe = |c: char| (c.is_ascii_graphic() && c != '\\') || c == ' ';
    if text.chars().all(safe) {
        return text.to_string();
    }
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, c| {
            if safe(c) {
                out.push(c);
            } else {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32));
            }
            out
        })
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
/// Every echo of the address goes through [`redact_for_echo`], so a refusal
/// of `http://svc:TOKEN@…` does not print `TOKEN`. **`shown` is
/// load-bearing in all four arms, not belt-and-braces in the last two**,
/// and this comment said the opposite until 2026-09-13. The reasoning it
/// gave was half right: [`Url::parse`] does refuse *userinfo* before the
/// host and port arms run, so no `user:pass@` address reaches them. But it
/// does not refuse a *query*, and a credential does not have to be
/// userinfo — `http://api.vendor.example/v1?api_key=SECRET` parses
/// cleanly, reaches the host arm, and `http://127.0.0.1/v1?api_key=SECRET`
/// reaches the port arm. In both, the only thing keeping the key out of the
/// log is `shown`. A reader who believed the old sentence would delete it
/// from those arms as redundant, which is the shape of every leak in this
/// file's history: a true statement about one kind of credential, read as a
/// statement about credentials. Called at the seams that
/// carry a credential — the connector pair in the API's, the deep brain's
/// and the fast brain's parsers and at `ConnectorFeed::open`; the deep
/// brain's hosted language-model listener; the fast brain's market-data
/// vendor — so the refusal names the variable there and the type here.
/// **At five of those six, not all six.** `ConnectorFeed::open`
/// (`qip-market-ingestion/src/connector_feed.rs`) calls this with a bare
/// `?` and adds no variable name, which `qip-api/src/feed.rs` already says
/// out loud where it works around the same gap. It matters more since the
/// host arm began withholding a host the redaction masked: on that one
/// path a refusal can name neither the host nor the variable. Said here
/// because the claim "every caller names the variable" was written into
/// this file and into a test comment as the justification for withholding,
/// and it was not true when it was written.
/// Refuses rather than rewriting: an address that is nearly right is a
/// deployment mistake somebody should see.
pub fn require_loopback_egress(base_url: &str) -> qip_core::Result<()> {
    let (shown, host_survived_redaction) = redact_parts(base_url);
    // Case-insensitively, because a scheme is case-insensitive and an
    // operator who typed `HTTPS://` is making exactly the mistake this arm
    // exists to explain. Matching it byte-for-byte sent them to the generic
    // parse refusal instead, which says the address is malformed rather
    // than naming the egress proxy they were supposed to point at.
    if base_url
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
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
    // The host is named only when the redaction kept it, and `host` and
    // `shown` therefore **cannot disagree** — which is the invariant, and is
    // not what two earlier versions of this comment said.
    //
    // A host is not safe to print merely because [`Url::parse`] accepted it.
    // The parser refuses *userinfo*, not a secret sitting where a host goes:
    // `http://hf_SECRET?x@127.0.0.1:9105` parses with the credential as its
    // host, so naming it re-printed the exact bytes `shown` had just masked,
    // putting two contradictory claims about one string into one refusal.
    // The earlier comment here said the opposite — "a parsed host cannot
    // carry a credential" — and survived, above its own correction, into the
    // commit that corrected it. It is deleted rather than argued with,
    // because the next reader acts on the first sentence they reach and this
    // file has now shipped two leaks that way.
    //
    // `host_survived_redaction` is `!masked_userinfo`, and that is exact
    // rather than cautious: an address reaching here is one the parser
    // accepted, so its authority holds no `@`; any `@` [`redact_parts`]
    // found is therefore past the host, and masking through it always takes
    // the host too. So a named host is always already a substring of
    // `shown`, and a withheld one is absent from both.
    //
    // Escaped either way: the parser refuses only `char::is_control` and
    // ASCII space in a host, so a host carrying U+2028 parses cleanly and,
    // printed raw, would split this record for any consumer that treats it
    // as a line break — the forged-record hole [`escape_controls`] closes
    // for the address beside it.
    //
    // What is lost by withholding it, stated plainly rather than waved
    // through: the operator loses the host from *this* sentence, and gets it
    // back only from the wrapper the caller adds. Five of the six call sites
    // name their configuration variable there; `ConnectorFeed::open`
    // (`qip-market-ingestion`) does not, and on that one path a refusal can
    // now name neither the host nor the variable. That is a real gap, it is
    // this change's cost, and it is recorded in ADR 0057 rather than left
    // for someone to discover — the fix is a wrapper at that call site, not
    // a relaxation here.
    let host = if host_survived_redaction {
        format!("`{}`", escape_controls(url.host()))
    } else {
        "one masked along with the credential it could not be told apart from — and the text          after `…@` below is what followed that credential, not the host this would have          connected to"
            .to_string()
    };
    if url.host() != LOOPBACK_HOST {
        return Err(qip_core::Error::invalid(format!(
            "the egress address names the host {host} (as written, with any credential, \
             query and fragment masked: {shown}). A vendor is reached only \
             through the egress proxy on loopback — http://127.0.0.1:<port>, the literal \
             address and not a name a resolver answers — which terminates TLS to the vendor \
             (ADR 0024) and reaches only the hosts its bootstrap names; this transport has no \
             TLS stack (ADR 0009), so a plaintext address off the instance would carry a \
             request in the clear to whatever answers there"
        )));
    }
    if !url.port_is_explicit() {
        // Named unconditionally here, unlike the arm above. This line is
        // reached only after `url.host() == LOOPBACK_HOST` succeeded, so the
        // host is the `LOOPBACK_HOST` constant and not operator text — it
        // cannot carry a credential, by construction rather than by a
        // judgement about its shape. Withholding it here protected nothing
        // and actively misdirected: it told an operator their host could not
        // be told apart from a credential at the exact moment the gate had
        // just confirmed it was loopback and the real fault was the port.
        let host = LOOPBACK_HOST;
        return Err(qip_core::Error::invalid(format!(
            "the egress address names the host `{host}` and names no port (as written, with \
             any credential, query and fragment masked: {shown}). The egress proxy's listeners \
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
    /// [`redact_for_echo`] — so the refusal of a credential-bearing URL is
    /// not itself the leak.
    ///
    /// The scheme boundary is found by [`split_scheme`] — the same function
    /// [`redact_for_echo`] uses — rather than by this function running its
    /// own `"://"` search, which is what let a scheme-less credential
    /// string be misread as carrying a scheme at all until 2026-09-13. A
    /// string [`split_scheme`] does not anchor a scheme onto now falls
    /// through to the "no scheme" arm below, whatever it contains further
    /// in, and that arm already redacts through [`redact_for_echo`].
    pub fn parse(raw: &str) -> HttpResult<Self> {
        let invalid = |detail: &str| HttpError::InvalidUrl {
            url: redact_for_echo(raw),
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
