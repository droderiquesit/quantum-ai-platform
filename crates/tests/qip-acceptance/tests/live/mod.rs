//! One loopback server, for every peer the live walk needs.
//!
//! `qip-market-ingestion`, `qip-brokers`, `qip-chain` and `qip-transport` each
//! have a harness of their own, and each is shaped by the one adapter it
//! serves: routing by target, routing by method and path, routing by JSON-RPC
//! method, or handing the bytes to a real [`MeshEndpoint`]. The walk in
//! `e2e_live.rs` needs all four *at once*, and reaching into another crate's
//! `tests/` module is not a thing Cargo will do — so this is the union of them,
//! written once here rather than four times in the walk.
//!
//! It is deliberately not a fifth copy of any of those. What it adds is the
//! only thing a walk needs that a per-adapter suite does not: one server type
//! that can be a vendor, a venue, a node and a mesh peer, so the composition
//! under test is the platform's rather than the harness's.
//!
//! Port 0 asks the operating system for a free port, so the walk runs alongside
//! every other test binary without a hard-coded number that fails on whichever
//! machine already uses it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use qip_transport::{EndpointResponse, MeshEndpoint, Method};

/// A request as the server received it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Request {
    pub(crate) method: String,
    /// The full request target, query string included. What a test searches to
    /// prove a credential never reached a URL.
    pub(crate) target: String,
    /// Header names lower-cased, as the client wrote them.
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
}

impl Request {
    /// The target with any query string removed.
    pub(crate) fn path(&self) -> &str {
        match self.target.split_once('?') {
            Some((path, _)) => path,
            None => &self.target,
        }
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The JSON-RPC method this request called, where the body is an envelope.
    pub(crate) fn rpc_method(&self) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(&self.body).ok()?;
        Some(value.get("method")?.as_str()?.to_string())
    }

    /// The first JSON-RPC parameter, as a string.
    ///
    /// Every call this walk's node serves is addressed by height, and the
    /// height is in that position. Answering from it rather than from a queue
    /// of scripted bodies means the fixture cannot silently drift out of step
    /// with the order the adapter happens to ask in.
    pub(crate) fn rpc_first_param(&self) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(&self.body).ok()?;
        Some(value.get("params")?.get(0)?.as_str()?.to_string())
    }
}

/// What the server answers with.
#[derive(Clone, Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Reply {
    pub(crate) fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

/// How a route recognises the requests it answers.
pub(crate) enum Match {
    /// An HTTP method and a substring of the request target.
    Target(&'static str, &'static str),
    /// A JSON-RPC method named in the request body.
    Rpc(&'static str),
}

/// What a route answers with.
enum Answer {
    /// Each reply in turn, the last repeating for ever, so a route that should
    /// behave consistently is written with one entry and a route whose second
    /// answer is the point of the test is written with two.
    Script(Vec<Reply>),
    /// Computed from the request. For a peer whose answer depends on what was
    /// asked — a node addressed by block height — where a positional script
    /// would encode the caller's call order as a fixture.
    Computed(Box<dyn Fn(&Request) -> Reply + Send + Sync>),
}

/// One thing the server knows how to answer.
pub(crate) struct Route {
    matcher: Match,
    answer: Answer,
    /// How many requests this route has taken, which is also the index into a
    /// scripted answer.
    taken: AtomicUsize,
}

impl Route {
    /// A route that answers the same way every time.
    pub(crate) fn new(matcher: Match, reply: Reply) -> Self {
        Self::in_turn(matcher, vec![reply])
    }

    /// A route that answers with each reply in turn, repeating the last.
    pub(crate) fn in_turn(matcher: Match, replies: Vec<Reply>) -> Self {
        assert!(
            !replies.is_empty(),
            "a route with no replies could not answer at all"
        );
        Self {
            matcher,
            answer: Answer::Script(replies),
            taken: AtomicUsize::new(0),
        }
    }

    /// A route that works its answer out from the request.
    pub(crate) fn computed(
        matcher: Match,
        responder: impl Fn(&Request) -> Reply + Send + Sync + 'static,
    ) -> Self {
        Self {
            matcher,
            answer: Answer::Computed(Box::new(responder)),
            taken: AtomicUsize::new(0),
        }
    }

    fn matches(&self, request: &Request) -> bool {
        match &self.matcher {
            Match::Target(method, needle) => {
                request.method == *method && request.target.contains(needle)
            }
            Match::Rpc(name) => request.rpc_method().as_deref() == Some(*name),
        }
    }

    fn next(&self, request: &Request) -> Reply {
        let index = self.taken.fetch_add(1, Ordering::SeqCst);
        match &self.answer {
            Answer::Script(replies) => {
                let last = replies.len().saturating_sub(1);
                replies[index.min(last)].clone()
            }
            Answer::Computed(responder) => responder(request),
        }
    }

    /// How many requests this route has answered.
    pub(crate) fn hits(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }
}

/// What the server does with a connection once it has read the request.
enum Peer {
    /// Answer from the routes, and 404 anything else.
    Routed(Arc<Vec<Route>>),
    /// Hand the bytes to a real mesh endpoint.
    Mesh(Box<MeshEndpoint>),
}

/// A listener on an ephemeral loopback port.
pub(crate) struct LoopbackServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    routes: Arc<Vec<Route>>,
    requests: Arc<Mutex<Vec<Request>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for LoopbackServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackServer")
            .field("address", &self.address)
            .field("served", &self.served.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl LoopbackServer {
    /// A server answering the routes given, and 404 for anything else.
    ///
    /// The 404 default is deliberate: an adapter that asks for a path the walk
    /// did not script fails on that path rather than on a hang, and the
    /// recorded request says which one it was.
    pub(crate) fn routed(routes: Vec<Route>) -> Self {
        let routes = Arc::new(routes);
        Self::serving(Peer::Routed(Arc::clone(&routes)), routes)
    }

    /// A server that is a real mesh peer.
    pub(crate) fn mesh(endpoint: MeshEndpoint) -> Self {
        Self::serving(Peer::Mesh(Box::new(endpoint)), Arc::new(Vec::new()))
    }

    fn serving(peer: Peer, routes: Arc<Vec<Route>>) -> Self {
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

        let thread_stop = Arc::clone(&stop);
        let thread_served = Arc::clone(&served);
        let thread_requests = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(StdDuration::from_secs(5)));
                        thread_served.fetch_add(1, Ordering::SeqCst);
                        let request = read_request(&stream).unwrap_or_default();
                        let framed = answer(&peer, &request);
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request);
                        write_all(stream, &framed);
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
            routes,
            requests,
            handle: Some(handle),
        }
    }

    /// The base URL, `http://127.0.0.1:port`.
    pub(crate) fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// How many connections have been served. The number a walk asserts when it
    /// claims something crossed a socket, and the number it asserts is
    /// unchanged when it claims nothing did.
    pub(crate) fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }

    pub(crate) fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Every request the server took for one method and path.
    pub(crate) fn requests_to(&self, method: &str, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == method && request.path() == path)
            .collect()
    }

    /// How many requests a route has answered, by the substring it matches on.
    pub(crate) fn hits(&self, needle: &str) -> usize {
        self.routes
            .iter()
            .filter(|route| match &route.matcher {
                Match::Target(_, target) => *target == needle,
                Match::Rpc(name) => *name == needle,
            })
            .map(Route::hits)
            .sum()
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A framed HTTP response, ready to write.
struct Framed {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn answer(peer: &Peer, request: &Request) -> Framed {
    match peer {
        Peer::Routed(routes) => {
            let reply = routes
                .iter()
                .find(|route| route.matches(request))
                .map_or_else(
                    || {
                        Reply::json(
                            404,
                            format!(
                                r#"{{"error":"no route for {} {}"}}"#,
                                request.method,
                                request.path()
                            ),
                        )
                    },
                    |route| route.next(request),
                );
            Framed {
                status: reply.status,
                content_type: "application/json",
                body: reply.body.into_bytes(),
            }
        }
        Peer::Mesh(endpoint) => {
            let method = Method::parse(&request.method).unwrap_or(Method::Get);
            let response = endpoint.handle(method, &request.target, request.body.as_bytes());
            Framed {
                status: response.status,
                content_type: EndpointResponse::CONTENT_TYPE,
                body: response.body,
            }
        }
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
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

    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

fn write_all(mut stream: TcpStream, framed: &Framed) {
    let mut out = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        framed.status,
        framed.content_type,
        framed.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&framed.body);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    // Closing is part of the framing this walk's clients expect, so it is
    // explicit rather than left to the drop at the end of the scope.
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
