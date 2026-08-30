//! A minimal HTTP/1.1 server.
//!
//! Written in-tree against `std::net` because the platform's dependency policy
//! is that a dependency has to earn its place, and what the API needs is a
//! request line, some headers, a body and a response.
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
//! for as long as the client stays — what [`crate::stream`] serves as
//! server-sent events. [`pump`] is the loop that writes one, and its error
//! handling is the load-bearing part: a client leaving an event stream does
//! not say so, it stops reading, and the failed write is the only notice the
//! server gets.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
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
    /// How long a connection may take to send its request.
    pub read_timeout: StdDuration,
    /// How long a response may take to write.
    pub write_timeout: StdDuration,
    /// Concurrent connections handled. Beyond this, connections queue in the
    /// listener backlog rather than each spawning a thread.
    pub max_concurrent: usize,
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
        }
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
/// everything an event stream knows lives in [`crate::stream`] and everything
/// a socket knows lives here.
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
    limits: ServerLimits,
    handler: Arc<dyn Handler>,
    shutdown: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
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
        let listener = TcpListener::bind(address)
            .map_err(|e| Error::io(format!("cannot bind {address}: {e}")))?;
        Ok(Self {
            listener,
            limits,
            handler,
            shutdown: Arc::new(AtomicBool::new(false)),
            requests: Arc::new(AtomicU64::new(0)),
        })
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
            let requests = self.requests.clone();
            let live_count = live.clone();
            live.fetch_add(1, Ordering::Relaxed);
            std::thread::spawn(move || {
                serve_connection(stream, handler.as_ref(), limits);
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
        serve_connection(stream, self.handler.as_ref(), self.limits);
        self.requests.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn serve_connection(mut stream: TcpStream, handler: &dyn Handler, limits: ServerLimits) {
    let _ = stream.set_read_timeout(Some(limits.read_timeout));
    let _ = stream.set_write_timeout(Some(limits.write_timeout));

    let peer = stream
        .peer_addr()
        .map(|address| address.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let request = match read_request(&stream, &peer, limits) {
        Ok(request) => request,
        Err(status) => {
            let response = Response::json(
                status,
                format!(
                    r#"{{"error":"{}"}}"#,
                    match status {
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
                let _ = pump(&mut stream, body.as_mut());
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
fn read_request(
    stream: &TcpStream,
    peer: &str,
    limits: ServerLimits,
) -> std::result::Result<Request, u16> {
    let mut reader = BufReader::new(stream);

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
    let mut body = vec![0u8; declared];
    if declared > 0 {
        reader.read_exact(&mut body).map_err(|_| 400u16)?;
    }

    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
        peer: peer.to_string(),
    })
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
