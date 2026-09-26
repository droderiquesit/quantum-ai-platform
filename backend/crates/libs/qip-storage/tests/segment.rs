//! `qip_storage::segment` — append, fsync-before-ack, roll, seal, recover,
//! retain.
//!
//! Byte-level facts (the segment file's fixed header length, and the
//! manifest's own path convention) are restated independently here rather
//! than imported from the crate under test, matching this suite's existing
//! convention in `tests/engine.rs`'s `frame_extents`: a test that shares the
//! reader with the code it checks proves less.

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_storage::segment::log::{Seal, SegmentLog, SegmentLogConfig, verify_seal_chain};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// --- fixtures ----------------------------------------------------------

/// magic (8) + format version (4) + reserved (4): restated independently of
/// `qip_storage::segment::file`'s own (crate-private) constant.
const SEGMENT_HEADER_LEN: u64 = 16;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-segment-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manual_clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)))
}

fn config_with(
    clock: Arc<dyn Clock>,
    roll_after_bytes: u64,
    archive_required: bool,
) -> SegmentLogConfig {
    SegmentLogConfig::new(clock)
        .with_roll_after_bytes(roll_after_bytes)
        .with_archive_required(archive_required)
}

/// Build a one-record batch. `tag` only feeds the event id and timestamp —
/// nothing in these tests reads it back — so callers are free to use it as a
/// counter without affecting anything the tests assert on.
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

fn sealed_segment_starts(log: &SegmentLog) -> Vec<u64> {
    let mut starts: Vec<u64> = log
        .segments()
        .into_iter()
        .filter(|s| s.sealed)
        .map(|s| s.start_offset)
        .collect();
    starts.sort_unstable();
    starts
}

/// Append batches until at least `want` segments have been sealed. Used so a
/// test's roll/seal coverage does not depend on hand-tuned batch counts
/// matching a hand-tuned roll threshold.
fn append_until_sealed_count(log: &SegmentLog, want: usize, next_tag: &mut u64) {
    let mut guard = 0;
    while log.segments().iter().filter(|s| s.sealed).count() < want {
        let batch = tagged_batch(*next_tag, &[0x42u8; 40]);
        log.append(&batch).unwrap();
        *next_tag += 1;
        guard += 1;
        assert!(
            guard < 10_000,
            "the roll threshold in this test must be small enough to reach {want} sealed \
             segments in a bounded number of appends"
        );
    }
}

/// The path of whichever `segment.<start>` file currently has the highest
/// start offset — the active one, by construction (only the active segment
/// is ever the newest file on disk).
fn active_segment_path(dir: &Path) -> PathBuf {
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(".qip-partial") {
            continue;
        }
        if let Some(digits) = name.strip_prefix("segment.")
            && let Ok(start) = digits.parse::<u64>()
            && best.as_ref().is_none_or(|(b, _)| start > *b)
        {
            best = Some((start, entry.path()));
        }
    }
    best.map(|(_, p)| p)
        .expect("a segment log always has at least one segment file")
}

fn segment_file_path(dir: &Path, start_offset: u64) -> PathBuf {
    dir.join(format!("segment.{start_offset:020}"))
}

// --- appends_crashes_and_recoveries_leave_dense_offsets_that_are_never_reused

/// Append a random-length **proper prefix** of a freshly encoded batch
/// directly to whatever the active segment file currently is, bypassing the
/// log entirely. A proper prefix of a validly encoded batch can only ever
/// decode as [`qip_events::event_fabric::codec::DecodeOutcome::Torn`] — see
/// the codec's own module documentation — so this is a faithful model of
/// "the next append was interrupted partway", the shape a real crash leaves,
/// without needing to interrupt a real write in flight.
fn inject_torn_fragment(dir: &Path, rng: &mut Xoshiro256) {
    let path = active_segment_path(dir);
    let filler_len = 1 + rng.below(40) as usize;
    let filler = vec![0xABu8; filler_len];
    let would_be_next = tagged_batch(999, &filler);
    let encoded = would_be_next.encode().unwrap();
    let cut = rng.below(encoded.len() as u64) as usize;
    if cut == 0 {
        return;
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(&encoded[..cut]).unwrap();
    // Deliberately no `sync_all()`: this stands in for bytes a crash left
    // behind before anything, including this "process", ever flushed them.
}

/// Property test, seeded so a failure is reproducible: across many
/// independent open/append/crash/reopen schedules, the offset a batch reads
/// back with must depend only on what recovery actually verifies on disk,
/// never on an in-memory count of what was merely attempted.
///
/// Mutation-tested against a **stronger** substitute for this test's named
/// mutation ("readable end = last appended instead of last fsynced"), and
/// the substitution is argued, not just asserted: swapping the order inside
/// `SegmentLog::append` (advancing `next_offset` before `sync` instead of
/// after) is unobservable by *any* non-racing test, seeded or not, because
/// on a real filesystem `sync` either fully completes before `append`
/// returns or the call returns `Err` before `next_offset` is touched either
/// way — the two orderings only differ during a window a real crash would
/// have to land inside, which cannot be scheduled deterministically. The
/// same failure mode is fully and deterministically observable one layer
/// down, in recovery's derivation of `next_offset` from the segment it
/// actually finds on disk, which is what this test drives through many
/// crash-injected schedules and where the substitute mutation is applied.
#[test]
fn appends_crashes_and_recoveries_leave_dense_offsets_that_are_never_reused() {
    for seed in [1u64, 7, 42, 1009, 99_991] {
        let mut rng = Xoshiro256::seeded(seed);
        let dir = temp_dir(&format!("dense-offsets-{seed}"));
        let clock = manual_clock();
        let mut acked: Vec<Vec<u8>> = Vec::new();

        for session in 0..5 {
            {
                let log = SegmentLog::open(&dir, config_with(clock.clone(), 320, false)).unwrap();
                assert_eq!(
                    log.high_water(),
                    acked.len() as u64,
                    "seed {seed} session {session}: recovery must resume from exactly what was \
                     durably acked, never more (a batch counted before it was fsynced) or less"
                );
                for (offset, expected) in acked.iter().enumerate() {
                    let batch = log.read(offset as u64).unwrap().unwrap_or_else(|| {
                        panic!("seed {seed} session {session}: offset {offset} was acked and must still read back")
                    });
                    assert_eq!(
                        &batch.records[0].payload, expected,
                        "seed {seed} session {session}: offset {offset}'s content must match what was acked, never a different batch reusing the same offset"
                    );
                }

                let appends_this_session = 1 + rng.below(4);
                for _ in 0..appends_this_session {
                    let filler_len = rng.below(30) as usize;
                    let mut payload = acked.len().to_le_bytes().to_vec();
                    payload.extend(std::iter::repeat_n(0x5Au8, filler_len));
                    let batch = tagged_batch(acked.len() as u64, &payload);
                    let offset = log.append(&batch).unwrap();
                    assert_eq!(
                        offset,
                        acked.len() as u64,
                        "seed {seed} session {session}: offsets must be assigned densely"
                    );
                    acked.push(payload);
                }
            } // the log drops here, as a process exiting cleanly would close its files

            inject_torn_fragment(&dir, &mut rng);
        }

        let log = SegmentLog::open(&dir, config_with(clock.clone(), 320, false)).unwrap();
        assert_eq!(
            log.high_water(),
            acked.len() as u64,
            "seed {seed}: final recovery must match every durably acked offset, no more and no fewer"
        );
        for (offset, expected) in acked.iter().enumerate() {
            let batch = log.read(offset as u64).unwrap().unwrap();
            assert_eq!(
                &batch.records[0].payload, expected,
                "seed {seed}: offset {offset} final check"
            );
        }
        assert_eq!(
            log.read(acked.len() as u64).unwrap(),
            None,
            "seed {seed}: nothing exists past the last acked offset"
        );
    }
}

// --- a_segment_truncated_at_every_byte_of_its_last_batch_recovers_whole_batches_only

#[test]
fn a_segment_truncated_at_every_byte_of_its_last_batch_recovers_whole_batches_only() {
    let dir = temp_dir("truncate-last-batch");
    let clock = manual_clock();
    let commits = 6usize;
    let mut batches = Vec::new();
    {
        let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
        for i in 0..commits {
            let payload = format!("payload-{i:03}").into_bytes();
            let batch = tagged_batch(i as u64, &payload);
            assert_eq!(log.append(&batch).unwrap(), i as u64);
            batches.push(batch);
        }
    }

    let path = segment_file_path(&dir, 0);
    let complete = std::fs::read(&path).unwrap();

    // Independently derived extents (see the module doc): re-encode the same
    // batches this test built, rather than asking the crate what it wrote.
    let mut extents = Vec::new();
    let mut cursor = SEGMENT_HEADER_LEN as usize;
    for batch in &batches {
        let len = batch.encode().unwrap().len();
        extents.push((cursor, cursor + len));
        cursor += len;
    }
    assert_eq!(
        cursor,
        complete.len(),
        "the test's premise: the independently computed extents must account for every byte \
         actually written"
    );

    for cut in (SEGMENT_HEADER_LEN as usize)..=complete.len() {
        std::fs::write(&path, &complete[..cut]).unwrap();

        let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false))
            .unwrap_or_else(|e| panic!("cut at {cut} must still open: {e}"));
        let survived = extents.iter().filter(|(_, end)| *end <= cut).count();

        assert_eq!(
            log.high_water(),
            survived as u64,
            "cut at {cut} should leave {survived} whole batches"
        );
        for i in 0..survived {
            let batch = log.read(i as u64).unwrap().unwrap();
            assert_eq!(
                batch.records[0].payload,
                format!("payload-{i:03}").into_bytes()
            );
        }
        assert_eq!(
            log.read(survived as u64).unwrap(),
            None,
            "cut at {cut}: batch {survived} was incomplete and must be gone, not returned"
        );

        let valid_end = if survived == 0 {
            SEGMENT_HEADER_LEN as usize
        } else {
            extents[survived - 1].1
        };
        let report = log.recovery();
        assert_eq!(report.batches_recovered, survived as u64);
        assert_eq!(
            report.recovered_from_a_torn_write(),
            cut > valid_end,
            "cut at {cut}: the report must say whether a partial batch was discarded"
        );
        assert_eq!(report.bytes_discarded, (cut - valid_end) as u64);
        if cut > valid_end {
            assert_eq!(report.torn_tail_at, Some(valid_end as u64));
        }

        // Not just reported clean — actually shortened on disk. A torn tail
        // that recovery merely ignores in memory but leaves sitting in the
        // file would still corrupt whatever the *next* append writes after
        // it, since an append always lands at the file's true end.
        let on_disk_len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(
            on_disk_len, valid_end as u64,
            "cut at {cut}: a torn tail must be truncated from the file itself, not merely \
             ignored in the recovery report"
        );
    }

    // A torn tail is removed rather than left in place, so the next append
    // reuses its offset instead of skipping over it.
    std::fs::write(&path, &complete[..complete.len() - 3]).unwrap();
    {
        let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
        assert!(log.recovery().recovered_from_a_torn_write());
        let extra = tagged_batch(999, b"after-recovery");
        let offset = log.append(&extra).unwrap();
        assert_eq!(
            offset,
            commits as u64 - 1,
            "the torn batch's offset must be reused by the next real append, not skipped"
        );
    }
    let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
    assert_eq!(log.high_water(), commits as u64);
    assert_eq!(
        log.read((commits - 1) as u64).unwrap().unwrap().records[0].payload,
        b"after-recovery".to_vec()
    );
}

// --- a_complete_batch_that_fails_its_crc_or_chain_refuses_to_open_naming_the_byte_offset

#[test]
fn a_complete_batch_that_fails_its_crc_or_chain_refuses_to_open_naming_the_byte_offset() {
    let dir = temp_dir("corrupt-middle");
    let clock = manual_clock();
    let batch0 = tagged_batch(0, b"AAAA-first-batch");
    let batch1 = tagged_batch(1, b"BBBB-second-batch");
    let batch2 = tagged_batch(2, b"CCCC-third-batch");
    let len0 = batch0.encode().unwrap().len() as u64;
    let len1 = batch1.encode().unwrap().len() as u64;

    {
        let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
        assert_eq!(log.append(&batch0).unwrap(), 0);
        assert_eq!(log.append(&batch1).unwrap(), 1);
        assert_eq!(log.append(&batch2).unwrap(), 2);
    }

    let path = segment_file_path(&dir, 0);
    let original = std::fs::read(&path).unwrap();
    let batch1_start = SEGMENT_HEADER_LEN + len0;
    // The last byte of batch1's encoding is its own trailing record CRC:
    // flipping it corrupts a checksum the codec already computed, which is
    // certain to mismatch, rather than data whose corruption is merely
    // probabilistic — and the batch is otherwise fully present, so nothing
    // about the cut here could be explained by a truncation.
    let target = (batch1_start + len1 - 1) as usize;
    let mut corrupted = original.clone();
    corrupted[target] ^= 0xFF;
    assert_ne!(
        corrupted[target], original[target],
        "the test's premise: the byte actually flips"
    );
    std::fs::write(&path, &corrupted).unwrap();

    let error = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&batch1_start.to_string()),
        "the refusal must name the byte offset of the damaged batch: {message}"
    );

    // The refusal must be the *whole* story: no partial log was left open,
    // and the file on disk is untouched (recovery only reads until it finds
    // the error, and never writes on the way to refusing). This is the
    // specific gap this packet calls out: a bug that treats the corruption
    // as a torn tail would truncate the file down to batch0 alone, silently
    // discarding batch1 *and* the genuinely valid batch2 after it.
    let untouched = std::fs::read(&path).unwrap();
    assert_eq!(
        untouched, corrupted,
        "open() must not rewrite the file on a refusal"
    );
}

// --- a_read_never_returns_an_offset_beyond_the_last_fsynced_one

#[test]
fn a_read_never_returns_an_offset_beyond_the_last_fsynced_one() {
    let dir = temp_dir("read-boundary");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
    log.append(&tagged_batch(0, b"first")).unwrap();
    log.append(&tagged_batch(1, b"second")).unwrap();
    let high_water = log.high_water();
    assert_eq!(
        high_water, 2,
        "the test's premise: exactly two batches were acknowledged"
    );

    // Write a third, fully valid, completely decodable batch straight to
    // the segment file, bypassing the log's own `append` entirely. This
    // models exactly the fact the high watermark exists to guard: bytes
    // that are physically present and would parse perfectly, but which
    // *this log* never itself appended and fsynced.
    let phantom = tagged_batch(2, b"phantom-never-acked");
    let encoded = phantom.encode().unwrap();
    let path = segment_file_path(&dir, 0);
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&encoded).unwrap();
    }

    assert_eq!(
        log.high_water(),
        high_water,
        "bytes written behind the log's back must not move its own watermark"
    );
    assert_eq!(
        log.read(high_water).unwrap(),
        None,
        "an offset at or beyond the high watermark must never be served, however decodable the \
         bytes physically sitting there already are"
    );
}

// --- retention_deletes_only_sealed_segments_and_never_an_unarchived_one_of_an_archive_required_stream

#[test]
fn retention_deletes_only_sealed_segments_and_never_an_unarchived_one_of_an_archive_required_stream()
 {
    let dir = temp_dir("retention");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock.clone(), 150, true)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 2, &mut tag);

    let sealed = sealed_segment_starts(&log);
    assert!(
        sealed.len() >= 2,
        "the test's premise: at least two segments are actually sealed before retention runs"
    );
    let archived_start = sealed[0];
    let unarchived_start = sealed[1];

    log.mark_archived(archived_start).unwrap();
    assert!(log.is_archived(archived_start).unwrap());
    assert!(!log.is_archived(unarchived_start).unwrap());

    let boundary = log.high_water();
    let report = log.retain_before(boundary).unwrap();

    assert!(
        report.deleted.contains(&archived_start),
        "an archived, sealed segment must be deleted once it is fully behind the boundary"
    );
    assert!(
        !report.deleted.contains(&unarchived_start),
        "an unarchived segment of an archive-required stream must never be deleted, however old"
    );
    assert!(
        report.withheld.contains(&unarchived_start),
        "a withheld segment must be reported, not silently skipped"
    );

    assert!(
        !segment_file_path(&dir, archived_start).exists(),
        "the deleted segment's file must actually be gone from disk"
    );
    assert!(
        segment_file_path(&dir, unarchived_start).exists(),
        "the withheld segment's file must survive on disk"
    );
    assert!(
        log.read(unarchived_start).unwrap().is_some(),
        "a withheld segment must still be fully readable, not half-deleted"
    );

    let active = log
        .segments()
        .into_iter()
        .find(|s| !s.sealed)
        .expect("a segment log always has exactly one active segment");
    assert!(
        !report.deleted.contains(&active.start_offset),
        "the active segment is never sealed and must never be eligible for retention"
    );
    assert!(segment_file_path(&dir, active.start_offset).exists());
}

// --- each_seal_records_the_previous_seals_hash_so_a_missing_segment_is_detectable

#[test]
fn each_seal_records_the_previous_seals_hash_so_a_missing_segment_is_detectable() {
    let dir = temp_dir("seal-chain");
    let clock = manual_clock();
    let log = SegmentLog::open(&dir, config_with(clock.clone(), 150, false)).unwrap();
    let mut tag = 0u64;
    append_until_sealed_count(&log, 3, &mut tag);

    let sealed = sealed_segment_starts(&log);
    assert!(
        sealed.len() >= 3,
        "the test's premise: at least three segments are actually sealed"
    );

    let seals: Vec<Seal> = sealed[..3]
        .iter()
        .map(|&start| {
            log.seal_of(start)
                .unwrap()
                .unwrap_or_else(|| panic!("segment {start} was sealed and must carry a seal"))
        })
        .collect();

    assert_eq!(
        seals[0].previous_seal_hash, None,
        "the first segment this log ever sealed has no predecessor to chain to"
    );
    for i in 1..seals.len() {
        assert_eq!(
            seals[i].previous_seal_hash,
            Some(seals[i - 1].content_hash.clone()),
            "each seal after the first must record the content hash of the segment sealed \
             immediately before it (segment index {i})"
        );
    }

    verify_seal_chain(&seals).expect("a genuinely unbroken chain must verify as unbroken");

    // Simulate a missing segment: the middle seal is simply absent from what
    // a verifier holds, exactly as it would be if that segment — and its
    // seal — had been deleted without ever being archived.
    let with_a_gap = vec![seals[0].clone(), seals[2].clone()];
    let error = verify_seal_chain(&with_a_gap).unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&seals[2].segment_start_offset.to_string()),
        "the refusal must name the segment whose link is broken: {error}"
    );
}

// --- a_manifest_record_written_before_a_crash_is_read_back_after_recovery_and_a_torn_one_is_refused

#[test]
fn a_manifest_record_written_before_a_crash_is_read_back_after_recovery_and_a_torn_one_is_refused()
{
    let dir = temp_dir("manifest");
    let clock = manual_clock();
    {
        let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
        assert_eq!(
            log.manifest_get("widget").unwrap(),
            None,
            "the test's premise: a key nobody has written yet reads back as absent, not an error"
        );
        log.manifest_put("widget", b"hello world").unwrap();
    }

    let log = SegmentLog::open(&dir, config_with(clock.clone(), 10_000_000, false)).unwrap();
    assert_eq!(
        log.manifest_get("widget").unwrap(),
        Some(b"hello world".to_vec()),
        "a manifest record written before the process ended must read back after recovery"
    );

    // Corrupt the record's bytes on disk directly (the manifest directory
    // convention is restated here rather than imported — see the module
    // doc). A torn or bit-flipped manifest must be refused, never served as
    // if it were the caller's data.
    let path = dir.join("manifest").join("widget");
    let mut bytes = std::fs::read(&path).unwrap();
    assert!(
        bytes.len() > 8,
        "the test's premise: the record actually carries payload bytes past its own header"
    );
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let error = log.manifest_get("widget").unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("checksum") || message.contains("torn") || message.contains("corrupt"),
        "the refusal must say the record failed its own integrity check: {message}"
    );
}
