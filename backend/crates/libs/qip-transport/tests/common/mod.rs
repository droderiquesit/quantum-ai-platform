//! A real loopback HTTP server, scripted per request.
//!
//! Every test in this crate talks to an actual socket. A mock client would
//! assert that the parser was called; it would not catch a client that reads
//! one byte too few, mishandles a chunk terminator, or waits forever on a peer
//! that says nothing — which are the three things this client exists to get
//! right.
//!
//! The listener binds port 0 and asks the operating system for a port, so tests
//! run concurrently without colliding and without a hard-coded number that
//! fails on whichever machine already uses it.
//!
//! Connections are served one at a time on a single thread. The client under
//! test is synchronous and one connection per request, so a sequential server
//! is the exact shape of what it does, and it keeps a slow response from
//! overlapping with the next assertion.

// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

/// A request as the server received it.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) headers: BTreeMap<String, String>,
    /// Every value seen for a header name, so a test can prove a header was
    /// written exactly once.
    pub(crate) header_counts: BTreeMap<String, usize>,
    pub(crate) body: Vec<u8>,
}

impl RawRequest {
    pub(crate) fn body_as_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// What the server should do with a connection.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    /// Exactly these bytes, whatever they are. For malformed responses.
    Raw(Vec<u8>),
    /// A well-formed response with a content-length body.
    Json { status: u16, body: String },
    /// A chunked response, one wire chunk per entry, plus a trailer.
    Chunked { status: u16, chunks: Vec<String> },
    /// Declare `declared` bytes, send `written`, then close. The peer that
    /// dies mid-body.
    Truncated { declared: usize, written: usize },
    /// A complete, well-formed response whose body is `bytes` long. For the
    /// peer that answers with more than this process will hold.
    Oversized { bytes: usize },
    /// A body with no content-length and no chunking, framed only by the close.
    Unframed { bytes: usize },
    /// Say nothing for this long, then answer 200. For the read timeout.
    Silent(StdDuration),
    /// Accept the connection and close it without writing anything.
    Hangup,
}

impl Action {
    pub(crate) fn json(status: u16, body: impl Into<String>) -> Self {
        Self::Json {
            status,
            body: body.into(),
        }
    }
}

/// A listener on an ephemeral loopback port.
pub(crate) struct TestServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RawRequest>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for TestServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestServer")
            .field("address", &self.address)
            .field("served", &self.served.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl TestServer {
    /// Spawn a server whose answer to the nth request is `responder(n, request)`.
    pub(crate) fn spawn<F>(responder: F) -> Self
    where
        F: Fn(usize, &RawRequest) -> Action + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let address = listener
            .local_addr()
            .expect("the listener has a local address")
            .to_string();
        listener
            .set_nonblocking(true)
            .expect("the listener can poll");

        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));

        let thread_stop = stop.clone();
        let thread_served = served.clone();
        let thread_requests = requests.clone();
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(StdDuration::from_secs(5)));
                        let index = thread_served.fetch_add(1, Ordering::SeqCst);
                        let request = read_request(&stream).unwrap_or_default();
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request.clone());
                        let action = responder(index, &request);
                        // Accepting and answering are separated so a
                        // deliberately slow answer does not also delay the
                        // *next* connection. A client under test that has
                        // timed out and retried must find the listener ready,
                        // exactly as it would against a real server; a
                        // sequential loop would instead measure the test
                        // harness. Which request is which is still decided on
                        // this thread, so the indices stay deterministic.
                        std::thread::spawn(move || write_action(stream, &action));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(StdDuration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            stop,
            served,
            requests,
            handle: Some(handle),
        }
    }

    /// Always answer the same way.
    pub(crate) fn always(action: Action) -> Self {
        Self::spawn(move |_, _| action.clone())
    }

    /// The base URL, `http://127.0.0.1:port`.
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// A URL under this server.
    pub(crate) fn url_for(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// How many connections have been served.
    pub(crate) fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }

    pub(crate) fn requests(&self) -> Vec<RawRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// An address nothing is listening on.
///
/// Obtained by binding a port and immediately dropping the listener, which is
/// the only way to name a free port without guessing one that a parallel test
/// has just taken.
pub(crate) fn address_with_no_listener() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let address = listener
        .local_addr()
        .expect("the listener has a local address")
        .to_string();
    drop(listener);
    format!("http://{address}")
}

fn read_request(stream: &TcpStream) -> Option<RawRequest> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut headers = BTreeMap::new();
    let mut header_counts: BTreeMap<String, usize> = BTreeMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        let header = header.trim_end().to_string();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            *header_counts.entry(name.clone()).or_insert(0) += 1;
            headers.insert(name, value.trim().to_string());
        }
    }

    let declared: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; declared];
    if declared > 0 {
        reader.read_exact(&mut body).ok()?;
    }

    Some(RawRequest {
        method,
        target,
        headers,
        header_counts,
        body,
    })
}

fn write_action(mut stream: TcpStream, action: &Action) {
    let bytes = match action {
        Action::Raw(bytes) => bytes.clone(),
        Action::Json { status, body } => framed(*status, "application/json", body.as_bytes()),
        Action::Chunked { status, chunks } => {
            let mut out = format!(
                "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ntransfer-encoding: \
                 chunked\r\nconnection: close\r\n\r\n"
            )
            .into_bytes();
            for chunk in chunks {
                out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                out.extend_from_slice(chunk.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            // Terminating chunk, one trailer, then the blank line.
            out.extend_from_slice(b"0\r\nx-checked: yes\r\n\r\n");
            out
        }
        Action::Truncated { declared, written } => {
            let mut out = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: \
                 {declared}\r\nconnection: close\r\n\r\n"
            )
            .into_bytes();
            out.extend(std::iter::repeat_n(b'x', *written));
            out
        }
        Action::Oversized { bytes } => {
            let body = vec![b'x'; *bytes];
            framed(200, "application/octet-stream", &body)
        }
        Action::Unframed { bytes } => {
            let mut out =
                b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n"
                    .to_vec();
            out.extend(std::iter::repeat_n(b'x', *bytes));
            out
        }
        Action::Silent(delay) => {
            std::thread::sleep(*delay);
            framed(200, "application/json", b"{}")
        }
        Action::Hangup => Vec::new(),
    };
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
    // Closing is the point of several of these actions, so it is explicit
    // rather than left to the drop at the end of the scope.
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn framed(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: \
         close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}
