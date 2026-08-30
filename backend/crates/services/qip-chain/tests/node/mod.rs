//! A real loopback JSON-RPC node, scripted per method.
//!
//! The adapter under test is only interesting where it meets a socket, so the
//! tests give it one. Unlike a plain HTTP fixture this one has to answer
//! *several different calls in one poll* — a head, a block, its receipts, maybe
//! the pending set — so the script is keyed by JSON-RPC method rather than by
//! request order. Keying on order would make every test depend on the number of
//! calls the adapter happens to make, which is exactly the thing a test should
//! be free to change.
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

/// A request as the node received it.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    /// Header names lower-cased, as the client wrote them.
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
}

impl RawRequest {
    /// The JSON-RPC method this request called, where the body is an envelope.
    pub(crate) fn rpc_method(&self) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(&self.body).ok()?;
        Some(value.get("method")?.as_str()?.to_string())
    }

    /// The JSON-RPC id, so a test can assert the call was addressed.
    pub(crate) fn rpc_id(&self) -> Option<u64> {
        let value: serde_json::Value = serde_json::from_str(&self.body).ok()?;
        value.get("id")?.as_u64()
    }
}

/// What the node answers with.
#[derive(Clone, Debug, Default)]
pub(crate) enum Behaviour {
    /// Answer each JSON-RPC call from the script.
    #[default]
    Rpc,
    /// A well-formed HTTP response with this exact body, whatever was asked.
    Body { status: u16, body: String },
    /// A complete, correctly framed response whose body is `bytes` long. For
    /// the node that answers with more than this process will hold.
    Oversized { bytes: usize },
    /// Declare `declared` bytes, send `written`, then close: the node that dies
    /// mid-body.
    Truncated { declared: usize, written: usize },
    /// Say nothing for this long, then answer. For the read timeout.
    Silent(StdDuration),
}

/// What the node says to each JSON-RPC method.
#[derive(Clone, Debug, Default)]
pub(crate) struct NodeScript {
    /// Per method, a queue of `result` members as JSON text. The last entry
    /// repeats, so a script that answers once answers forever.
    results: BTreeMap<String, Vec<String>>,
    /// Per method, an `error` member as JSON text, which wins over a result.
    errors: BTreeMap<String, String>,
    behaviour: Behaviour,
}

impl NodeScript {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Answer `method` with this `result`, for every call.
    pub(crate) fn answering(mut self, method: &str, result: impl Into<String>) -> Self {
        self.results
            .entry(method.to_string())
            .or_default()
            .push(result.into());
        self
    }

    /// Answer `method` with a JSON-RPC error object.
    pub(crate) fn failing(mut self, method: &str, error: impl Into<String>) -> Self {
        self.errors.insert(method.to_string(), error.into());
        self
    }

    pub(crate) fn behaving(mut self, behaviour: Behaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    /// The body to answer one request with, and the queue position consumed.
    fn answer(&mut self, request: &RawRequest) -> Vec<u8> {
        match &self.behaviour {
            Behaviour::Body { status, body } => return framed(*status, body.as_bytes()),
            Behaviour::Oversized { bytes } => {
                let body = vec![b'x'; *bytes];
                return framed(200, &body);
            }
            Behaviour::Truncated { declared, written } => {
                let mut out = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: \
                     {declared}\r\nconnection: close\r\n\r\n"
                )
                .into_bytes();
                out.extend(std::iter::repeat_n(b'x', *written));
                return out;
            }
            Behaviour::Silent(delay) => std::thread::sleep(*delay),
            Behaviour::Rpc => {}
        }

        let id = request.rpc_id().unwrap_or(0);
        let Some(method) = request.rpc_method() else {
            return framed(
                400,
                br#"{"jsonrpc":"2.0","id":0,"error":{"code":-32700,"message":"not an envelope"}}"#,
            );
        };
        if let Some(error) = self.errors.get(&method) {
            let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"error":{error}}}"#);
            return framed(200, body.as_bytes());
        }
        let result = match self.results.get_mut(&method) {
            Some(queue) if queue.len() > 1 => queue.remove(0),
            Some(queue) => queue.first().cloned().unwrap_or_else(|| "null".to_string()),
            None => {
                let body = format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32601,"message":"the method {method} is not in this test node's script"}}}}"#
                );
                return framed(200, body.as_bytes());
            }
        };
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#);
        framed(200, body.as_bytes())
    }
}

/// A listener on an ephemeral loopback port.
pub(crate) struct TestNode {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<RawRequest>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for TestNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestNode")
            .field("address", &self.address)
            .field("served", &self.served.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl TestNode {
    pub(crate) fn running(script: NodeScript) -> Self {
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
        let script = Arc::new(Mutex::new(script));

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
                        thread_requests
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(request.clone());
                        // Answering on its own thread so a deliberately slow
                        // answer delays the client under test and not the next
                        // connection: a client that has timed out must find the
                        // listener ready, exactly as it would against a real
                        // node.
                        let script = script.clone();
                        std::thread::spawn(move || {
                            let bytes = script
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .answer(&request);
                            write_all(stream, &bytes);
                        });
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

    /// The JSON-RPC methods called, in order.
    pub(crate) fn methods_called(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter_map(RawRequest::rpc_method)
            .collect()
    }
}

impl Drop for TestNode {
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
    let mut body = String::new();
    if declared > 0 {
        let mut raw = vec![0u8; declared];
        reader.read_exact(&mut raw).ok()?;
        body = String::from_utf8_lossy(&raw).into_owned();
    }

    Some(RawRequest {
        method,
        target,
        headers,
        body,
    })
}

fn write_all(mut stream: TcpStream, bytes: &[u8]) {
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
    // Closing is the point of two of the behaviours, so it is explicit rather
    // than left to the drop at the end of the scope.
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
