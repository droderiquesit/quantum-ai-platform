//! ADR 0111 through the composition root: `qip_fabricd::start_with_seams`,
//! the function `main` reaches through `start`, with two real listeners per
//! instance and the client SDK on the other end of a real socket. The only
//! seam is the managed log itself — one in-memory log shared by two
//! instances standing in for one Spanner database shared by two zones —
//! because the Spanner adapter's own transaction logic is held by
//! `src/spanner.rs`'s scripted-peer tests.
//!
//! RES-7112 / FABRIC-7035 / FABRIC-8086: P0/P1 acknowledged only after the
//! managed commit; the leader killed, the standby promoted by the lease, the
//! producer retrying by epoch, and no acknowledged record lost.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use qip_core::error::Result;
use qip_core::{Duration, ManualClock, SystemClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, BATCH_SCHEMA_VERSION, QosClass};
use qip_fabricd::config::{self, Config};
use qip_fabricd::{Running, start_with_seams};
use qip_storage::MemoryBlobStore;
use qip_streaming::event_fabric::managed::{ManagedLog, MemoryManagedLog};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{FetchRequest, Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

const OUTCOMES: &str = "outcomes.managed";
const JOURNAL: &str = "journal.local";
const WRITER: &str = "writer";
const READER: &str = "reader";
const TTL_MS: &str = "1500";

static ROOTS: AtomicU64 = AtomicU64::new(0);

fn stream(name: &str, class: &str, retention: &str, overload: &str, ack: &str, rf: u32) -> String {
    format!(
        r#"{{"name":"{name}","qos_class":"{class}","partition_key":"cell",
            "ordering":"per_partition","retention":"{retention}","replication_factor":{rf},
            "mirroring":"none","overload_policy":"{overload}","ack_profile":"{ack}",
            "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
            "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}"#
    )
}

fn catalogue(outcomes_ack: &str, outcomes_rf: u32) -> String {
    let streams = [
        stream(
            OUTCOMES,
            "p1_outcomes",
            "irreplaceable",
            "refuse_producer",
            outcomes_ack,
            outcomes_rf,
        ),
        stream(
            JOURNAL,
            "p2_market_journal",
            "event_anchored",
            "throttle_with_gap",
            "leader_only",
            1,
        ),
    ]
    .join(",");
    format!(
        r#"{{"streams":[{streams}],"grants":[
            {{"identity":"{WRITER}","stream":"{OUTCOMES}","permission":"produce","key_scope":"any"}},
            {{"identity":"{READER}","stream":"{OUTCOMES}","permission":"consume","key_scope":"any"}}]}}"#
    )
}

fn token(identity: &str) -> String {
    format!("managed-fixture-token-for-{identity}-000000000000000000000")
}

/// A started instance and the directory it owns.
struct Instance {
    root: PathBuf,
    running: Option<Running>,
}

impl Instance {
    fn address(&self) -> String {
        format!("http://{}", self.running.as_ref().unwrap().address())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Start an instance named `instance` on `catalogue`, seated at `managed`
/// when one is given.
fn start(
    instance: &str,
    catalogue: &str,
    managed: Option<Arc<dyn ManagedLog>>,
) -> (PathBuf, Result<Running>) {
    let unique = ROOTS.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "qip-fabricd-managed-{instance}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("streams.json"), catalogue).unwrap();
    let identities = [WRITER, READER]
        .iter()
        .map(|id| {
            format!(
                "{id} {}\n",
                qip_core::hash::sha256_hex(token(id).as_bytes())
            )
        })
        .collect::<String>();
    std::fs::write(root.join("identities"), identities).unwrap();
    let path = |name: &str| root.join(name).display().to_string();
    let mut variables: BTreeMap<String, String> = [
        (config::DATA_DIR, path("data")),
        (config::CATALOGUE, path("streams.json")),
        (config::IDENTITIES_FILE, path("identities")),
        (config::ARCHIVE_DIR, path("archive")),
        (config::LISTEN, "127.0.0.1:0".to_string()),
        (config::HEALTH_LISTEN, "127.0.0.1:0".to_string()),
        (config::PARTITIONS, "1".to_string()),
        (config::HOUSEKEEPING_MS, "20".to_string()),
        // ADR 0112: an in-process broker that owns both ends of its socket,
        // so plaintext, which only `local` admits. The managed seam is an
        // in-memory log, never a deployed Spanner, so `local` is the honest
        // answer for every instance this file starts.
        (config::DEPLOYMENT, "local".to_string()),
        (
            qip_transport::tls::TLS_MODE_VARIABLE,
            "plaintext".to_string(),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    if managed.is_some() {
        for (name, value) in [
            (config::MANAGED_LOG, "spanner"),
            (
                config::SPANNER_DATABASE,
                "projects/qip-test/instances/fabric/databases/fabric",
            ),
            (config::REGION, "europe-west2"),
            (config::INSTANCE, instance),
            (config::LEASE_TTL_MS, TTL_MS),
            // Never dialled: the managed log is the seam below.
            (qip_storage::gcp::ENDPOINT_VARIABLE, "http://127.0.0.1:9"),
            (qip_storage::gcp::METADATA_VARIABLE, "1"),
        ] {
            variables.insert(name.to_string(), value.to_string());
        }
    }
    let started = Config::parse(&variables).and_then(|config| {
        start_with_seams(
            &config,
            Arc::new(SystemClock),
            Some(Arc::new(MemoryBlobStore::new())),
            managed,
        )
    });
    (root, started)
}

fn instance(name: &str, catalogue: &str, managed: Option<Arc<dyn ManagedLog>>) -> Instance {
    let (root, running) = start(name, catalogue, managed);
    Instance {
        root,
        running: Some(running.expect("the instance starts")),
    }
}

fn timeouts() -> Timeouts {
    Timeouts::new(
        StdDuration::from_secs(5),
        StdDuration::from_secs(30),
        StdDuration::from_secs(30),
    )
}

fn producer(address: &str, ack_profile: AckProfile) -> Producer {
    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(HttpTransport::new(
            address.to_string(),
            BearerToken::new(token(WRITER)).unwrap(),
        )),
        stream: OUTCOMES.to_string(),
        partition: 0,
        producer_id: WRITER.to_string(),
        qos_class: QosClass::P1Outcomes,
        ack_profile,
        retry_policy: RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        },
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(ThreadSleeper),
        retry_seed: 3,
        breaker_seed: 3,
        timeouts: timeouts(),
    })
    .unwrap();
    producer.init().unwrap();
    producer
}

fn batch(payload: &str) -> Batch {
    Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("evt-{payload}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: payload.as_bytes().to_vec(),
        }],
    )
    .unwrap()
}

fn payload_at(address: &str, offset: u64) -> Vec<u8> {
    let mut reader = HttpTransport::new(
        address.to_string(),
        BearerToken::new(token(READER)).unwrap(),
    );
    let answer = reader
        .call(
            Request::Fetch(FetchRequest {
                stream: OUTCOMES.to_string(),
                partition: 0,
                offset,
                max_bytes: 1,
            }),
            timeouts(),
        )
        .unwrap();
    let Response::Fetch(fetched) = answer else {
        panic!("a fetch is answered with batches: {answer:?}");
    };
    let bytes = qip_core::hash::from_hex(fetched.batches()).unwrap();
    match Batch::decode(&bytes).unwrap() {
        DecodeOutcome::Complete(batch) => batch.records[0].payload.clone(),
        DecodeOutcome::Torn => panic!("offset {offset} is not served"),
    }
}

/// RES-7112: two instances, one managed log. The first leads and every
/// acknowledgement carries the lease epoch; the second follows and refuses
/// to acknowledge; the first is killed, database time passes its lease, the
/// second's own lease loop promotes it, the producer re-initialises against
/// it and carries its sequence on, and every record acknowledged before the
/// kill is served by the survivor.
///
/// Mutation: in `start_with_seams`, drop the lease thread (`lease_thread =
/// None`) — the survivor never takes the lease, the post-kill produce never
/// succeeds and the deadline assertion fires. Mutation: in
/// `Broker::tend_leases`, skip `catch_up` for a follower — the survivor's
/// fetch of offset 0 after the kill finds nothing and fires.
#[test]
fn a_killed_leader_is_replaced_by_its_follower_with_no_acknowledged_record_lost() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let log: Arc<MemoryManagedLog> = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let catalogue = catalogue("managed_majority", 3);
    let mut leader = instance("zone-a", &catalogue, Some(log.clone()));
    let survivor = instance("zone-b", &catalogue, Some(log.clone()));

    let mut writer = producer(&leader.address(), AckProfile::ManagedMajority);
    for n in 0..3 {
        let ack = writer.send(batch(&format!("fill-{n}"))).unwrap();
        assert_eq!(ack.base_offset(), n);
        assert_eq!(ack.managed_commit_epoch(), Some(1));
    }
    let mut stray = producer(&survivor.address(), AckProfile::ManagedMajority);
    assert!(
        stray.send(batch("stray")).is_err(),
        "premise: the follower acknowledges nothing"
    );

    leader.running.take().unwrap().stop();
    database_time.advance(Duration::from_millis(1_501));

    let mut writer = producer(&survivor.address(), AckProfile::ManagedMajority);
    let deadline = Instant::now() + StdDuration::from_secs(60);
    let ack = loop {
        match writer.send(batch("fill-3")) {
            Ok(ack) => break ack,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(StdDuration::from_millis(50));
                writer = producer(&survivor.address(), AckProfile::ManagedMajority);
            }
            Err(error) => panic!("the survivor never took the lease: {error}"),
        }
    };
    assert_eq!(ack.base_offset(), 3, "dense across the failover");
    assert_eq!(ack.managed_commit_epoch(), Some(2));
    for n in 0..4u64 {
        assert_eq!(
            payload_at(&survivor.address(), n),
            format!("fill-{n}").into_bytes()
        );
    }
    drop(survivor);
}

/// The SDK's half of FABRIC-8086: a producer that requires a managed
/// majority refuses an acknowledgement from a broker running the stream on
/// one disk, rather than reading an fsync as "survives a zone".
///
/// Mutation: in `Producer::await_ack_profile`, return `Ok(ack)` for
/// `ManagedMajority` without checking `managed_commit_epoch` — the local
/// acknowledgement is accepted and the `expect_err` fires.
#[test]
fn a_managed_majority_producer_refuses_an_acknowledgement_from_a_local_broker() {
    let local = instance("local", &catalogue("archived", 1), None);
    let mut writer = producer(&local.address(), AckProfile::ManagedMajority);
    let refused = writer
        .send(batch("fill"))
        .expect_err("a local acknowledgement is not a managed one");
    assert!(
        refused.to_string().contains("without a managed commit"),
        "{refused}"
    );
}

/// ADR 0111 §1 at start-up: a broker seated at the managed consensus refuses
/// a catalogue that leaves a P1 stream on the local path, and a broker with
/// no managed log refuses a catalogue that declares one managed — both
/// before a port is bound.
///
/// Mutation: in `Broker::declare_stream`, delete the converse refusal (a
/// seated broker with an unmanaged P0/P1 stream) — the first start succeeds
/// and its `expect_err` fires.
#[test]
fn a_seated_broker_refuses_a_local_p1_stream_and_a_local_broker_refuses_a_managed_one() {
    let time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let log: Arc<dyn ManagedLog> = Arc::new(MemoryManagedLog::new(time));
    let (root, seated) = start("seated", &catalogue("archived", 1), Some(log));
    let error = seated.expect_err("a seated broker with a local P1 stream");
    assert!(error.to_string().contains("managed_majority"), "{error}");
    let _ = std::fs::remove_dir_all(root);

    let (root, local) = start("local", &catalogue("managed_majority", 3), None);
    let error = local.expect_err("a local broker with a managed stream");
    assert!(error.to_string().contains("no managed log"), "{error}");
    let _ = std::fs::remove_dir_all(root);
}
