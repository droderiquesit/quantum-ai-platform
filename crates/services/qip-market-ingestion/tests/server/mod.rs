//! A real loopback HTTP server, scripted per request.
//!
//! The adapter under test is only interesting where it meets a socket, so the
//! tests give it one. The same technique as `qip-transport`'s own harness, cut
//! down to the four answers this crate needs: a framed response, one too large
//! to hold, one that stops half way, and one that never comes.
//!
//! Port 0 asks the operating system for a free port, so these tests run
//! alongside every other test binary without a hard-coded number that fails on
//! whichever machine already uses it.

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
    /// Header names lower-cased, as the client wrote them.
    pub(crate) headers: BTreeMap<String, String>,
}

/// What the server should do with a connection.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    /// A well-formed response with a content-length body.
    Json { status: u16, body: String },
    /// A complete, well-formed response whose body is `bytes` long. For the
    /// peer that answers with more than this process will hold.
    Oversized { bytes: usize },
    /// Declare `declared` bytes, send `written`, then close: the peer that
    /// dies mid-body.
    Truncated { declared: usize, written: usize },
    /// Say nothing for this long, then answer. For the read timeout.
    Silent(StdDuration),
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
    /// Always answer the same way.
    pub(crate) fn always(action: Action) -> Self {
        Self::serving(move |_| action.clone())
    }

    /// Answer by request target, and by how many times that target has been hit.
    ///
    /// [`Self::always`] cannot describe a snapshot-plus-increment adapter: one
    /// poll makes two requests to two different paths, and the cases worth
    /// testing are exactly the ones where the second poll's answer differs from
    /// the first's — a gap appearing, a rebuild being served.
    ///
    /// Each route is a substring matched against the request target and a
    /// script of answers for it; the last answer repeats once the script runs
    /// out, so a test only writes the responses it cares about. A target
    /// matching no route is answered with a 404 naming it, so a mis-specified
    /// route fails as a route problem rather than as a mysterious refusal.
    ///
    /// `allow(dead_code)` because each integration-test binary compiles this
    /// module on its own, so a constructor only one binary needs is genuinely
    /// unused in the others. The attribute sits on this item rather than on the
    /// module so that an item no binary uses is still reported.
    #[allow(dead_code)]
    pub(crate) fn routed(routes: Vec<(&str, Vec<Action>)>) -> Self {
        let mut routes: Vec<(String, Vec<Action>, usize)> = routes
            .into_iter()
            .map(|(needle, actions)| (needle.to_string(), actions, 0usize))
            .collect();
        Self::serving(move |request| {
            for (needle, actions, hits) in routes.iter_mut() {
                if !request.target.contains(needle.as_str()) {
                    continue;
                }
                let index = (*hits).min(actions.len().saturating_sub(1));
                *hits += 1;
                return match actions.get(index) {
                    Some(action) => action.clone(),
                    None => Action::json(500, r#"{"error":"a route with no answers"}"#),
                };
            }
            Action::json(
                404,
                format!(
                    r#"{{"error":"no route matches","target":"{}"}}"#,
                    request.target
                ),
            )
        })
    }

    /// The shared listener, driven by whatever decides each answer.
    fn serving(mut responder: impl FnMut(&RawRequest) -> Action + Send + 'static) -> Self {
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
                        thread_served.fetch_add(1, Ordering::SeqCst);
                        let request = read_request(&stream).unwrap_or_default();
                        let action = responder(&request);
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request);
                        // Answering on its own thread so a deliberately slow
                        // answer delays the client under test and not the next
                        // connection: a client that has timed out must find the
                        // listener ready, exactly as it would against a real
                        // server.
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

    /// The base URL, `http://127.0.0.1:port`.
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.address)
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
/// the only way to name a free port without guessing one a parallel test has
/// just taken.
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
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let declared: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if declared > 0 {
        let mut body = vec![0u8; declared];
        reader.read_exact(&mut body).ok()?;
    }

    Some(RawRequest {
        method,
        target,
        headers,
    })
}

fn write_action(mut stream: TcpStream, action: &Action) {
    let bytes = match action {
        Action::Json { status, body } => framed(*status, "application/json", body.as_bytes()),
        Action::Oversized { bytes } => {
            let body = vec![b'x'; *bytes];
            framed(200, "application/json", &body)
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
        Action::Silent(delay) => {
            std::thread::sleep(*delay);
            framed(200, "application/json", b"{}")
        }
    };
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
    // Closing is the point of two of these actions, so it is explicit rather
    // than left to the drop at the end of the scope.
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
