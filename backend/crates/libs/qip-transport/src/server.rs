//! HTTP/1.1 server, moved from `qip-api` (ADR 0100 §1).
//!
//! Written in-tree against `std::net` because the platform's dependency policy
//! is that a dependency has to earn its place, and what an HTTP surface needs
//! is a request line, some headers, a body and a response. `qip-fabricd` and
//! `qip-ledgerd` have to serve without depending on an application crate, and
//! a second hand-written parser beside this one would be new attack surface
//! nobody asked for — so this is the one server, and every caller depends on
//! it rather than each maintaining its own.
//!
//! The limits are the interesting part. Every one of them exists because its
//! absence is a way for an unauthenticated caller to consume resources: a
//! request with no size limit, a header list with no count limit, a connection
//! that never sends anything and never closes. They are enforced while reading
//! rather than after, so an oversized request is refused before it has been
//! buffered.
//!
//! There are two shapes a request can be answered in. A [`Response`] is the
//! whole answer, buffered and measured, which is what every REST endpoint
//! returns. A [`ResponseStream`] is an answer with no length that is written
//! for as long as the client stays — what a caller's own event-stream module
//! serves as server-sent events. [`pump`] is the loop that writes one, and its
//! error handling is the load-bearing part: a client leaving an event stream
//! does not say so, it stops reading, and the failed write is the only notice
//! the server gets.
//!
//! # Mutual TLS (ADR 0112)
//!
//! [`Server::bind_tls`] serves the same handlers over TLS 1.3 with a
//! **required** client certificate: a peer without one chained to the
//! configured roots fails the handshake and never reaches a handler. The
//! identity the certificate was admitted under is handed to the handler in
//! the [`PEER_IDENTITY_HEADER`] header, which the server strips from every
//! request on every connection first — plaintext included — so a client
//! can never supply it itself. A server bound with [`Server::bind`] is
//! plaintext and says so; the choice is the composition root's, made once
//! through [`crate::tls::LinkSecurity`].

use qip_core::error::{Error, Result};
use qip_tls::rustls;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::tls::TlsServer;

/// The header a handler reads the TLS-verified peer identity from.
///
/// Written only by the server, after the handshake, from the name the
/// peer's certificate was admitted under ([`crate::tls::PeerNames`]); any
/// copy a client sent is removed before the handler sees the request, on
/// plaintext and TLS connections alike.
pub const PEER_IDENTITY_HEADER: &str = "x-qip-peer-identity";
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration as StdDuration;

/// Limits applied to every connection.
///
/// Defaults chosen for an internal API behind an authenticating proxy. Each is
/// a refusal path an unauthenticated caller can reach, so none of them is
/// optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerLimits {
    /// Largest request line, in bytes. A URL longer than this is either an
    /// attack or a bug.
    pub max_request_line: usize,
    /// Largest single header line.
    pub max_header_line: usize,
    /// Most headers accepted. An unbounded header list is an unbounded
    /// allocation.
    pub max_headers: usize,
    /// Largest body accepted.
    pub max_body: usize,
    /// How long any one read may wait for a byte. A bound on silence, not on
    /// the connection: a peer that sends one byte every few seconds resets
    /// it on every byte, which is what [`Self::connection_deadline`] is for.
    pub read_timeout: StdDuration,
    /// How long a response may take to write.
    pub write_timeout: StdDuration,
    /// Concurrent connections handled. Beyond this, connections queue in the
    /// listener backlog rather than each spawning a thread.
    pub max_concurrent: usize,
    /// The wall-clock budget from accepting a connection to having read its
    /// request in full — the TLS handshake included.
    ///
    /// [`Self::read_timeout`] alone let a host that dribbled a ClientHello
    /// one byte at a time hold a worker for as long as it cared to, because
    /// each byte restarted the per-read clock; with enough such hosts every
    /// worker was a stalled handshake and no legitimate peer was served.
    /// This bound is not reset by anything the peer does. It ends when the
    /// request has been read, so a handler's own work and a streamed
    /// response are not limited by it.
    pub connection_deadline: StdDuration,
    /// The most connections that may be at once in the phase before the
    /// server knows who they are — the TLS handshake, or for a plaintext
    /// server the request read — as a share of [`Self::max_concurrent`].
    ///
    /// Beyond it a new connection is closed at once in the accept loop
    /// without a thread being spawned for it. What this buys is bounded:
    /// hosts that cannot authenticate can occupy at most this many workers
    /// and never the whole pool, so connections already past their handshake
    /// keep being served. It does not stop such hosts from refusing *new*
    /// legitimate handshakes while they hold the slots — that is bounded in
    /// time by [`Self::connection_deadline`], and in reach by the VPC
    /// firewall that decides who may open a connection at all.
    pub max_handshaking: usize,
    /// The most bytes read from a peer whose certificate chained and whose
    /// name the server does not admit, before it is answered `403`.
    ///
    /// That read exists only so the kernel does not reset the connection
    /// over an unread request and discard the answer. It was bounded by the
    /// full request limits — a megabyte of body and fifteen seconds a read —
    /// which handed a refused workload a worker for as long as the most
    /// generous admitted one. A few kilobytes is a request line and its
    /// headers; anything past it is unread, and the refusal is the same.
    pub refused_request_bytes: usize,
    /// How long that read may take in all, for the same reason.
    pub refused_request_deadline: StdDuration,
    /// Connections, out of [`Self::max_concurrent`], that only a request the
    /// handler classes [`AdmissionClass::Priority`] may hold (R12-FABRIC-8089).
    ///
    /// The connection cap alone is class-blind: a flood of bulk producers —
    /// telemetry, research — can hold every slot while every stream quota is
    /// honoured, and a control or outcome producer is answered `503` at the
    /// accept loop without any quota having been exceeded. With a reserve, a
    /// connection the handler classes [`AdmissionClass::Bulk`] is admitted
    /// only while fewer than `max_concurrent - reserved_for_priority` bulk
    /// connections are being served, and is otherwise answered `503` as soon
    /// as its request head has been read, before its body is, so the slot it
    /// took at accept is given back within one head read rather than held for
    /// a whole body. Zero — the default — keeps the cap class-blind, which is
    /// right for a server whose handler does not classify. [`Server::bind`]
    /// refuses a reserve that leaves no slot for bulk traffic at all.
    pub reserved_for_priority: usize,
}

/// How a server shares its connection slots between the requests it serves
/// (R12-FABRIC-8089): decided by [`Handler::admission_class`] from the request
/// head alone — the path, the headers and the authenticated peer — because the
/// slot has to be decided before the body is read, and the body is where a
/// bulk producer's megabyte is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionClass {
    /// May use every slot, the reserved ones included: control and outcome
    /// traffic that must not be starved.
    Priority,
    /// May use only the slots outside the reserve: telemetry, research and
    /// anything the handler does not recognise. The default, so a handler
    /// that classifies nothing reserves nothing for anyone by accident.
    Bulk,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_request_line: 8 * 1024,
            max_header_line: 8 * 1024,
            max_headers: 64,
            max_body: 1024 * 1024,
            read_timeout: StdDuration::from_secs(15),
            write_timeout: StdDuration::from_secs(30),
            max_concurrent: 64,
            // A handshake and a request inside the VPC take milliseconds; a
            // megabyte body at a congested hundred kilobytes a second still
            // fits. Twenty seconds is the bound, not the expectation.
            connection_deadline: StdDuration::from_secs(20),
            // A quarter of the pool: unauthenticated hosts cannot take the
            // other three quarters from connections already admitted.
            max_handshaking: 16,
            refused_request_bytes: 4 * 1024,
            refused_request_deadline: StdDuration::from_secs(2),
            reserved_for_priority: 0,
        }
    }
}

/// A socket whose reads share one wall-clock deadline.
///
/// Each read is given the smaller of the per-read timeout and the time left
/// before the deadline, and a read with no time left fails as timed out
/// without touching the socket. [`Self::lift`] ends the budget once the
/// request has been read, after which reads fall back to the per-read
/// timeout alone, exactly as before the deadline existed.
struct Budgeted {
    socket: TcpStream,
    deadline: Option<std::time::Instant>,
    per_read: StdDuration,
}

impl Budgeted {
    fn new(socket: TcpStream, limits: &ServerLimits) -> Self {
        Self {
            socket,
            deadline: std::time::Instant::now().checked_add(limits.connection_deadline),
            per_read: limits.read_timeout,
        }
    }

    /// Bring the deadline forward to `at` if it is sooner; never later.
    fn tighten(&mut self, at: Option<std::time::Instant>) {
        self.deadline = match (self.deadline, at) {
            (Some(current), Some(at)) => Some(current.min(at)),
            (current, at) => current.or(at),
        };
    }

    fn lift(&mut self) {
        self.deadline = None;
        let _ = self.socket.set_read_timeout(Some(self.per_read));
    }
}

impl Read for Budgeted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(deadline) = self.deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the connection's deadline passed before its request was read",
                ));
            }
            self.socket
                .set_read_timeout(Some(remaining.min(self.per_read)))?;
        }
        self.socket.read(buf)
    }
}

impl Write for Budgeted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.socket.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}

/// A connection that ends its read deadline once its request is read.
trait Deadlined: Read + Write {
    fn lift_deadline(&mut self);
}

impl Deadlined for Budgeted {
    fn lift_deadline(&mut self) {
        self.lift();
    }
}

impl Deadlined for rustls::StreamOwned<rustls::ServerConnection, Budgeted> {
    fn lift_deadline(&mut self) {
        self.sock.lift();
    }
}

/// One place in [`ServerLimits::max_handshaking`], released when dropped so
/// that every early return on the handshake path gives it back.
struct HandshakeSlot(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One of the slots a bulk connection may hold (every slot outside
/// [`ServerLimits::reserved_for_priority`]), released when dropped for the
/// same reason as [`HandshakeSlot`].
struct BulkSlot(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for BulkSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// An HTTP method.
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

    /// Whether the method may change state.
    pub const fn is_mutating(&self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Delete)
    }
}

/// A parsed request.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    /// Path with the query string removed, percent-decoded, and normalised.
    pub path: String,
    pub query: BTreeMap<String, String>,
    /// Header names lower-cased, so lookups do not depend on what the client
    /// chose to capitalise.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    /// The peer address, for logging and rate limiting.
    pub peer: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str)
    }

    /// The path split into segments, with empties removed.
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }

    pub fn body_as_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.body)
            .map_err(|_| Error::invalid("request body is not valid UTF-8"))
    }
}

/// A response to send.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), content_type.to_string())],
            body,
        }
    }

    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self::new(
            status,
            "application/json; charset=utf-8",
            body.into().into_bytes(),
        )
    }

    pub fn html(status: u16, body: impl Into<String>) -> Self {
        Self::new(status, "text/html; charset=utf-8", body.into().into_bytes())
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self::new(
            status,
            "text/plain; charset=utf-8",
            body.into().into_bytes(),
        )
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// The security headers every response carries.
    ///
    /// The content-security policy is the important one: `default-src 'none'`
    /// with only `'self'` for styles means an injected script cannot execute
    /// even if one is somehow reflected into a page. The web UI renders no
    /// JavaScript at all, so nothing legitimate is broken by forbidding it.
    pub fn with_security_headers(mut self) -> Self {
        let headers = [
            (
                "content-security-policy",
                "default-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
            ),
            ("x-content-type-options", "nosniff"),
            ("x-frame-options", "DENY"),
            ("referrer-policy", "no-referrer"),
            (
                "strict-transport-security",
                "max-age=31536000; includeSubDomains",
            ),
            ("cache-control", "no-store"),
        ];
        for (name, value) in headers {
            self.headers.push((name.to_string(), value.to_string()));
        }
        self
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            204 => "No Content",
            303 => "See Other",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Unknown",
        }
    }

    /// Encode the status line and headers of a response whose body has no
    /// declared length.
    ///
    /// Deliberately omits `content-length`: the length of a live stream is not
    /// known when the head is written and will not be known until the client
    /// leaves, so the body is delimited by the close of the connection
    /// instead. Declaring a length that later turns out to be wrong is worse
    /// than declaring none — a client would stop reading at the wrong byte and
    /// treat a truncated event as a complete one.
    ///
    /// `connection` is left to the caller for the same reason: [`encode`] can
    /// hard-code `close` because it has just written the entire body, and a
    /// stream cannot.
    ///
    /// [`encode`]: Response::encode
    fn encode_open_head(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason()).into_bytes();
        for (name, value) in &self.headers {
            // The same sanitisation `encode` applies, and for the same reason:
            // CR or LF in a header value would let a caller inject headers or
            // a whole second response.
            let sanitised: String = value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
            out.extend_from_slice(format!("{name}: {sanitised}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out
    }

    fn encode(&self, include_body: bool) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason()).into_bytes();
        for (name, value) in &self.headers {
            // A header value containing CR or LF would let a caller inject
            // headers or a second response. Nothing legitimate needs them.
            let sanitised: String = value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
            out.extend_from_slice(format!("{name}: {sanitised}\r\n").as_bytes());
        }
        out.extend_from_slice(format!("content-length: {}\r\n", self.body.len()).as_bytes());
        out.extend_from_slice(b"connection: close\r\n\r\n");
        if include_body {
            out.extend_from_slice(&self.body);
        }
        out
    }
}

/// Something that answers requests.
pub trait Handler: Send + Sync {
    fn handle(&self, request: &Request) -> Response;

    /// Which slots `request` may hold (R12-FABRIC-8089), decided from its
    /// head: `request.body` is empty here because the body has not been read
    /// yet — that is the point. The peer identity header, when the listener
    /// is mutual TLS, is already in place and is the server's own. The
    /// default classes everything [`AdmissionClass::Bulk`], so a handler that
    /// does not override this grants nobody the reserve.
    fn admission_class(&self, _request: &Request) -> AdmissionClass {
        AdmissionClass::Bulk
    }

    /// Answer `request` with a live stream instead of a buffered response.
    ///
    /// [`StreamDecision::NotAStream`] — the default, and the answer for every
    /// handler that has no streams — hands the request back to [`handle`].
    ///
    /// The three-way answer exists so that a stream request is put through its
    /// handler's authorisation ladder exactly once. A two-way answer would
    /// force a refused caller through the ladder a second time in `handle`,
    /// which for a rate-limited API means one request spending two of the
    /// caller's allowance. It also keeps the dangerous case impossible to
    /// reach by accident: a stream head is a `200` written before the first
    /// event, and there is no later point at which it can be taken back, so a
    /// handler that has not authorised a caller must answer
    /// [`StreamDecision::Refused`] with the status that says why.
    ///
    /// [`handle`]: Handler::handle
    fn stream(&self, _request: &Request) -> StreamDecision {
        StreamDecision::NotAStream
    }
}

/// A shared handler is a handler, so the middlewares that wrap one can be
/// nested through the erased type the composition root builds.
///
/// The root decides at run time how many wrappers a process has — a wallet
/// statement feed, a capital-fabric declaration, both, or neither — and the
/// only type that can hold "whatever was decided" is `Arc<dyn Handler>`.
/// Without this, each wrapper would have to be applied to a concrete inner
/// type, and the root would carry one match arm per combination of feeds: two
/// feeds is four arms, and the third feed anyone adds is eight. The
/// forwarding is total — both methods, no behaviour of its own.
impl<H: Handler + ?Sized> Handler for std::sync::Arc<H> {
    fn handle(&self, request: &Request) -> Response {
        (**self).handle(request)
    }

    fn admission_class(&self, request: &Request) -> AdmissionClass {
        (**self).admission_class(request)
    }

    fn stream(&self, request: &Request) -> StreamDecision {
        (**self).stream(request)
    }
}

/// What a handler decided about streaming one request.
pub enum StreamDecision {
    /// Not a request for a stream. [`Handler::handle`] answers it.
    NotAStream,
    /// A request for a stream that the handler will not serve. The response
    /// carries the status that says why — an unrecognised credential, a role
    /// that is not enough, an exhausted allowance — and is written like any
    /// other.
    Refused(Response),
    /// Take the connection over and write events until the client leaves.
    Accepted(Box<dyn ResponseStream>),
}

impl std::fmt::Debug for StreamDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAStream => f.write_str("NotAStream"),
            Self::Refused(response) => f.debug_tuple("Refused").field(&response.status).finish(),
            Self::Accepted(_) => f.write_str("Accepted"),
        }
    }
}

/// A response body written to the socket as it becomes available.
///
/// The [`Handler`] contract answers a request with a complete [`Response`],
/// which is the right shape for every REST endpoint here and the wrong one for
/// a live stream: a server-sent-event response has no length, is written over
/// minutes, and ends when the client goes away rather than when the server has
/// finished. This trait is the second shape, kept deliberately narrow — a
/// producer of byte frames, and the headers that introduce them — so that
/// everything an event stream knows lives in the caller's own stream module
/// and everything a socket knows lives here.
pub trait ResponseStream: Send {
    /// Headers the response head carries, in addition to the security headers.
    ///
    /// A header named here *replaces* the default of the same name rather than
    /// appearing beside it. A stream that must say `cache-control: no-cache`
    /// would otherwise emit it next to the `no-store` every other response
    /// carries, and a proxy reading two directives is entitled to honour
    /// either — which is how an event stream ends up served from a cache.
    fn headers(&self) -> Vec<(String, String)>;

    /// The next frame to write, or `None` when the stream is over.
    ///
    /// Blocking, and meant to be: the pacing of a live stream — how long to
    /// wait for the next event, when to send a heartbeat, when to stop — is
    /// the producer's decision, not the socket's.
    ///
    /// An implementation must return within a bounded time even when nothing
    /// has happened. The only way this server learns a client has gone is a
    /// failed write, so a producer that blocks indefinitely waiting for an
    /// event is a connection that is never discovered to be dead, and a thread
    /// held against [`ServerLimits::max_concurrent`] for as long as the process
    /// runs.
    fn next_frame(&mut self) -> Option<Vec<u8>>;
}

/// Why a streamed response stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    /// The producer had nothing more to send and said so.
    SourceFinished,
    /// A write failed, which is how this server learns the client has gone.
    ClientDisconnected,
}

/// What one streamed response did before it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamOutcome {
    /// Frames written, not counting the response head.
    pub frames: usize,
    /// Bytes written, including the response head.
    pub bytes: usize,
    pub end: StreamEnd,
}

/// Write a streamed response to `sink` until it ends or the client goes.
///
/// The error handling is the whole point. A client that closes an event stream
/// does not say so — it stops reading, and the server finds out when a write
/// fails with a broken pipe or a connection reset. That failure is the *normal*
/// end of a stream and is treated as one: the loop stops and reports
/// [`StreamEnd::ClientDisconnected`]. Propagating it, or panicking on it, would
/// turn every closed browser tab into an incident and — because each connection
/// is served on its own thread — a panic here would poison nothing but would
/// still be logged as a failure that never happened.
///
/// Takes a `dyn Write` rather than a `TcpStream` so that the disconnect path
/// can be exercised by a test with a writer that fails on demand. A path that
/// only runs when a real client vanishes mid-stream is a path that is never
/// tested.
pub fn pump(sink: &mut dyn Write, body: &mut dyn ResponseStream) -> StreamOutcome {
    let head = stream_head(body);
    let mut bytes = head.len();
    if sink.write_all(&head).is_err() || sink.flush().is_err() {
        return StreamOutcome {
            frames: 0,
            bytes: 0,
            end: StreamEnd::ClientDisconnected,
        };
    }

    let mut frames = 0;
    while let Some(frame) = body.next_frame() {
        if sink.write_all(&frame).is_err() || sink.flush().is_err() {
            return StreamOutcome {
                frames,
                bytes,
                end: StreamEnd::ClientDisconnected,
            };
        }
        // Flushed after every frame. An event that sits in a buffer waiting for
        // company is not a live event, and the whole reason a client opened
        // this connection instead of polling is that it wants the event now.
        frames += 1;
        bytes += frame.len();
    }
    StreamOutcome {
        frames,
        bytes,
        end: StreamEnd::SourceFinished,
    }
}

/// The response head that introduces a stream.
///
/// The security headers are the same ones every other response carries, with
/// any the stream names itself replacing rather than joining them.
fn stream_head(body: &dyn ResponseStream) -> Vec<u8> {
    let mut response =
        Response::new(200, "text/event-stream; charset=utf-8", Vec::new()).with_security_headers();
    for (name, value) in body.headers() {
        let lowered = name.to_ascii_lowercase();
        response
            .headers
            .retain(|(existing, _)| existing.to_ascii_lowercase() != lowered);
        response.headers.push((name, value));
    }
    response.encode_open_head()
}

impl<F> Handler for F
where
    F: Fn(&Request) -> Response + Send + Sync,
{
    fn handle(&self, request: &Request) -> Response {
        self(request)
    }
}

/// The server.
pub struct Server {
    listener: TcpListener,
    /// `Some` for a mutual-TLS server: every connection must complete a
    /// handshake presenting a client certificate before it is read.
    tls: Option<TlsServer>,
    limits: ServerLimits,
    handler: Arc<dyn Handler>,
    shutdown: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    /// Connections accepted by [`Server::serve`] and not yet past the
    /// handshake (or, plaintext, the request read) — the count
    /// [`ServerLimits::max_handshaking`] caps.
    handshaking: Arc<std::sync::atomic::AtomicUsize>,
    /// Connections past their request head whose handler classed them
    /// [`AdmissionClass::Bulk`] and that are still being served — the count
    /// held under `max_concurrent - reserved_for_priority`.
    bulk: Arc<std::sync::atomic::AtomicUsize>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("local_addr", &self.listener.local_addr().ok())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Bind to an address.
    pub fn bind(address: &str, handler: Arc<dyn Handler>, limits: ServerLimits) -> Result<Self> {
        if limits.reserved_for_priority >= limits.max_concurrent {
            // Refused rather than clamped: a reserve that swallows the whole
            // cap would answer every bulk request 503 for ever, and a server
            // that quietly lowered it would be one whose configuration says
            // one thing and whose behaviour is another.
            return Err(Error::invalid(format!(
                "reserved_for_priority ({}) must leave at least one of the {} connection slots \
                 for bulk traffic",
                limits.reserved_for_priority, limits.max_concurrent
            )));
        }
        let listener = TcpListener::bind(address)
            .map_err(|e| Error::io(format!("cannot bind {address}: {e}")))?;
        Ok(Self {
            listener,
            tls: None,
            limits,
            handler,
            shutdown: Arc::new(AtomicBool::new(false)),
            requests: Arc::new(AtomicU64::new(0)),
            handshaking: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            bulk: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    /// Bind a mutual-TLS server (ADR 0112): TLS 1.3, a client certificate
    /// required on every connection and checked against `tls.peers`.
    pub fn bind_tls(
        address: &str,
        handler: Arc<dyn Handler>,
        limits: ServerLimits,
        tls: TlsServer,
    ) -> Result<Self> {
        let mut server = Self::bind(address, handler, limits)?;
        server.tls = Some(tls);
        Ok(server)
    }

    /// Whether this server requires mutual TLS.
    pub const fn is_mutual_tls(&self) -> bool {
        self.tls.is_some()
    }

    pub fn local_address(&self) -> Result<String> {
        self.listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(|e| Error::io(format!("no local address: {e}")))
    }

    /// A handle that stops the accept loop.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// Connections [`Self::serve`] holds that have not yet finished their
    /// handshake (plaintext: their request read).
    pub fn handshaking(&self) -> usize {
        self.handshaking.load(Ordering::Relaxed)
    }

    /// A handle on [`Self::handshaking`] that outlives a move into the
    /// serving thread.
    pub fn handshaking_handle(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::clone(&self.handshaking)
    }

    /// Connections classed [`AdmissionClass::Bulk`] that are being served
    /// right now, out of `max_concurrent - reserved_for_priority`.
    pub fn bulk_connections(&self) -> usize {
        self.bulk.load(Ordering::Relaxed)
    }

    /// Serve until the shutdown handle is set.
    ///
    /// One thread per connection, capped by the limits. A thread pool would be
    /// better under load; this is an internal API where the honest answer is
    /// that the load is small and the simplicity is worth more.
    pub fn serve(&self) -> Result<()> {
        let live = Arc::new(AtomicU64::new(0));
        for stream in self.listener.incoming() {
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };

            if self.handshaking.load(Ordering::Relaxed) >= self.limits.max_handshaking {
                // Closed at once, on this thread, with nothing written: no
                // worker is spent on a connection the server cannot yet tell
                // from the ones holding every handshake slot, and a TLS
                // client could not read a plaintext refusal anyway.
                drop(stream);
                continue;
            }
            if live.load(Ordering::Relaxed) as usize >= self.limits.max_concurrent {
                // Refuse rather than queue unboundedly: a server that accepts
                // every connection under load fails by exhausting memory,
                // which is harder to diagnose than a refusal.
                let mut stream = stream;
                let response =
                    Response::json(503, r#"{"error":"the server is at its connection limit"}"#)
                        .with_security_headers();
                let _ = stream.write_all(&response.encode(true));
                continue;
            }

            let handler = self.handler.clone();
            let limits = self.limits;
            let tls = self.tls.clone();
            let requests = self.requests.clone();
            let live_count = live.clone();
            live.fetch_add(1, Ordering::Relaxed);
            // Taken here, before the spawn, so the check above sees every
            // connection already accepted and not only those whose thread
            // has started.
            self.handshaking.fetch_add(1, Ordering::Relaxed);
            let slot = HandshakeSlot(Arc::clone(&self.handshaking));
            let bulk = Arc::clone(&self.bulk);
            std::thread::spawn(move || {
                accept_connection(
                    stream,
                    handler.as_ref(),
                    limits,
                    tls.as_ref(),
                    Some(slot),
                    &bulk,
                );
                requests.fetch_add(1, Ordering::Relaxed);
                live_count.fetch_sub(1, Ordering::Relaxed);
            });
        }
        Ok(())
    }

    /// Handle exactly one connection, for tests and for a single-shot mode.
    pub fn serve_once(&self) -> Result<()> {
        let (stream, _) = self
            .listener
            .accept()
            .map_err(|e| Error::io(format!("accept failed: {e}")))?;
        accept_connection(
            stream,
            self.handler.as_ref(),
            self.limits,
            self.tls.as_ref(),
            None,
            &self.bulk,
        );
        self.requests.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Set the socket's limits and hand the connection to the plaintext or the
/// TLS path. The TLS path never falls through to the plaintext one.
fn accept_connection(
    stream: TcpStream,
    handler: &dyn Handler,
    limits: ServerLimits,
    tls: Option<&TlsServer>,
    slot: Option<HandshakeSlot>,
    bulk: &Arc<std::sync::atomic::AtomicUsize>,
) {
    let _ = stream.set_read_timeout(Some(limits.read_timeout));
    let _ = stream.set_write_timeout(Some(limits.write_timeout));
    let peer = stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    // The deadline starts at accept, so the handshake spends from it.
    let socket = Budgeted::new(stream, &limits);
    match tls {
        None => {
            let mut socket = socket;
            serve_connection(&mut socket, &peer, None, handler, limits, slot, bulk);
        }
        Some(tls) => serve_tls_connection(socket, &peer, handler, limits, tls, slot, bulk),
    }
}

/// Complete a mutual-TLS handshake, admit the peer's certificate, then serve
/// the request over the session.
///
/// A peer that presents no certificate, or one the roots do not chain,
/// fails inside `complete_io` and the connection is dropped there: no
/// request is read and no handler runs. The handshake is bounded by the
/// per-read timeout and, in all, by [`ServerLimits::connection_deadline`].
fn serve_tls_connection(
    mut socket: Budgeted,
    peer: &str,
    handler: &dyn Handler,
    limits: ServerLimits,
    tls: &TlsServer,
    slot: Option<HandshakeSlot>,
    bulk: &Arc<std::sync::atomic::AtomicUsize>,
) {
    let Ok(mut session) = rustls::ServerConnection::new(Arc::clone(&tls.config)) else {
        return;
    };
    while session.is_handshaking() {
        if session.complete_io(&mut socket).is_err() {
            return;
        }
    }
    // Past the handshake the peer has a certificate the roots chain; it no
    // longer counts against the slots unauthenticated hosts compete for.
    drop(slot);
    let admitted = session
        .peer_certificates()
        .and_then(<[_]>::first)
        .map(|leaf| tls.peers.admit(leaf));
    let mut stream = rustls::StreamOwned::new(session, socket);
    match admitted {
        Some(Ok(identity)) => serve_connection(
            &mut stream,
            peer,
            identity.as_deref(),
            handler,
            limits,
            None,
            bulk,
        ),
        // Unreachable with a verifier that requires a certificate, and
        // refused rather than assumed: a session with no peer certificate is
        // not an authenticated one.
        None | Some(Err(_)) => {
            // Read the request — handed to no handler — before answering.
            // Answering first and closing with the client's request still
            // unread in the receive buffer makes the kernel reset the
            // connection, and a reset discards the 403 the client had not
            // yet read: the refused workload saw a dropped socket, not a
            // refusal, in more than half of loopback runs. Whatever the read
            // returns, the answer is the same 403.
            //
            // Bounded far tighter than an admitted request, in bytes and in
            // time: this read is a courtesy to a peer the server has already
            // refused, and it held a worker for as long as any admitted
            // request could until both bounds were added.
            stream
                .sock
                .tighten(std::time::Instant::now().checked_add(limits.refused_request_deadline));
            let refused = ServerLimits {
                max_request_line: limits.refused_request_bytes,
                max_header_line: limits.refused_request_bytes,
                max_body: limits.refused_request_bytes,
                ..limits
            };
            let _ = read_request(
                (&mut stream).take(limits.refused_request_bytes as u64),
                peer,
                refused,
            );
            let response = Response::json(
                403,
                r#"{"error":"the client certificate does not name a workload this server admits"}"#,
            )
            .with_security_headers();
            let _ = stream.write_all(&response.encode(true));
            let _ = stream.flush();
        }
    }
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

fn serve_connection<S: Deadlined>(
    stream: &mut S,
    peer: &str,
    identity: Option<&str>,
    handler: &dyn Handler,
    limits: ServerLimits,
    slot: Option<HandshakeSlot>,
    bulk: &Arc<std::sync::atomic::AtomicUsize>,
) {
    // The head first and the body second, with the admission class decided
    // between them (R12-FABRIC-8089): a bulk connection the server has no
    // slot for is answered before its body is read, so the slot it took at
    // accept is held for one head read and not for a megabyte.
    let mut reader = BufReader::new(&mut *stream);
    let mut slot = slot;
    let read = read_head(&mut reader, peer, limits).and_then(|(mut request, declared)| {
        // The identity header is the server's to write and nobody else's,
        // and it is in place before the handler classes the request, so the
        // class is decided on the authenticated peer and not on a header
        // the client chose to send.
        request.headers.remove(PEER_IDENTITY_HEADER);
        if let Some(identity) = identity {
            request
                .headers
                .insert(PEER_IDENTITY_HEADER.to_string(), identity.to_string());
        }
        // A plaintext peer is known once its head is in — the credential is
        // a header — so this is where it leaves the handshake slots, which
        // bound hosts the server cannot yet tell apart. Its body is bounded
        // below by its class's share of the pool and by the connection
        // deadline, as a TLS peer's already is past its handshake; until
        // 2026-10-25 the slot was held through the body too, so sixteen
        // stalled bodies closed the listener to every new peer at once.
        slot.take();
        let bulk_slot = match handler.admission_class(&request) {
            AdmissionClass::Priority => None,
            AdmissionClass::Bulk => {
                let bulk_cap = limits
                    .max_concurrent
                    .saturating_sub(limits.reserved_for_priority);
                // Taken before it is compared, so two bulk connections racing
                // for the last slot cannot both read it as free; the one
                // over the cap gives it straight back.
                if bulk.fetch_add(1, Ordering::Relaxed) >= bulk_cap {
                    bulk.fetch_sub(1, Ordering::Relaxed);
                    return Err(503);
                }
                Some(BulkSlot(Arc::clone(bulk)))
            }
        };
        request.body = read_body(&mut reader, declared)?;
        Ok((request, bulk_slot))
    });
    drop(reader);
    // The request is in (or refused), so the deadline that bounded getting
    // it ends — a handler's work and a streamed response are bounded by the
    // write timeout, not by it. A head that could not be read gives its
    // handshake slot back here.
    drop(slot);
    stream.lift_deadline();
    let (request, _bulk_slot) = match read {
        Ok(read) => read,
        Err(status) => {
            let response = Response::json(
                status,
                format!(
                    r#"{{"error":"{}"}}"#,
                    match status {
                        503 => "the server is at its connection limit for bulk traffic",
                        413 => "the request exceeds a server limit",
                        408 => "the request timed out",
                        _ => "the request could not be parsed",
                    }
                ),
            )
            .with_security_headers();
            let _ = stream.write_all(&response.encode(true));
            let _ = stream.flush();
            return;
        }
    };

    // A live stream takes the connection over, because its response has no
    // length and is written for as long as the client stays. Offered before
    // `handle` and never for `HEAD`: a HEAD asks for headers without a body,
    // and answering it by holding the connection open for minutes writing
    // events is the opposite of what was asked.
    let mut refusal = None;
    if request.method != Method::Head {
        match handler.stream(&request) {
            StreamDecision::Accepted(mut body) => {
                let _ = pump(stream, body.as_mut());
                return;
            }
            StreamDecision::Refused(response) => refusal = Some(response),
            StreamDecision::NotAStream => {}
        }
    }

    let head_only = request.method == Method::Head;
    let mut response = refusal
        .unwrap_or_else(|| handler.handle(&request))
        .with_security_headers();
    if head_only {
        response.body.clear();
    }

    let _ = stream.write_all(&response.encode(true));
    let _ = stream.flush();
}

/// Read and parse one request, or the status to refuse it with.
fn read_request<R: Read>(
    stream: R,
    peer: &str,
    limits: ServerLimits,
) -> std::result::Result<Request, u16> {
    let mut reader = BufReader::new(stream);
    let (mut request, declared) = read_head(&mut reader, peer, limits)?;
    request.body = read_body(&mut reader, declared)?;
    Ok(request)
}

/// Read and parse a request's line and headers, or the status to refuse it
/// with: the [`Request`] with an empty body, and the body length it declared,
/// already checked against [`ServerLimits::max_body`] so [`read_body`] never
/// allocates past it. Split from [`read_body`] so the server can class a
/// request (R12-FABRIC-8089) between the two.
fn read_head<R: Read>(
    reader: &mut BufReader<R>,
    peer: &str,
    limits: ServerLimits,
) -> std::result::Result<(Request, usize), u16> {
    // The request line, size-capped while reading.
    let mut line = String::new();
    let read = reader
        .by_ref()
        .take(limits.max_request_line as u64)
        .read_line(&mut line)
        .map_err(|_| 400u16)?;
    if read == 0 {
        return Err(400);
    }
    if read >= limits.max_request_line {
        return Err(413);
    }

    let mut parts = line.split_whitespace();
    let method = parts.next().and_then(Method::parse).ok_or(400u16)?;
    let target = parts.next().ok_or(400u16)?;
    let version = parts.next().unwrap_or("HTTP/1.1");
    if !version.starts_with("HTTP/1.") {
        return Err(400);
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    let path = normalise_path(raw_path).ok_or(400u16)?;
    let query = raw_query.map(parse_query).unwrap_or_default();

    // Headers, capped in both count and individual size.
    let mut headers = BTreeMap::new();
    loop {
        if headers.len() >= limits.max_headers {
            return Err(413);
        }
        let mut header = String::new();
        let read = reader
            .by_ref()
            .take(limits.max_header_line as u64)
            .read_line(&mut header)
            .map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        if read >= limits.max_header_line {
            return Err(413);
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(400);
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    // The body, only as long as content-length says and never longer than the
    // limit. A declared length above the limit is refused before anything is
    // read, so the allocation never happens.
    let declared: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if declared > limits.max_body {
        return Err(413);
    }

    Ok((
        Request {
            method,
            path,
            query,
            headers,
            body: Vec::new(),
            peer: peer.to_string(),
        },
        declared,
    ))
}

/// Read exactly the body [`read_head`] declared, or the status to refuse the
/// request with.
fn read_body<R: Read>(
    reader: &mut BufReader<R>,
    declared: usize,
) -> std::result::Result<Vec<u8>, u16> {
    let mut body = vec![0u8; declared];
    if declared > 0 {
        reader.read_exact(&mut body).map_err(|_| 400u16)?;
    }
    Ok(body)
}

/// Percent-decode and normalise a path, refusing anything that escapes.
///
/// `..` is rejected outright rather than resolved. Resolving it correctly is
/// possible and getting it subtly wrong is the classic path-traversal bug, so
/// the safer answer is that nothing legitimate in this API needs it.
pub fn normalise_path(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw)?;
    if !decoded.starts_with('/') {
        return None;
    }
    // A null byte or a control character in a path is never legitimate and is
    // how a decoded path smuggles a separator past a later check.
    if decoded.chars().any(|c| c.is_control()) {
        return None;
    }
    if decoded.split('/').any(|segment| segment == "..") {
        return None;
    }
    // Collapse repeated separators so `/a//b` and `/a/b` route identically.
    let mut normalised = String::with_capacity(decoded.len());
    let mut previous_was_slash = false;
    for c in decoded.chars() {
        if c == '/' {
            if previous_was_slash {
                continue;
            }
            previous_was_slash = true;
        } else {
            previous_was_slash = false;
        }
        normalised.push(c);
    }
    Some(normalised)
}

/// Percent-decode, refusing a malformed escape rather than passing it through.
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn parse_query(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((
                percent_decode(&key.replace('+', " "))?,
                percent_decode(&value.replace('+', " "))?,
            ))
        })
        .collect()
}
