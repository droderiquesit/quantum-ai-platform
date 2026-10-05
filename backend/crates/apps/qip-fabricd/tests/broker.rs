//! `qip-fabricd` driven the way a deployment would drive it: the real
//! composition (`qip_fabricd::start_with_archive`, which `main` calls through
//! `start`), two real listeners on loopback, and the client SDK's production
//! transport, producer and consumer on the other end of a real socket.
//!
//! Before this file the SDK had only ever spoken to scripted transports and
//! the broker library had only ever been called directly. Each was proven
//! and they did not fit. Joining them found five disagreements, each of
//! which is fixed in the libraries and held here by a test whose mutation
//! restores the old behaviour and fails:
//!
//! * a producer sequence the broker counts per record and the SDK counted
//!   per batch, so the batch after any multi-record batch was refused;
//! * a consumer offset the broker counts per batch and the SDK counted per
//!   record, so the batches after a multi-record batch were skipped;
//! * a producer's sequence, which the broker carries across epochs and the
//!   SDK reset to zero, so a restarted producer was refused for good;
//! * a cell identity the grant schema writes `reflex:<cell>`, which the
//!   identities file refused to load;
//! * `archived_through`, an exclusive bound the SDK's quorum wait compared
//!   with `>=`, so a P0 or P1 batch was reported safe one segment early —
//!   and with nothing sealing a quiet partition's segment by age, safe was
//!   never reached honestly at all.
//!
//! Every existing suite passed throughout, because every one of them used
//! one-record batches at offsets where the two counts agree.
//!
//! Nothing here places, amends or cancels an order. The fabric stores and
//! serves batches of bytes other processes wrote.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration as StdDuration, Instant};

use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Duration, SystemClock};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_events::event_fabric::schema_id::Shape;
use qip_fabricd::config::Config;
use qip_fabricd::{Running, start_with_archive};
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_streaming::event_fabric::partition::partition_for;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig, SubscriptionEvent};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{
    AdminIsolateRequest, AdminReleaseRequest, FetchRequest, MetadataRequest, ProduceRequest,
    ProducerInitRequest, Refusal, Request, Response,
};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::http::{ClientLimits, HttpClient};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

// --- the catalogue this suite serves ------------------------------------------

const CONTROL: &str = "control.test";
const OUTCOMES: &str = "outcomes.test";
const JOURNAL: &str = "journal.test";
const RESEARCH: &str = "research.test";
const TELEMETRY: &str = "telemetry.test";

const CELL_A: &str = "reflex:cell-a";
// A cell whose key routes to a different partition than `cell-a`'s under four
// partitions; the isolation test asserts that premise before relying on it.
const CELL_B: &str = "reflex:cell-d";
const CONTROLLER: &str = "release-controller";
const AUDITOR: &str = "auditor";
const OPERATOR: &str = "operator";
const RESEARCHER: &str = "researcher";

const IDENTITIES: [&str; 6] = [CELL_A, CELL_B, CONTROLLER, AUDITOR, OPERATOR, RESEARCHER];

const PARTITIONS: u32 = 4;

/// How long something that should happen may take on a loaded machine. Only
/// ever the bound on a wait for a condition that is expected to come.
const EVENTUALLY: StdDuration = StdDuration::from_secs(60);

fn stream_json(name: &str, class: &str, retention: &str, overload: &str, ack: &str) -> String {
    format!(
        r#"{{"name":"{name}","qos_class":"{class}","partition_key":"cell",
            "ordering":"per_partition","retention":"{retention}","replication_factor":1,
            "mirroring":"none","overload_policy":"{overload}","ack_profile":"{ack}",
            "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
            "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}"#
    )
}

/// The grants every test starts from, as `(identity, stream, permission,
/// key scope)`. The researcher holds research and nothing else, which is the
/// premise FABRIC-018's check starts from.
fn base_grants() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    let mut grants = vec![
        (CELL_A, JOURNAL, "produce", "own_key"),
        (CELL_A, OUTCOMES, "produce", "own_key"),
        (CELL_A, CONTROL, "consume", "own_key"),
        (CELL_B, JOURNAL, "produce", "own_key"),
        (CONTROLLER, CONTROL, "produce", "any"),
        (RESEARCHER, RESEARCH, "produce", "any"),
        (RESEARCHER, RESEARCH, "consume", "any"),
    ];
    for stream in [CONTROL, OUTCOMES, JOURNAL, RESEARCH, TELEMETRY] {
        grants.push((AUDITOR, stream, "consume", "any"));
        grants.push((OPERATOR, stream, "admin", "any"));
    }
    grants
}

fn catalogue_json(grants: &[(&str, &str, &str, &str)], journal_ack: &str) -> String {
    let streams = [
        stream_json(
            CONTROL,
            "p0_control",
            "irreplaceable",
            "refuse_producer",
            "quorum",
        ),
        stream_json(
            OUTCOMES,
            "p1_outcomes",
            "irreplaceable",
            "refuse_producer",
            "quorum",
        ),
        stream_json(
            JOURNAL,
            "p2_market_journal",
            "event_anchored",
            "throttle_with_gap",
            journal_ack,
        ),
        stream_json(
            RESEARCH,
            "p3_research",
            "episodic",
            "allow_backlog",
            "leader_only",
        ),
        stream_json(
            TELEMETRY,
            "p4_telemetry",
            "derived_state",
            "sample_or_shed",
            "none",
        ),
    ]
    .join(",");
    let grants = grants
        .iter()
        .map(|(identity, stream, permission, scope)| {
            format!(
                r#"{{"identity":"{identity}","stream":"{stream}","permission":"{permission}","key_scope":"{scope}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"streams":[{streams}],"grants":[{grants}]}}"#)
}

/// A token for `identity` at the broker numbered `fabric`: long enough and
/// inside the bearer alphabet. A fixture string in a test, not a credential
/// for anything.
///
/// Different for every broker this suite starts, because the tests run in
/// parallel on kernel-assigned ports: a port one test's broker released can
/// be the port another's is given a moment later, and a client still holding
/// the old address must be refused there rather than answered by a stranger.
fn token(fabric: u64, identity: &str) -> String {
    format!(
        "fixture-token-{fabric}-for-{}-000000000000000000000000",
        identity.replace(':', "-")
    )
}

// --- the fixture ----------------------------------------------------------------

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

/// A blob store whose `put` can be made to block, standing in for an object
/// store that has stopped answering.
#[derive(Debug)]
struct GatedStore {
    inner: MemoryBlobStore,
    open: Mutex<bool>,
    changed: Condvar,
    /// Puts that have arrived, whether or not they have been let through.
    arrived: AtomicU64,
}

impl GatedStore {
    fn new() -> Self {
        Self {
            inner: MemoryBlobStore::new(),
            open: Mutex::new(true),
            changed: Condvar::new(),
            arrived: AtomicU64::new(0),
        }
    }

    fn set_open(&self, open: bool) {
        *self.open.lock().unwrap() = open;
        self.changed.notify_all();
    }
}

impl BlobStore for GatedStore {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.arrived.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
        drop(open);
        self.inner.put(key, bytes)
    }
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.get(key)
    }
    fn delete(&self, key: &str) -> Result<bool> {
        self.inner.delete(key)
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix)
    }
}

struct Fabric {
    number: u64,
    root: PathBuf,
    config: Config,
    archive: Arc<GatedStore>,
    running: Option<Running>,
}

impl Fabric {
    fn start(label: &str) -> Self {
        Self::start_with(label, &catalogue_json(&base_grants(), "leader_only"))
            .expect("the suite's own catalogue starts a broker")
    }

    fn start_with(label: &str, catalogue: &str) -> Result<Self> {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "qip-fabricd-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("streams.json"), catalogue).unwrap();
        let identities = IDENTITIES
            .iter()
            .map(|identity| {
                format!(
                    "{identity} {}\n",
                    qip_core::hash::sha256_hex(token(unique, identity).as_bytes())
                )
            })
            .collect::<String>();
        std::fs::write(root.join("identities"), identities).unwrap();

        let variables: BTreeMap<String, String> = [
            (qip_fabricd::config::DATA_DIR, root.join("data")),
            (qip_fabricd::config::CATALOGUE, root.join("streams.json")),
            (
                qip_fabricd::config::IDENTITIES_FILE,
                root.join("identities"),
            ),
            (qip_fabricd::config::ARCHIVE_DIR, root.join("archive")),
        ]
        .into_iter()
        .map(|(name, path)| (name.to_string(), path.display().to_string()))
        .chain(
            [
                (qip_fabricd::config::LISTEN, "127.0.0.1:0"),
                (qip_fabricd::config::HEALTH_LISTEN, "127.0.0.1:0"),
                (qip_fabricd::config::PARTITIONS, "4"),
                // The smallest segment the log allows, so a handful of
                // batches seals one.
                (qip_fabricd::config::SEGMENT_BYTES, "4096"),
                (qip_fabricd::config::HOUSEKEEPING_MS, "20"),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string())),
        )
        .collect();
        let archive = Arc::new(GatedStore::new());
        let started = Config::parse(&variables).and_then(|config| {
            let running =
                start_with_archive(&config, Arc::new(SystemClock), Some(archive.clone()))?;
            Ok((config, running))
        });
        let (config, running) = match started {
            Ok(started) => started,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&root);
                return Err(error);
            }
        };
        Ok(Self {
            number: unique,
            root,
            config,
            archive,
            running: Some(running),
        })
    }

    fn running(&self) -> &Running {
        self.running.as_ref().expect("the broker is running")
    }

    /// Stop the process's threads and start it again on the same
    /// directories: a new leader epoch over the same partitions.
    fn restart(&mut self) {
        self.running.take().expect("running").stop();
        self.running = Some(
            start_with_archive(
                &self.config,
                Arc::new(SystemClock),
                Some(self.archive.clone()),
            )
            .expect("a broker restarts on the directories it just released"),
        );
    }

    fn transport(&self, identity: &str) -> HttpTransport {
        HttpTransport::new(
            format!("http://{}", self.running().address()),
            BearerToken::new(token(self.number, identity)).unwrap(),
        )
    }

    fn call(&self, identity: &str, request: Request) -> Result<Response> {
        self.transport(identity).call(request, timeouts())
    }

    fn producer(&self, identity: &str, stream: &str, partition: u32, class: QosClass) -> Producer {
        Producer::new(ProducerConfig {
            transport: Box::new(self.transport(identity)),
            stream: stream.to_string(),
            partition,
            producer_id: identity.to_string(),
            qos_class: class,
            ack_profile: class.ack_floor(),
            retry_policy: patient(),
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 7,
            breaker_seed: 7,
            timeouts: timeouts(),
        })
        .unwrap()
    }

    fn consumer(&self, identity: &str, stream: &str, partition: u32, group: &str) -> Consumer {
        self.consumer_with_credit(identity, stream, partition, group, 1024 * 1024)
    }

    /// A consumer whose every fetch carries `credit` bytes of credit. One
    /// byte of credit is answered with exactly one batch — the broker always
    /// serves at least one — so the consumer has to ask for every offset
    /// itself instead of being handed several batches in one answer.
    fn consumer_with_credit(
        &self,
        identity: &str,
        stream: &str,
        partition: u32,
        group: &str,
        credit: u32,
    ) -> Consumer {
        Consumer::new(ConsumerConfig {
            transport: Box::new(self.transport(identity)),
            stream: stream.to_string(),
            partition,
            group_id: group.to_string(),
            retry_policy: RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 9,
            breaker_seed: 9,
            timeouts: timeouts(),
            fetch_credit_bytes: credit,
        })
        .unwrap()
    }

    fn health(&self, path: &str) -> (u16, String) {
        let response = HttpClient::new(ClientLimits::default())
            .get(&format!("http://{}{path}", self.running().health_address()))
            .expect("the health listener answers");
        (
            response.status,
            String::from_utf8_lossy(&response.body).to_string(),
        )
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        // Let a stalled archive go, or the housekeeping thread never joins.
        self.archive.set_open(true);
        if let Some(running) = self.running.take() {
            running.stop();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn timeouts() -> Timeouts {
    Timeouts::new(
        StdDuration::from_secs(10),
        StdDuration::from_secs(60),
        StdDuration::from_secs(60),
    )
}

/// A retry policy that polls for up to a minute, far longer than any test
/// holds the archive stalled, so a wait that ends early ended because it was
/// answered and not because its budget ran out on a loaded machine: what a quorum
/// producer's wait for `archived_through` needs against a real archiver.
fn patient() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3000,
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(20),
        multiplier: 1,
        jitter_basis_points: 0,
    }
}

fn own_partition(identity: &str) -> u32 {
    partition_for(identity.strip_prefix("reflex:").unwrap(), PARTITIONS).unwrap()
}

/// A batch of `payloads.len()` records for a stream of `class`, as a writer
/// builds it: not yet stamped by a drain.
fn batch_of(class: QosClass, tag: &str, payloads: &[Vec<u8>]) -> Batch {
    let records = payloads
        .iter()
        .enumerate()
        .map(|(index, payload)| Record {
            event_id: format!("{tag}-{index}"),
            trace_id: None,
            source_timestamp_ns: 1_700_000_000_000_000_000 + index as i64,
            payload: payload.clone(),
        })
        .collect();
    Batch::new(
        MessageType::Data,
        class.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )
    .unwrap()
}

fn payloads_of(batch: &Batch) -> Vec<Vec<u8>> {
    batch.records.iter().map(|r| r.payload.clone()).collect()
}

/// Every batch a fetch response carries, in order.
fn decode_all(hex: &str) -> Vec<Batch> {
    let bytes = qip_core::hash::from_hex(hex).expect("a fetch answers with hex");
    let mut batches = Vec::new();
    let mut rest = bytes.as_slice();
    while !rest.is_empty() {
        let DecodeOutcome::Complete(batch) = Batch::decode(rest).expect("a fetched batch decodes")
        else {
            panic!("a fetch never answers with a torn batch");
        };
        let consumed = batch.encode().unwrap().len();
        batches.push(batch);
        rest = &rest[consumed..];
    }
    batches
}

fn fetch_all(fabric: &Fabric, identity: &str, stream: &str, partition: u32) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    loop {
        let offset = batches.len() as u64;
        let Response::Fetch(fetched) = fabric
            .call(
                identity,
                Request::Fetch(FetchRequest {
                    stream: stream.to_string(),
                    partition,
                    offset,
                    max_bytes: 1024 * 1024,
                }),
            )
            .unwrap()
        else {
            panic!("a granted fetch is answered with a fetch");
        };
        let more = decode_all(fetched.batches());
        if more.is_empty() {
            return batches;
        }
        batches.extend(more);
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(StdDuration::from_millis(10));
    }
}

fn refusal_of(response: Response) -> Refusal {
    match response {
        Response::Refused(refusal) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn metadata(fabric: &Fabric, identity: &str, stream: &str, partition: u32) -> (u64, u64, u64) {
    match fabric
        .call(
            identity,
            Request::Metadata(MetadataRequest {
                stream: stream.to_string(),
                partition,
            }),
        )
        .unwrap()
    {
        Response::Metadata(m) => (m.leader_epoch(), m.high_watermark(), m.archived_through()),
        other => panic!("expected metadata, got {other:?}"),
    }
}

// --- FABRIC-015 / the SDK seam --------------------------------------------------

/// The SDK's producer and consumer, over the production HTTP transport,
/// exchange batches of more than one record through the serving broker.
///
/// The failure this prevents was in the tree: both libraries passed their
/// own suites, every one of which used one-record batches, where a record
/// count and a batch count are the same number. After a three-record batch
/// at offset 0 the SDK advanced its sequence by one and the broker by three,
/// so the next batch was refused as behind the deduplication window; and a
/// consumer that received offset 0 asked next for offset 3.
///
/// Mutation (run, failed, restored): `Consumer::advance_past` back to
/// `base_offset + records.len()` — `next_offset` is 3 after the first batch,
/// not 1. And `Producer::send` advancing its sequence by one — batch 1 is
/// refused, "behind the deduplication window".
#[test]
fn the_sdk_and_the_served_broker_agree_on_sequences_and_offsets_for_multi_record_batches() {
    let fabric = Fabric::start("sdk");
    let (status, body) = fabric.health("/healthz");
    assert_eq!(status, 200, "premise: the broker reports ready: {body}");

    let partition = own_partition(CELL_A);
    let sent: Vec<Vec<Vec<u8>>> = vec![
        vec![b"a0".to_vec(), b"a1".to_vec(), b"a2".to_vec()],
        vec![b"b0".to_vec(), b"b1".to_vec()],
        vec![b"c0".to_vec()],
    ];
    assert!(
        sent.iter().any(|records| records.len() > 1),
        "premise: at least one batch carries more than one record"
    );

    let mut producer = fabric.producer(CELL_A, JOURNAL, partition, QosClass::P2MarketJournal);
    producer.init().unwrap();
    for (index, records) in sent.iter().enumerate() {
        let ack = producer
            .send(batch_of(
                QosClass::P2MarketJournal,
                &format!("b{index}"),
                records,
            ))
            .unwrap_or_else(|error| panic!("batch {index} was not acknowledged: {error}"));
        assert_eq!(
            ack.base_offset(),
            index as u64,
            "offsets are dense, one per batch"
        );
    }

    // One byte of credit, so each answer carries one batch and the consumer
    // must compute every next offset itself. With a large credit the broker
    // hands over all three batches in one answer and the consumer never asks
    // for a second offset at all — which is how this assertion first passed
    // against a consumer that still advanced by the record count.
    let mut consumer = fabric.consumer_with_credit(AUDITOR, JOURNAL, partition, "audit", 1);
    consumer.join().unwrap();
    let first = consumer
        .fetch()
        .unwrap()
        .expect("the first batch is delivered");
    assert_eq!(
        first.batch.records.len(),
        3,
        "premise: a three-record batch"
    );
    assert_eq!(
        consumer.next_offset(),
        1,
        "after a three-record batch at offset 0 the next offset is 1, not 3"
    );
    let mut delivered = vec![payloads_of(&first.batch)];
    while let Some(fetched) = consumer.fetch().unwrap() {
        delivered.push(payloads_of(&fetched.batch));
    }
    assert_eq!(
        delivered, sent,
        "every batch is delivered once, in append order"
    );
    assert_eq!(consumer.next_offset(), 3);
}

// --- FABRIC-006 -----------------------------------------------------------------

/// A sequence of records acknowledged on each durable class is replayed
/// byte for byte, in order, by a fresh consumer after the broker restarts —
/// and a restarted producer continues its stream instead of being refused.
///
/// The second half is the failure this prevents and it has happened: the
/// broker carries a producer's sequence across epochs (ADR 0100 §4) and the
/// SDK reset it to zero on `init`, so a producer that restarted after
/// writing anything had its next batch refused as behind the window, for
/// good. P0 and P1 here are produced on a quorum acknowledgement, so this
/// also needs the broker to seal a quiet partition's segment by age and
/// archive it; before `Broker::seal_aged` those two sends never returned.
///
/// Mutation (run, failed, restored): the `producers/init` handler answering
/// `next_sequence: 0` — the post-restart send is refused.
#[test]
fn a_restarted_broker_replays_every_durable_class_byte_for_byte_and_a_restarted_producer_continues()
{
    let mut fabric = Fabric::start("replay");
    let own = own_partition(CELL_A);
    let cases: [(&str, &str, u32, QosClass); 4] = [
        (CONTROLLER, CONTROL, 0, QosClass::P0Control),
        (CELL_A, OUTCOMES, own, QosClass::P1Outcomes),
        (CELL_A, JOURNAL, own, QosClass::P2MarketJournal),
        (RESEARCHER, RESEARCH, 1, QosClass::P3Research),
    ];
    let (epoch_before, _, _) = metadata(&fabric, AUDITOR, JOURNAL, own);

    let mut acknowledged: BTreeMap<&str, Vec<Vec<Vec<u8>>>> = BTreeMap::new();
    for (identity, stream, partition, class) in cases {
        let mut producer = fabric.producer(identity, stream, partition, class);
        producer.init().unwrap();
        for index in 0..3u8 {
            let records = vec![vec![index; 600], vec![index + 100; 600]];
            let ack = producer
                .send(batch_of(class, &format!("{stream}-{index}"), &records))
                .unwrap_or_else(|error| panic!("{stream} batch {index}: {error}"));
            if class.ack_floor() == qip_events::event_fabric::policy::AckProfile::Quorum {
                assert!(
                    ack.archived_through() > ack.base_offset(),
                    "{stream}: a quorum acknowledgement covers the batch it acknowledges"
                );
            }
            acknowledged.entry(stream).or_default().push(records);
        }
    }
    assert_eq!(
        acknowledged.len(),
        4,
        "premise: four durable classes were written"
    );

    fabric.restart();
    let (epoch_after, _, _) = metadata(&fabric, AUDITOR, JOURNAL, own);
    assert!(
        epoch_after > epoch_before,
        "premise: this is a new broker incarnation ({epoch_before} then {epoch_after})"
    );

    for (_, stream, partition, _) in cases {
        let replayed: Vec<Vec<Vec<u8>>> = fetch_all(&fabric, AUDITOR, stream, partition)
            .iter()
            .map(payloads_of)
            .collect();
        assert_eq!(
            replayed, acknowledged[stream],
            "{stream}: a fresh consumer replays what was acknowledged, in order"
        );
    }

    // The producer restarts too: a new `Producer` under the same id.
    let mut producer = fabric.producer(CELL_A, JOURNAL, own, QosClass::P2MarketJournal);
    let epoch = producer.init().unwrap();
    assert!(epoch > 1, "the restarted producer fences its predecessor");
    let ack = producer
        .send(batch_of(
            QosClass::P2MarketJournal,
            "after-restart",
            &[b"later".to_vec()],
        ))
        .expect("a restarted producer continues its dense stream");
    assert_eq!(
        ack.base_offset(),
        3,
        "appended after what the log already held"
    );
}

// --- FABRIC-018 -----------------------------------------------------------------

/// Every produce, consume and admin request is authorised against the
/// catalogue's grants for the verified identity, and a grant added to the
/// catalogue is honoured by the running process.
///
/// Asserts its premise first: the researcher *can* use the one stream it is
/// granted, so the refusals below are the ACL and not a broker that refuses
/// everything.
///
/// Mutation (run, failed, restored): `Service::refuse_unless_granted`
/// answering `Ok(None)` unconditionally — the researcher's produce to P1 is
/// acknowledged.
#[test]
fn an_identity_is_refused_whatever_its_grants_do_not_cover_until_the_catalogue_grants_it() {
    let fabric = Fabric::start("acl");
    let (epoch, _, _) = metadata(&fabric, AUDITOR, CONTROL, 0);

    let mut granted = fabric.producer(RESEARCHER, RESEARCH, 0, QosClass::P3Research);
    granted.init().unwrap();
    granted
        .send(batch_of(QosClass::P3Research, "r", &[b"finding".to_vec()]))
        .expect("premise: the researcher may produce to the stream it is granted");

    // Produce, consume and admin on streams it holds no grant on.
    let init = |stream: &str| {
        Request::ProducerInit(ProducerInitRequest {
            stream: stream.to_string(),
            partition: 0,
            producer_id: RESEARCHER.to_string(),
        })
    };
    assert_eq!(
        refusal_of(fabric.call(RESEARCHER, init(OUTCOMES)).unwrap()),
        Refusal::AclDenied
    );
    let mut forged = batch_of(QosClass::P1Outcomes, "forged", &[b"fill".to_vec()]);
    stamp_drain(&mut forged, RESEARCHER, 1, 0);
    let produce = Request::Produce(
        ProduceRequest::new(
            OUTCOMES,
            0,
            qip_core::hash::to_hex(&forged.encode().unwrap()),
        )
        .unwrap(),
    );
    assert_eq!(
        refusal_of(fabric.call(RESEARCHER, produce).unwrap()),
        Refusal::AclDenied
    );
    let fetch_control = || {
        Request::Fetch(FetchRequest {
            stream: CONTROL.to_string(),
            partition: 0,
            offset: 0,
            max_bytes: 4096,
        })
    };
    assert_eq!(
        refusal_of(fabric.call(RESEARCHER, fetch_control()).unwrap()),
        Refusal::AclDenied
    );
    let isolate = Request::AdminIsolate(AdminIsolateRequest {
        stream: RESEARCH.to_string(),
        partition: 0,
        operator: RESEARCHER.to_string(),
        reason: "mine".to_string(),
    });
    assert_eq!(
        refusal_of(fabric.call(RESEARCHER, isolate).unwrap()),
        Refusal::AclDenied,
        "a produce grant on a stream is not an admin grant on it"
    );
    assert_eq!(
        fetch_all(&fabric, AUDITOR, OUTCOMES, 0).len(),
        0,
        "nothing the researcher was refused reached the P1 stream"
    );

    // A cell may write its own key's partition and no other.
    let own = own_partition(CELL_A);
    let other = (0..PARTITIONS).find(|p| *p != own).unwrap();
    let cell_init = |partition: u32| {
        Request::ProducerInit(ProducerInitRequest {
            stream: JOURNAL.to_string(),
            partition,
            producer_id: CELL_A.to_string(),
        })
    };
    assert!(matches!(
        fabric.call(CELL_A, cell_init(own)).unwrap(),
        Response::ProducerInit(_)
    ));
    assert_eq!(
        refusal_of(fabric.call(CELL_A, cell_init(other)).unwrap()),
        Refusal::KeyOutOfScope
    );

    // A token the identities file does not hold is not an identity at all.
    let stranger = HttpTransport::new(
        format!("http://{}", fabric.running().address()),
        BearerToken::new("an-unknown-token-000000000000000000000000".to_string()).unwrap(),
    )
    .call(fetch_control(), timeouts())
    .expect_err("an unrecognised token is not answered");
    assert!(stranger.to_string().contains("401"), "{stranger}");

    // Grant the researcher the P0 stream in the catalogue, and nothing else
    // changes: same process, same leader epoch, no restart.
    let mut grants = base_grants();
    grants.push((RESEARCHER, CONTROL, "consume", "any"));
    std::fs::write(
        &fabric.config.catalogue,
        catalogue_json(&grants, "leader_only"),
    )
    .unwrap();
    wait_until("the running broker honours the new grant", || {
        matches!(
            fabric.call(RESEARCHER, fetch_control()).unwrap(),
            Response::Fetch(_)
        )
    });
    assert_eq!(
        metadata(&fabric, AUDITOR, CONTROL, 0).0,
        epoch,
        "the grant was honoured by the same broker incarnation, not by a restart"
    );
    assert_eq!(
        refusal_of(fabric.call(RESEARCHER, init(CONTROL)).unwrap()),
        Refusal::AclDenied,
        "a consume grant is not a produce grant"
    );
}

// --- FABRIC-024 -----------------------------------------------------------------

/// A batch naming a schema the stream never registered is refused at
/// produce and no consumer receives it; and an incompatible registration is
/// refused by the running broker's registry.
///
/// Mutation (run, failed, restored): `Broker::produce_to` skipping
/// `require_registered_schema` — the batch is acknowledged and fetched.
#[test]
fn an_unregistered_schema_is_refused_before_any_consumer_can_fetch_it() {
    let fabric = Fabric::start("schema");
    let partition = own_partition(CELL_A);
    let mut producer = fabric.producer(CELL_A, JOURNAL, partition, QosClass::P2MarketJournal);
    producer.init().unwrap();
    producer
        .send(batch_of(
            QosClass::P2MarketJournal,
            "ok",
            &[b"registered".to_vec()],
        ))
        .expect("premise: a batch under the stream's registered schema is accepted");

    let mut unregistered = Batch::new(
        MessageType::Data,
        99,
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: "x".to_string(),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: b"unregistered".to_vec(),
        }],
    )
    .unwrap();
    stamp_drain(&mut unregistered, CELL_A, producer.epoch().unwrap(), 1);
    let request = Request::Produce(
        ProduceRequest::new(
            JOURNAL,
            partition,
            qip_core::hash::to_hex(&unregistered.encode().unwrap()),
        )
        .unwrap(),
    );
    assert_eq!(
        refusal_of(fabric.call(CELL_A, request).unwrap()),
        Refusal::SchemaRefused
    );
    let fetched = fetch_all(&fabric, AUDITOR, JOURNAL, partition);
    assert_eq!(fetched.len(), 1, "only the registered batch is fetchable");
    assert_eq!(payloads_of(&fetched[0]), vec![b"registered".to_vec()]);

    // The registry itself, reached directly rather than through CI: a
    // registration that retypes the stream's shape under the same version is
    // refused, and the same shape under a bumped version is admitted.
    let broker = fabric.running().service().broker();
    let id = QosClass::P2MarketJournal.batch_schema_id();
    let retyped = Shape::of(&"not an object").unwrap();
    let error = broker
        .register_schema(JOURNAL, id, BATCH_SCHEMA_VERSION, retyped.clone())
        .expect_err("an incompatible registration is refused at runtime");
    assert!(error.to_string().contains("version"), "{error}");
    broker
        .register_schema(JOURNAL, id, BATCH_SCHEMA_VERSION + 1, retyped)
        .expect("a deliberate version bump is admitted");
}

// --- FABRIC-028 -----------------------------------------------------------------

/// An operator isolates one partition: produce to it is refused naming the
/// operator and the reason, the other partitions keep taking writes, the
/// retained records stay readable, the isolation survives a restart, and a
/// release restores service. Both actions are recorded under the identity
/// whose token was verified.
///
/// Mutation (run, failed, restored): `Broker::produce_to` skipping the
/// isolation check — the produce to the isolated partition is acknowledged.
#[test]
fn an_operator_isolates_one_partition_and_only_that_partition_refuses_produce_until_released() {
    let mut fabric = Fabric::start("isolate");
    let parked = own_partition(CELL_A);
    let open = own_partition(CELL_B);
    assert_ne!(
        parked, open,
        "premise: the two cells' keys route to different partitions"
    );

    let mut a = fabric.producer(CELL_A, JOURNAL, parked, QosClass::P2MarketJournal);
    let mut b = fabric.producer(CELL_B, JOURNAL, open, QosClass::P2MarketJournal);
    a.init().unwrap();
    b.init().unwrap();
    let before = vec![b"before".to_vec()];
    a.send(batch_of(QosClass::P2MarketJournal, "a0", &before))
        .expect("premise: the partition takes writes before it is isolated");
    b.send(batch_of(QosClass::P2MarketJournal, "b0", &before))
        .unwrap();

    let isolate = |operator: &str| {
        Request::AdminIsolate(AdminIsolateRequest {
            stream: JOURNAL.to_string(),
            partition: parked,
            operator: operator.to_string(),
            reason: "suspect feed".to_string(),
        })
    };
    let borrowed = fabric
        .call(OPERATOR, isolate("somebody-else"))
        .expect_err("an isolation under a name other than the verified identity is refused");
    assert!(borrowed.to_string().contains("403"), "{borrowed}");
    match fabric.call(OPERATOR, isolate(OPERATOR)).unwrap() {
        Response::AdminIsolate(isolated) => assert_eq!(isolated.isolated_at_offset, 1),
        other => panic!("expected an isolation, got {other:?}"),
    }

    let refused = a
        .send(batch_of(
            QosClass::P2MarketJournal,
            "a1",
            &[b"during".to_vec()],
        ))
        .expect_err("produce to an isolated partition is refused");
    assert!(
        refused.to_string().contains("isolated by operator")
            && refused.to_string().contains("suspect feed"),
        "the refusal names who and why: {refused}"
    );
    b.send(batch_of(
        QosClass::P2MarketJournal,
        "b1",
        &[b"during".to_vec()],
    ))
    .expect("another partition is unaffected");
    let retained = fetch_all(&fabric, AUDITOR, JOURNAL, parked);
    assert_eq!(
        retained.iter().map(payloads_of).collect::<Vec<_>>(),
        vec![before.clone()],
        "the isolated partition's records are intact and readable, and nothing was added"
    );

    fabric.restart();
    let mut a = fabric.producer(CELL_A, JOURNAL, parked, QosClass::P2MarketJournal);
    a.init().unwrap();
    a.send(batch_of(
        QosClass::P2MarketJournal,
        "a2",
        &[b"restart".to_vec()],
    ))
    .expect_err("a restart does not lift an isolation");

    let release = Request::AdminRelease(AdminReleaseRequest {
        stream: JOURNAL.to_string(),
        partition: parked,
        operator: OPERATOR.to_string(),
    });
    assert!(matches!(
        fabric.call(OPERATOR, release).unwrap(),
        Response::AdminRelease(_)
    ));
    a.send(batch_of(
        QosClass::P2MarketJournal,
        "a3",
        &[b"after".to_vec()],
    ))
    .expect("lifting the isolation restores service");

    let log = fabric.running().service().broker().admin_log().unwrap();
    let actions: Vec<(&str, &str, u32)> = log
        .iter()
        .map(|entry| {
            (
                entry.action.as_str(),
                entry.operator.as_str(),
                entry.partition,
            )
        })
        .collect();
    assert_eq!(
        actions,
        vec![("isolate", OPERATOR, parked), ("release", OPERATOR, parked)],
        "both actions are recorded, attributed, in order, across the restart"
    );
}

// --- FABRIC-012 -----------------------------------------------------------------

/// Sealed segments are archived exactly once each, the open segment never
/// is, and an archive that has stopped answering delays no append — while a
/// producer on a quorum acknowledgement is, correctly, not told its record
/// is safe until the archive has it.
///
/// Mutation (run, failed, restored): `Broker::archive_sealed` taking the
/// partition's produce lock for the duration of the upload — every append
/// to that partition waits on the stalled store and the P2 sends below time
/// out. And `Producer::await_ack_profile` back to `>=` — the P1 send at
/// offset 0 returns while the archive is still stalled.
#[test]
fn sealed_segments_are_archived_exactly_once_and_a_stalled_archive_never_delays_an_append() {
    let fabric = Fabric::start("archive");
    let partition = own_partition(CELL_A);
    fabric.archive.set_open(false);
    let arrived_at_start = fabric.archive.arrived.load(Ordering::SeqCst);

    // A P1 producer on a quorum acknowledgement, on a thread of its own: it
    // must still be waiting for as long as the archive is stalled.
    let quorum_done = Arc::new(AtomicBool::new(false));
    let quorum = {
        let mut producer = fabric.producer(CELL_A, OUTCOMES, partition, QosClass::P1Outcomes);
        let done = quorum_done.clone();
        std::thread::spawn(move || {
            producer.init().unwrap();
            let ack = producer.send(batch_of(QosClass::P1Outcomes, "fill", &[vec![7u8; 64]]));
            done.store(true, Ordering::SeqCst);
            ack
        })
    };

    // P2 takes a leader-only acknowledgement. Forty batches of ~1.5 kB seal
    // a 4 kB segment every few appends, so the archiver has work throughout.
    let mut producer = fabric.producer(CELL_A, JOURNAL, partition, QosClass::P2MarketJournal);
    producer.init().unwrap();
    let mut sent = Vec::new();
    for index in 0..40u8 {
        let records = vec![vec![index; 1500]];
        producer
            .send(batch_of(
                QosClass::P2MarketJournal,
                &format!("j{index}"),
                &records,
            ))
            .unwrap_or_else(|error| {
                panic!("append {index} waited on the stalled archive: {error}")
            });
        sent.push(records);
    }
    wait_until("the archiver has reached the stalled store", || {
        fabric.archive.arrived.load(Ordering::SeqCst) > arrived_at_start
    });
    wait_until("the P1 batch has been appended", || {
        metadata(&fabric, AUDITOR, OUTCOMES, partition).1 == 1
    });
    assert_eq!(
        fabric.archive.inner.list("segments/").unwrap().len(),
        0,
        "premise: the archive really is stalled — nothing has been stored"
    );
    let (_, high_watermark, archived_through) = metadata(&fabric, AUDITOR, JOURNAL, partition);
    assert_eq!(
        (high_watermark, archived_through),
        (40, 0),
        "all forty appends were acknowledged with nothing archived"
    );
    assert!(
        !quorum_done.load(Ordering::SeqCst),
        "a quorum producer is not told its record is safe while the archive has not got it"
    );

    fabric.archive.set_open(true);
    let ack = quorum
        .join()
        .unwrap()
        .expect("the quorum producer is acknowledged once the archive catches up");
    assert!(ack.archived_through() > ack.base_offset());

    let broker = fabric.running().service().broker();
    let sealed = broker.sealed_segment_starts(JOURNAL, partition).unwrap();
    assert!(
        sealed.len() >= 5,
        "premise: several segments sealed ({})",
        sealed.len()
    );
    wait_until("every sealed journal segment is archived", || {
        let sealed = broker.sealed_segment_starts(JOURNAL, partition).unwrap();
        let (_, high_watermark, archived_through) = metadata(&fabric, AUDITOR, JOURNAL, partition);
        // Sealing by age closes the tail too, so in the end everything is.
        archived_through == high_watermark && !sealed.is_empty()
    });

    // Exactly once: one stored object per sealed segment across the two
    // partitions written, each named by the hash of its own bytes.
    let journal_segments = broker.sealed_segment_starts(JOURNAL, partition).unwrap();
    let outcome_segments = broker.sealed_segment_starts(OUTCOMES, partition).unwrap();
    let objects = fabric.archive.inner.list("segments/").unwrap();
    assert_eq!(
        objects.len(),
        journal_segments.len() + outcome_segments.len(),
        "one archived object per sealed segment: {objects:?}"
    );
    for key in &objects {
        let bytes = fabric.archive.inner.get(key).unwrap().unwrap();
        assert!(
            key.ends_with(&qip_core::hash::sha256_hex(&bytes)),
            "{key} is named by its content"
        );
    }
    // The archive pass keeps running; the count must not grow.
    std::thread::sleep(StdDuration::from_millis(100));
    assert_eq!(
        fabric.archive.inner.list("segments/").unwrap().len(),
        objects.len(),
        "a later pass archives nothing a second time"
    );
    assert_eq!(
        fetch_all(&fabric, AUDITOR, JOURNAL, partition)
            .iter()
            .map(payloads_of)
            .collect::<Vec<_>>(),
        sent,
        "archiving changed nothing a consumer reads"
    );
}

// --- FABRIC-073, and the recording sites ------------------------------------------

/// Health and metrics are served from a listener of their own, and when the
/// protocol's listener stops they keep answering and say so.
///
/// Also the one place the broker's metric series are shown to move on a real
/// request: a produce is counted by stream, class and outcome, and a refused
/// one by its reason.
///
/// Mutation (run, failed, restored): `Running::stop_data_plane` not clearing
/// `serving` — health answers 200 `ready` for a broker that serves nothing.
#[test]
fn health_and_metrics_keep_answering_and_report_the_outage_when_the_data_plane_stops() {
    let mut fabric = Fabric::start("health");
    let partition = own_partition(CELL_A);
    let mut producer = fabric.producer(CELL_A, JOURNAL, partition, QosClass::P2MarketJournal);
    producer.init().unwrap();
    producer
        .send(batch_of(QosClass::P2MarketJournal, "m", &[b"one".to_vec()]))
        .unwrap();
    let other = (0..PARTITIONS).find(|p| *p != partition).unwrap();
    let mut out_of_scope = batch_of(QosClass::P2MarketJournal, "n", &[b"two".to_vec()]);
    stamp_drain(&mut out_of_scope, CELL_A, 1, 0);
    let refused = fabric
        .call(
            CELL_A,
            Request::Produce(
                ProduceRequest::new(
                    JOURNAL,
                    other,
                    qip_core::hash::to_hex(&out_of_scope.encode().unwrap()),
                )
                .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(refusal_of(refused), Refusal::KeyOutOfScope);

    let (status, body) = fabric.health("/healthz");
    assert_eq!(status, 200, "premise: ready while serving: {body}");
    assert!(body.contains(r#""serving":true"#), "{body}");
    let (status, exposition) = fabric.health("/metrics");
    assert_eq!(status, 200);
    let has = |needle: &[&str]| {
        exposition
            .lines()
            .any(|line| needle.iter().all(|part| line.contains(part)))
    };
    assert!(
        has(&[
            "qip_event_fabric_append",
            r#"stream="journal.test""#,
            r#"class="p2_market_journal""#,
            r#"outcome="success""#
        ]),
        "the accepted append is counted:\n{exposition}"
    );
    assert!(
        has(&["qip_event_fabric_refusals", r#"reason="key_out_of_scope""#]),
        "the refused append is counted by its reason:\n{exposition}"
    );

    fabric.running.as_mut().unwrap().stop_data_plane();
    let dead = fabric
        .call(
            AUDITOR,
            Request::Metadata(MetadataRequest {
                stream: JOURNAL.to_string(),
                partition,
            }),
        )
        .expect_err("premise: the protocol listener no longer answers");
    assert!(!dead.to_string().is_empty());

    let (status, body) = fabric.health("/healthz");
    assert_eq!(
        status, 503,
        "health reports the outage instead of sharing it: {body}"
    );
    assert!(body.contains(r#""serving":false"#), "{body}");
    assert!(body.contains(r#""status":"not_ready""#), "{body}");
    let (status, exposition) = fabric.health("/metrics");
    assert_eq!(status, 200, "metrics are still scrapable");
    assert!(exposition.contains("qip_event_fabric_leader_epoch"));
}

// --- FABRIC-034 -----------------------------------------------------------------

/// A subscriber registered on a partition receives each batch appended
/// afterwards, in order, without its own thread ever issuing a fetch.
///
/// Mutation (run, failed, restored): `Consumer::subscribe`'s idle arm
/// returning instead of sleeping and polling again — the fetch thread exits
/// on the first empty answer and nothing appended later is delivered.
#[test]
fn a_subscriber_receives_batches_appended_after_it_subscribed_without_fetching_itself() {
    let fabric = Fabric::start("subscribe");
    let partition = own_partition(CELL_A);
    let consumer = fabric.consumer(AUDITOR, JOURNAL, partition, "live");
    let subscription = consumer.subscribe(2, Duration::from_millis(5)).unwrap();
    // Give the fetch thread time to find the partition empty at least once.
    std::thread::sleep(StdDuration::from_millis(50));
    assert_eq!(
        metadata(&fabric, AUDITOR, JOURNAL, partition).1,
        0,
        "premise: nothing had been appended when the subscription was registered"
    );

    let mut producer = fabric.producer(CELL_A, JOURNAL, partition, QosClass::P2MarketJournal);
    producer.init().unwrap();
    let (results, receive) = std::sync::mpsc::channel();
    let receiver = std::thread::spawn(move || {
        for _ in 0..6 {
            match subscription.recv() {
                Some(SubscriptionEvent::Delivered(fetched)) => {
                    results.send(payloads_of(&fetched.batch)).unwrap();
                }
                other => panic!("the subscription ended early: {other:?}"),
            }
        }
    });
    let mut sent = Vec::new();
    for index in 0..6u8 {
        let records = vec![vec![index; 8]];
        producer
            .send(batch_of(
                QosClass::P2MarketJournal,
                &format!("s{index}"),
                &records,
            ))
            .unwrap();
        sent.push(records);
    }
    let mut delivered = Vec::new();
    for _ in 0..6 {
        delivered.push(
            receive
                .recv_timeout(EVENTUALLY)
                .expect("each appended batch is delivered to the subscriber"),
        );
    }
    receiver.join().unwrap();
    assert_eq!(delivered, sent);
}

// --- FABRIC-015 / FABRIC-041 ------------------------------------------------------

/// Random produce, fetch, consumer-restart and broker-restart sequences
/// against the serving broker: every acknowledged offset is the next dense
/// one and is never reused, every fetched range is a slice of the append
/// order, the leader epoch never decreases along a partition, and a
/// consumer group that commits and is replaced is delivered every
/// acknowledged batch at least once.
///
/// Mutation (run, failed, restored): the `fetch` handler passing
/// `r.offset + 1` to the broker — the first fetched range is not the slice
/// that was asked for.
#[test]
fn random_produce_fetch_and_restart_sequences_keep_offsets_dense_and_fetches_in_append_order() {
    let mut fabric = Fabric::start("property");
    let mut rng = Xoshiro256::seeded(0x00FA_B21C);
    let partitions = [own_partition(CELL_A), own_partition(CELL_B)];
    let cells = [CELL_A, CELL_B];
    // What each partition holds, by offset, as the model.
    let mut model: [Vec<Vec<Vec<u8>>>; 2] = [Vec::new(), Vec::new()];
    let mut producers: Vec<Producer> = Vec::new();
    let reopen = |fabric: &Fabric| -> Vec<Producer> {
        (0..2)
            .map(|i| {
                let mut producer =
                    fabric.producer(cells[i], JOURNAL, partitions[i], QosClass::P2MarketJournal);
                producer.init().unwrap();
                producer
            })
            .collect()
    };
    producers.extend(reopen(&fabric));
    // Batches delivered to the group "ledger-like", across its restarts.
    let mut delivered: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
    let mut restarts = 0;
    let mut fetches = 0;
    let mut last_epoch = 0;

    for step in 0..160u32 {
        let which = (rng.next_u64() % 2) as usize;
        match rng.next_u64() % 10 {
            0..=4 => {
                let count = 1 + (rng.next_u64() % 3) as usize;
                let records: Vec<Vec<u8>> = (0..count)
                    .map(|i| format!("step-{step}-record-{i}").into_bytes())
                    .collect();
                let ack = producers[which]
                    .send(batch_of(
                        QosClass::P2MarketJournal,
                        &format!("p{step}"),
                        &records,
                    ))
                    .unwrap_or_else(|error| panic!("step {step}: append refused: {error}"));
                assert_eq!(
                    ack.base_offset(),
                    model[which].len() as u64,
                    "step {step}: the acknowledged offset is the next dense one, never a reused one"
                );
                model[which].push(records);
            }
            5..=7 => {
                if model[which].is_empty() {
                    continue;
                }
                let from = rng.next_u64() % model[which].len() as u64;
                let max_bytes = 64 + (rng.next_u64() % 4096) as u32;
                let Response::Fetch(fetched) = fabric
                    .call(
                        AUDITOR,
                        Request::Fetch(FetchRequest {
                            stream: JOURNAL.to_string(),
                            partition: partitions[which],
                            offset: from,
                            max_bytes,
                        }),
                    )
                    .unwrap()
                else {
                    panic!("a granted fetch is answered with a fetch");
                };
                let batches = decode_all(fetched.batches());
                assert!(
                    !batches.is_empty(),
                    "step {step}: an offset below the watermark always yields a batch"
                );
                for (index, batch) in batches.iter().enumerate() {
                    let offset = from + index as u64;
                    assert_eq!(batch.base_offset, offset, "step {step}: a contiguous slice");
                    assert_eq!(
                        payloads_of(batch),
                        model[which][offset as usize],
                        "step {step}: offset {offset} holds what was appended there"
                    );
                    assert!(batch.leader_epoch >= 1);
                }
                fetches += 1;
            }
            8 => {
                // A member of the group reads a little, commits, and dies.
                let mut consumer = fabric.consumer_with_credit(
                    AUDITOR,
                    JOURNAL,
                    partitions[which],
                    "ledger-like",
                    1,
                );
                consumer.join().unwrap();
                if !delivered[which].is_empty() {
                    consumer.resume().unwrap();
                }
                for _ in 0..3 {
                    let Some(fetched) = consumer.fetch().unwrap() else {
                        break;
                    };
                    delivered[which].push(fetched.batch.base_offset);
                    consumer.commit(fetched.batch.base_offset).unwrap();
                }
            }
            _ => {
                if restarts == 4 {
                    continue;
                }
                producers.clear();
                fabric.restart();
                producers.extend(reopen(&fabric));
                restarts += 1;
                let (epoch, high_watermark, _) =
                    metadata(&fabric, AUDITOR, JOURNAL, partitions[which]);
                assert!(
                    epoch > last_epoch,
                    "step {step}: each restart is a new leader epoch"
                );
                last_epoch = epoch;
                assert_eq!(
                    high_watermark,
                    model[which].len() as u64,
                    "step {step}: a restart neither loses nor invents an offset"
                );
            }
        }
    }
    assert!(
        restarts >= 2 && fetches >= 10 && model.iter().all(|m| m.len() >= 10),
        "premise: the sequence exercised restarts ({restarts}), fetches ({fetches}) and appends"
    );

    // Along each whole partition: dense offsets, the append order, and a
    // leader epoch and logical timestamp that never go backwards.
    for which in 0..2 {
        let all = fetch_all(&fabric, AUDITOR, JOURNAL, partitions[which]);
        assert_eq!(all.len(), model[which].len());
        let mut previous: Option<&Batch> = None;
        for (offset, batch) in all.iter().enumerate() {
            assert_eq!(batch.base_offset, offset as u64);
            assert_eq!(payloads_of(batch), model[which][offset]);
            if let Some(previous) = previous {
                assert!(batch.leader_epoch >= previous.leader_epoch);
                let stamp =
                    |b: &Batch| (b.logical_timestamp.physical_ns, b.logical_timestamp.logical);
                assert!(
                    stamp(batch) > stamp(previous),
                    "offset {offset}: the logical timestamp advances along the partition"
                );
            }
            previous = Some(batch);
        }
        assert!(
            all.last().unwrap().leader_epoch > all.first().unwrap().leader_epoch,
            "premise: this partition was written under more than one leader epoch"
        );

        // At least once: finish the group's read with one more member, then
        // every acknowledged offset has been delivered to the group.
        let mut consumer =
            fabric.consumer_with_credit(AUDITOR, JOURNAL, partitions[which], "ledger-like", 1);
        consumer.join().unwrap();
        if !delivered[which].is_empty() {
            consumer.resume().unwrap();
        }
        while let Some(fetched) = consumer.fetch().unwrap() {
            delivered[which].push(fetched.batch.base_offset);
            consumer.commit(fetched.batch.base_offset).unwrap();
        }
        for offset in 0..model[which].len() as u64 {
            assert!(
                delivered[which].contains(&offset),
                "partition {which}: offset {offset} was never delivered to the group"
            );
        }
    }
}

/// Creating a topic is declaring it in the catalogue the broker starts on,
/// and a catalogue that gives a durable class an at-most-once
/// acknowledgement does not start a broker.
///
/// Asserts its premise first: the same catalogue with the acknowledgement
/// left at the class's floor starts, and so does the telemetry stream on
/// `none`, so the refusal is about the class and not the file.
///
/// Mutation (run, failed, restored): `StreamPolicy::new` no longer comparing
/// the acknowledgement profile with the class's floor — the broker starts.
#[test]
fn a_broker_does_not_start_on_a_catalogue_that_gives_a_durable_class_an_at_most_once_profile() {
    let sound = Fabric::start_with("floor-ok", &catalogue_json(&base_grants(), "leader_only"))
        .expect("premise: the catalogue starts with every class at its floor");
    assert_eq!(sound.health("/healthz").0, 200);
    drop(sound);

    let error = Fabric::start_with("floor-weak", &catalogue_json(&base_grants(), "none"))
        .err()
        .expect("a P2 stream acknowledging at `none` is refused at start");
    let message = error.to_string();
    assert!(
        message.contains("p2_market_journal") && message.contains("weaker"),
        "the refusal names the class and what is wrong: {message}"
    );
}
