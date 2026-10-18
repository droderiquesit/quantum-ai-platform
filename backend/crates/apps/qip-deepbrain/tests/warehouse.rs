//! Resolved outcomes reaching the research warehouse (R12-DATA-9054), against
//! a loopback socket standing in for the TLS-terminating proxy a deployment
//! puts in front of BigQuery.
//!
//! The server below is the mesh suite's, trimmed to what this file scripts:
//! it records each request and answers from a list. It is copied rather than
//! shared for the reason that suite gives — a scaffold reached across crates
//! would be an edge between two crates' tests nothing else has.

#![allow(clippy::panic_in_result_fn)]

use qip_core::{Duration, Timestamp};
use qip_deepbrain::warehouse::{OUTCOMES_TABLE, OutcomeSink};
use qip_learning_engine::evaluation::{Evaluation, Verdict};
use qip_mesh::bigquery::{BigQueryConfig, BigQueryWarehouse};
use qip_storage::gcp::{GcpAccess, StaticToken};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Request {
    target: String,
    body: serde_json::Value,
}

struct Server {
    address: String,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Request>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Answer the n-th request with the n-th body, repeating the last.
    fn script(bodies: Vec<&'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("{e}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("{e}"))
            .to_string();
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|e| panic!("{e}"));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (thread_stop, thread_requests) = (stop.clone(), requests.clone());
        let handle = std::thread::spawn(move || {
            let mut served = 0usize;
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                        if let Some(request) = read_request(&stream) {
                            thread_requests
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(request);
                        }
                        let body = bodies
                            .get(served)
                            .or(bodies.last())
                            .copied()
                            .unwrap_or("{}");
                        served += 1;
                        respond(stream, body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            requests,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?.to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let declared: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; declared];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        target,
        body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    })
}

fn respond(mut stream: TcpStream, body: &str) {
    let out = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn sink(server: &Server) -> OutcomeSink {
    let access = GcpAccess::unconfigured()
        .with_endpoint(&server.url())
        .unwrap_or_else(|e| panic!("{e}"))
        .with_tokens(Arc::new(
            StaticToken::new("test-token-value").unwrap_or_else(|e| panic!("{e}")),
        ));
    let warehouse =
        BigQueryWarehouse::new(BigQueryConfig::new("proj", "research").with_access(access))
            .unwrap_or_else(|e| panic!("{e}"));
    OutcomeSink::new(warehouse)
}

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn graded(id: &str, verdict: Verdict) -> Evaluation {
    Evaluation {
        hypothesis_id: id.to_string(),
        class: "momentum".to_string(),
        subject: "XNYS:ACME".to_string(),
        verdict,
        expected_move_bps: 50.0,
        realised_move_bps: 42.0,
        magnitude_ratio: 0.84,
        confidence: 0.7,
        realised_pnl: 12.5,
        contributors: vec!["macro-analyst".to_string()],
        evaluated_at: now(),
        rationale: "free text that stays in the event log".to_string(),
        horizon: Duration::from_days(1),
    }
}

/// The failure this prevents: the warehouse adapter, its dataset and its
/// alerts existed and no shipped binary ever wrote an outcome to it, so a
/// research query for "which theses were wrong, and in which regime" ran
/// against an empty table. Mutation: make `ship` return before the insert and
/// the server sees no request.
#[test]
fn a_resolved_outcome_is_streamed_once_and_keyed_by_its_hypothesis() {
    let server = Server::script(vec!["{}"]);
    let mut sink = sink(&server);
    let window = vec![
        graded("hyp-1", Verdict::Vindicated),
        graded("hyp-2", Verdict::Wrong),
    ];
    assert_eq!(
        sink.pending(&window),
        2,
        "premise: two grades are unshipped"
    );

    let round = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((round.sent, round.rejected, round.held), (2, 0, 0));
    let requests = server.requests();
    let [request] = requests.as_slice() else {
        panic!("expected one insert; got {requests:?}");
    };
    assert!(
        request
            .target
            .ends_with(&format!("/tables/{OUTCOMES_TABLE}/insertAll")),
        "{}",
        request.target
    );
    let rows = request.body["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["insertId"], "hyp-1");
    assert_eq!(rows[1]["json"]["verdict"], "wrong");
    assert_eq!(rows[0]["json"]["subject"], "XNYS:ACME");
    assert!(
        rows[0]["json"].get("rationale").is_none(),
        "free text is not a warehouse column"
    );

    // Shipped once: the same window sends nothing the second time.
    assert_eq!(sink.pending(&window), 0);
    let again = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.sent, 0);
    assert_eq!(server.requests().len(), 1, "a shipped grade was sent again");
}

/// A row the warehouse rejects is held and offered again; the rest are not.
#[test]
fn a_rejected_row_is_held_and_offered_again_while_the_accepted_one_is_not() {
    let server = Server::script(vec![
        r#"{"insertErrors":[{"index":1,"errors":[{"reason":"invalid","message":"bad"}]}]}"#,
        "{}",
    ]);
    let mut sink = sink(&server);
    let window = vec![
        graded("hyp-1", Verdict::Vindicated),
        graded("hyp-2", Verdict::Wrong),
    ];
    let first = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((first.sent, first.rejected, first.held), (1, 1, 1));
    assert_eq!(sink.pending(&window), 1);

    let second = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(second.sent, 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let retried = requests[1].body["rows"].as_array().expect("rows");
    assert_eq!(retried.len(), 1, "only the rejected row was offered again");
    assert_eq!(retried[0]["insertId"], "hyp-2");
}

#[test]
fn a_deployment_naming_no_warehouse_gets_none_and_a_half_named_one_is_refused() {
    let clock: Arc<dyn qip_core::Clock> = Arc::new(qip_core::ManualClock::new(now()));
    let lookup = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        }
    };
    let none =
        OutcomeSink::from_lookup(&lookup(&[]), clock.clone()).unwrap_or_else(|e| panic!("{e}"));
    assert!(none.is_none(), "an unset target is the warehouse off");

    for half in [
        &[("QIP_BIGQUERY_PROJECT", "proj")][..],
        &[("QIP_BIGQUERY_DATASET", "research")][..],
    ] {
        let refused = OutcomeSink::from_lookup(&lookup(half_static(half)), clock.clone())
            .expect_err("a half-named warehouse was accepted");
        assert!(
            refused.message().contains("BigQuery") || refused.message().contains("warehouse"),
            "{}",
            refused.message()
        );
    }

    // Both named but no endpoint or credential: refused, naming what is missing.
    let unreachable = OutcomeSink::from_lookup(
        &lookup(&[
            ("QIP_BIGQUERY_PROJECT", "proj"),
            ("QIP_BIGQUERY_DATASET", "research"),
        ]),
        clock.clone(),
    )
    .expect_err("a warehouse with no endpoint was accepted");
    assert!(
        unreachable.message().contains("no endpoint")
            && unreachable.message().contains("no credential"),
        "{}",
        unreachable.message()
    );

    // Fully named: a sink.
    let sink = OutcomeSink::from_lookup(
        &lookup(&[
            ("QIP_BIGQUERY_PROJECT", "proj"),
            ("QIP_BIGQUERY_DATASET", "research"),
            ("QIP_GCP_ENDPOINT", "http://127.0.0.1:9"),
            ("QIP_GCP_ACCESS_TOKEN", "test-token-value"),
        ]),
        clock,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(sink.is_some());
}

fn half_static(pairs: &[(&str, &str)]) -> &'static [(&'static str, &'static str)] {
    Box::leak(
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    &*Box::leak(k.to_string().into_boxed_str()),
                    &*Box::leak(v.to_string().into_boxed_str()),
                )
            })
            .collect::<Vec<(&'static str, &'static str)>>()
            .into_boxed_slice(),
    )
}

/// The wiring, through the run loop a binary runs: a thesis the platform
/// graded reaches the warehouse on the next cycle, with nothing in the test
/// calling `ship`. Mutation: delete the `loops.warehouse` arm from
/// `node::run_with` and the server sees no request.
#[test]
fn the_run_loop_ships_a_graded_thesis_to_the_warehouse() {
    use qip_core::{Clock, Context, ManualClock};
    use qip_deepbrain::config::DeepBrainConfig;
    use qip_deepbrain::node::{self, Loops};
    use qip_deepbrain::status::NodeStatus;
    use qip_financial::universe::Universe;
    use qip_kernel::{Platform, PlatformConfig};
    use qip_learning_engine::evaluation::{Outcome, ThesisClaim};
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use qip_storage::{ChainArchive, MemoryKeyValueStore};
    use std::sync::atomic::AtomicBool;

    let start = now();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(start));
    let platform_config = PlatformConfig::default();
    let context = Context::new(clock.clone(), platform_config.seed);
    let mut platform = Platform::new(
        platform_config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let claim = ThesisClaim {
        hypothesis_id: "hyp-loop".to_string(),
        class: "price_dislocation".to_string(),
        subject: "obj-AAA".to_string(),
        formed_at: start.saturating_sub(Duration::from_days(2)),
        resolves_at: start,
        direction: 1.0,
        expected_move_bps: 200.0,
        falsifiers: vec!["it reverts".to_string()],
        confidence: 0.7,
        contributors: Vec::new(),
    };
    let outcome = Outcome {
        hypothesis_id: "hyp-loop".to_string(),
        observed_at: start,
        realised_move_bps: 180.0,
        realised_pnl: 0.0,
        falsifiers_triggered: Vec::new(),
        mechanism_confirmed: None,
    };
    platform
        .learn_from(&[claim], &[outcome], start)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        platform.evaluations().len(),
        1,
        "premise: the platform holds one grade"
    );

    let server = Server::script(vec!["{}"]);
    let mut sink = sink(&server);
    let node_config = DeepBrainConfig {
        cycle_interval: Duration::from_millis(1),
        max_cycles: Some(1),
        ..DeepBrainConfig::default()
    };
    let roster = qip_deepbrain::roster::clear(start).unwrap_or_else(|e| panic!("{e}"));
    let status = Arc::new(Mutex::new(NodeStatus::opening(
        &roster,
        &node_config,
        start,
    )));
    let archive = ChainArchive::open(Arc::new(MemoryKeyValueStore::default()))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut shipped = Vec::new();
    node::run_with(
        &mut platform,
        &archive,
        &node_config,
        &status,
        &Arc::new(AtomicBool::new(false)),
        &clock,
        0,
        None,
        Loops {
            warehouse: Some(&mut sink),
            ..Loops::default()
        },
        |outcome| {
            assert!(
                outcome.side_failures.is_empty(),
                "{:?}",
                outcome.side_failures
            );
            shipped.extend(outcome.warehouse.clone());
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].body["rows"][0]["insertId"], "hyp-loop");
    assert_eq!(shipped.len(), 1);
    assert_eq!(shipped[0].sent, 1);
}
