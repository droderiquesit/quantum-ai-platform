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
    start_after(instance, catalogue, managed, |_| {})
}

/// [`start`], running `before_start` on the instance's root directory once
/// it exists and before the broker opens: what a previous process left
/// behind.
fn start_after(
    instance: &str,
    catalogue: &str,
    managed: Option<Arc<dyn ManagedLog>>,
    before_start: impl FnOnce(&std::path::Path),
) -> (PathBuf, Result<Running>) {
    let unique = ROOTS.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "qip-fabricd-managed-{instance}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    before_start(&root);
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
    served_at(address, offset).unwrap_or_else(|| panic!("offset {offset} is not served"))
}

/// The payload at `offset` if the instance serves it now, `None` if the
/// fetch is answered but holds no complete batch there yet.
fn served_at(address: &str, offset: u64) -> Option<Vec<u8>> {
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
    if bytes.is_empty() {
        return None;
    }
    match Batch::decode(&bytes).unwrap() {
        DecodeOutcome::Complete(batch) => Some(batch.records[0].payload.clone()),
        DecodeOutcome::Torn => None,
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
/// `Broker::tend_leases`, skip `catch_up` for a follower — the survivor
/// never serves offset 2 while it follows and the warm-follower assertion
/// fires (this comment said the post-kill fetch fires; it does not, because
/// promotion catches up as well).
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
    // The follower catches up while it follows, before anything fails. The
    // round-2 mutation pass skipped `catch_up` in `tend_leases`'s
    // `HeldByOther` arm and the test still passed, because promotion catches
    // up too; what that mutation loses is a warm follower, so it is asserted
    // here, before the kill, where promotion cannot paper over it.
    let warm_by = Instant::now() + StdDuration::from_secs(30);
    while served_at(&survivor.address(), 2).is_none() {
        assert!(
            Instant::now() < warm_by,
            "the follower never caught up with the leader's committed records"
        );
        std::thread::sleep(StdDuration::from_millis(50));
    }

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

/// The instance and epoch the lease names; the holder string the broker
/// writes is the instance name and its process epoch (`zone-a#1`).
fn lease_of(
    log: &MemoryManagedLog,
    partition: &qip_streaming::event_fabric::managed::PartitionRef,
) -> Option<(String, u64)> {
    log.lease_of(partition).map(|(holder, epoch)| {
        (
            holder.split('#').next().unwrap_or_default().to_string(),
            epoch,
        )
    })
}

/// RES-8103: a leader that is drained hands its lease back, so the follower
/// leads at once — with no database time passing — and no record the
/// drained leader acknowledged is lost. The drained leader stops
/// acknowledging before it is stopped, which is what makes the handover
/// safe to do while producers are still writing.
///
/// Mutation: in `Broker::relinquish_leases`, delete the `seat.log.release`
/// call (keep the `set_lease(.., None)`) — the leader refuses, the lease is
/// never lapsed, the follower is never promoted without the clock moving and
/// the deadline assertion fires. Mutation: in `MemoryManagedLog::release`,
/// return `Ok(true)` without lapsing the row — same failure.
#[test]
fn a_drained_leader_hands_its_lease_to_the_follower_with_no_acknowledged_record_lost() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 18)));
    let log: Arc<MemoryManagedLog> = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let catalogue = catalogue("managed_majority", 3);
    let leader = instance("zone-a", &catalogue, Some(log.clone()));
    let follower = instance("zone-b", &catalogue, Some(log.clone()));

    let mut writer = producer(&leader.address(), AckProfile::ManagedMajority);
    for n in 0..3 {
        let ack = writer.send(batch(&format!("fill-{n}"))).unwrap();
        assert_eq!(ack.managed_commit_epoch(), Some(1));
    }
    let partition = qip_streaming::event_fabric::managed::PartitionRef::new(OUTCOMES, 0);
    assert_eq!(
        lease_of(&log, &partition),
        Some(("zone-a".to_string(), 1)),
        "premise: the leader holds the lease before it is drained"
    );

    let report = leader.running.as_ref().unwrap().drain().unwrap();
    assert_eq!(
        report.released,
        vec![(OUTCOMES.to_string(), 0)],
        "the drain handed back the one managed partition it led: {report:?}"
    );
    let mut refused = producer(&leader.address(), AckProfile::ManagedMajority);
    assert!(
        refused.send(batch("after-drain")).is_err(),
        "a drained leader acknowledges nothing more"
    );

    // No `database_time.advance`: a lapsed-by-TTL takeover cannot happen
    // here, so a promotion is the release's doing.
    let mut writer = producer(&follower.address(), AckProfile::ManagedMajority);
    let deadline = Instant::now() + StdDuration::from_secs(30);
    let ack = loop {
        match writer.send(batch("fill-3")) {
            Ok(ack) => break ack,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(StdDuration::from_millis(50));
                writer = producer(&follower.address(), AckProfile::ManagedMajority);
            }
            Err(error) => panic!("the follower never took the released lease: {error}"),
        }
    };
    assert_eq!(ack.base_offset(), 3, "dense across the handover");
    assert_eq!(ack.managed_commit_epoch(), Some(2));
    for n in 0..4u64 {
        assert_eq!(
            payload_at(&follower.address(), n),
            format!("fill-{n}").into_bytes()
        );
    }
}

/// The operator's lever on a broker nobody holds a handle to: a `DRAIN` file
/// in its data directory. The drained broker hands its lease back and then
/// does not take it again, even when the lease lapses with nobody left to
/// renew it — a draining broker that went on renewing would be the old
/// leader again one pass after it was drained.
///
/// Mutation: in the lease thread, delete the `drain_file.exists()` check —
/// the file is never read, the follower is never promoted and the deadline
/// assertion fires. Mutation: delete the `continue` after `relinquish_leases`
/// in the same loop — the drained broker renews, takes the lapsed lease at
/// epoch 3 and the final `lease_of` assertion fires.
#[test]
fn a_drain_file_hands_the_lease_over_and_the_drained_broker_does_not_take_it_back() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 18)));
    let log: Arc<MemoryManagedLog> = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let catalogue = catalogue("managed_majority", 3);
    let leader = instance("zone-a", &catalogue, Some(log.clone()));
    let follower = instance("zone-b", &catalogue, Some(log.clone()));
    let partition = qip_streaming::event_fabric::managed::PartitionRef::new(OUTCOMES, 0);
    let mut writer = producer(&leader.address(), AckProfile::ManagedMajority);
    writer.send(batch("fill-0")).unwrap();
    assert_eq!(
        lease_of(&log, &partition),
        Some(("zone-a".to_string(), 1)),
        "premise: the leader leads at epoch 1"
    );

    std::fs::write(leader.root.join("data").join(qip_fabricd::DRAIN_FILE), b"").unwrap();
    let deadline = Instant::now() + StdDuration::from_secs(30);
    while lease_of(&log, &partition) != Some(("zone-b".to_string(), 2)) {
        assert!(
            Instant::now() < deadline,
            "the follower never took the lease the drain file released: {:?}",
            lease_of(&log, &partition)
        );
        std::thread::sleep(StdDuration::from_millis(50));
    }

    // Lapse the follower's lease with nobody to renew it, then give the
    // drained broker several passes at it.
    follower.running.as_ref().unwrap().drain().unwrap();
    database_time.advance(Duration::from_millis(1_501));
    std::thread::sleep(StdDuration::from_millis(1_800));
    assert_eq!(
        lease_of(&log, &partition),
        Some(("zone-b".to_string(), 2)),
        "a drained broker takes no lease, lapsed or not"
    );
}

/// A `GET /health` on `address`, the body parsed.
fn health_body(address: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(StdDuration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let body = response.split("\r\n\r\n").nth(1).unwrap();
    serde_json::from_str(body).unwrap()
}

/// A drain is a request answered once. The file lives in the persistent data
/// directory, so one left in place drained the restarted broker too: an
/// upgrade that drained each broker in turn left both draining for good and
/// every P0/P1 produce refused until someone deleted the files by hand.
///
/// Mutation: delete the start-up `remove_file` of `DRAIN_FILE` in
/// `start_composed` — the stale file survives, the lease thread drains the
/// restarted broker and the produce below is refused. Mutation: delete the
/// `remove_file(&drain_file)` in the lease thread — the file written to a
/// running broker is still there after the follower has taken over and the
/// last assertion fires.
#[test]
fn a_drain_file_is_answered_once_and_a_restarted_broker_leads_again() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 18)));
    let log: Arc<MemoryManagedLog> = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let catalogue = catalogue("managed_majority", 3);

    // What the pre-stop hook left behind in the old process's data directory.
    let (root, started) = start_after("zone-a", &catalogue, Some(log.clone()), |root| {
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data").join(qip_fabricd::DRAIN_FILE), b"").unwrap();
    });
    let restarted = Instance {
        root,
        running: Some(started.expect("the restarted instance starts")),
    };
    let stale = restarted.root.join("data").join(qip_fabricd::DRAIN_FILE);
    assert!(
        !stale.exists(),
        "a DRAIN file from the previous process is cleared at start"
    );
    // Longer than two renewal periods (TTL/3): a broker that read the stale
    // file would have drained on the first of them.
    std::thread::sleep(StdDuration::from_millis(1_300));
    let mut writer = producer(&restarted.address(), AckProfile::ManagedMajority);
    writer
        .send(batch("after-restart"))
        .expect("a restarted broker takes leases again and acknowledges");

    // A file written to a running broker is removed once it has acted.
    let partition = qip_streaming::event_fabric::managed::PartitionRef::new(OUTCOMES, 0);
    let follower = instance("zone-b", &catalogue, Some(log.clone()));
    assert_eq!(
        lease_of(&log, &partition),
        Some(("zone-a".to_string(), 1)),
        "premise: the restarted broker leads"
    );
    let live = restarted.root.join("data").join(qip_fabricd::DRAIN_FILE);
    std::fs::write(&live, b"").unwrap();
    let deadline = Instant::now() + StdDuration::from_secs(30);
    while lease_of(&log, &partition) != Some(("zone-b".to_string(), 2)) {
        assert!(
            Instant::now() < deadline,
            "the follower never took the lease the drain file released"
        );
        std::thread::sleep(StdDuration::from_millis(50));
    }
    assert!(
        !live.exists(),
        "the lease thread acted on the DRAIN file and left it to drain the next process too"
    );
    drop(follower);
}

/// A drained broker stays a follower, as `Running::drain` promises: the new
/// leader's commits reach its disk and its health body says it leads
/// nothing. The draining pass used to hand leases back and `continue` past
/// the only code that follows, so the drained broker served a high
/// watermark frozen at the drain and `/health` repeated the pre-drain
/// `leading` count.
///
/// Mutation: delete the `broker.follow_leases()` call in the draining branch
/// of the lease thread — the drained broker never serves offset 3 and the
/// deadline assertion fires. Mutation: in `Broker::follow_leases`, call
/// `acquire` before `catch_up` — the drained broker takes the lease back
/// at once and the successor never leads: the produce loop panics.
#[test]
fn a_drained_broker_keeps_following_and_reports_that_it_leads_nothing() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 18)));
    let log: Arc<MemoryManagedLog> = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let catalogue = catalogue("managed_majority", 3);
    let drained = instance("zone-a", &catalogue, Some(log.clone()));
    let successor = instance("zone-b", &catalogue, Some(log.clone()));

    let mut writer = producer(&drained.address(), AckProfile::ManagedMajority);
    for n in 0..3 {
        writer.send(batch(&format!("fill-{n}"))).unwrap();
    }
    let health_address = drained
        .running
        .as_ref()
        .unwrap()
        .health_address()
        .to_string();
    assert_eq!(
        health_body(&health_address)["leases"]["leading"],
        1,
        "premise: before the drain /health says this broker leads one partition"
    );

    drained.running.as_ref().unwrap().drain().unwrap();
    let mut writer = producer(&successor.address(), AckProfile::ManagedMajority);
    let deadline = Instant::now() + StdDuration::from_secs(30);
    loop {
        match writer.send(batch("fill-3")) {
            Ok(ack) => {
                assert_eq!(ack.base_offset(), 3, "premise: dense across the handover");
                break;
            }
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(StdDuration::from_millis(50));
                writer = producer(&successor.address(), AckProfile::ManagedMajority);
            }
            Err(error) => panic!("the successor never took the released lease: {error}"),
        }
    }

    let deadline = Instant::now() + StdDuration::from_secs(30);
    while served_at(&drained.address(), 3).is_none() {
        assert!(
            Instant::now() < deadline,
            "the drained broker never followed the successor's commit at offset 3"
        );
        std::thread::sleep(StdDuration::from_millis(50));
    }
    assert_eq!(
        served_at(&drained.address(), 3),
        Some(b"fill-3".to_vec()),
        "the drained broker serves the successor's record byte for byte"
    );
    let lease_body = health_body(&health_address)["leases"].clone();
    assert_eq!(
        lease_body["leading"], 0,
        "a drained broker leads nothing: {lease_body}"
    );
    assert_eq!(
        lease_body["following"], 1,
        "a drained broker follows its partition: {lease_body}"
    );
    // Lapse the successor and give the drained broker passes at it: following
    // must not turn into leading.
    successor.running.as_ref().unwrap().drain().unwrap();
    database_time.advance(Duration::from_millis(1_501));
    std::thread::sleep(StdDuration::from_millis(1_800));
    let partition = qip_streaming::event_fabric::managed::PartitionRef::new(OUTCOMES, 0);
    assert_eq!(
        lease_of(&log, &partition),
        Some(("zone-b".to_string(), 2)),
        "following without a lease does not take the lapsed one"
    );
}

/// `name{labels}` in `running`'s registry as a histogram's count, or the
/// gauge's value for a gauge.
fn reading(running: &Running, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    use qip_observability::metrics::MetricValue;
    let wanted: qip_observability::metrics::Labels = labels
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    running
        .metrics()
        .snapshot()
        .series
        .into_iter()
        .find(|s| s.name == name && s.labels == wanted)
        .and_then(|s| match s.value {
            MetricValue::Histogram(h) => Some(h.count as f64),
            MetricValue::Gauge(v) => Some(v),
            _ => None,
        })
}

/// Poll `check` for up to thirty seconds; the archiver pass that evaluates
/// the fabric's objectives runs every twenty milliseconds.
#[track_caller]
fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + StdDuration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(StdDuration::from_millis(20));
    }
}

/// R12-OBS-9146: a P1 produce acknowledged by the managed log moves
/// `qip_event_fabric_quorum_ack_ms{class="p1_outcomes",outcome="committed"}`
/// by one per commit, through the composition root `main` reaches, and the
/// `fabric-publish-quorum-p99` objective (R12-OBS-9160) is then evaluated
/// from it on the broker's own `/metrics` registry. The series carries the
/// class and the outcome and nothing else: never the stream, a key or an
/// offset.
///
/// Mutation: in `start_composed`, seat the broker at the log it was given
/// rather than `timed(log)` - the commit is never timed and the first
/// `eventually` fires.
#[test]
fn a_managed_produce_moves_the_quorum_ack_series_and_the_publish_quorum_objective() {
    let time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 24)));
    let log: Arc<dyn ManagedLog> = Arc::new(MemoryManagedLog::new(time));
    let leader = instance("zone-a", &catalogue("managed_majority", 3), Some(log));
    let running = leader.running.as_ref().unwrap();
    let committed = [("class", "p1_outcomes"), ("outcome", "committed")];
    assert_eq!(
        reading(running, "qip_event_fabric_quorum_ack_ms", &committed),
        None,
        "premise: no commit has been timed before a produce"
    );

    let mut writer = producer(&leader.address(), AckProfile::ManagedMajority);
    for n in 0..2 {
        let ack = writer.send(batch(&format!("fill-{n}"))).unwrap();
        assert!(
            ack.managed_commit_epoch().is_some(),
            "premise: a quorum ack"
        );
    }
    eventually("the two managed commits were never timed", || {
        reading(running, "qip_event_fabric_quorum_ack_ms", &committed) == Some(2.0)
    });
    let labelled: Vec<_> = running
        .metrics()
        .snapshot()
        .series
        .into_iter()
        .filter(|s| s.name == "qip_event_fabric_quorum_ack_ms")
        .map(|s| s.labels.keys().cloned().collect::<Vec<_>>())
        .collect();
    assert!(!labelled.is_empty());
    for keys in labelled {
        assert_eq!(
            keys,
            ["class", "outcome"],
            "a label beyond class and outcome"
        );
    }
    eventually(
        "the publish-quorum objective never evaluated the commits",
        || {
            reading(
                running,
                "qip_slo_observations",
                &[("objective", "fabric-publish-quorum-p99")],
            )
            .is_some_and(|n| n >= 2.0)
        },
    );
}

/// The default path, one broker and no managed log: the archiver pass
/// publishes the fabric's objectives on the broker's registry, broker
/// availability measured from the serving gauge the listener wrote, and
/// publish quorum with zero observations and no budget, because a broker
/// with no managed log has no quorum to time.
///
/// Mutation: in `FabricdTelemetry::archive_lag`, delete the
/// `self.evaluate_objectives()` call - no fabric objective is published
/// and the availability `eventually` fires.
#[test]
fn a_broker_with_no_managed_log_publishes_its_objectives_and_reads_publish_quorum_unmeasured() {
    let local = instance("local", &catalogue("archived", 1), None);
    let running = local.running.as_ref().unwrap();
    eventually("broker availability was never evaluated", || {
        reading(
            running,
            "qip_slo_observations",
            &[("objective", "fabric-broker-availability")],
        )
        .is_some_and(|n| n >= 1.0)
    });
    assert_eq!(
        reading(
            running,
            "qip_slo_observations",
            &[("objective", "fabric-publish-quorum-p99")]
        ),
        Some(0.0),
        "publish quorum is published, and unmeasured"
    );
    assert_eq!(
        reading(
            running,
            "qip_slo_budget_consumed",
            &[("objective", "fabric-publish-quorum-p99")]
        ),
        None,
        "an unmeasured objective publishes no budget"
    );
}
