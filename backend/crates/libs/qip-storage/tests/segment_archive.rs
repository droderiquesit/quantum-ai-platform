//! `qip_storage::segment::archive` — content-addressed archive, manifest
//! verification and hydrate. See ADR 0100 §3.
//!
//! Byte-level facts (the segment file naming convention) are restated
//! independently here rather than imported from the crate under test,
//! matching `tests/segment.rs`'s own convention: a test that shares the
//! reader with the code it checks proves less.

use qip_core::error::Result;
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_storage::segment::archive::{ArchiveReader, Archiver};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// --- fixtures ------------------------------------------------------------

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-segment-archive-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manual_clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)))
}

fn config_with(clock: Arc<dyn Clock>, roll_after_bytes: u64) -> SegmentLogConfig {
    SegmentLogConfig::new(clock)
        .with_roll_after_bytes(roll_after_bytes)
        .with_archive_required(true)
}

/// Build a one-record batch. `tag` only feeds the event id and timestamp, and
/// nothing here reads it back, so callers are free to use it as a counter.
fn tagged_batch(tag: u64, payload: &[u8]) -> Batch {
    let record = Record {
        event_id: format!("evt-{tag}"),
        trace_id: None,
        source_timestamp_ns: tag as i64,
        payload: payload.to_vec(),
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

fn segment_file_path(dir: &std::path::Path, start_offset: u64) -> PathBuf {
    dir.join(format!("segment.{start_offset:020}"))
}

/// Append batches until at least `want` segments have been sealed, so a
/// test's roll/seal coverage does not depend on hand-tuned batch counts
/// matching a hand-tuned roll threshold. Returns every encoded batch
/// appended, in offset order, so a caller can check the original bytes
/// byte-for-byte later.
fn append_until_sealed_count(log: &SegmentLog, want: usize, next_tag: &mut u64) -> Vec<Vec<u8>> {
    let mut encoded = Vec::new();
    let mut guard = 0;
    while log.segments().iter().filter(|s| s.sealed).count() < want {
        let batch = tagged_batch(*next_tag, &[0x37u8; 400]);
        encoded.push(batch.encode().unwrap());
        log.append(&batch).unwrap();
        *next_tag += 1;
        guard += 1;
        assert!(
            guard < 10_000,
            "the roll threshold in this test must be small enough to reach {want} sealed \
             segments in a bounded number of appends"
        );
    }
    encoded
}

fn no_entitlements() -> BTreeSet<String> {
    BTreeSet::new()
}

// --- a_sealed_segment_verifies_from_its_manifest_alone_and_one_flipped_byte_fails_it

#[test]
fn a_sealed_segment_verifies_from_its_manifest_alone_and_one_flipped_byte_fails_it() {
    let dir = temp_dir("verify");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock, 500)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 1, &mut tag);

    let sealed_start = log
        .segments()
        .into_iter()
        .find(|s| s.sealed)
        .expect("the test's premise: at least one segment is sealed before archiving")
        .start_offset;

    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&blob_store));
    let manifest = archiver
        .archive(&log, sealed_start, "orders", 3, &no_entitlements())
        .unwrap();

    assert!(
        manifest.record_count > 0 && manifest.bytes > 0,
        "the test's premise: a real, non-empty segment was archived"
    );

    let keys = blob_store.list("").unwrap();
    assert_eq!(
        keys.len(),
        1,
        "the test's premise: exactly one object was archived"
    );
    let stored = blob_store.get(&keys[0]).unwrap().unwrap();

    // Verifiable from the manifest alone: no `SegmentLog`, no directory, just
    // the manifest this call returned and the bytes the archive holds.
    manifest
        .verify(&stored)
        .expect("the untouched archived object must verify against its own manifest");

    let mut flipped = stored.clone();
    let target = flipped.len() / 2;
    flipped[target] ^= 0xFF;
    assert_ne!(
        flipped[target], stored[target],
        "the test's premise: the byte actually flips"
    );
    assert_eq!(
        flipped.len(),
        stored.len(),
        "the test's premise: the flip changes content, not length, so a length-only check could \
         not catch it"
    );
    let error = manifest
        .verify(&flipped)
        .expect_err("a single flipped byte, anywhere in the object, must fail verification");
    let message = error.to_string();
    assert!(
        message.contains("content hash"),
        "the refusal must name the content hash, not merely disagree silently: {message}"
    );
}

// --- re_archiving_is_a_no_op_and_an_object_whose_name_is_not_its_hash_fails_verification

/// Delegates every call to an inner store but counts `put` calls, so this
/// test can prove re-archiving an already-archived segment never re-uploads
/// it.
#[derive(Debug)]
struct CountingBlobStore {
    inner: MemoryBlobStore,
    puts: AtomicU64,
}

impl CountingBlobStore {
    fn new() -> Self {
        Self {
            inner: MemoryBlobStore::new(),
            puts: AtomicU64::new(0),
        }
    }

    fn put_count(&self) -> u64 {
        self.puts.load(Ordering::SeqCst)
    }
}

impl BlobStore for CountingBlobStore {
    fn put(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.puts.fetch_add(1, Ordering::SeqCst);
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

#[test]
fn re_archiving_is_a_no_op_and_an_object_whose_name_is_not_its_hash_fails_verification() {
    let dir = temp_dir("no-op");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock, 500)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 1, &mut tag);
    let sealed_start = log
        .segments()
        .into_iter()
        .find(|s| s.sealed)
        .expect("the test's premise: at least one segment is sealed")
        .start_offset;
    let segment_bytes = std::fs::read(segment_file_path(&dir, sealed_start)).unwrap();

    let store = Arc::new(CountingBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&store) as Arc<dyn BlobStore>);

    let first = archiver
        .archive(&log, sealed_start, "orders", 0, &no_entitlements())
        .unwrap();
    assert_eq!(
        store.put_count(),
        1,
        "the test's premise: archiving once uploads exactly once"
    );

    // The object is content-addressed: its key must be derived from the
    // SHA-256 of the segment's own bytes, never from its offset. If it were
    // named by offset instead, this is exactly the property that would stop
    // holding (this test's own named mutation).
    let expected_hash = qip_core::hash::sha256_hex(&segment_bytes);
    let keys = store.inner.list("").unwrap();
    assert_eq!(keys.len(), 1, "the test's premise: one object was stored");
    assert!(
        keys[0].contains(&expected_hash),
        "the archived object must be named by the SHA-256 of its own bytes, not by its offset: \
         got key {:?}, expected it to contain {expected_hash}",
        keys[0]
    );

    // Re-archiving is a no-op: no second upload, and the same manifest comes
    // back.
    let second = archiver
        .archive(&log, sealed_start, "orders", 0, &no_entitlements())
        .unwrap();
    assert_eq!(
        store.put_count(),
        1,
        "re-archiving an already-archived segment must never re-upload it"
    );
    assert_eq!(
        first, second,
        "re-archiving an already-archived segment must return the same manifest"
    );
}

// --- archived_through_advances_only_after_a_verified_upload

/// A store whose `put` reports success but whose `get` always answers
/// `None` — the shape a device that acknowledges a write it never actually
/// committed would take.
#[derive(Debug, Default)]
struct UploadNeverReadableBlobStore;

impl BlobStore for UploadNeverReadableBlobStore {
    fn put(&self, _key: &str, _bytes: Vec<u8>) -> Result<()> {
        Ok(())
    }

    fn get(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn delete(&self, _key: &str) -> Result<bool> {
        Ok(false)
    }

    fn list(&self, _prefix: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[test]
fn archived_through_advances_only_after_a_verified_upload() {
    let dir = temp_dir("verified-upload");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock, 500)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 1, &mut tag);
    let sealed_start = log
        .segments()
        .into_iter()
        .find(|s| s.sealed)
        .expect("the test's premise: at least one segment is sealed")
        .start_offset;

    assert!(
        !log.is_archived(sealed_start).unwrap(),
        "the test's premise: the segment starts out unarchived"
    );

    let archiver = Archiver::new(Arc::new(UploadNeverReadableBlobStore));
    let result = archiver.archive(&log, sealed_start, "orders", 0, &no_entitlements());

    assert!(
        result.is_err(),
        "archiving must fail when the upload cannot be read back for verification"
    );
    assert!(
        !log.is_archived(sealed_start).unwrap(),
        "archived_through must not advance on an unverified upload"
    );
}

// --- reading_from_offset_zero_across_archive_and_hot_log_equals_the_original_byte_for_byte

#[test]
fn reading_from_offset_zero_across_archive_and_hot_log_equals_the_original_byte_for_byte() {
    let dir = temp_dir("hydrate");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock, 500)).unwrap();
    let mut tag = 0u64;
    let mut encoded = append_until_sealed_count(&log, 2, &mut tag);
    // One more append so there is a genuinely active (unsealed) segment on
    // top of the two sealed ones, exercising the hot-log fallback too.
    let extra = tagged_batch(tag, &[0x99u8; 40]);
    encoded.push(extra.encode().unwrap());
    log.append(&extra).unwrap();

    let sealed_starts: Vec<u64> = {
        let mut starts: Vec<u64> = log
            .segments()
            .into_iter()
            .filter(|s| s.sealed)
            .map(|s| s.start_offset)
            .collect();
        starts.sort_unstable();
        starts
    };
    assert!(
        sealed_starts.len() >= 2,
        "the test's premise: at least two segments are sealed before archiving"
    );
    let active_exists = log.segments().into_iter().any(|s| !s.sealed);
    assert!(
        active_exists,
        "the test's premise: an active (unsealed) segment exists on top of the sealed ones"
    );

    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&blob_store));
    let archived_start = sealed_starts[0];
    let unarchived_start = sealed_starts[1];
    archiver
        .archive(&log, archived_start, "orders", 0, &no_entitlements())
        .unwrap();
    assert!(log.is_archived(archived_start).unwrap());
    assert!(
        !log.is_archived(unarchived_start).unwrap(),
        "the test's premise: the second sealed segment is deliberately left unarchived, so \
         reading it must fall back to the hot log"
    );

    // Remove the archived segment's own file directly (bypassing
    // `retain_before`, which would also forget the segment's metadata this
    // reader depends on — see the module documentation's stated limitation).
    // What remains on disk after this can only answer for the unarchived
    // segment and the active one; the deleted range can only be answered
    // from the archive, so a correct read of it *proves* the archive path
    // was used rather than merely being consistent with it.
    std::fs::remove_file(segment_file_path(&dir, archived_start)).unwrap();

    let reader = ArchiveReader::new(Arc::clone(&blob_store));
    let high_water = log.high_water();
    assert_eq!(
        high_water,
        encoded.len() as u64,
        "the test's premise: every batch this test appended was actually acknowledged"
    );

    for offset in 0..high_water {
        let batch = reader
            .read(&log, offset)
            .unwrap_or_else(|e| panic!("offset {offset}: read failed: {e}"))
            .unwrap_or_else(|| panic!("offset {offset}: must still be readable"));
        let round_tripped = batch.encode().unwrap();
        assert_eq!(
            round_tripped, encoded[offset as usize],
            "offset {offset}: the archive-then-hot read must equal the original byte stream, \
             byte for byte"
        );
    }
}
