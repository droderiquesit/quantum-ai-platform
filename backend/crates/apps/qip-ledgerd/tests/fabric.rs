//! `qip-ledgerd` started the way `main` starts it — `qip_ledgerd::start`,
//! a real read-API listener, one partition worker per partition — against a
//! real broker listener on loopback, with P1 outcome records on the stream
//! shaped exactly as the edge node's writer shapes them.
//!
//! Before these tests the store was proven alone and the binary could not
//! start: the consumer was a placeholder and the read API's `serve` always
//! failed. These hold the path end to end (LEDGER-8134, LEDGER-5013,
//! FABRIC-8092, ADR 0106): a fill on the stream is booked, a restart neither
//! loses nor doubles one, a live fill is refused at the store while another
//! cell keeps booking, and the ledger's lag is visible to the broker's group.
//!
//! The broker here is composed from the `qip-streaming` and `qip-transport`
//! libraries rather than by depending on the `qip-fabricd` app, because
//! nothing may depend on an app (`.claude/rules/architecture/00-boundaries.md`).
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use qip_contracts::ledger::{Account, Direction, LedgerEvent};
use qip_contracts::message::BookSide;
use qip_contracts::reflex::{
    ChainSpan, ChainVersion, Decision, JournalEntry, OutcomeRecord, seal_v2,
};
use qip_core::canonical::canonical_json;
use qip_core::{
    Clock, CorrelationId, Decimal, EventId, Lineage, SystemClock, Timestamp, sha256_hex,
};
use qip_events::AnyEvent;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass, StreamPolicy};
use qip_events::topic::Topic;
use qip_ledgerd::config::LedgerConfig;
use qip_ledgerd::consumer::Connect;
use qip_ledgerd::store::{GENESIS, ParkReason};
use qip_ledgerd::{Running, start};
use qip_observability::metrics::{MetricValue, names};
use qip_portfolio::ledger::{PaperFill, post};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service};
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport};
use qip_transport::http::{ClientLimits, HttpClient};
use qip_transport::retry::ThreadSleeper;
use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};

const STREAM: &str = "outcomes.test";
const GROUP: &str = "ledger";
const LEDGER: &str = "ledger";
const PRODUCER: &str = "reflex:cell-a";
const VENUE: &str = "sim-xnys";
const UNIT: &str = "USD";
const PARTITIONS: u32 = 2;
const EVENTUALLY: StdDuration = StdDuration::from_secs(60);

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

/// A fixture string inside the bearer alphabet, not a credential for
/// anything.
fn token(unique: u64) -> String {
    format!("fixture-token-ledger-{unique}-0000000000000000000000000000")
}

// --- the broker -----------------------------------------------------------------

struct DataPlane(Arc<Service>);

impl Handler for DataPlane {
    fn handle(&self, request: &Request) -> Response {
        let handled = self
            .0
            .handle(request.header(auth::HEADER), &request.path, &request.body);
        Response::new(handled.status, "application/json", handled.body)
    }
}

struct Fabric {
    root: PathBuf,
    unique: u64,
    service: Arc<Service>,
    address: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
    sequence: BTreeMap<u32, (u64, u64)>,
}

impl Fabric {
    fn start() -> Self {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "qip-ledgerd-fabric-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let catalogue = format!(
            r#"{{"streams":[{{"name":"{STREAM}","qos_class":"p1_outcomes","partition_key":"cell",
                "ordering":"per_partition","retention":"derived_state","replication_factor":1,
                "mirroring":"none","overload_policy":"refuse_producer","ack_profile":"archived",
                "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
                "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
                "seal_age_ms":1000,"peak_bytes_per_second":1048576,"topics":[]}}],
              "grants":[{{"identity":"{LEDGER}","stream":"{STREAM}","permission":"consume","key_scope":"any"}}]}}"#
        );
        let catalogue = Catalogue::parse(catalogue.as_bytes()).unwrap();
        let identities = IdentityTable::parse(&format!(
            "{LEDGER} {}\n",
            sha256_hex(token(unique).as_bytes())
        ))
        .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let broker = Arc::new(Broker::open(root.join("broker"), clock.clone()).unwrap());
        let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
            SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required())
        });
        let service =
            Arc::new(Service::new(broker, identities, &catalogue, PARTITIONS, segments).unwrap());
        let server = Server::bind(
            "127.0.0.1:0",
            Arc::new(DataPlane(service.clone())),
            ServerLimits::default(),
        )
        .unwrap();
        let address = server.local_address().unwrap();
        let stop = server.shutdown_handle();
        std::thread::spawn(move || {
            let _ = server.serve();
        });
        Self {
            root,
            unique,
            service,
            address,
            stop,
            sequence: BTreeMap::new(),
        }
    }

    /// Append one batch of `records` to `partition`, as a drain would.
    fn produce(&mut self, partition: u32, records: Vec<Record>) {
        let broker = self.service.broker().clone();
        let (epoch, next) = *self.sequence.entry(partition).or_insert_with(|| {
            (
                broker.init_producer(STREAM, partition, PRODUCER).unwrap(),
                0,
            )
        });
        let count = records.len() as u64;
        let mut batch = Batch::new(
            MessageType::Data,
            QosClass::P1Outcomes.batch_schema_id(),
            BATCH_SCHEMA_VERSION,
            PayloadCodec::CanonicalJson,
            records,
        )
        .unwrap();
        stamp_drain(&mut batch, PRODUCER, epoch, next);
        broker
            .produce_to(STREAM, partition, batch)
            .unwrap()
            .expect("the broker takes the batch");
        self.sequence.insert(partition, (epoch, next + count));
    }

    fn connect(&self) -> Connect {
        let url = format!("http://{}", self.address);
        let unique = self.unique;
        Arc::new(move || {
            let transport: Box<dyn FabricTransport + Send> = Box::new(HttpTransport::new(
                url.clone(),
                BearerToken::new(token(unique))?,
            ));
            Ok(transport)
        })
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.address);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// --- the journal the cells write ------------------------------------------------

/// One cell's journal for one session, sealed as `qip-edge` seals it, and
/// turned into P1 records as `qip-edge-node`'s writer turns it.
struct Journal {
    cell: String,
    entries: Vec<JournalEntry>,
}

impl Journal {
    fn new(cell: &str) -> Self {
        Self {
            cell: cell.to_string(),
            entries: Vec::new(),
        }
    }

    fn seal(&mut self, decision: Decision) -> JournalEntry {
        let sequence = self.entries.len() as u64;
        let previous = self
            .entries
            .last()
            .map_or_else(|| GENESIS.to_string(), |e| e.digest.clone());
        let at = Timestamp::from_secs(1_790_000_000 + sequence as i64);
        let (decision, digest) = seal_v2(&previous, sequence, at, decision);
        let entry = JournalEntry {
            sequence,
            at,
            decision,
            digest,
            version: ChainVersion::V2,
        };
        self.entries.push(entry.clone());
        entry
    }

    fn record<B: serde::Serialize>(&self, topic: Topic, position: u64, body: &B) -> Record {
        let payload = serde_json::to_value(body).unwrap();
        let id = format!("{}-{position}-{topic}", self.cell);
        let at = Timestamp::from_secs(1_790_000_000 + position as i64);
        let event = AnyEvent {
            event_id: EventId::from_string(id.clone()),
            topic,
            schema_version: 1,
            occurred_at: at,
            recorded_at: at,
            sequence: 0,
            lineage: Lineage::root(
                CorrelationId::from_string("COR00000000000000000000009"),
                "qip-edge-node.outbox",
            ),
            idempotency_key: Some(id),
            payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
            payload,
        };
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()
    }

    /// A fill, as the P1 `OutcomeRecorded` record the writer emits.
    fn fill(&mut self, order: &str, quantity: u64, price: &str, simulated: bool) -> Record {
        let entry = self.seal(Decision::Filled {
            order_id: order.to_string(),
            venue: VENUE.to_string(),
            object: "OBJ-ACME".to_string(),
            quantity: quantity.to_string(),
            price: price.to_string(),
            simulated,
            shares: vec![("alpha".to_string(), quantity.to_string())],
            side: Some(BookSide::Bid),
            quote_unit: Some(UNIT.to_string()),
            fee: None,
        });
        let outcome = OutcomeRecord {
            cell: self.cell.clone(),
            session: 0,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry: entry.clone(),
        };
        self.record(Topic::ReflexOutcomeRecorded, entry.sequence, &outcome)
    }

    /// `count` non-outcome entries, as the `ChainSpan` that covers them.
    fn span(&mut self, count: u64) -> Record {
        let first = self.entries.len() as u64;
        for n in 0..count {
            self.seal(Decision::EdgePriced {
                opportunity: format!("opp-{}", first + n),
                net: "0.1".to_string(),
                positive: true,
            });
        }
        let last = self.entries.last().unwrap();
        let span = ChainSpan {
            cell: self.cell.clone(),
            session: 0,
            first_seq: first,
            last_seq: last.sequence,
            tail_digest: last.digest.clone(),
        };
        self.record(Topic::ReflexChainSpan, first, &span)
    }
}

// --- the ledger -----------------------------------------------------------------

fn ledger_config(dir: &std::path::Path, fabric: &Fabric) -> LedgerConfig {
    let variables: BTreeMap<String, String> = [
        (qip_ledgerd::config::ENVIRONMENT, "workstation".to_string()),
        (qip_ledgerd::config::DATA_DIR, dir.display().to_string()),
        (qip_ledgerd::config::LISTEN, "127.0.0.1:0".to_string()),
        (qip_ledgerd::config::CONSUMER_GROUP, GROUP.to_string()),
        (
            qip_ledgerd::config::FABRIC_URL,
            format!("http://{}", fabric.address),
        ),
        (qip_ledgerd::config::FABRIC_STREAM, STREAM.to_string()),
        (
            qip_ledgerd::config::FABRIC_PARTITIONS,
            PARTITIONS.to_string(),
        ),
        (qip_ledgerd::config::POLL_MS, "20".to_string()),
        // ADR 0112: the test owns both ends of a loopback socket, so
        // plaintext, which only a local deployment may choose.
        (qip_ledgerd::config::DEPLOYMENT, "local".to_string()),
        (
            qip_transport::tls::TLS_MODE_VARIABLE,
            "plaintext".to_string(),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    LedgerConfig::parse(&variables).unwrap()
}

fn start_ledger(dir: &std::path::Path, fabric: &Fabric) -> Running {
    start(
        &ledger_config(dir, fabric),
        Arc::new(SystemClock),
        Arc::new(ThreadSleeper),
        fabric.connect(),
    )
    .expect("the ledger starts against a running broker")
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(StdDuration::from_millis(20));
    }
}

fn http_get(address: &str, path: &str) -> (u16, String) {
    let response = HttpClient::new(ClientLimits::default())
        .get(&format!("http://{address}{path}"))
        .expect("the ledger's listener answers");
    (
        response.status,
        String::from_utf8_lossy(&response.body).to_string(),
    )
}

/// The balances of `fills` by the pure posting rule, never touching a store.
fn refold(fills: &[(&str, u64, &str)], cell: &str) -> BTreeMap<(Account, String), Decimal> {
    let mut journal = Journal::new(cell);
    let mut balances = BTreeMap::new();
    for (order, quantity, price) in fills {
        let _ = journal.fill(order, *quantity, price, true);
        let entry = journal.entries.last().unwrap().clone();
        let outcome = OutcomeRecord {
            cell: cell.to_string(),
            session: 0,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry,
        };
        let event: LedgerEvent = post(&PaperFill::try_from(&outcome).unwrap()).unwrap();
        for posting in event.postings() {
            let slot = balances
                .entry((posting.account.clone(), posting.unit.clone()))
                .or_insert(Decimal::ZERO);
            *slot = match posting.direction {
                Direction::Debit => *slot + posting.amount,
                Direction::Credit => *slot - posting.amount,
            };
        }
    }
    balances
}

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-ledgerd-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// LEDGER-8134 / LEDGER-5013 / FABRIC-8092: a ledger started by `start`
/// books what the stream holds — including a fill that is the *second*
/// record of a multi-record batch — answers it over its read API, reports
/// ready, commits its lag to the broker's group, and across a restart books
/// only what is new: every fill exactly once.
///
/// Mutation: make `consumer::delivery_offset` ignore the record index (every
/// record of a batch at one offset) — the span and the fill of batch 0 share
/// a delivery offset with different hashes, the store parks cell-a as an
/// integrity break, and the first wait times out. Mutation: let `step`
/// apply records below `position` (pass `0` as `from` to `decode_batch`) and
/// make the store's duplicate arm post again — the restart books o-1 and o-2
/// a second time and the "each fill once" assertion fails.
#[test]
fn a_started_ledger_books_every_fill_on_the_stream_once_across_a_restart() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let span = cell.span(2);
    let first = cell.fill("o-1", 10, "100.5", true);
    fabric.produce(0, vec![span, first]);
    let second = cell.fill("o-2", 4, "101", true);
    fabric.produce(0, vec![second]);

    let dir = temp_dir("restart");
    let ledger = start_ledger(&dir, &fabric);
    assert!(
        ledger.is_ready(),
        "ready once storage is proven and the port bound"
    );
    wait_until("both fills are booked", || {
        ledger.store().events().unwrap().len() == 2
    });
    assert_eq!(
        ledger.store().balances().unwrap(),
        refold(&[("o-1", 10, "100.5"), ("o-2", 4, "101")], "cell-a")
    );
    let (status, body) = http_get(ledger.address(), "/health");
    assert_eq!(status, 200, "{body}");
    let (status, body) = http_get(
        ledger.address(),
        &format!("/api/v1/ledger/balance?account=venue:{VENUE}&unit={UNIT}"),
    );
    assert_eq!(status, 200, "{body}");
    let expected_venue = refold(&[("o-1", 10, "100.5"), ("o-2", 4, "101")], "cell-a")
        .get(&(
            Account::Venue {
                venue: VENUE.to_string(),
            },
            UNIT.to_string(),
        ))
        .copied()
        .expect("premise: a fill moves the venue's cash");
    assert_eq!(body, expected_venue.to_string());
    wait_until("the group checkpoint reaches the last batch", || {
        fabric
            .service
            .broker()
            .committed_offset(GROUP, STREAM, 0)
            .unwrap()
            == Some(1)
    });
    ledger.stop();

    let third = cell.fill("o-3", 1, "99", true);
    fabric.produce(0, vec![third]);
    let ledger = start_ledger(&dir, &fabric);
    wait_until("the third fill is booked", || {
        ledger.store().events().unwrap().len() >= 3
    });
    // Give a doubling worker every chance to show itself.
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(ledger.store().events().unwrap().len(), 3, "each fill once");
    assert_eq!(
        ledger.store().balances().unwrap(),
        refold(
            &[("o-1", 10, "100.5"), ("o-2", 4, "101"), ("o-3", 1, "99")],
            "cell-a"
        )
    );
    assert!(ledger.store().parked().unwrap().is_empty());
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0106 through the composed path: a fill on the stream not marked
/// simulated is refused, counted, and parks its own cell, while a paper fill
/// from another cell on another partition is booked. Mutation: in
/// `LedgerStore::apply`, delete the `Some((Err(refusal), false))` live-fill
/// arm — the live fill is then refused as a malformed fill
/// (`RefusedFill`), and the park-reason assertion fails.
#[test]
fn a_live_fill_on_the_stream_parks_its_cell_and_another_cell_keeps_booking() {
    let mut fabric = Fabric::start();
    let mut live = Journal::new("cell-live");
    let mut paper = Journal::new("cell-paper");
    let live_fill = live.fill("o-live", 5, "100", false);
    let paper_fill = paper.fill("o-paper", 2, "50", true);
    fabric.produce(0, vec![live_fill]);
    fabric.produce(1, vec![paper_fill]);

    let dir = temp_dir("live");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("the paper fill is booked and the live cell parked", || {
        ledger.store().events().unwrap().len() == 1 && !ledger.store().parked().unwrap().is_empty()
    });
    let parked = ledger.store().parked().unwrap();
    assert_eq!(
        parked.get("cell-live").map(|p| p.reason),
        Some(ParkReason::LiveFill)
    );
    assert!(!parked.contains_key("cell-paper"));
    assert_eq!(
        ledger.store().balances().unwrap(),
        refold(&[("o-paper", 2, "50")], "cell-paper")
    );
    let refused: u64 = ledger
        .metrics()
        .snapshot()
        .series
        .iter()
        .filter(|s| s.name == names::LEDGER_LIVE_FILL_REFUSED)
        .map(|s| match &s.value {
            MetricValue::Counter(v) => *v,
            _ => 0,
        })
        .sum();
    assert!(refused >= 1, "the refusal is counted");
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
