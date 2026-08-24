//! A real loopback HTTP server, routed and scripted per request.
//!
//! The live gateway is only interesting where it meets a socket, so the tests
//! give it one rather than a double. Bringing a REST venue session up costs
//! three requests to the health path before an order is sent at all — connect,
//! authenticate and heartbeat each ask the venue whether it is there — so one
//! canned answer per connection cannot express a test that scripts a submit.
//! Each route therefore holds a list of actions consumed in turn, with the last
//! repeating for ever, and anything unrouted answers 404 so a test whose
//! adapter asks for a path nobody scripted fails on that path instead of
//! hanging.
//!
//! Requests are recorded in full, headers and body, because the claims worth
//! making here are about what left the process.
//!
//! Port 0 asks the operating system for a free port, so these tests run beside
//! every other test binary without a hard-coded number.

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
    /// The full request target, query string included. What a test searches to
    /// prove a credential never reached a URL.
    pub(crate) target: String,
    /// Header names lower-cased, as the client wrote them.
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
}

impl RawRequest {
    pub(crate) fn path(&self) -> &str {
        match self.target.split_once('?') {
            Some((path, _)) => path,
            None => &self.target,
        }
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// What the server should do with a connection.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    /// A well-formed response with a content-length body.
    Json { status: u16, body: String },
    /// Say nothing for this long, then answer. For the read timeout — which is
    /// the ambiguous submit, and the reason this module exists.
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

/// One method and path the server knows about, and how it answers.
#[derive(Debug)]
pub(crate) struct Route {
    method: String,
    path: String,
    actions: Vec<Action>,
    taken: AtomicUsize,
}

impl Route {
    /// A route that answers the same way every time.
    pub(crate) fn new(method: &str, path: &str, action: Action) -> Self {
        Self::in_turn(method, path, vec![action])
    }

    /// A route that answers with each action in turn, repeating the last.
    pub(crate) fn in_turn(method: &str, path: &str, actions: Vec<Action>) -> Self {
        assert!(
            !actions.is_empty(),
            "a route with no actions could not answer at all"
        );
        Self {
            method: method.to_string(),
            path: path.to_string(),
            actions,
            taken: AtomicUsize::new(0),
        }
    }

    fn matches(&self, request: &RawRequest) -> bool {
        self.method == request.method && self.path == request.path()
    }

    fn next(&self) -> Action {
        let index = self.taken.fetch_add(1, Ordering::SeqCst);
        let last = self.actions.len().saturating_sub(1);
        self.actions[index.min(last)].clone()
    }
}

/// A listener on an ephemeral loopback port.
pub(crate) struct TestVenue {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RawRequest>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for TestVenue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestVenue")
            .field("address", &self.address)
            .field("served", &self.served.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl TestVenue {
    /// A venue answering the routes given, and 404 for anything else.
    pub(crate) fn routed(routes: Vec<Route>) -> Self {
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
        let routes = Arc::new(routes);

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
                        let action = routes
                            .iter()
                            .find(|route| route.matches(&request))
                            .map_or_else(
                                || {
                                    Action::json(
                                        404,
                                        format!(
                                            "{{\"error\":\"no route for {} {}\"}}",
                                            request.method,
                                            request.path()
                                        ),
                                    )
                                },
                                Route::next,
                            );
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request);
                        // Answering on its own thread so a deliberately slow
                        // answer delays the client under test and not the next
                        // connection: a client that has timed out must find the
                        // listener ready, exactly as it would against a real
                        // venue.
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

    /// How many connections have been served. The number a test asserts is zero
    /// when it claims nothing was sent.
    pub(crate) fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }

    pub(crate) fn requests(&self) -> Vec<RawRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Every request the venue took for one method and path.
    pub(crate) fn requests_to(&self, method: &str, path: &str) -> Vec<RawRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == method && request.path() == path)
            .collect()
    }
}

impl Drop for TestVenue {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
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
    let mut body = String::new();
    if declared > 0 {
        let mut bytes = vec![0u8; declared];
        reader.read_exact(&mut bytes).ok()?;
        body = String::from_utf8_lossy(&bytes).into_owned();
    }

    Some(RawRequest {
        method,
        target,
        headers,
        body,
    })
}

fn write_action(mut stream: TcpStream, action: &Action) {
    let bytes = match action {
        Action::Json { status, body } => framed(*status, body.as_bytes()),
        Action::Silent(delay) => {
            std::thread::sleep(*delay);
            framed(200, b"{}")
        }
    };
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn framed(status: u16, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: \
         {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}
