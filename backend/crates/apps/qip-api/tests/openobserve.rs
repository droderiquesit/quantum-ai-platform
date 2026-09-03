//! The OpenObserve drain thread (ADR 0028): it posts on its own interval, and
//! a peer that refuses or vanishes costs a counted, logged failure — never a
//! crash and never a stalled loop.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::{Clock, SystemClock};
use qip_observability::Telemetry;
use qip_observability::logs::Severity;
use qip_observability::metrics::names;
use qip_transport::{ClientLimits, HttpClient};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

// --- a minimal real loopback server, scripted per request -------------------
//
// A mock client would only prove this module calls `HttpClient::send`; it
// would not prove the drain thread wakes on its own interval and posts twice
// without being asked. Every test here talks to an actual socket for that
// reason, the same choice `qip-transport`'s own HTTP client tests make.

/// One request the server actually received. The authorization header is kept
/// because the property under test is *where* the credential travelled: a test
/// that only sees the path cannot tell a header from a query string.
#[derive(Clone, Debug)]
struct Captured {
    path: String,
    authorization: Option<String>,
}

struct TestServer {
    address: String,
    stop: Arc<AtomicBool>,
    served: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Captured>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    /// Always answer `status` with an empty JSON body.
    fn always(status: u16) -> Self {
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
                        if let Some(captured) = read_request(&stream) {
                            thread_requests
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push(captured);
                        }
                        thread_served.fetch_add(1, Ordering::SeqCst);
                        write_response(stream, status);
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

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<Captured> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|request| request.path.clone())
            .collect()
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

/// An address with a bound-then-dropped listener: nothing answers there, so a
/// connect attempt is refused rather than merely slow.
fn address_with_no_listener() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let address = listener
        .local_addr()
        .expect("the listener has a local address")
        .to_string();
    drop(listener);
    format!("http://{address}")
}

fn read_request(stream: &TcpStream) -> Option<Captured> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let path = line.split_whitespace().nth(1)?.to_string();

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
    let mut body = vec![0u8; declared];
    if declared > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Captured {
        path,
        authorization: headers.get("authorization").cloned(),
    })
}

fn write_response(mut stream: TcpStream, status: u16) {
    let body = b"{}";
    let response = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

// --- the tests ------------------------------------------------------------

fn client() -> HttpClient {
    HttpClient::new(ClientLimits {
        connect_timeout: StdDuration::from_millis(300),
        read_timeout: StdDuration::from_millis(500),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    })
}

#[test]
fn three_drain_passes_post_to_both_endpoints_the_adr_names() -> Result<()> {
    let server = TestServer::always(200);
    let telemetry = Telemetry::silent();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let config = qip_api::OpenObserveConfig::parse(&server.url(), Some("qip"), Some("1"), None)?;
    let http_client = client();

    // The premise: nothing has been posted yet.
    assert_eq!(server.served(), 0, "the premise: no request has landed yet");

    for _ in 0..3 {
        qip_api::openobserve::export_once(&telemetry, &http_client, &config, clock.now())?;
    }

    // Two signals (metrics, traces) per pass, three passes: six requests, and
    // both endpoint paths ADR 0028 names must both have been used.
    assert_eq!(
        server.served(),
        6,
        "three drain passes over two signals must produce six requests"
    );
    let paths = server.paths();
    assert!(
        paths.iter().any(|p| p == "/api/qip/v1/metrics"),
        "the metrics endpoint ADR 0028 names was never posted to: {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "/api/qip/traces"),
        "the traces endpoint ADR 0028 names was never posted to: {paths:?}"
    );
    Ok(())
}

#[test]
fn a_spawned_drain_thread_wakes_on_its_configured_interval_without_being_polled() -> Result<()> {
    // Unlike the test above, this one drives the real `spawn` thread and its
    // real `std::thread::sleep`, proving the interval is honoured end to end
    // rather than only through the function the thread body calls.
    let server = TestServer::always(200);
    let telemetry = Telemetry::silent();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let config = qip_api::OpenObserveConfig::parse(&server.url(), Some("qip"), Some("1"), None)?;

    let handle = qip_api::openobserve::spawn(telemetry, config, clock)?;

    // Bounded poll rather than a fixed sleep: fast on a quiet machine, still
    // correct on a loaded one, and it fails loudly rather than hanging if the
    // thread never wakes at all.
    let deadline = std::time::Instant::now() + StdDuration::from_secs(5);
    while server.served() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(StdDuration::from_millis(20));
    }
    assert!(
        server.served() >= 2,
        "the drain thread never posted on its own; served() = {}",
        server.served()
    );
    drop(handle);
    Ok(())
}

#[test]
fn the_credential_travels_in_the_authorization_header_and_never_in_the_url() -> Result<()> {
    // The credential is a distinctive literal so the assertions can look for
    // it in the one place it belongs and in the one place it must never be.
    // A credential in a path is a credential in the collector's access log,
    // in every proxy between here and it, and in this process's own logs.
    const CREDENTIAL: &str = "Basic cWlwLWRyYWluOmEtdGVzdC1zZWNyZXQ";

    let server = TestServer::always(200);
    let telemetry = Telemetry::silent();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let config = qip_api::OpenObserveConfig::parse(
        &server.url(),
        Some("qip"),
        Some("1"),
        Some(CREDENTIAL.to_string()),
    )?;

    assert_eq!(server.served(), 0, "the premise: no request has landed yet");
    qip_api::openobserve::export_once(&telemetry, &client(), &config, clock.now())?;

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        2,
        "the premise: one pass posts both signals, so there are two requests to inspect"
    );
    for request in &requests {
        assert_eq!(
            request.authorization.as_deref(),
            Some(CREDENTIAL),
            "the credential did not travel in the authorization header: {request:?}"
        );
        assert!(
            !request.path.contains("cWlw"),
            "the credential reached the request line, where it lands in every access log \
             between here and the collector: {}",
            request.path
        );
    }
    Ok(())
}

/// The premise behind `OpenObserveConfig::parse` refusing a credential that
/// carries a line break: the transport does not refuse it, it *repairs* it,
/// and the repaired value is what the collector sees. Proven over a real
/// socket because `HttpRequest`'s headers are private — nothing short of the
/// wire can show what was actually sent.
#[test]
fn a_credential_with_a_line_break_would_reach_the_collector_mutilated_rather_than_rejected() {
    let server = TestServer::always(200);
    let configured = "Bearer abc\r\nx-injected: 1";
    let request = qip_transport::HttpRequest::json(
        qip_transport::Method::Post,
        &format!("{}/api/qip/traces", server.url()),
        b"{}".to_vec(),
    )
    .expect("a well-formed URL builds a request")
    .with_header("authorization", configured);

    let response = client().send(&request).expect("the send was not refused");
    assert!(
        response.is_success(),
        "the premise: the transport sent it rather than failing on the line break"
    );

    let received = server
        .requests()
        .first()
        .and_then(|captured| captured.authorization.clone())
        .expect("the server saw an authorization header");
    assert_ne!(
        received, configured,
        "the premise: what reached the collector is not what was configured"
    );
    assert_eq!(
        received, "Bearer abcx-injected: 1",
        "the transport strips the CR and the LF and sends the rest, silently"
    );
}

#[test]
fn a_post_failure_is_counted_and_logged_and_the_process_survives_it() -> Result<()> {
    // No listener: every attempt is a connect failure, the shape a collector
    // outage actually takes.
    let dead_address = address_with_no_listener();
    let telemetry = Telemetry::new("qip-api", Arc::new(SystemClock));
    telemetry.logger.set_minimum_severity(Severity::Debug);
    let config = qip_api::OpenObserveConfig::parse(&dead_address, Some("qip"), Some("1"), None)?;
    let http_client = client();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    // Premise: nothing has been recorded yet.
    let before = telemetry
        .metrics
        .snapshot()
        .counter_total(names::TELEMETRY_EXPORT_FAILURES);
    assert_eq!(before, 0, "the premise: no failure has been recorded yet");

    // Three full passes through the exact two functions `spawn`'s loop calls
    // — `export_once` then `record` — against a peer that is not there. That
    // this loop runs to completion and reaches the assertions below is part
    // of the proof: a panic here would fail the test before any assertion
    // ran, and principle 10 says a dead collector must not produce one.
    for _ in 0..3 {
        let pass =
            qip_api::openobserve::export_once(&telemetry, &http_client, &config, clock.now())?;
        assert!(
            !pass.metrics.is_ok(),
            "a connect failure to an address with no listener must not read as success"
        );
        assert!(!pass.traces.is_ok());
        qip_api::openobserve::record(&telemetry, &pass);
    }

    let after = telemetry
        .metrics
        .snapshot()
        .counter_total(names::TELEMETRY_EXPORT_FAILURES);
    assert_eq!(
        after, 6,
        "three passes over two signals must record six failures, not zero and not a crash"
    );
    let attempts = telemetry
        .metrics
        .snapshot()
        .counter_total(names::TELEMETRY_EXPORT_ATTEMPTS);
    assert_eq!(
        attempts, 6,
        "an attempt is counted whether or not it succeeds"
    );

    let warnings = telemetry.logger.at_least(Severity::Warn);
    assert!(
        warnings.len() >= 6,
        "each of the six failed sends must be logged at warn or above, not swallowed: {} \
         warning(s) recorded",
        warnings.len()
    );
    assert!(
        warnings
            .iter()
            .any(|record| record.message.contains("OpenObserve")),
        "a failure log must name what it was trying to reach: {warnings:?}"
    );
    Ok(())
}
