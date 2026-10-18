//! RES-9170 follow-up: a disk rebuilt from the archive must re-archive under
//! the same segment boundaries the archive already holds, or the manifest
//! index answers archived offsets as absent.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::MemoryBlobStore;
use qip_storage::segment::archive::{ArchiveReader, Archiver};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;

const STREAM: &str = "outcomes";
const PER_SEGMENT: u64 = 10;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-rebuild-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn policy() -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "cell".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::Irreplaceable,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: AckProfile::Archived,
        byte_quota_per_producer: 1 << 24,
        message_quota_per_producer: 1 << 20,
        lag_limit: 1 << 20,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 24,
    })
    .unwrap()
}

/// A broker on a fresh disk whose segments never roll by size, so every seal
/// in this file is an age seal — the case the log's own rolling cannot
/// reproduce on a rebuilt disk.
fn broker_on(label: &str, clock: &Arc<ManualClock>, store: &Arc<MemoryBlobStore>) -> Broker {
    let as_clock: Arc<dyn Clock> = clock.clone();
    let broker = Broker::open(temp_dir(label), as_clock.clone())
        .unwrap()
        .with_archive_reader(store.clone());
    broker
        .declare_stream(
            STREAM,
            1,
            policy(),
            SegmentLogConfig::new(as_clock)
                .with_archive_required(true)
                .with_roll_after_bytes(1 << 30),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    broker
}

fn produce(broker: &Broker, sequence: u64) {
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("evt-{sequence}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: vec![u8::try_from(sequence).unwrap(); 256],
        }],
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-a", 1, sequence);
    broker.produce_to(STREAM, 0, batch).unwrap().unwrap();
}

/// Produce `count` batches from `first`, then let them age past the stream's
/// seal age, seal them and archive what sealed.
fn produce_aged_segment(
    broker: &Broker,
    clock: &ManualClock,
    archiver: &Archiver,
    first: u64,
    count: u64,
) {
    for sequence in first..first + count {
        produce(broker, sequence);
    }
    clock.advance(Duration::from_secs(61));
    assert_eq!(
        broker.seal_aged().unwrap(),
        1,
        "premise: the age seal fired"
    );
    broker.archive_sealed(archiver).unwrap();
}

/// The exact failure the review of #236 reproduced: segments sealed by age,
/// the disk lost, a rebuild, new records, a re-archive. Before the rebuild
/// sealed at the archived boundaries it replayed 0..40 into one segment that
/// the next seal re-archived as [0..60) under key 0, leaving the old keys 10,
/// 20 and 30 beside it, so offset 45 read through key 30's old [30..40)
/// manifest and came back absent — to a fetch once retention had deleted the
/// local copy, and to the next rebuild, which stopped at 40 and handed
/// offsets 40..60 to new records.
///
/// Mutation: delete the `state.log.seal_active()?` call in
/// `Broker::rebuild_from_archive` — offset 40 reads back absent and the
/// first sweep fails; no other line of this test is needed to see it.
#[test]
fn a_rebuilt_disk_reseals_where_the_archive_sealed_so_new_offsets_are_never_read_back_absent() {
    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 18)));
    let store = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(store.clone()).with_manifest_index();

    let original = broker_on("original", &clock, &store);
    for segment in 0..4 {
        produce_aged_segment(
            &original,
            &clock,
            &archiver,
            segment * PER_SEGMENT,
            PER_SEGMENT,
        );
    }
    assert_eq!(
        original.metadata(STREAM, 0).unwrap().archived_through(),
        4 * PER_SEGMENT,
        "premise: four age-sealed segments are archived"
    );

    // The disk is lost; the archive is not.
    let rebuilt = broker_on("rebuilt", &clock, &store);
    let report = rebuilt.rebuild_from_archive().unwrap();
    assert_eq!(
        report.replayed.get(&(STREAM.to_string(), 0)),
        Some(&(4 * PER_SEGMENT)),
        "premise: the rebuild replayed all forty offsets"
    );
    produce_aged_segment(
        &rebuilt,
        &clock,
        &archiver,
        4 * PER_SEGMENT,
        2 * PER_SEGMENT,
    );

    // A reader per offset: one reader caches the last segment it verified,
    // and a cached [0..60) manifest answers every later offset without the
    // listing that picks the stale key.
    for offset in 0..6 * PER_SEGMENT {
        let batch = ArchiveReader::new(store.clone())
            .read_indexed(STREAM, 0, offset)
            .unwrap()
            .unwrap_or_else(|| panic!("archived offset {offset} reads back absent from the index"));
        assert_eq!(batch.base_offset, offset);
    }

    // A fetch served after retention deleted the local copy goes through the
    // same index.
    assert!(
        rebuilt.retain_hot(1).unwrap() > 0,
        "premise: local copies were deleted"
    );
    assert!(
        !rebuilt
            .fetch(STREAM, 0, 45, 1)
            .unwrap()
            .batches()
            .to_string()
            .is_empty(),
        "offset 45 is archived, so a fetch after retention must still serve it"
    );

    // A second loss rebuilds the whole sixty, not forty.
    let again = broker_on("again", &clock, &store);
    let second = again.rebuild_from_archive().unwrap();
    assert_eq!(
        second.replayed.get(&(STREAM.to_string(), 0)),
        Some(&(6 * PER_SEGMENT)),
        "the second rebuild stopped early and would hand archived offsets to new records"
    );
}
