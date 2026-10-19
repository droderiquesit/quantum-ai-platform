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
        let handled = self.0.handle(
            request.header(auth::HEADER),
            None,
            &request.path,
            &request.body,
        );
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
        Self::start_with(None)
    }

    /// A broker that reads retained-away segments back from `archive` and
    /// rolls a segment per batch, so every batch but the last is sealed and
    /// can be archived and retained away.
    fn start_archived(archive: Arc<qip_storage::MemoryBlobStore>) -> Self {
        Self::start_with(Some(archive))
    }

    fn start_with(archive: Option<Arc<qip_storage::MemoryBlobStore>>) -> Self {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "qip-ledgerd-fabric-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The archived broker declares the outcomes stream as production
        // does (`infrastructure/event-fabric/streams.local.json`):
        // irreplaceable, so its sealed segments are archived.
        let retention = if archive.is_some() {
            "irreplaceable"
        } else {
            "derived_state"
        };
        let catalogue = format!(
            r#"{{"streams":[{{"name":"{STREAM}","qos_class":"p1_outcomes","partition_key":"cell",
                "ordering":"per_partition","retention":"{retention}","replication_factor":1,
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
        let roll_every_batch = archive.is_some();
        let mut broker = Broker::open(root.join("broker"), clock.clone()).unwrap();
        if let Some(store) = archive {
            broker = broker.with_archive_reader(store);
        }
        let broker = Arc::new(broker);
        let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
            let config = SegmentLogConfig::new(clock.clone())
                .with_archive_required(policy.archive_required());
            if roll_every_batch {
                config.with_roll_after_bytes(1)
            } else {
                config
            }
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

/// The observations `name` holds as a histogram, summed over its series.
fn histogram_count(ledger: &Running, name: &str) -> u64 {
    ledger
        .metrics()
        .snapshot()
        .series
        .iter()
        .filter(|s| s.name == name)
        .map(|s| match &s.value {
            MetricValue::Histogram(h) => h.count,
            _ => 0,
        })
        .sum()
}

/// OBS-9149: the ledger-commit latency series moves because a delivery was
/// committed through the consumer, once per committed fill — not because a
/// test called the recorder. The only other test of the series calls
/// `commit_latency_ms` directly, so deleting the recording site in
/// `PartitionWorker::apply_until_committed` passed it.
///
/// Mutation: delete `self.telemetry.commit_latency_ms(elapsed)` in
/// `consumer.rs` — the series is never written, the count is zero, and the
/// equality fails.
#[test]
fn every_fill_the_consumer_commits_is_observed_once_on_the_ledger_commit_latency_series() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let first = cell.fill("o-1", 3, "100", true);
    fabric.produce(0, vec![first]);
    let second = cell.fill("o-2", 2, "101", true);
    fabric.produce(0, vec![second]);

    let dir = temp_dir("latency");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("both fills are booked", || {
        ledger.store().events().unwrap().len() == 2
    });
    assert_eq!(
        histogram_count(&ledger, names::LEDGER_COMMIT_LATENCY_MS),
        2,
        "one observation per fill the store committed"
    );
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// FABRIC-10078: after a restart the broker's group checkpoint and the
/// store's cursor name the same batch. The store's cursor is what resumes
/// (the group offset is advisory), so a group that stopped following it
/// would report a lag the ledger does not have — and a warm consumer that
/// trusted it would replay or skip.
///
/// Mutation: delete the `self.consumer.commit(batch.base_offset)` call in
/// `PartitionWorker::step` — the group checkpoint never reaches the batch
/// the store resumed past, and the wait times out.
#[test]
fn after_a_restart_the_broker_group_checkpoint_names_the_batch_the_store_cursor_resumes_past() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let first = cell.fill("o-1", 10, "100.5", true);
    fabric.produce(0, vec![first]);
    let dir = temp_dir("checkpoint");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("the first fill is booked", || {
        ledger.store().events().unwrap().len() == 1
    });
    ledger.stop();

    let second = cell.fill("o-2", 4, "101", true);
    let third = cell.fill("o-3", 1, "99", true);
    fabric.produce(0, vec![second]);
    fabric.produce(0, vec![third]);
    let ledger = start_ledger(&dir, &fabric);
    wait_until("all three fills are booked", || {
        ledger.store().events().unwrap().len() == 3
    });
    let label = qip_ledgerd::consumer::partition_label(STREAM, 0);
    let next = ledger.store().resume_offset(&label).unwrap();
    assert!(next > 0, "premise: the store's cursor moved");
    let resumed_past = qip_ledgerd::consumer::batch_of(next - 1);
    assert_eq!(resumed_past, 2, "premise: three batches, the last at 2");
    wait_until("the group checkpoint names the store's batch", || {
        fabric
            .service
            .broker()
            .committed_offset(GROUP, STREAM, 0)
            .unwrap()
            == Some(resumed_past)
    });
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A local backend whose commits can be refused, as a lost or unwritable
/// disk refuses them. Reads keep answering so the test can see the cursor.
#[derive(Debug)]
struct Refusable {
    inner: qip_ledgerd::backend::LocalBackend,
    refusing: Arc<std::sync::atomic::AtomicBool>,
}

impl qip_ledgerd::backend::LedgerBackend for Refusable {
    fn get(&self, key: &str) -> qip_core::error::Result<Option<serde_json::Value>> {
        self.inner.get(key)
    }

    fn scan_prefix(
        &self,
        prefix: &str,
    ) -> qip_core::error::Result<Vec<(String, serde_json::Value)>> {
        self.inner.scan_prefix(prefix)
    }

    fn commit(&self, batch: qip_ledgerd::backend::LedgerBatch) -> qip_core::error::Result<()> {
        if self.refusing.load(Ordering::SeqCst) {
            return Err(qip_core::error::Error::io(
                "the ledger's store refuses writes (fixture)",
            ));
        }
        self.inner.commit(batch)
    }

    fn kind(&self) -> &'static str {
        "refusable"
    }
}

/// One partition worker over `store`, run on its own thread until `stop`.
fn run_worker(
    fabric: &Fabric,
    store: Arc<qip_ledgerd::store::LedgerStore>,
    metrics: Arc<qip_observability::metrics::Metrics>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    let worker = qip_ledgerd::consumer::PartitionWorker::new(
        qip_ledgerd::consumer::WorkerConfig {
            stream: STREAM.to_string(),
            partition: 0,
            group: GROUP.to_string(),
            poll: qip_core::Duration::from_millis(20),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
        },
        (fabric.connect())().unwrap(),
        store,
        qip_ledgerd::telemetry::LedgerTelemetry::new(metrics),
        Arc::default(),
        stop,
    )
    .unwrap();
    std::thread::spawn(move || worker.run())
}

/// RES-10128: the writer replays idempotently when its store is restored,
/// not only when its process restarts. While the store refuses writes under
/// a running worker nothing is booked and neither the store's cursor nor the
/// group checkpoint moves past a record it did not apply; once writes are
/// accepted again the same records are booked once. And a store lost
/// outright — a fresh, empty one in its place — rebuilds from the retained
/// stream to the same balances, because the store's cursor, not the group's,
/// is what resumes.
///
/// Mutation: in `PartitionWorker::apply_until_committed`, answer a store
/// error with `Ok(Disposition::Advanced)` rather than retrying — the worker
/// skips the refused fills, the group checkpoint moves past them, and the
/// "group checkpoint holds" assertion fails.
#[test]
fn a_store_that_refuses_writes_and_then_recovers_or_is_lost_replays_to_a_clean_delivery() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let first = cell.fill("o-1", 10, "100.5", true);
    fabric.produce(0, vec![first]);
    let second = cell.fill("o-2", 4, "101", true);
    fabric.produce(0, vec![second]);
    let clean = refold(&[("o-1", 10, "100.5"), ("o-2", 4, "101")], "cell-a");
    let label = qip_ledgerd::consumer::partition_label(STREAM, 0);

    let dir = temp_dir("refusing");
    std::fs::create_dir_all(&dir).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let refusing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let metrics = Arc::new(qip_observability::metrics::Metrics::new("qip-ledgerd"));
    let store = Arc::new(
        qip_ledgerd::store::LedgerStore::with_backend(
            Box::new(Refusable {
                inner: qip_ledgerd::backend::LocalBackend::open(
                    &dir,
                    qip_storage::EngineConfig::new(clock.clone()),
                )
                .unwrap(),
                refusing: refusing.clone(),
            }),
            qip_ledgerd::telemetry::LedgerTelemetry::new(metrics.clone()),
        )
        .unwrap(),
    );
    refusing.store(true, Ordering::SeqCst);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = run_worker(&fabric, store.clone(), metrics.clone(), stop.clone());

    let retries = || {
        metrics
            .snapshot()
            .series
            .iter()
            .filter(|s| s.name == names::LEDGER_STORE_RETRIES)
            .map(|s| match &s.value {
                MetricValue::Counter(v) => *v,
                _ => 0,
            })
            .sum::<u64>()
    };
    wait_until("the worker has been refused at least twice", || {
        retries() >= 2
    });
    assert!(store.events().unwrap().is_empty(), "nothing booked");
    assert_eq!(store.resume_offset(&label).unwrap(), 0, "the cursor holds");
    assert_eq!(
        fabric
            .service
            .broker()
            .committed_offset(GROUP, STREAM, 0)
            .unwrap(),
        None,
        "the group checkpoint holds"
    );

    refusing.store(false, Ordering::SeqCst);
    wait_until("both fills are booked once writes are accepted", || {
        store.events().unwrap().len() == 2
    });
    std::thread::sleep(StdDuration::from_millis(300));
    assert_eq!(store.events().unwrap().len(), 2, "each fill once");
    assert_eq!(store.balances().unwrap(), clean);
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();

    // The store is lost: a fresh one, at a fresh directory, under the same
    // consumer group whose checkpoint is past every fill.
    let lost = temp_dir("lost");
    std::fs::create_dir_all(&lost).unwrap();
    let fresh = Arc::new(
        qip_ledgerd::store::LedgerStore::open(
            &lost,
            qip_storage::EngineConfig::new(clock),
            qip_ledgerd::telemetry::LedgerTelemetry::new(metrics.clone()),
        )
        .unwrap(),
    );
    assert_eq!(fresh.resume_offset(&label).unwrap(), 0, "premise: empty");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = run_worker(&fabric, fresh.clone(), metrics, stop.clone());
    wait_until("the fresh store rebuilds both fills", || {
        fresh.events().unwrap().len() == 2
    });
    assert_eq!(fresh.balances().unwrap(), clean);
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&lost);
}

/// OBS-7126: the ledger evaluates its own commit objective. The only
/// production evaluator was the deep brain's, against a registry that never
/// holds the ledger's commit latency, so `ledger-commit` read unmeasured
/// wherever it was evaluated. A ledger that commits fills now reports the
/// objective measured over exactly the commits it observed.
///
/// Mutation: delete `self.telemetry.evaluate_objectives()` in
/// `PartitionWorker::step` — no status is recorded and the wait for the
/// `ledger-commit` observations gauge times out. Mutation: drop the
/// `service == "ledger"` filter in `LedgerTelemetry::evaluate_objectives` —
/// `decision-latency` is written here as unmeasured and the last assertion
/// fails.
#[test]
fn a_ledger_that_commits_fills_reports_its_commit_objective_measured() {
    let mut fabric = Fabric::start();
    let mut cell = Journal::new("cell-a");
    let first = cell.fill("o-1", 3, "100", true);
    fabric.produce(0, vec![first]);
    let second = cell.fill("o-2", 2, "101", true);
    fabric.produce(0, vec![second]);

    let dir = temp_dir("objective");
    let ledger = start_ledger(&dir, &fabric);
    wait_until("both fills are booked", || {
        ledger.store().events().unwrap().len() == 2
    });
    let gauge = |name: &str| -> Option<f64> {
        ledger
            .metrics()
            .snapshot()
            .series
            .iter()
            .find(|s| {
                s.name == name
                    && s.labels.get("objective").map(String::as_str) == Some("ledger-commit")
            })
            .map(|s| match &s.value {
                MetricValue::Gauge(v) => *v,
                _ => f64::NAN,
            })
    };
    wait_until("the objective has seen both commits", || {
        gauge(names::SLO_OBSERVATIONS) == Some(2.0)
    });
    let consumed = gauge(names::SLO_BUDGET_CONSUMED).expect("a measured objective");
    assert!(consumed.is_finite() && consumed >= 0.0, "{consumed}");
    // Only the ledger's own objectives: a path another process measures
    // is never written here as unmeasured.
    let others = ledger
        .metrics()
        .snapshot()
        .series
        .iter()
        .filter(|s| {
            s.name == names::SLO_OBSERVATIONS
                && s.labels.get("objective").map(String::as_str) == Some("decision-latency")
        })
        .count();
    assert_eq!(others, 0);
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// RES-9175: a derived store is rehydrated from immutable objects. Once the
/// broker has archived the sealed segments of the outcomes stream and
/// retained them away, a ledger started on an empty directory books the
/// whole history — every batch but the hot one served from the archive — to
/// exactly the balances the original store holds.
///
/// It also holds the worker's handling of a group that is ahead of its
/// store: before the fix this test exposed, a rebuilding ledger committed
/// each replayed batch backwards, the broker refused the rewind, and the
/// consumer's circuit opened on every batch — 24 batches took 53 seconds.
/// The group's checkpoint is never moved back.
///
/// Mutation: in `ArchiveReader`-backed `Broker::read_batch`, answer
/// `Ok(None)` for a retained-away offset instead of hydrating — the fresh
/// ledger books nothing and the wait times out. Mutation: never set
/// `commit_deferred` in `PartitionWorker::commit_group` — every replayed
/// batch is refused, the rebuild takes about 53 seconds, and the bound
/// assertion fails.
#[test]
fn a_ledger_started_on_an_empty_store_rebuilds_the_original_balances_from_archived_segments() {
    let archive = Arc::new(qip_storage::MemoryBlobStore::new());
    let mut fabric = Fabric::start_archived(archive.clone());
    let mut cell = Journal::new("cell-a");
    // Enough one-fill batches that the log's minimum segment size seals
    // several segments.
    let orders: Vec<String> = (1..=24).map(|n| format!("o-{n}")).collect();
    let prices = ["100.5", "101", "99", "98.25"];
    let fills: Vec<(&str, u64, &str)> = orders
        .iter()
        .enumerate()
        .map(|(n, order)| (order.as_str(), 1 + n as u64 % 5, prices[n % prices.len()]))
        .collect();
    for (order, quantity, price) in &fills {
        let fill = cell.fill(order, *quantity, price, true);
        fabric.produce(0, vec![fill]);
    }

    let original_dir = temp_dir("original");
    let original = start_ledger(&original_dir, &fabric);
    wait_until("the original store books every fill", || {
        original.store().events().unwrap().len() == fills.len()
    });
    let original_balances = original.store().balances().unwrap();
    original.stop();
    assert_eq!(original_balances, refold(&fills, "cell-a"), "premise");

    let broker = fabric.service.broker();
    let archived = broker
        .archive_sealed(
            &qip_storage::segment::archive::Archiver::new(archive.clone()).with_manifest_index(),
        )
        .unwrap();
    assert!(
        archived >= 2,
        "premise: the sealed segments archived, got {archived}"
    );
    let deleted = broker.retain_hot(1).unwrap();
    assert!(
        deleted >= 2,
        "premise: local history retained away, got {deleted}"
    );

    let rebuilt_dir = temp_dir("rehydrated");
    let rebuilt = start_ledger(&rebuilt_dir, &fabric);
    // The group already committed past every batch this store replays. A
    // worker that commits each one backwards is refused, and the transport's
    // circuit opens on every refusal: 24 batches took 53 seconds. Caught-up
    // deferral takes one refusal, so the rebuild is bounded well inside the
    // wait.
    let started = Instant::now();
    wait_until("the fresh store books every fill", || {
        rebuilt.store().events().unwrap().len() == fills.len()
    });
    let took = started.elapsed();
    assert!(
        took < StdDuration::from_secs(20),
        "the rebuild was throttled by refused commits: {took:?}"
    );
    wait_until("the caught-up worker commits the last batch", || {
        rebuilt
            .statuses()
            .get(&0)
            .is_some_and(|s| s.last_error.is_none())
    });
    assert_eq!(
        fabric
            .service
            .broker()
            .committed_offset(GROUP, STREAM, 0)
            .unwrap(),
        Some(fills.len() as u64 - 1),
        "the group checkpoint was not moved back"
    );
    assert_eq!(rebuilt.store().balances().unwrap(), original_balances);
    assert!(rebuilt.store().parked().unwrap().is_empty());
    rebuilt.stop();
    let _ = std::fs::remove_dir_all(&original_dir);
    let _ = std::fs::remove_dir_all(&rebuilt_dir);
}

/// OBS-9162: the ledger declares and measures its other three objectives —
/// outcome-to-ledger lag, reconciliation mismatch age and transaction abort
/// rate — beside the commit objective. Each is backed by a series the
/// running worker records, and each reads measured once the series has
/// moved: the first two are readings judged against a ceiling the catalogue
/// declares, the third is the share of commits that were not refused or in
/// conflict.
///
/// The stream carries one booked fill and one live fill, which the ledger
/// refuses and parks its cell for, so the abort rate has a failure to count
/// and the mismatch age a park to age.
///
/// Mutation: delete the `self.check(names::LEDGER_LAG_CHECKS, ..)` call in
/// `LedgerTelemetry::lag` — `ledger-lag` never has an observation and the
/// wait times out. Mutation: empty the `bad` list of the abort objective in
/// `critical_path_slos` — the refusal counts as good and the budget-consumed
/// assertion fails. Mutation: in `LedgerStore::record_parked_keys`, skip
/// inserting into `parked_since` — the age stays zero and the
/// `> 0.0` assertion fails.
#[test]
fn a_ledger_with_a_booked_fill_and_a_parked_cell_measures_lag_mismatch_age_and_abort_rate() {
    let mut fabric = Fabric::start();
    let mut booked = Journal::new("cell-a");
    let good = booked.fill("o-1", 3, "100", true);
    fabric.produce(0, vec![good]);
    let mut stray = Journal::new("cell-b");
    let live = stray.fill("o-2", 2, "101", false);
    fabric.produce(0, vec![live]);

    let dir = temp_dir("objectives");
    let ledger = start_ledger(&dir, &fabric);
    wait_until(
        "the live fill's cell is parked and the good fill booked",
        || {
            ledger.store().parked().unwrap().len() == 1
                && ledger.store().events().unwrap().len() == 1
        },
    );
    let gauge = |name: &str, objective: Option<&str>| -> Option<f64> {
        ledger
            .metrics()
            .snapshot()
            .series
            .iter()
            .find(|s| {
                s.name == name
                    && s.labels.get("objective").map(String::as_str) == objective
                    && (objective.is_some() || s.labels.is_empty())
            })
            .map(|s| match &s.value {
                MetricValue::Gauge(v) => *v,
                _ => f64::NAN,
            })
    };
    for objective in ["ledger-lag", "ledger-mismatch-age", "ledger-abort-rate"] {
        wait_until(&format!("{objective} has an observation"), || {
            gauge(names::SLO_OBSERVATIONS, Some(objective)).is_some_and(|n| n >= 1.0)
        });
    }
    wait_until("the abort objective has seen both commits", || {
        gauge(names::SLO_OBSERVATIONS, Some("ledger-abort-rate")) == Some(2.0)
    });
    let consumed = gauge(names::SLO_BUDGET_CONSUMED, Some("ledger-abort-rate"))
        .expect("a measured objective records its budget");
    assert!(
        consumed > 1.0,
        "one refusal in two commits spends far more than the budget a 99.9% target allows: \
         {consumed}"
    );
    let age = gauge(names::LEDGER_MISMATCH_AGE_SECONDS, None).expect("the age gauge is recorded");
    assert!(age > 0.0, "a parked cell has a positive age: {age}");
    ledger.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
