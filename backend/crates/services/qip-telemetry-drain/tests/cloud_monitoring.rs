//! The Cloud Monitoring push (ADR 0121, R12-OBS-9143) against a real
//! loopback socket: what goes on the wire, and what a silent peer costs.
//!
//! A socket rather than a mock client because the two things this exporter
//! must get right are both properties of the wire — the bearer token in the
//! header and the encoded series in the body — and of time: a peer that
//! accepts and says nothing must cost one read timeout, not the thread.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_telemetry_drain::cloud_monitoring::{
    CloudMonitoringConfig, FailureReason, MonitoringClient, RequestOutcome, push_limits, push_once,
    record_push,
};
use qip_transport::ClientLimits;
use qip_transport::kms::BearerSource;
use serde_json::Value;

const START: Timestamp = Timestamp::from_secs(1_790_000_000);
const NOW: Timestamp = Timestamp::from_secs(1_790_000_060);

#[derive(Debug)]
struct Bearer(std::result::Result<String, String>);

impl BearerSource for Bearer {
    fn bearer(&self) -> Result<String> {
        self.0.clone().map_err(Error::denied)
    }
}

/// One request as the loopback API received it.
#[derive(Debug)]
struct Received {
    request_line: String,
    authorization: Option<String>,
    body: Value,
}

/// Serve `connections` requests, answering each 200, and hand back what
/// arrived.
fn api(connections: usize) -> (u16, std::thread::JoinHandle<Vec<Received>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).to_string();
            let header = |wanted: &str| {
                head.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case(wanted)
                        .then(|| value.trim().to_string())
                })
            };
            let length: usize = header("content-length").unwrap().parse().unwrap();
            let mut body = vec![0u8; length];
            stream.read_exact(&mut body).unwrap();
            seen.push(Received {
                request_line: head.lines().next().unwrap().to_string(),
                authorization: header("authorization"),
                body: serde_json::from_slice(&body).unwrap(),
            });
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        }
        seen
    });
    (port, thread)
}

fn config(port: u16) -> CloudMonitoringConfig {
    CloudMonitoringConfig::parse(
        "qip-dev-01",
        Some(&format!("http://127.0.0.1:{port}")),
        Some("europe-west2"),
        None,
    )
    .unwrap()
}

fn telemetry_with_one_of_each() -> Telemetry {
    let telemetry = Telemetry::silent();
    telemetry
        .metrics
        .increment(names::ORDERS_FILLED, labels([("venue", "XNYS")]), 5);
    telemetry
        .metrics
        .gauge(names::KILL_SWITCH_TRIPPED, labels([]), 0.0);
    telemetry
        .metrics
        .observe_latency_ms(names::EXECUTION_LATENCY_MS, labels([]), 3.0);
    telemetry
}

/// The body on the wire is the snapshot's encoding, unaltered, under the
/// bearer token the source handed out, at the API's `timeSeries.create`
/// path. Mutation: drop the `authorization` header in `send_body`, and the
/// header assertion fails with `None`.
#[test]
fn a_push_posts_the_encoded_snapshot_with_the_bearer_token_to_the_projects_time_series() {
    let telemetry = telemetry_with_one_of_each();
    let (port, server) = api(1);
    let config = config(port);
    let task = config.task("qip-api", "4242-1");
    let expected = telemetry
        .metrics
        .snapshot()
        .to_cloud_monitoring_time_series(&task, START, NOW)
        .request_bodies();
    assert_eq!(expected.len(), 1, "premise: three series make one body");

    let client = MonitoringClient::new(&config, push_limits(), None).unwrap();
    let pass = push_once(
        &telemetry,
        &client,
        &config,
        &task,
        &Bearer(Ok("ya29.test-token".to_string())),
        START,
        NOW,
    );
    assert_eq!(
        pass.requests,
        vec![RequestOutcome::Sent {
            status: 200,
            series: 3
        }]
    );
    let received = server.join().unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].request_line,
        "POST /v3/projects/qip-dev-01/timeSeries HTTP/1.1"
    );
    assert_eq!(
        received[0].authorization.as_deref(),
        Some("Bearer ya29.test-token")
    );
    assert_eq!(received[0].body, expected[0]);
    let counter = received[0].body["timeSeries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|series| {
            series["metric"]["type"] == "custom.googleapis.com/qip/qip_orders_filled_total"
        })
        .unwrap();
    assert_eq!(counter["metricKind"], "CUMULATIVE");

    record_push(&telemetry, &pass);
    let snapshot = telemetry.metrics.snapshot();
    assert_eq!(
        snapshot.counter(names::METRICS_EXPORT_REQUESTS, &labels([])),
        1
    );
    assert_eq!(snapshot.counter_total(names::METRICS_EXPORT_FAILURES), 0);
}

/// A peer that accepts the connection and never answers costs one read
/// timeout, is counted as a timeout, and abandons the rest of the pass
/// rather than paying the timeout again for every remaining body.
/// Mutation: set `read_timeout` in the test's limits to the server's hold
/// time, and the elapsed-time assertion fails.
#[test]
fn a_silent_api_times_out_is_counted_and_abandons_the_rest_of_the_pass() {
    let telemetry = Telemetry::silent();
    let gates: Vec<String> = (0..250).map(|i| format!("gate_{i:03}")).collect();
    for gate in &gates {
        telemetry
            .metrics
            .count(names::ORDERS_REFUSED, labels([("gate", gate.as_str())]));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hold = Duration::from_secs(2);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        listener.set_nonblocking(true).unwrap();
        // Read what the client writes, so the client is not stopped by a
        // full socket buffer before it reaches the read it must time out
        // on; then say nothing.
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut sink = [0u8; 16 * 1024];
        while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
        std::thread::sleep(hold);
        drop(stream);
        // A second connection would mean the pass did not stop.
        listener.accept().is_ok()
    });

    let config = config(port);
    let task = config.task("qip-api", "4242-1");
    let limits = ClientLimits {
        read_timeout: Duration::from_millis(300),
        ..push_limits()
    };
    let client = MonitoringClient::new(&config, limits, None).unwrap();
    let began = Instant::now();
    let pass = push_once(
        &telemetry,
        &client,
        &config,
        &task,
        &Bearer(Ok("ya29.test-token".to_string())),
        START,
        NOW,
    );
    let elapsed = began.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "the push waited {elapsed:?} on a silent peer"
    );
    assert_eq!(pass.requests.len(), 1, "{pass:?}");
    assert!(
        matches!(
            pass.requests[0],
            RequestOutcome::Failed {
                reason: FailureReason::Timeout,
                series: 200,
                ..
            }
        ),
        "{pass:?}"
    );
    assert_eq!(pass.abandoned, 50);
    assert!(!pass.landed_or_idle());
    assert!(!server.join().unwrap(), "a second request was attempted");

    record_push(&telemetry, &pass);
    let snapshot = telemetry.metrics.snapshot();
    assert_eq!(
        snapshot.counter(
            names::METRICS_EXPORT_FAILURES,
            &labels([("reason", "timeout")])
        ),
        1
    );
    assert_eq!(
        snapshot.counter(
            names::METRICS_EXPORT_SERIES_DROPPED,
            &labels([("reason", "request_failed")])
        ),
        250
    );
}

/// No token, no socket: a pass whose bearer source fails is counted under
/// `token` and opens no connection.
#[test]
fn a_pass_without_a_token_opens_no_connection_and_is_counted() {
    let telemetry = telemetry_with_one_of_each();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = config(port);
    let task = config.task("qip-api", "4242-1");
    let client = MonitoringClient::new(&config, push_limits(), None).unwrap();
    let pass = push_once(
        &telemetry,
        &client,
        &config,
        &task,
        &Bearer(Err("the metadata server answered HTTP 404".to_string())),
        START,
        NOW,
    );
    assert!(
        matches!(
            pass.requests.as_slice(),
            [RequestOutcome::Failed {
                reason: FailureReason::Token,
                series: 3,
                ..
            }]
        ),
        "{pass:?}"
    );
    assert!(
        listener.accept().is_err(),
        "a connection was opened without a token"
    );
    record_push(&telemetry, &pass);
    assert_eq!(
        telemetry.metrics.snapshot().counter(
            names::METRICS_EXPORT_FAILURES,
            &labels([("reason", "token")])
        ),
        1
    );
}
