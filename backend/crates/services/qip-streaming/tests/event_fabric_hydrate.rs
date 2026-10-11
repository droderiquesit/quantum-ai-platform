//! FABRIC-8079: closed segments are archived with a manifest, and a fetch of
//! history whose local copy has been retained away is hydrated from the
//! archive — byte-identically, and never from an object that fails its
//! manifest.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_streaming::event_fabric::broker::Broker;

const STREAM: &str = "outcomes";
const BATCHES: u64 = 12;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-hydrate-{label}-{}-{unique}",
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

/// A broker reading its archive back from `store`, with `BATCHES` batches of
/// about a kilobyte produced to one partition rolled at the log's minimum
/// segment size, so most of them sit in sealed segments.
fn filled(label: &str, store: Arc<MemoryBlobStore>) -> (Broker, PathBuf) {
    let dir = temp_dir(label);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let broker = Broker::open(&dir, clock.clone())
        .unwrap()
        .with_archive_reader(store);
    broker
        .declare_stream(
            STREAM,
            1,
            policy(),
            SegmentLogConfig::new(clock)
                .with_archive_required(true)
                .with_roll_after_bytes(1),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    for sequence in 0..BATCHES {
        let mut batch = Batch::new(
            MessageType::Data,
            1,
            1,
            PayloadCodec::CanonicalJson,
            vec![Record {
                event_id: format!("evt-{sequence}"),
                trace_id: None,
                source_timestamp_ns: 1,
                payload: vec![u8::try_from(sequence).unwrap(); 1024],
            }],
        )
        .unwrap();
        stamp_drain(&mut batch, "cell-a", 1, sequence);
        broker.produce_to(STREAM, 0, batch).unwrap().unwrap();
    }
    (broker, dir)
}

fn fetch_one(broker: &Broker, offset: u64) -> qip_core::Result<String> {
    broker
        .fetch(STREAM, 0, offset, 1)
        .map(|response| response.batches().to_string())
}

/// The local copy of archived, indexed history is retained away and a fetch
/// of it is hydrated from the archive byte-identically; an archive object
/// that fails its manifest is refused rather than served.
///
/// Mutation: in `Broker::read_batch`, return the local `None` without
/// consulting the archive — the hydrated fetches come back empty and the
/// byte-identity assertion fires. Mutation: in
/// `ArchiveReader::read_indexed`, drop `manifest.verify(&bytes)?` — the
/// tampered object is served and the `is_err` assertion fires.
#[test]
fn retained_history_is_hydrated_from_the_archive_byte_for_byte_and_a_tampered_object_is_refused() {
    let store = Arc::new(MemoryBlobStore::new());
    let (broker, dir) = filled("hydrate", store.clone());
    let baseline: Vec<String> = (0..BATCHES)
        .map(|offset| fetch_one(&broker, offset).unwrap())
        .collect();
    assert!(
        baseline.iter().all(|hex| !hex.is_empty()),
        "premise: all local"
    );

    let archived = broker
        .archive_sealed(&Archiver::new(store.clone()).with_manifest_index())
        .unwrap();
    assert!(
        archived >= 2,
        "premise: several sealed segments archived, got {archived}"
    );
    let deleted = broker.retain_hot(1).unwrap();
    assert!(
        deleted >= 2,
        "premise: local copies were deleted, got {deleted}"
    );

    let objects: Vec<String> = store.list("segments/sha256/").unwrap();
    assert!(!objects.is_empty(), "premise: archived objects exist");
    let originals: Vec<(String, Vec<u8>)> = objects
        .iter()
        .map(|key| (key.clone(), store.get(key).unwrap().unwrap()))
        .collect();
    for (key, bytes) in &originals {
        let mut tampered = bytes.clone();
        let middle = tampered.len() / 2;
        tampered[middle] ^= 0xFF;
        store.put(key, tampered).unwrap();
    }
    // Offset 0 sits in the first sealed segment, which retention deleted, so
    // the only copy left to serve it is the tampered one.
    assert!(
        fetch_one(&broker, 0).is_err(),
        "a tampered archive object is refused, not served"
    );
    for (key, bytes) in originals {
        store.put(&key, bytes).unwrap();
    }
    for (offset, expected) in baseline.iter().enumerate() {
        assert_eq!(
            &fetch_one(&broker, offset as u64).unwrap(),
            expected,
            "offset {offset} hydrates to the bytes the broker served before retention"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A segment archived without an index entry is never retained away: with
/// no index nothing could hydrate it, so deleting it would delete history.
///
/// Mutation: in `Broker::retain_hot`, drop the `!reader.is_indexed(..)`
/// condition — the unindexed segments are deleted and both assertions fire.
#[test]
fn a_segment_archived_without_an_index_is_kept_on_disk() {
    let store = Arc::new(MemoryBlobStore::new());
    let (broker, dir) = filled("unindexed", store.clone());
    let archived = broker
        .archive_sealed(&Archiver::new(store.clone()))
        .unwrap();
    assert!(archived >= 2, "premise: segments archived, got {archived}");
    assert_eq!(broker.retain_hot(1).unwrap(), 0);
    assert!(!fetch_one(&broker, 0).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
