//! The ledger of record on Spanner (ADR 0113; R12-LEDGER-8007, 8024, 8134).
//!
//! Every test runs the ledger's own logic over `SpannerBackend` against an
//! in-process Spanner REST double bound on a loopback port: the requests
//! the adapter writes are the bytes the double reads, and the double holds
//! rows, sessions and read-write transactions with the one property the
//! fence depends on — a commit that writes a key aborts every other open
//! transaction that read it, as Spanner's locking does.
//!
//! What the double does not prove is that Google answers the way it does:
//! the request shapes follow the published REST reference, and the first
//! run against a real instance is the evidence for that, not this file.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "../../../libs/qip-transport/tests/support/certs.rs"]
mod certs;
#[path = "support/pki.rs"]
mod pki;

use qip_contracts::ledger::Settlement;
use qip_contracts::ledger_operation::{Book, LedgerOperationFields, OperationKind, Phase};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2};
use qip_contracts::validated::Validated;
use qip_core::{Clock, ManualClock, SystemClock, Timestamp, dec, sha256_hex};
use qip_ledgerd::config::LedgerConfig;
use qip_ledgerd::consumer::Connect;
use qip_ledgerd::spanner::SpannerBackend;
use qip_ledgerd::store::{
    Delivery, Disposition, GENESIS, LedgerStore, OperationDisposition, P1Record,
};
use qip_ledgerd::telemetry::LedgerTelemetry;
use qip_observability::metrics::Metrics;
use qip_storage::gcp::spanner::{SpannerConfig, SpannerDatabase, SpannerKv};
use qip_storage::{EngineConfig, GcpAccess, StaticToken};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport};
use qip_transport::retry::Sleeper;
use qip_transport::server::{Handler, Method, Request, Response, Server, ServerLimits};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const DATABASE: &str = "projects/qip-test/instances/ledger/databases/positions";

// --- the Spanner double ---------------------------------------------------------

#[derive(Default)]
struct Transaction {
    reads: BTreeSet<String>,
    aborted: bool,
}

#[derive(Default)]
struct State {
    rows: BTreeMap<String, String>,
    sessions: u64,
    next_transaction: u64,
    transactions: BTreeMap<String, Transaction>,
    /// Mutations per successful commit, in order.
    commits: Vec<usize>,
}

#[derive(Default)]
struct FakeSpanner {
    state: Mutex<State>,
}

fn error(status: u16, message: &str) -> Response {
    Response::json(
        status,
        json!({ "error": { "message": message } }).to_string(),
    )
}

fn key_of(value: &Value) -> Option<String> {
    value.as_array()?.first()?.as_str().map(str::to_string)
}

impl FakeSpanner {
    fn read(state: &mut State, body: &Value) -> Response {
        let limit: usize = body["limit"]
            .as_str()
            .and_then(|l| l.parse().ok())
            .unwrap_or(usize::MAX);
        let mut keys: Vec<String> = Vec::new();
        if let Some(wanted) = body["keySet"]["keys"].as_array() {
            for key in wanted.iter().filter_map(key_of) {
                if state.rows.contains_key(&key) {
                    keys.push(key);
                }
            }
        } else if let Some(ranges) = body["keySet"]["ranges"].as_array() {
            for range in ranges {
                let end = key_of(&range["endOpen"]).unwrap_or_default();
                let (start, open) = match key_of(&range["startOpen"]) {
                    Some(start) => (start, true),
                    None => (key_of(&range["startClosed"]).unwrap_or_default(), false),
                };
                for key in state.rows.keys() {
                    let after = if open { *key > start } else { *key >= start };
                    if after && *key < end {
                        keys.push(key.clone());
                    }
                }
            }
        } else {
            return error(400, "keySet has neither keys nor ranges");
        }
        keys.truncate(limit);
        if let Some(id) = body["transaction"]["id"].as_str() {
            let Some(transaction) = state.transactions.get_mut(id) else {
                return error(404, "no such transaction");
            };
            transaction.reads.extend(keys.iter().cloned());
        }
        let rows: Vec<Value> = keys
            .iter()
            .map(|key| json!([key, state.rows[key]]))
            .collect();
        Response::json(200, json!({ "rows": rows }).to_string())
    }

    fn commit(state: &mut State, body: &Value) -> Response {
        let committing = body["transactionId"].as_str().map(str::to_string);
        if let Some(id) = &committing {
            match state.transactions.get(id) {
                None => return error(404, "no such transaction"),
                Some(transaction) if transaction.aborted => {
                    state.transactions.remove(id);
                    return error(409, "ABORTED: another transaction wrote what this one read");
                }
                Some(_) => {}
            }
        }
        let mutations = body["mutations"].as_array().cloned().unwrap_or_default();
        let mut written = BTreeSet::new();
        for mutation in &mutations {
            if let Some(values) = mutation["insertOrUpdate"]["values"].as_array() {
                for value in values {
                    let pair = value.as_array().cloned().unwrap_or_default();
                    let (Some(key), Some(text)) = (
                        pair.first().and_then(Value::as_str),
                        pair.get(1).and_then(Value::as_str),
                    ) else {
                        return error(400, "a value is not [k, v]");
                    };
                    state.rows.insert(key.to_string(), text.to_string());
                    written.insert(key.to_string());
                }
            } else if let Some(keys) = mutation["delete"]["keySet"]["keys"].as_array() {
                for key in keys.iter().filter_map(key_of) {
                    state.rows.remove(&key);
                    written.insert(key);
                }
            } else {
                return error(400, "an unknown mutation");
            }
        }
        if let Some(id) = &committing {
            state.transactions.remove(id);
        }
        for transaction in state.transactions.values_mut() {
            if transaction.reads.iter().any(|read| written.contains(read)) {
                transaction.aborted = true;
            }
        }
        state.commits.push(mutations.len());
        Response::json(
            200,
            json!({ "commitTimestamp": "2026-10-11T00:00:00Z" }).to_string(),
        )
    }
}

impl Handler for FakeSpanner {
    fn handle(&self, request: &Request) -> Response {
        if !matches!(request.method, Method::Post) {
            return error(405, "POST only");
        }
        if request.header("authorization").is_none() {
            return error(401, "no credential");
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let mut state = self.state.lock().unwrap();
        let path = request.path.strip_prefix("/v1/").unwrap_or_default();
        if path == format!("{DATABASE}/sessions") {
            state.sessions += 1;
            let name = format!("{DATABASE}/sessions/s{}", state.sessions);
            return Response::json(200, json!({ "name": name }).to_string());
        }
        let Some((session, method)) = path.rsplit_once(':') else {
            return error(404, "no route");
        };
        if !session.starts_with(&format!("{DATABASE}/sessions/")) {
            return error(404, "no such session");
        }
        match method {
            "beginTransaction" => {
                state.next_transaction += 1;
                let id = format!("t{}", state.next_transaction);
                state
                    .transactions
                    .insert(id.clone(), Transaction::default());
                Response::json(200, json!({ "id": id }).to_string())
            }
            "read" => Self::read(&mut state, &body),
            "commit" => Self::commit(&mut state, &body),
            "rollback" => {
                if let Some(id) = body["transactionId"].as_str() {
                    state.transactions.remove(id);
                }
                Response::json(200, "{}")
            }
            _ => error(404, "no such method"),
        }
    }
}

struct Double {
    fake: Arc<FakeSpanner>,
    address: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Double {
    fn start() -> Self {
        let fake = Arc::new(FakeSpanner::default());
        let server = Server::bind("127.0.0.1:0", fake.clone(), ServerLimits::default())
            .expect("the double binds");
        let address = server.local_address().expect("address");
        let stop = server.shutdown_handle();
        let thread = std::thread::spawn(move || {
            let _ = server.serve();
        });
        Self {
            fake,
            address,
            stop,
            thread: Some(thread),
        }
    }

    fn access(&self) -> GcpAccess {
        GcpAccess::unconfigured()
            .with_endpoint(&format!("http://{}", self.address))
            .expect("loopback endpoint")
            .with_tokens(Arc::new(
                StaticToken::new("fixture-spanner-access").expect("a token"),
            ))
    }

    fn kv(&self) -> SpannerKv {
        SpannerKv::new(SpannerConfig::new(
            SpannerDatabase::new("qip-test", "ledger", "positions").expect("names"),
            self.access(),
        ))
        .expect("a configured client")
    }

    fn ledger(&self, writer: &str) -> LedgerStore {
        let backend = SpannerBackend::claim(self.kv(), writer).expect("the writer claims");
        LedgerStore::with_backend(Box::new(backend), telemetry()).expect("the ledger opens")
    }

    fn commits(&self) -> Vec<usize> {
        self.fake.state.lock().unwrap().commits.clone()
    }
}

impl Drop for Double {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// --- fixtures -------------------------------------------------------------------

fn telemetry() -> LedgerTelemetry {
    LedgerTelemetry::new(Arc::new(Metrics::new("qip-ledgerd")))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Two simulated fills on one cell's chain, as P1 deliveries.
fn two_fills() -> Vec<Delivery> {
    let mut previous = GENESIS.to_string();
    let mut deliveries = Vec::new();
    for (sequence, (order, side, price)) in [
        ("o-0", BookSide::Bid, "10.50"),
        ("o-1", BookSide::Ask, "11.25"),
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = sequence as u64;
        let at = Timestamp::from_secs(1_790_000_000 + sequence as i64);
        let decision = Decision::Filled {
            order_id: order.to_string(),
            venue: "sim-xnys".to_string(),
            object: "OBJ-ACME".to_string(),
            quantity: "10".to_string(),
            price: price.to_string(),
            simulated: true,
            shares: vec![("alpha".to_string(), "10".to_string())],
            side: Some(side),
            quote_unit: Some("USD".to_string()),
            fee: None,
        };
        let (decision, digest) = seal_v2(&previous, sequence, at, decision);
        previous = digest.clone();
        let record = P1Record::Outcome(Box::new(OutcomeRecord {
            cell: "cell-a".to_string(),
            session: 1,
            journal_sequence: sequence,
            journal_digest: digest.clone(),
            entry: JournalEntry {
                sequence,
                at,
                decision,
                digest,
                version: ChainVersion::V2,
            },
        }));
        deliveries.push(Delivery {
            partition: "p0".to_string(),
            offset: sequence,
            record_hash: sha256_hex(&serde_json::to_vec(&record).expect("serialises")),
            record,
        });
    }
    deliveries
}

fn local_ledger() -> (LedgerStore, std::path::PathBuf) {
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledger-spanner-local-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(1_790_000_000)));
    let store = LedgerStore::open(&dir, EngineConfig::new(clock), telemetry()).expect("opens");
    (store, dir)
}

// --- tests --------------------------------------------------------------------

/// ADR 0113 §1 and R12-LEDGER-8007's check: a delivery commits through the
/// Spanner adapter in **one** transaction, and the ledger on Spanner books
/// what the same logic books on the local store. The reopened ledger passes
/// the history check against what Spanner holds, so the balances there are
/// the fold of the postings there.
///
/// Mutation: in `SpannerBackend::commit`, commit each mutation in its own
/// `kv.commit(None, …)` — a delivery becomes several commits and the
/// one-commit assertion fails.
#[test]
fn a_ledger_on_spanner_books_each_delivery_in_one_transaction_and_holds_what_the_local_store_holds()
{
    let double = Double::start();
    let ledger = double.ledger("writer-a");
    let (local, dir) = local_ledger();
    for delivery in two_fills() {
        let before = double.commits().len();
        let on_spanner = ledger.apply(&delivery).expect("applies on Spanner");
        assert!(
            matches!(on_spanner, Disposition::Posted { .. }),
            "premise: the fill posts, got {on_spanner:?}"
        );
        let commits = double.commits();
        assert_eq!(
            commits.len(),
            before + 1,
            "one delivery, one Spanner transaction"
        );
        assert!(
            commits[before] >= 4,
            "the transaction carries the postings' balances, the chain record, the tail and the \
             offset together: {} mutations",
            commits[before]
        );
        assert_eq!(local.apply(&delivery).expect("applies locally"), on_spanner);
    }
    let operation = Validated::new(LedgerOperationFields {
        schema_version: 1,
        operation_id: "treasury-1".to_string(),
        origin: "capital-desk".to_string(),
        sequence: 1,
        recorded_at: Timestamp::from_secs(1_790_000_100),
        settlement: Settlement::Simulated,
        kind: OperationKind::TreasuryAllocation {
            treasury: "paper-pool".to_string(),
            book: Book {
                cell: "cell-a".to_string(),
                strategy: "alpha".to_string(),
            },
            unit: "USD".to_string(),
            amount: dec!("1000"),
            phase: Phase::Establish,
        },
    })
    .expect("admissible");
    let before = double.commits().len();
    assert_eq!(
        ledger.record_operation(&operation).expect("records"),
        OperationDisposition::Posted { postings: 2 }
    );
    assert_eq!(double.commits().len(), before + 1);
    local.record_operation(&operation).expect("records locally");

    let on_spanner = ledger.balances().expect("balances");
    assert!(!on_spanner.is_empty(), "premise: something was booked");
    assert_eq!(on_spanner, local.balances().expect("local balances"));
    drop(ledger);
    let reopened = double.ledger("writer-b");
    assert_eq!(reopened.balances().expect("balances"), on_spanner);
    assert_eq!(reopened.backend_kind(), "spanner");
    let _ = std::fs::remove_dir_all(dir);
}

/// Two `qip-ledgerd` processes on one database never both write: the newer
/// claim fences the older out, the older's commit is refused with nothing
/// applied, and the newer books the same record as if the older had never
/// tried. The failure this prevents is a replaced machine and its
/// replacement interleaving postings on one chain tail.
///
/// Mutation: delete the writer-row comparison in `SpannerBackend::commit`
/// — the fenced writer's fill books and the balances are non-empty after
/// its refused apply.
#[test]
fn a_second_writer_claiming_the_spanner_ledger_fences_the_first_out() {
    let double = Double::start();
    let first = double.ledger("writer-old");
    let second = double.ledger("writer-new");
    let fills = two_fills();
    let refused = first
        .apply(&fills[0])
        .expect_err("the fenced writer is refused");
    assert!(
        refused
            .to_string()
            .contains("another qip-ledgerd has claimed the ledger"),
        "refused for the wrong reason: {refused}"
    );
    assert!(
        second.balances().expect("balances").is_empty(),
        "the fenced writer booked something"
    );
    assert!(matches!(
        second.apply(&fills[0]).expect("the current writer applies"),
        Disposition::Posted { .. }
    ));
}

/// A deployed ledger's configuration: the deployment of the same name, so
/// both of its links are mutual TLS (ADR 0112) — the agreement `parse`
/// requires. The certificate is `pki`'s, minted for the run and on disk,
/// because `start` loads it for the read API's listener; the fabric half
/// goes through `unreachable_broker`, which never presents it.
fn spanner_config(environment: &str, pki: &pki::Pki) -> LedgerConfig {
    let deployment = match environment {
        "staging" => "stage",
        other => other,
    };
    let mut variables: BTreeMap<String, String> = [
        (qip_ledgerd::config::ENVIRONMENT, environment),
        (qip_ledgerd::config::DEPLOYMENT, deployment),
        (qip_ledgerd::config::LISTEN, "127.0.0.1:0"),
        (qip_ledgerd::config::CONSUMER_GROUP, "ledger"),
        (qip_ledgerd::config::FABRIC_URL, "https://127.0.0.1:9"),
        (qip_ledgerd::config::FABRIC_STREAM, "reflex-outcomes"),
        (qip_ledgerd::config::FABRIC_PARTITIONS, "1"),
        (qip_ledgerd::config::POLL_MS, "20"),
        (qip_ledgerd::config::SPANNER_PROJECT, "qip-test"),
        (qip_ledgerd::config::SPANNER_INSTANCE, "ledger"),
        (qip_ledgerd::config::SPANNER_DATABASE, "positions"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    variables.extend(pki.variables());
    LedgerConfig::parse(&variables).expect("a deployed Spanner configuration loads")
}

/// Waits a millisecond whatever it is asked: the partition workers in these
/// tests retry a broker that is never there, and the test should not wait
/// out their production backoff before it can stop them.
#[derive(Debug)]
struct BriefSleeper;

impl Sleeper for BriefSleeper {
    fn sleep(&self, _duration: qip_core::Duration) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn unreachable_broker() -> Connect {
    Arc::new(|| {
        let transport: Box<dyn FabricTransport + Send> = Box::new(HttpTransport::new(
            "http://127.0.0.1:9".to_string(),
            BearerToken::new("fixture-token-ledger-spanner-000000000000000000000000".to_string())?,
        ));
        Ok(transport)
    })
}

/// The composition `main` runs: a deployed ledger whose Spanner database it
/// cannot reach refuses to start — with no credential, or at an address
/// nothing answers on — and there is no local store it could have fallen
/// back to. The premise half: the same configuration against a reachable
/// database starts, reports ready, and names its store on `/health`.
///
/// Mutation: in `qip_ledgerd::open_store`, answer a `SpannerKv::new` error
/// by opening a `LocalBackend` in a temporary directory — the unconfigured
/// start succeeds and the first assertion fails.
#[test]
fn a_deployed_ledger_starts_only_on_a_spanner_database_it_can_reach() {
    let tls_dir =
        std::env::temp_dir().join(format!("qip-ledgerd-spanner-tls-{}", std::process::id()));
    let pki = pki::Pki::mint(&tls_dir);
    let config = spanner_config("prod", &pki);
    let refused = qip_ledgerd::start_with_access(
        &config,
        Arc::new(SystemClock),
        Arc::new(BriefSleeper),
        unreachable_broker(),
        GcpAccess::unconfigured(),
    )
    .expect_err("no credential, no start");
    assert!(
        refused.to_string().contains("Cloud Spanner"),
        "refused for the wrong reason: {refused}"
    );

    let nobody = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let silent = format!("http://{}", nobody.local_addr().expect("address"));
    drop(nobody);
    let closed = GcpAccess::unconfigured()
        .with_endpoint(&silent)
        .expect("endpoint")
        .with_tokens(Arc::new(
            StaticToken::new("fixture-spanner-access").expect("token"),
        ));
    assert!(
        qip_ledgerd::start_with_access(
            &config,
            Arc::new(SystemClock),
            Arc::new(BriefSleeper),
            unreachable_broker(),
            closed,
        )
        .is_err(),
        "an unreachable database starts a ledger"
    );

    let double = Double::start();
    let running = qip_ledgerd::start_with_access(
        &config,
        Arc::new(SystemClock),
        Arc::new(BriefSleeper),
        unreachable_broker(),
        double.access(),
    )
    .expect("premise: a reachable database starts the ledger");
    assert!(running.is_ready());
    assert_eq!(running.store().backend_kind(), "spanner");
    // A deployed ledger's read API is mutual TLS, so health is read by a
    // client presenting a certificate the ledger's roots chain.
    let response = pki::get(&pki.client(&pki.reader), running.address(), "/health")
        .expect("health answers a certified reader");
    let body: Value = serde_json::from_slice(&response.body).expect("health is JSON");
    assert_eq!(body["store"], json!("spanner"));
    assert!(
        double.commits().len() >= 2,
        "premise: the start claimed the writer row and committed its probe"
    );
    running.stop();
    let _ = std::fs::remove_dir_all(&tls_dir);
}
