//! Hot replay indexes are separate from cold archive (blueprint TICK-039).
//!
//! The replay index lives in the archived segment's [`Manifest`] — metadata
//! the [`SegmentLog`]'s manifest store holds — while the segment's bytes live
//! in a [`BlobStore`]. A replay resolves an offset to a byte range through the
//! index and decodes only that range.
//!
//! The previous version of this file built its own `Vec<(u64, u64)>`, binary
//! searched it, and asserted the arithmetic of the vector it had just built.
//! It never touched `qip_storage`, so it passed with `Manifest::replay_index`
//! deleted, with `Archiver::archive` writing every byte start as zero, and
//! with `ArchiveReader` ignoring the index entirely. These tests drive the
//! real archiver and the real reader.
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, MessageType, PayloadCodec, Record};
use qip_storage::segment::archive::{ArchiveReader, Archiver, Manifest};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-acceptance-hot-replay-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open_log(dir: &std::path::Path) -> SegmentLog {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)));
    // A roll threshold that seals a segment holding several records, so the
    // index has a middle to find.
    let config = SegmentLogConfig::new(clock)
        .with_roll_after_bytes(4_000)
        .with_archive_required(true);
    SegmentLog::open(dir, config).unwrap()
}

fn tagged_batch(tag: u64) -> Batch {
    let record = Record {
        event_id: format!("evt-{tag}"),
        trace_id: None,
        source_timestamp_ns: tag as i64,
        payload: vec![0x37u8; 400],
    };
    Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap()
}

/// Append until one segment seals; return its start offset. Every batch's
/// event id is `evt-<offset>` because the log starts at zero and each batch
/// holds one record.
fn seal_one_segment(log: &SegmentLog) -> u64 {
    let mut tag = 0u64;
    while !log.segments().iter().any(|s| s.sealed) {
        let offset = log.append(&tagged_batch(tag)).unwrap();
        assert_eq!(offset, tag, "premise: offsets and tags advance together");
        tag += 1;
        assert!(tag < 1_000, "the roll threshold must seal a segment");
    }
    log.segments()
        .into_iter()
        .find(|s| s.sealed)
        .unwrap()
        .start_offset
}

fn archive(dir: &std::path::Path) -> (SegmentLog, Arc<dyn BlobStore>, Manifest, Vec<u8>) {
    let log = open_log(dir);
    let start = seal_one_segment(&log);
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let manifest = Archiver::new(Arc::clone(&store))
        .archive(&log, start, "market.tick", 0, &BTreeSet::new())
        .unwrap();
    let keys = store.list("").unwrap();
    assert_eq!(keys.len(), 1, "premise: exactly one object was archived");
    let bytes = store.get(&keys[0]).unwrap().unwrap();
    (log, store, manifest, bytes)
}

/// The index the archiver wrote — not one the test built — has one entry per
/// record, in offset order, and each entry's byte start, cut against the next,
/// decodes to exactly the record at that offset. That is "direct access": the
/// manifest alone says where record *k* is.
///
/// Mutation: in `Archiver::archive`, map each extent to `(start_offset + i, 0)`
/// instead of `extent.start` — every entry then points at the first batch and
/// the decode of a middle record yields `evt-0`.
#[test]
fn the_archived_manifest_index_locates_every_record_by_offset_without_scanning_the_object() {
    let dir = temp_dir("index");
    let (_log, _store, manifest, bytes) = archive(&dir);

    // Premise: a segment with a middle, so "found the first" is not "found it".
    assert!(
        manifest.record_count >= 3,
        "premise: the sealed segment holds several records, got {}",
        manifest.record_count
    );
    assert_eq!(manifest.replay_index.len() as u64, manifest.record_count);
    assert_eq!(manifest.bytes, bytes.len() as u64);

    for (k, (offset, byte_start)) in manifest.replay_index.iter().enumerate() {
        assert_eq!(
            *offset,
            manifest.base_offset + k as u64,
            "ascending, gap-free"
        );
        let end = manifest
            .replay_index
            .get(k + 1)
            .map(|(_, next)| *next as usize)
            .unwrap_or(bytes.len());
        assert!(
            (*byte_start as usize) < end,
            "entry {k} names a non-empty range"
        );
        let batch = match Batch::decode(&bytes[*byte_start as usize..end]).unwrap() {
            DecodeOutcome::Complete(batch) => batch,
            DecodeOutcome::Torn => panic!("entry {k}: the indexed range is not one whole batch"),
        };
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].event_id, format!("evt-{offset}"));
    }
}

/// A replay of an archived segment reads through the manifest's index, from
/// the blob store, with the hot segment file gone. The middle offset is the
/// discriminating one: a reader that ignored the index would serve the first
/// batch, or fail without the hot file.
///
/// Mutation: in `ArchiveReader::hydrate`, replace
/// `let start = manifest.replay_index[index].1 as usize;` with
/// `let start = 0;` — the middle read then decodes `evt-0` (or a torn range)
/// and the event-id assertion fails.
#[test]
fn a_replay_of_an_archived_middle_record_is_served_through_the_index_with_the_hot_file_gone() {
    let dir = temp_dir("reader");
    let (log, store, manifest, _bytes) = archive(&dir);
    assert!(
        manifest.record_count >= 3,
        "premise: a segment with a middle"
    );

    let segment_file = dir.join(format!("segment.{:020}", manifest.base_offset));
    assert!(segment_file.exists(), "premise: the hot file existed");
    std::fs::remove_file(&segment_file).unwrap();

    let reader = ArchiveReader::new(Arc::clone(&store));
    let middle = manifest.base_offset + manifest.record_count / 2;
    for offset in [manifest.base_offset, middle, manifest.last_offset] {
        let batch = reader
            .read(&log, offset)
            .unwrap_or_else(|e| panic!("offset {offset}: {e}"))
            .unwrap_or_else(|| panic!("offset {offset}: must be readable from the archive"));
        assert_eq!(batch.records[0].event_id, format!("evt-{offset}"));
    }
}

/// The index is bounded by the segment, not by the stream: one entry per
/// record the seal declares. An index that grew with the archive (say, one
/// that accumulated across segments) would make the "hot" half unbounded.
///
/// Mutation: in `Archiver::archive`, build `replay_index` from
/// `scan.extents.iter().chain(scan.extents.iter())` — the length assertion
/// fails at twice the record count.
#[test]
fn the_replay_index_holds_exactly_one_entry_per_record_the_seal_declares() {
    let dir = temp_dir("bound");
    let (_log, _store, manifest, _bytes) = archive(&dir);
    assert!(manifest.record_count > 0, "premise: a non-empty segment");
    assert_eq!(manifest.replay_index.len() as u64, manifest.record_count);
    assert_eq!(
        manifest.last_offset - manifest.base_offset + 1,
        manifest.record_count
    );
}
