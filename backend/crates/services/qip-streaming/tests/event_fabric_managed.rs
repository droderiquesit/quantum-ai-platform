//! ADR 0111 §1 at the broker core: P0/P1 acknowledged only after the
//! managed log commits them, leadership by a lease the managed log fences,
//! followers that hold the committed log, and failover with no committed
//! loss — FABRIC-7035, FABRIC-8086, RES-7112.
//!
//! Two brokers sharing one `MemoryManagedLog` are two `qip-fabricd`
//! instances in two zones sharing one Spanner database; the managed log's
//! own clock is the database's time, advanced by hand so a lease expires
//! exactly when a test says it does.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::managed::{
    CommitOutcome, CommittedBatch, Lease, LeaseOutcome, ManagedLog, MemoryManagedLog, PartitionRef,
};

const STREAM: &str = "outcomes";
const TTL_MS: u64 = 3_000;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-managed-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn spec(class: QosClass, ack: AckProfile, rf: u32) -> StreamPolicySpec {
    StreamPolicySpec {
        qos_class: class,
        partition_key: "cell".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::Irreplaceable,
        replication_factor: rf,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: ack,
        byte_quota_per_producer: 1 << 24,
        message_quota_per_producer: 1 << 20,
        lag_limit: 1 << 20,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 24,
    }
}

fn managed_policy() -> StreamPolicy {
    StreamPolicy::new(spec(QosClass::P1Outcomes, AckProfile::ManagedMajority, 3)).unwrap()
}

fn archived_policy() -> StreamPolicy {
    StreamPolicy::new(spec(QosClass::P1Outcomes, AckProfile::Archived, 1)).unwrap()
}

fn database_time() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)))
}

/// A seated broker with the managed stream declared on one partition.
fn seated(
    label: &str,
    log: Arc<dyn ManagedLog>,
    clock: Arc<dyn Clock>,
    instance: &str,
) -> (Broker, PathBuf) {
    let dir = temp_dir(label);
    let broker = Broker::open(&dir, clock.clone())
        .unwrap()
        .with_managed_log(log, instance)
        .unwrap();
    broker
        .declare_stream(
            STREAM,
            1,
            managed_policy(),
            SegmentLogConfig::new(clock).with_archive_required(true),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    (broker, dir)
}

fn batch(producer: &str, epoch: u64, sequence: u64, payload: &str) -> Batch {
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("evt-{producer}-{sequence}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: payload.as_bytes().to_vec(),
        }],
    )
    .unwrap();
    stamp_drain(&mut batch, producer, epoch, sequence);
    batch
}

fn fetched_bytes(broker: &Broker, offset: u64) -> Vec<u8> {
    let response = broker.fetch(STREAM, 0, offset, 1).unwrap();
    qip_core::hash::from_hex(response.batches()).unwrap()
}

fn payload_at(broker: &Broker, offset: u64) -> Vec<u8> {
    match Batch::decode(&fetched_bytes(broker, offset)).unwrap() {
        DecodeOutcome::Complete(batch) => batch.records[0].payload.clone(),
        DecodeOutcome::Torn => panic!("a fetched batch is complete"),
    }
}

fn partition() -> PartitionRef {
    PartitionRef::new(STREAM, 0)
}

/// FABRIC-8086 / ADR 0111 §1: a managed batch is acknowledged only once the
/// managed log holds it, and the acknowledgement says so with the lease
/// epoch; a broker cut off from the managed log acknowledges nothing and
/// appends nothing.
///
/// Mutation: in `Broker::produce_managed`, ignore the commit's result
/// (`let _ = seat.log.commit(..)`) — the cut-off produce is acknowledged and
/// appended, and the `expect_err` and the high-watermark assertion fire.
#[test]
fn a_managed_batch_is_acknowledged_only_after_the_managed_log_commits_it() {
    let time = database_time();
    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let (broker, dir) = seated("ack", log.clone(), time.clone(), "zone-a");

    let report = broker.tend_leases(TTL_MS).unwrap();
    assert_eq!(report.leading.get(&(STREAM.to_string(), 0)), Some(&1));

    let ack = broker
        .produce_to(STREAM, 0, batch("cell-a", 1, 0, "fill-0"))
        .unwrap()
        .unwrap();
    assert_eq!(ack.managed_commit_epoch(), Some(1));
    assert_eq!(log.committed_through(&partition()).unwrap(), 1);
    let row = &log.read(&partition(), 0, 1).unwrap()[0];
    assert_eq!(
        row.bytes,
        fetched_bytes(&broker, 0),
        "the record is the commit"
    );

    log.partition_from_database(broker.managed_holder().unwrap(), true);
    let refused = broker
        .produce_to(STREAM, 0, batch("cell-a", 1, 1, "fill-1"))
        .expect_err("a broker that cannot reach the managed log acknowledges nothing");
    assert!(
        refused.to_string().contains("outcome is unknown"),
        "{refused}"
    );
    assert_eq!(broker.metadata(STREAM, 0).unwrap().high_watermark(), 1);
    assert_eq!(log.committed_through(&partition()).unwrap(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// ADR 0111 §1 and §3 at declaration: an unseated broker refuses a managed
/// stream rather than serving it from one disk, and a seated one refuses a
/// P1 stream left on the local path.
///
/// Mutation: delete the `policy.is_managed() && self.managed.is_none()`
/// refusal in `declare_stream` — the unseated declaration succeeds and the
/// first `expect_err` fires.
#[test]
fn an_unseated_broker_refuses_a_managed_stream_and_a_seated_one_refuses_a_local_p1() {
    let time = database_time();
    let dir = temp_dir("declare");
    let unseated = Broker::open(dir.join("a"), time.clone()).unwrap();
    unseated
        .declare_stream(
            "local",
            1,
            archived_policy(),
            SegmentLogConfig::new(time.clone()),
        )
        .expect("premise: the local path still declares an archived P1 stream");
    let error = unseated
        .declare_stream(
            STREAM,
            1,
            managed_policy(),
            SegmentLogConfig::new(time.clone()),
        )
        .expect_err("managed_majority on one disk");
    assert!(error.to_string().contains("no managed log"), "{error}");

    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let seated = Broker::open(dir.join("b"), time.clone())
        .unwrap()
        .with_managed_log(log, "zone-a")
        .unwrap();
    let error = seated
        .declare_stream("local", 1, archived_policy(), SegmentLogConfig::new(time))
        .expect_err("a P1 stream off the managed path on a seated broker");
    assert!(error.to_string().contains("managed_majority"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

/// RES-7112 / FABRIC-7035: a follower holds the leader's committed log
/// byte for byte, takes the lease when the leader's expires, and continues
/// the log densely with no committed record lost; the deposed leader's next
/// commit is fenced, acknowledges nothing, and leaves its producer table as
/// it was — so no two instances ever commit in one epoch.
///
/// Mutation: in `produce_managed`'s `Fenced` arm, drop `rewind(checkpoint)`
/// — the deposed broker's table keeps the refused sequence and the
/// `next_sequence` assertion fires. Mutation: in `MemoryManagedLog::acquire`,
/// keep the old epoch on takeover — the takeover epoch assertion fires and
/// the deposed commit is not fenced.
#[test]
fn a_follower_holds_the_committed_log_and_takes_over_with_no_committed_loss() {
    let time = database_time();
    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let (leader, dir_a) = seated("leader", log.clone(), time.clone(), "zone-a");
    let (follower, dir_b) = seated("follower", log.clone(), time.clone(), "zone-b");

    assert_eq!(leader.tend_leases(TTL_MS).unwrap().leading.len(), 1);
    for sequence in 0..5 {
        leader
            .produce_to(
                STREAM,
                0,
                batch("cell-a", 1, sequence, &format!("fill-{sequence}")),
            )
            .unwrap()
            .unwrap();
    }
    let report = follower.tend_leases(TTL_MS).unwrap();
    assert_eq!(report.following.len(), 1, "premise: the follower follows");
    for offset in 0..5 {
        assert_eq!(
            fetched_bytes(&follower, offset),
            fetched_bytes(&leader, offset),
            "the follower's log is the leader's at offset {offset}"
        );
    }
    follower
        .produce_to(STREAM, 0, batch("cell-a", 1, 5, "stray"))
        .expect_err("a follower holds no lease and commits nothing");

    // The leader stops renewing; the database's time passes the lease.
    time.advance(Duration::from_millis(i64::try_from(TTL_MS).unwrap() + 1));
    let report = follower.tend_leases(TTL_MS).unwrap();
    assert_eq!(report.leading.get(&(STREAM.to_string(), 0)), Some(&2));

    let before = leader.next_sequence(STREAM, 0, "cell-a").unwrap();
    let fenced = leader
        .produce_to(STREAM, 0, batch("cell-a", 1, 5, "late-from-the-deposed"))
        .expect_err("the deposed leader's commit is fenced");
    assert!(fenced.to_string().contains("superseded"), "{fenced}");
    assert_eq!(leader.next_sequence(STREAM, 0, "cell-a").unwrap(), before);

    let ack = follower
        .produce_to(STREAM, 0, batch("cell-a", 1, 5, "fill-5"))
        .unwrap()
        .unwrap();
    assert_eq!(ack.base_offset(), 5, "dense across the failover");
    assert_eq!(ack.managed_commit_epoch(), Some(2));
    for offset in 0..5 {
        assert_eq!(
            payload_at(&follower, offset),
            format!("fill-{offset}").into_bytes()
        );
    }
    assert_eq!(log.committed_epochs(&partition()), vec![1, 1, 1, 1, 1, 2]);
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}

/// A managed log that commits and then loses the answer once — the
/// unknown-outcome case a network partition produces mid-commit.
#[derive(Debug)]
struct LosesOneAnswer {
    inner: Arc<MemoryManagedLog>,
    lose: AtomicBool,
}

impl ManagedLog for LosesOneAnswer {
    fn acquire(&self, p: &PartitionRef, holder: &str, ttl_ms: u64) -> Result<LeaseOutcome> {
        self.inner.acquire(p, holder, ttl_ms)
    }
    fn commit(&self, lease: &Lease, row: &CommittedBatch) -> Result<CommitOutcome> {
        let outcome = self.inner.commit(lease, row)?;
        if self.lose.swap(false, Ordering::SeqCst) {
            return Err(Error::timeout("the commit's answer was lost"));
        }
        Ok(outcome)
    }
    fn read(&self, p: &PartitionRef, from: u64, limit: u32) -> Result<Vec<CommittedBatch>> {
        self.inner.read(p, from, limit)
    }
    fn committed_through(&self, p: &PartitionRef) -> Result<u64> {
        self.inner.committed_through(p)
    }
    fn commit_offset(&self, group: &str, p: &PartitionRef, offset: u64) -> Result<()> {
        self.inner.commit_offset(group, p, offset)
    }
    fn committed_offset(&self, group: &str, p: &PartitionRef) -> Result<Option<u64>> {
        self.inner.committed_offset(group, p)
    }
}

/// A commit whose answer is lost is not acknowledged, and the producer's
/// retry of the same batch is answered as a duplicate at the offset it
/// really landed at — not committed a second time.
///
/// Mutation: in `produce_managed`'s `Err` arm, drop
/// `cursor.caught_up_epoch = None` — the retry is admitted against a log that
/// never learned of the landed batch, the commit conflicts, and the retry's
/// `unwrap` fires.
#[test]
fn a_lost_commit_answer_is_resolved_by_catch_up_so_the_retry_is_a_duplicate() {
    let time = database_time();
    let inner = Arc::new(MemoryManagedLog::new(time.clone()));
    let log = Arc::new(LosesOneAnswer {
        inner: inner.clone(),
        lose: AtomicBool::new(true),
    });
    let (broker, dir) = seated("lost", log, time.clone(), "zone-a");
    broker.tend_leases(TTL_MS).unwrap();

    broker
        .produce_to(STREAM, 0, batch("cell-a", 1, 0, "fill-0"))
        .expect_err("premise: the answer is lost, so nothing is acknowledged");
    assert_eq!(inner.committed_through(&partition()).unwrap(), 1);

    let retry = broker
        .produce_to(STREAM, 0, batch("cell-a", 1, 0, "fill-0"))
        .unwrap()
        .unwrap();
    assert_eq!(retry.base_offset(), 0);
    assert_eq!(
        inner.committed_through(&partition()).unwrap(),
        1,
        "not twice"
    );
    assert_eq!(payload_at(&broker, 0), b"fill-0".to_vec());
    let _ = std::fs::remove_dir_all(dir);
}

/// RES-7112: a consumer group's checkpoint on a managed partition is held
/// by the managed log, so after a failover the new leader answers the
/// offset the group committed through the old one.
///
/// Mutation: in `Broker::committed_offset`, return the local checkpoint
/// alone — the new leader has none and the assertion fires.
#[test]
fn a_consumer_resumes_from_its_committed_offset_on_whichever_instance_leads() {
    let time = database_time();
    let log = Arc::new(MemoryManagedLog::new(time.clone()));
    let (leader, dir_a) = seated("ckpt-a", log.clone(), time.clone(), "zone-a");
    let (follower, dir_b) = seated("ckpt-b", log.clone(), time.clone(), "zone-b");
    leader.tend_leases(TTL_MS).unwrap();
    for sequence in 0..3 {
        leader
            .produce_to(STREAM, 0, batch("cell-a", 1, sequence, "x"))
            .unwrap()
            .unwrap();
    }
    leader.commit_offset("ledger", STREAM, 0, 1).unwrap();
    assert_eq!(
        follower.committed_offset("ledger", STREAM, 0).unwrap(),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(dir_a);
    let _ = std::fs::remove_dir_all(dir_b);
}
