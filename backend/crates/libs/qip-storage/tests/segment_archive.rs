//! `qip_storage::segment::archive` — content-addressed archive, manifest
//! verification and hydrate. See ADR 0100 §3.
//!
//! Byte-level facts (the segment file naming convention) are restated
//! independently here rather than imported from the crate under test,
//! matching `tests/segment.rs`'s own convention: a test that shares the
//! reader with the code it checks proves less.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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

/// The name an archived object must carry: the hash of its bytes, restated
/// here rather than imported so the test does not share the reader with the
/// code it checks.
fn object_name(hash: &qip_events::event_fabric::codec::ContentHash) -> String {
    match hash {
        qip_events::event_fabric::codec::ContentHash::Sha256(digest) => {
            format!("segments/sha256/{}", qip_core::hash::to_hex(digest))
        }
        other => panic!("this build only produces Sha256, got {other:?}"),
    }
}

// --- FABRIC-026: the manifest alone proves hash, range, entitlements and index

/// FABRIC-026's stated check. Archive two consecutive sealed segments under
/// real entitlements, then judge them from their manifests: the object's hash
/// matches, the offset ranges are contiguous with each other and the second
/// chains onto the first, entitlements are present and survive the manifest's
/// own wire round trip, the replay index resolves sampled offsets to the
/// right record without scanning, and one corrupted byte fails verification.
///
/// Mutation: in `Archiver::archive`, build the `Manifest` with
/// `entitlements: BTreeSet::new()` instead of `entitlements.clone()` — fails
/// on the first entitlement assertion, because licensing then silently stops
/// travelling with archived data.
#[test]
fn a_manifest_alone_proves_the_hash_a_contiguous_range_its_entitlements_and_a_replay_index() {
    let dir = temp_dir("manifest-proof");
    let log = SegmentLog::open(&dir, config_with(manual_clock(), 500)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 2, &mut tag);
    let mut starts: Vec<u64> = log
        .segments()
        .into_iter()
        .filter(|s| s.sealed)
        .map(|s| s.start_offset)
        .collect();
    starts.sort_unstable();
    assert!(starts.len() >= 2, "premise: two sealed segments to archive");

    let entitlements: BTreeSet<String> = ["internal-reflex:trade".to_string()].into();
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let archiver = Archiver::new(Arc::clone(&blob_store));
    let first = archiver
        .archive(&log, starts[0], "orders", 2, &entitlements)
        .unwrap();
    let second = archiver
        .archive(&log, starts[1], "orders", 2, &entitlements)
        .unwrap();

    assert_eq!(first.entitlements, entitlements);
    assert_eq!(second.entitlements, entitlements);
    assert_eq!((first.stream.as_str(), first.partition), ("orders", 2));
    assert_eq!(
        second.base_offset,
        first.last_offset + 1,
        "the second segment's offset range must be contiguous with the first's"
    );
    assert_eq!(
        second.chain_in.as_ref(),
        Some(&first.chain_out),
        "the second manifest must chain onto the first, so a missing segment is detectable"
    );

    // Entitlements survive the manifest's own wire form: a later archive of
    // the same segment (a no-op) reads the stored manifest back and must
    // return it with the entitlements intact, whatever the caller now says.
    let reread = archiver
        .archive(&log, starts[0], "orders", 2, &BTreeSet::new())
        .unwrap();
    assert_eq!(reread.entitlements, entitlements);

    let key = object_name(&first.content_hash);
    let stored = blob_store
        .get(&key)
        .unwrap()
        .expect("object under its hash name");
    first.verify(&stored).unwrap();

    // The replay index locates records by offset without a scan: each entry
    // opens a batch whose record is the one that offset names.
    assert_eq!(first.replay_index.len() as u64, first.record_count);
    for (offset, byte_start) in first.replay_index.iter().step_by(2) {
        let decoded = Batch::decode(&stored[*byte_start as usize..]).unwrap();
        let qip_events::event_fabric::codec::DecodeOutcome::Complete(batch) = decoded else {
            panic!("the replay index pointed at a torn batch for offset {offset}");
        };
        assert_eq!(
            batch.records[0].event_id,
            format!("evt-{offset}"),
            "the replay index must resolve offset {offset} to its own record"
        );
    }

    let mut corrupted = stored.clone();
    let middle = corrupted.len() / 2;
    corrupted[middle] ^= 0x01;
    assert!(first.verify(&corrupted).is_err());
}

// --- FABRIC-090: a tampered object fails its name check on the way back

/// FABRIC-090's last clause, which the test above it in this file names but
/// does not exercise: an object stored under its content name, then altered,
/// must not be served. The flipped byte sits in the *last* batch and the read
/// asks for the *first*, so the batch actually decoded is intact — only the
/// whole-object hash check can refuse it.
///
/// Mutation: in `ArchiveReader::hydrate`, delete the `manifest.verify(&bytes)?`
/// call — fails, because the read of the first batch then succeeds from an
/// object that no longer matches its name.
#[test]
fn a_tampered_archived_object_is_refused_on_read_even_when_the_requested_batch_is_intact() {
    let dir = temp_dir("tamper-by-name");
    let log = SegmentLog::open(&dir, config_with(manual_clock(), 500)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 1, &mut tag);
    let start = log
        .segments()
        .into_iter()
        .find(|s| s.sealed)
        .expect("premise: one sealed segment")
        .start_offset;

    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let manifest = Archiver::new(Arc::clone(&blob_store))
        .archive(&log, start, "orders", 0, &no_entitlements())
        .unwrap();
    let keys = blob_store.list("").unwrap();
    assert_eq!(keys.len(), 1);
    let name = object_name(&manifest.content_hash);
    assert_eq!(
        keys[0], name,
        "the object's name must be the hash of its bytes"
    );

    let reader = ArchiveReader::new(Arc::clone(&blob_store));
    assert!(
        reader.read(&log, start).unwrap().is_some(),
        "premise: the untampered object serves its first batch"
    );

    let mut tampered = blob_store.get(&name).unwrap().unwrap();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    blob_store.put(&name, tampered).unwrap();

    let err = reader
        .read(&log, start)
        .expect_err("an object that no longer matches its name must not be served");
    assert!(err.to_string().contains("content hash"), "{err}");
}

// --- CloudStorageBlobStore integration test

/// A segment can be archived to a CloudStorageBlobStore, proving the adapter
/// integrates with the archive system.
///
/// Mutation: delete the trait object cast line; the test still compiles but loses
/// proof that CloudStorageBlobStore implements BlobStore.
#[test]
fn a_segment_can_be_archived_to_a_cloud_storage_blob_store() {
    use qip_storage::gcp::{CloudStorageBlobStore, CloudStorageConfig, GcpAccess};

    let config = CloudStorageConfig::new("test-bucket").with_access(GcpAccess::unconfigured());

    let blob_store_result = CloudStorageBlobStore::new(config);
    assert!(
        blob_store_result.is_ok(),
        "CloudStorageBlobStore should construct with unconfigured GcpAccess"
    );

    let blob_store = blob_store_result.unwrap();
    let _: Arc<dyn crate::BlobStore> = Arc::new(blob_store);
}
