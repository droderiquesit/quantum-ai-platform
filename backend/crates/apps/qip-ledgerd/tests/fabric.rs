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
const OBJECT: &str = "OBJ-ACME";
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
        self.record_under(
            topic,
            position,
            body,
            CorrelationId::from_string("COR00000000000000000000009"),
        )
    }

    fn record_under<B: serde::Serialize>(
        &self,
        topic: Topic,
        position: u64,
        body: &B,
        correlation: CorrelationId,
    ) -> Record {
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
            lineage: Lineage::root(correlation, "qip-edge-node.outbox"),
            idempotency_key: Some(id),
            payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
            payload,
        };
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()
    }

    /// A fill, as the P1 `OutcomeRecorded` record the writer emits.
    fn fill(&mut self, order: &str, quantity: u64, price: &str, simulated: bool) -> Record {
        self.fill_on(order, quantity, price, simulated, BookSide::Bid, None)
    }

    /// [`Self::fill`] on a chosen side, with the fee the venue reported, if
    /// it reported one.
    fn fill_on(
        &mut self,
        order: &str,
        quantity: u64,
        price: &str,
        simulated: bool,
        side: BookSide,
        fee: Option<&str>,
    ) -> Record {
        let entry = self.seal(Decision::Filled {
            order_id: order.to_string(),
            venue: VENUE.to_string(),
            object: OBJECT.to_string(),
            quantity: quantity.to_string(),
            price: price.to_string(),
            simulated,
            shares: vec![("alpha".to_string(), quantity.to_string())],
            side: Some(side),
            quote_unit: Some(UNIT.to_string()),
            fee: fee.map(str::to_string),
        });
        let outcome = OutcomeRecord {
            cell: self.cell.clone(),
            session: 0,
            journal_sequence: entry.sequence,
            journal_digest: entry.digest.clone(),
            entry: entry.clone(),
        };
        // The lineage names the order's chain, as the writer stamps it.
        let correlation = outcome.correlation().expect("a fill names its order");
        self.record_under(
            Topic::ReflexOutcomeRecorded,
            entry.sequence,
            &outcome,
            correlation,
        )
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

/// A record rewritten after the writer built it, as a producer that is newer
/// than the ledger, or a hand on the wire, would leave it.
fn rewritten(record: &Record, edit: impl FnOnce(&mut AnyEvent)) -> Record {
    let mut event = record.decode_payload(PayloadCodec::CanonicalJson).unwrap();
    edit(&mut event);
    Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()
}

/// R12-CONTRACT-7089 through the composed path: the ledger opens every
/// record as a `FabricEnvelope`, so a record of a schema version it does not
/// read, and a record whose payload no longer matches its hash, each stall
/// their partition with the reason on `/health` and book nothing from that
/// batch — not even the good fill that preceded the bad record in it —
/// while the other partition keeps booking.
///
/// Mutation: in `consumer::decode_batch`, never open the envelope (guard
/// the `open_decoded` branch with `false`) — the version-2 stall's reason no
/// longer names the fabric envelope, and the tampered fill is no longer
/// refused at all; the first assertion fails (verified).
#[test]
fn a_record_the_envelope_refuses_stalls_its_partition_and_books_nothing_from_its_batch() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let mut other = Journal::new("cell-b");
    let good = cell.fill("o-good", 3, "100", true);
    let newer = rewritten(&cell.fill("o-newer", 1, "100", true), |e| {
        e.schema_version = 2;
    });
    let fine = other.fill("o-fine", 2, "50", true);
    fabric.produce(0, vec![good, newer]);
    fabric.produce(1, vec![fine]);

    let dir = temp_dir("envelope-version");
    let ledger = start_ledger(&dir, &fabric);
    wait_until(
        "the other partition books and partition 0 reports why it stalls",
        || {
            ledger.store().events().unwrap().len() == 1
                && ledger
                    .statuses()
                    .get(&0)
                    .and_then(|s| s.last_error.clone())
                    .is_some()
        },
    );
    let reason = ledger.statuses()[&0].last_error.clone().unwrap();
    assert!(reason.contains("fabric envelope"), "{reason}");
    assert!(reason.contains("schema version 2"), "{reason}");
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(
        ledger.store().balances().unwrap(),
        refold(&[("o-fine", 2, "50")], "cell-b"),
        "nothing from the refused batch was booked"
    );
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);

    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let tampered = rewritten(&cell.fill("o-tampered", 9, "100", true), |e| {
        // The payload changes; the hash the producer sealed does not.
        e.payload["entry"]["decision"]["Filled"]["quantity"] = serde_json::json!("9000");
    });
    fabric.produce(0, vec![tampered]);
    let dir = temp_dir("envelope-tamper");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("partition 0 reports the tamper", || {
        ledger
            .statuses()
            .get(&0)
            .and_then(|s| s.last_error.clone())
            .is_some()
    });
    let reason = ledger.statuses()[&0].last_error.clone().unwrap();
    assert!(reason.contains("payload hash"), "{reason}");
    assert!(ledger.store().events().unwrap().is_empty());
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fill whose lineage names another order's chain (or none of the
/// derivable ones) is refused at the consumer: the partition stalls, the
/// reason names the lineage, and nothing from the batch is booked — not the
/// good fill ahead of it. A well-formed fill books, and the posted event
/// names the chain its lineage carried.
///
/// Mutation: in `consumer::decode_batch`, drop the `outcome.correlation()`
/// comparison — the edited fill books and the stall never reports.
#[test]
fn a_fill_whose_lineage_was_edited_stalls_its_partition_and_a_good_one_names_its_chain() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let good = cell.fill("o-good", 3, "100", true);
    let forged = rewritten(&cell.fill("o-forged", 1, "100", true), |e| {
        e.lineage = Lineage::root(
            CorrelationId::from_string("COR00000000000000000000009"),
            "qip-edge-node.outbox",
        );
    });
    fabric.produce(0, vec![good, forged]);
    let dir = temp_dir("lineage");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("partition 0 reports the lineage", || {
        ledger
            .statuses()
            .get(&0)
            .and_then(|s| s.last_error.clone())
            .is_some()
    });
    let reason = ledger.statuses()[&0].last_error.clone().unwrap();
    assert!(reason.contains("lineage correlation"), "{reason}");
    assert!(
        ledger.store().events().unwrap().is_empty(),
        "nothing from the refused batch was booked"
    );
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);

    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    fabric.produce(0, vec![cell.fill("o-ok", 3, "100", true)]);
    let dir = temp_dir("lineage-ok");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("the fill is booked", || {
        ledger.store().events().unwrap().len() == 1
    });
    let event = ledger.store().events().unwrap().remove(0);
    assert_eq!(
        event.chain_correlation(),
        qip_contracts::reflex::chain_correlation("cell-a", 0, "o-ok")
    );
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// One balance read back over the started ledger's read API.
fn balance_over_http(address: &str, account: &str, unit: &str) -> Decimal {
    let (status, body) = http_get(
        address,
        &format!("/api/v1/ledger/balance?account={account}&unit={unit}"),
    );
    assert_eq!(status, 200, "{body}");
    Decimal::parse(&body).unwrap_or_else(|| panic!("a balance is decimal text: {body:?}"))
}

fn dec(text: &str) -> Decimal {
    Decimal::parse(text).unwrap()
}

/// LEDGER-5003, LEDGER-5004, LEDGER-5007 and LEDGER-5015 through the
/// composed path: two paper fills on the stream — a purchase whose venue
/// reported a fee, and one whose venue reported none — are booked by the
/// started ledger, and what its read API answers is the ledger's own state:
///
/// - **multi-asset (5003):** the trading book holds the instrument and the
///   cash as two balances in two units, each readable by unit, and the
///   trial balance closes to zero in *each* unit separately;
/// - **authoritative for cash and positions (5004):** the position and the
///   cash a desk would ask for are read from the ledger, after the fills;
/// - **venue-reported fees (5007):** the fees account holds exactly the
///   reported fee, and the fill with no report books none — not zero
///   estimated, nothing;
/// - **trading subledger (5015):** the trade's legs land on
///   `trading:<cell>/<strategy>` against `venue:<venue>`, and roll up into a
///   global trial balance that still closes.
///
/// Mutation: in `PaperFill::try_from`, book an unreported fee as a
/// reported 0.25 — the trading cash reads -1409.5 and the cash assertion
/// fails. Mutation: in `qip_portfolio::ledger::post`, drop the fee legs —
/// `LedgerEvent::new` refuses a reported fee with no fee postings, nothing
/// is booked, and the first wait times out. Mutation: key the
/// store's balances by account only (`balance_key` ignores the unit) — the
/// instrument and cash legs net into one number and the per-unit position
/// assertion fails. Mutation: post the instrument leg of a purchase to the
/// venue on both sides (route it out of the trading subledger) — the
/// position reads zero and the subledger assertion fails.
#[test]
fn fills_on_the_stream_book_per_unit_positions_cash_and_reported_fees_that_close_in_every_unit() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let charged = cell.fill_on("o-fee", 10, "100.5", true, BookSide::Ask, Some("0.25"));
    let free = cell.fill_on("o-nofee", 4, "101", true, BookSide::Ask, None);
    fabric.produce(0, vec![charged, free]);

    let dir = temp_dir("units");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("both fills are booked", || {
        ledger.store().events().unwrap().len() == 2
    });
    let address = ledger.address().to_string();
    let trading = "trading:cell-a/alpha";
    let venue = format!("venue:{VENUE}");
    let fees = format!("fees:{VENUE}");

    // Bought 10 + 4 of the instrument: the position, in the instrument's
    // own unit, held by the trading book against the venue.
    assert_eq!(balance_over_http(&address, trading, OBJECT), dec("14"));
    assert_eq!(balance_over_http(&address, &venue, OBJECT), dec("-14"));
    // Paid 10 x 100.5 + 4 x 101 = 1409 to the venue, and the one reported
    // fee of 0.25 to the fees subledger: the cash, in the quote unit.
    assert_eq!(balance_over_http(&address, trading, UNIT), dec("-1409.25"));
    assert_eq!(balance_over_http(&address, &venue, UNIT), dec("1409"));
    assert_eq!(
        balance_over_http(&address, &fees, UNIT),
        dec("0.25"),
        "exactly the reported fee, and nothing for the fill that reported none"
    );
    let events = ledger.store().events().unwrap();
    let unreported = events
        .iter()
        .find(|event| event.source().order_id == "o-nofee")
        .expect("premise: the unreported fill was booked");
    assert!(
        unreported
            .postings()
            .iter()
            .all(|posting| !matches!(posting.account, Account::Fees { .. })),
        "a fill with no reported fee posts nothing to fees"
    );

    let (status, body) = http_get(&address, "/api/v1/ledger/trial-balance");
    assert_eq!(status, 200, "{body}");
    let totals: BTreeMap<String, String> = serde_json::from_str(&body).unwrap();
    assert_eq!(
        totals.keys().cloned().collect::<Vec<_>>(),
        vec![OBJECT.to_string(), UNIT.to_string()],
        "premise: two units moved"
    );
    for (unit, total) in &totals {
        assert_eq!(
            dec(total),
            Decimal::ZERO,
            "the trial balance closes in {unit}"
        );
    }
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
