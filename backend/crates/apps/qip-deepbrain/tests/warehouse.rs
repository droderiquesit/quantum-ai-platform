//! Resolved outcomes reaching the research warehouse (R12-DATA-9054,
//! R12-DATA-8127, ADR 0240: staged in Cloud Storage and batch-loaded), against
//! a loopback socket standing in for the TLS-terminating proxy a deployment
//! puts in front of BigQuery.
//!
//! The server below is the mesh suite's, trimmed to what this file scripts:
//! it records each request and answers from a list. It is copied rather than
//! shared for the reason that suite gives — a scaffold reached across crates
//! would be an edge between two crates' tests nothing else has.

#![allow(clippy::panic_in_result_fn)]

use qip_core::{Duration, Timestamp};
use qip_deepbrain::warehouse::{OUTCOMES_TABLE, OutcomeSink, STAGING_PREFIX};
use qip_learning_engine::evaluation::{Evaluation, Verdict};
use qip_mesh::bigquery::{BigQueryConfig, BigQueryWarehouse};
use qip_storage::gcp::{CloudStorageBlobStore, CloudStorageConfig, GcpAccess, StaticToken};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Request {
    target: String,
    body: serde_json::Value,
    raw: Vec<u8>,
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
        raw: body,
    })
}

/// Answer with `body`, or with the status a `STATUS:<code>:` prefix names.
fn respond(mut stream: TcpStream, body: &str) {
    let (status, body) = match body.strip_prefix("STATUS:") {
        Some(rest) => {
            let (code, body) = rest.split_once(':').unwrap_or((rest, "{}"));
            (code.parse::<u16>().unwrap_or(500), body)
        }
        None => (200, body),
    };
    let out = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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
    let warehouse = BigQueryWarehouse::new(
        BigQueryConfig::new("proj", "research")
            .with_access(access.clone())
            .with_location("us-east4"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let staging =
        CloudStorageBlobStore::new(CloudStorageConfig::new("staging").with_access(access))
            .unwrap_or_else(|e| panic!("{e}"));
    OutcomeSink::new(warehouse, Box::new(staging), "staging")
}

/// Answers, in order, for one staged batch: the upload, then the load job.
const UPLOADED: &str = "{}";
const RUNNING: &str = r#"{"status":{"state":"RUNNING"}}"#;
const DONE: &str = r#"{"status":{"state":"DONE"}}"#;
const FAILED: &str =
    r#"{"status":{"state":"DONE","errorResult":{"reason":"invalid","message":"bad row"}}}"#;
const LOST: &str = "STATUS:503:{}";
const NO_SUCH_JOB: &str = r#"STATUS:404:{"error":{"message":"Not found: Job"}}"#;

fn job_of(request: &Request) -> String {
    request.body["jobReference"]["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("not a load request: {}", request.target))
        .to_string()
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
/// against an empty table. Under ADR 0240 the grades go in by batch load:
/// one newline-delimited object staged under the expiring prefix, one load
/// job naming it, and the grades counted as shipped only when the job is
/// done.
///
/// Mutation: in `OutcomeSink::ship`, mark a batch shipped as soon as its job
/// is accepted (`LoadState::Pending` arm extends `shipped`) — the first
/// cycle reports the grades sent before the table holds them and this fails.
#[test]
fn a_resolved_outcome_is_staged_loaded_once_and_counted_only_when_its_job_is_done() {
    let server = Server::script(vec![UPLOADED, RUNNING, DONE]);
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

    let first = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        (first.sent, first.staged, first.rejected, first.held),
        (0, 2, 0, 2),
        "a load that was accepted is not a load that happened"
    );
    let requests = server.requests();
    let [upload, load] = requests.as_slice() else {
        panic!("expected an upload and a load; got {requests:?}");
    };
    assert!(
        upload.target.starts_with("/upload/storage/v1/b/staging/o")
            && upload.target.contains(&format!(
                "name={}{OUTCOMES_TABLE}",
                STAGING_PREFIX.replace('/', "%2F")
            )),
        "{}",
        upload.target
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(upload.raw.clone())
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["hypothesis_id"], "hyp-1");
    assert_eq!(lines[1]["verdict"], "wrong");
    assert!(
        lines[0].get("rationale").is_none(),
        "free text is not a warehouse column"
    );
    assert_eq!(load.target, "/bigquery/v2/projects/proj/jobs");
    let source = load.body["configuration"]["load"]["sourceUris"][0]
        .as_str()
        .expect("a source");
    assert!(
        source.starts_with("gs://staging/warehouse-staging/resolved_outcomes/"),
        "{source}"
    );

    // The next cycle reads the job, finds it done, and stages nothing new.
    let second = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((second.sent, second.staged, second.held), (2, 0, 0));
    assert_eq!(sink.pending(&window), 0);
    let third = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((third.sent, third.staged), (0, 0));
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        3,
        "a shipped grade was sent again: {requests:?}"
    );
    assert!(
        requests.iter().all(|r| !r.target.contains("insertAll")),
        "nothing streams rows"
    );
}

/// A load that fails releases its grades, which go out again in a new
/// batch under a new job.
///
/// Mutation: in `ship`'s settling loop, treat `LoadState::Failed` as done —
/// the failed grades are counted shipped and never staged again.
#[test]
fn a_failed_load_releases_its_grades_to_a_new_batch() {
    let server = Server::script(vec![UPLOADED, RUNNING, FAILED, UPLOADED, RUNNING]);
    let mut sink = sink(&server);
    let window = vec![graded("hyp-1", Verdict::Vindicated)];
    sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    let later = now().saturating_add(Duration::from_mins(1));
    let second = sink.ship(&window, later).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((second.sent, second.rejected, second.staged), (0, 1, 1));
    let requests = server.requests();
    assert_eq!(requests.len(), 5, "{requests:?}");
    let first_job = job_of(&requests[1]);
    let second_job = job_of(&requests[4]);
    assert_ne!(first_job, second_job, "a released batch is a new job");
    assert_eq!(sink.pending(&window), 1);
}

/// A load whose answer was lost is the same job next cycle: the batch was
/// recorded pending before the request, so the next cycle asks BigQuery
/// about that job id and finds it, instead of staging the grades again under
/// a new one and loading them twice (ADR 0240 decision 2).
///
/// Mutation: record the batch pending only after `load` answers — the lost
/// answer leaves nothing pending and the next cycle stages a second job.
#[test]
fn a_load_whose_answer_was_lost_is_found_by_its_id_and_never_loaded_twice() {
    let server = Server::script(vec![UPLOADED, LOST, RUNNING, DONE]);
    let mut sink = sink(&server);
    let window = vec![graded("hyp-1", Verdict::Vindicated)];
    assert!(
        sink.ship(&window, now()).is_err(),
        "premise: the load's answer was lost"
    );
    let later = now().saturating_add(Duration::from_mins(1));
    let second = sink.ship(&window, later).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((second.sent, second.staged), (0, 0), "nothing staged twice");
    let third = sink.ship(&window, later).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(third.sent, 1);
    let requests = server.requests();
    let loads = requests
        .iter()
        .filter(|r| r.target == "/bigquery/v2/projects/proj/jobs")
        .count();
    assert_eq!(loads, 1, "the grades were submitted twice: {requests:?}");
    let job = job_of(&requests[1]);
    assert!(
        requests[2]
            .target
            .contains(&format!("/jobs/{job}?location=us-east4")),
        "{}",
        requests[2].target
    );
}

/// A batch whose job was never created is released and staged again under
/// the same id, because nothing about it ran.
///
/// Mutation: treat a 404 like any unreadable state (the `NotFound` arm
/// removed) — the batch stays pending forever and is never staged again.
#[test]
fn a_batch_whose_job_never_existed_is_staged_again_under_the_same_id() {
    let server = Server::script(vec![UPLOADED, LOST, NO_SUCH_JOB, UPLOADED, RUNNING]);
    let mut sink = sink(&server);
    let window = vec![graded("hyp-1", Verdict::Vindicated)];
    assert!(
        sink.ship(&window, now()).is_err(),
        "premise: the load failed"
    );
    let second = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(second.staged, 1, "the released batch was not staged again");
    let requests = server.requests();
    assert_eq!(requests.len(), 5, "{requests:?}");
    assert_eq!(job_of(&requests[1]), job_of(&requests[4]));
}

/// A grade whose load fails three times is quarantined: held out of every
/// later batch and reported, rather than reloaded every cycle as a billed job
/// that fails the same way.
///
/// Mutation: drop the quarantine filter from `fresh` — a fourth batch is
/// staged and this fails.
#[test]
fn a_grade_whose_load_keeps_failing_is_quarantined_not_reloaded_every_cycle() {
    let server = Server::script(vec![UPLOADED, FAILED, UPLOADED, FAILED, UPLOADED, FAILED]);
    let mut sink = sink(&server);
    let window = vec![graded("hyp-1", Verdict::Wrong)];
    for _ in 0..3 {
        let round = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(round.rejected, 1, "premise: each load fails");
    }
    let fourth = sink.ship(&window, now()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((fourth.staged, fourth.quarantined), (0, 1));
    assert_eq!(
        server.requests().len(),
        6,
        "a quarantined grade was staged again"
    );
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

    // Reachable and staged but naming no location: refused, because a load
    // job's state is found only in its location.
    let unlocated = OutcomeSink::from_lookup(
        &lookup(&[
            ("QIP_BIGQUERY_PROJECT", "proj"),
            ("QIP_BIGQUERY_DATASET", "research"),
            (
                "QIP_BIGQUERY_STAGING_BUCKET",
                "qip-dev-segments-us-central1",
            ),
            ("QIP_GCP_ENDPOINT", "http://127.0.0.1:9"),
            ("QIP_GCP_ACCESS_TOKEN", "test-token-value"),
        ]),
        clock.clone(),
    )
    .expect_err("a warehouse with no location was accepted");
    assert!(
        unlocated.message().contains("QIP_BIGQUERY_LOCATION"),
        "{}",
        unlocated.message()
    );

    // Reachable but naming no staging bucket: refused, because the warehouse
    // is fed only by batch load (ADR 0240) and has no streaming fallback.
    let unstaged = OutcomeSink::from_lookup(
        &lookup(&[
            ("QIP_BIGQUERY_PROJECT", "proj"),
            ("QIP_BIGQUERY_DATASET", "research"),
            ("QIP_BIGQUERY_LOCATION", "us-east4"),
            ("QIP_GCP_ENDPOINT", "http://127.0.0.1:9"),
            ("QIP_GCP_ACCESS_TOKEN", "test-token-value"),
        ]),
        clock.clone(),
    )
    .expect_err("a warehouse with no staging bucket was accepted");
    assert!(
        unstaged.message().contains("QIP_BIGQUERY_STAGING_BUCKET"),
        "{}",
        unstaged.message()
    );

    // Fully named: a sink.
    let sink = OutcomeSink::from_lookup(
        &lookup(&[
            ("QIP_BIGQUERY_PROJECT", "proj"),
            ("QIP_BIGQUERY_DATASET", "research"),
            (
                "QIP_BIGQUERY_STAGING_BUCKET",
                "qip-dev-segments-us-central1",
            ),
            ("QIP_BIGQUERY_LOCATION", "us-east4"),
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
/// graded is staged and its load submitted on the next cycle, with nothing
/// in the test calling `ship`. Mutation: delete the `loops.warehouse` arm
/// from `node::run_with` and the server sees no request.
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

    let server = Server::script(vec![UPLOADED, RUNNING]);
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
    assert_eq!(requests.len(), 2, "an upload and a load: {requests:?}");
    assert!(
        String::from_utf8_lossy(&requests[0].raw).contains("\"hypothesis_id\":\"hyp-loop\""),
        "the staged batch carries the graded thesis"
    );
    assert_eq!(requests[1].target, "/bigquery/v2/projects/proj/jobs");
    assert_eq!(shipped.len(), 1);
    assert_eq!(shipped[0].staged, 1);
}
