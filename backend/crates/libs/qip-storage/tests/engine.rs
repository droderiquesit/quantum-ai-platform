//! The embedded storage engine: durability, recovery, atomicity, integrity.
//!
//! These tests assert the properties the engine's documentation claims, in the
//! terms the documentation uses. Two of them deserve a word about method.
//!
//! *Power loss* cannot be produced by a test. What it produces on disk can:
//! the last record of the log is cut short. So the crash tests truncate the log
//! at **every byte offset** of the affected record and assert that the store
//! opens, every complete record survives, and the incomplete one is gone. That
//! is an exhaustive check of the recovery rule rather than a sample of it.
//!
//! *Process death* is real here — a child process writes, acknowledges, and
//! calls `abort`, which gives it no chance to flush anything from user space.
//! The parent then reopens the directory and looks for the acknowledged write.

use qip_core::error::Error;
use qip_core::{Clock, ManualClock, Timestamp};
use qip_storage::engine::{Durability, DurableStore, EngineConfig, WriteBatch};
use qip_storage::kv::{KeyValueStore, KeyValueStoreExt};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// --- fixtures ---------------------------------------------------------------

static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-engine-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(Timestamp::from_civil(2026, 8, 23)))
}

fn config() -> EngineConfig {
    EngineConfig::new(clock())
}

/// The live log file. There is exactly one; the previous generation is deleted
/// as the last step of a checkpoint.
fn log_path(directory: &Path) -> PathBuf {
    let mut found: Vec<PathBuf> = std::fs::read_dir(directory)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("wal."))
                .unwrap_or(false)
        })
        .collect();
    found.sort();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one live log in {directory:?}"
    );
    found.remove(0)
}

/// Walk a frame file the way the engine does, but with the format restated
/// here rather than imported — a test that shares the reader with the code it
/// checks proves less.
///
/// Returns `(start, end)` for each complete frame.
fn frame_extents(bytes: &[u8]) -> Vec<(usize, usize)> {
    const FILE_HEADER: usize = 16;
    const FRAME_HEADER: usize = 4 + 4 + 32;
    let mut out = Vec::new();
    let mut offset = FILE_HEADER;
    while offset + FRAME_HEADER <= bytes.len() {
        assert_eq!(
            &bytes[offset..offset + 4],
            b"QWAL",
            "frame magic at {offset}"
        );
        let mut length = [0u8; 4];
        length.copy_from_slice(&bytes[offset + 4..offset + 8]);
        let end = offset + FRAME_HEADER + u32::from_le_bytes(length) as usize;
        if end > bytes.len() {
            break;
        }
        out.push((offset, end));
        offset = end;
    }
    out
}

fn record(index: usize) -> serde_json::Value {
    json!({ "index": index, "note": "a".repeat(64) })
}

// --- the port contract ------------------------------------------------------

#[test]
fn the_engine_satisfies_the_key_value_port_contract() {
    let dir = temp_dir("contract");
    let store = DurableStore::open(&dir, config()).unwrap();

    assert!(store.is_empty().unwrap());
    store.put("a/1", json!({ "x": 1 })).unwrap();
    store.put("a/2", json!({ "x": 2 })).unwrap();
    store.put("b/1", json!({ "x": 3 })).unwrap();

    assert_eq!(store.len().unwrap(), 3);
    assert_eq!(store.get("a/1").unwrap(), Some(json!({ "x": 1 })));
    assert_eq!(store.get("missing").unwrap(), None);
    assert_eq!(store.keys_with_prefix("a/").unwrap(), vec!["a/1", "a/2"]);
    assert!(store.delete("a/1").unwrap());
    assert!(
        !store.delete("a/1").unwrap(),
        "deleting twice is not an error"
    );
    assert_eq!(store.len().unwrap(), 2);

    // The port's typed helpers work through the trait object, unchanged.
    let erased_dir = temp_dir("contract-erased");
    let erased: Arc<dyn KeyValueStore> =
        Arc::new(DurableStore::open(&erased_dir, config()).unwrap());
    erased
        .put_as("positions/AAPL", &json!({ "quantity": 100 }))
        .unwrap();
    let read: serde_json::Value = erased.get_as("positions/AAPL").unwrap().unwrap();
    assert_eq!(read["quantity"], 100);

    drop(store);
    drop(erased);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&erased_dir);
}

#[test]
fn prefix_and_range_scans_come_back_in_key_order() {
    let dir = temp_dir("scans");
    let store = DurableStore::open(&dir, config()).unwrap();
    for key in ["a", "ab", "abc", "b", "ba", "c"] {
        store.put(key, json!(key)).unwrap();
    }

    assert_eq!(
        store.keys_with_prefix("ab").unwrap(),
        vec!["ab", "abc"],
        "a prefix scan must not return the shorter key it is a prefix of"
    );
    assert_eq!(store.keys_with_prefix("").unwrap().len(), 6);
    assert_eq!(store.keys_with_prefix("zz").unwrap(), Vec::<String>::new());

    let scanned: Vec<String> = store
        .scan_prefix("b")
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(scanned, vec!["b", "ba"]);

    let ranged: Vec<String> = store
        .range("ab", "b")
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(ranged, vec!["ab", "abc"], "the range is half-open");
    assert!(
        store.range("b", "a").is_err(),
        "a reversed range is invalid"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- durability -------------------------------------------------------------

/// Set on the child process of the crash test; holds the directory to write to.
const CRASH_DIRECTORY: &str = "QIP_STORAGE_CRASH_DIRECTORY";

#[test]
fn an_acknowledged_write_survives_the_process_being_killed() {
    // Child role. `abort` raises SIGABRT: no unwinding, no destructors, no
    // flush of anything this process was still holding. Whatever is on disk is
    // there because `put` and `commit` put it there before they returned.
    if let Ok(directory) = std::env::var(CRASH_DIRECTORY) {
        let store = DurableStore::open(&directory, config()).unwrap();
        store.put("acknowledged", json!({ "n": 1 })).unwrap();
        store
            .commit(
                WriteBatch::new()
                    .put("left", json!(7))
                    .put("right", json!(7)),
            )
            .unwrap();
        std::process::abort();
    }

    let dir = temp_dir("kill");
    let executable = std::env::current_exe().unwrap();
    let status = std::process::Command::new(executable)
        .arg("--exact")
        .arg("an_acknowledged_write_survives_the_process_being_killed")
        .env(CRASH_DIRECTORY, &dir)
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "the child was supposed to die mid-flight, not to finish"
    );

    let store = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(
        store.get("acknowledged").unwrap(),
        Some(json!({ "n": 1 })),
        "a write acknowledged before SIGABRT must still be there"
    );
    assert_eq!(store.get("left").unwrap(), Some(json!(7)));
    assert_eq!(store.get("right").unwrap(), Some(json!(7)));
    assert_eq!(store.recovery().log_records_applied, 2);
    assert!(
        !store.recovery().recovered_from_a_torn_write(),
        "the child died between commits, so there is no torn tail to find"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_default_durability_is_the_one_that_survives_power_loss() {
    assert_eq!(config().durability(), Durability::Synchronous);
    assert!(Durability::Synchronous.survives_power_loss());
    assert!(
        !Durability::OsBuffered.survives_power_loss(),
        "the buffered setting must not claim a guarantee it does not give"
    );
    assert_eq!(Durability::default(), Durability::Synchronous);
}

// --- crash recovery ---------------------------------------------------------

#[test]
fn truncating_the_log_at_every_byte_offset_recovers_a_prefix_consistent_state() {
    let dir = temp_dir("truncate-every-offset");
    let commits = 10usize;
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        for i in 0..commits {
            store.put(&format!("key-{i:03}"), record(i)).unwrap();
        }
    }

    let path = log_path(&dir);
    let complete = std::fs::read(&path).unwrap();
    let extents = frame_extents(&complete);
    assert_eq!(extents.len(), commits, "one record per commit, no more");

    // Every offset from the end of the file header to the end of the file:
    // this covers the header, the length, the digest and the payload of every
    // record, not just the last one.
    for cut in 16..=complete.len() {
        std::fs::write(&path, &complete[..cut]).unwrap();

        let store = DurableStore::open(&dir, config())
            .unwrap_or_else(|e| panic!("cut at {cut} must still open: {e}"));
        let survived = extents.iter().filter(|(_, end)| *end <= cut).count();

        assert_eq!(
            store.len().unwrap(),
            survived,
            "cut at {cut} should leave {survived} records"
        );
        for i in 0..survived {
            assert_eq!(
                store.get(&format!("key-{i:03}")).unwrap(),
                Some(record(i)),
                "cut at {cut}: record {i} was complete and must be intact"
            );
        }
        for i in survived..commits {
            assert_eq!(
                store.get(&format!("key-{i:03}")).unwrap(),
                None,
                "cut at {cut}: record {i} was incomplete and must be gone"
            );
        }

        // Everything past the last complete record is discarded, and the
        // report says exactly how much and from where.
        let valid_end = if survived == 0 {
            16
        } else {
            extents[survived - 1].1
        };
        let report = store.recovery();
        assert_eq!(report.log_records_applied as usize, survived);
        assert_eq!(
            report.recovered_from_a_torn_write(),
            cut > valid_end,
            "cut at {cut}: the report must say whether a partial record was discarded"
        );
        assert_eq!(report.bytes_discarded, (cut - valid_end) as u64);
        if cut > valid_end {
            assert_eq!(report.torn_tail_at, Some(valid_end as u64));
            assert_eq!(report.torn_records_discarded, 1);
        }
    }

    // A torn tail is removed rather than left in place, so the next append
    // does not bury a partial record in the middle of the log.
    std::fs::write(&path, &complete[..complete.len() - 3]).unwrap();
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        assert!(store.recovery().recovered_from_a_torn_write());
        store.put("after-recovery", json!(true)).unwrap();
    }
    let store = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(store.get("after-recovery").unwrap(), Some(json!(true)));
    assert_eq!(store.len().unwrap(), commits);

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tail_of_zeroes_is_read_as_a_torn_write_not_as_data() {
    // A crash can leave a block allocated and never written. Zeroes are not a
    // record, and they are not corruption either.
    let dir = temp_dir("zero-tail");
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        for i in 0..4 {
            store.put(&format!("key-{i}"), record(i)).unwrap();
        }
    }
    let path = log_path(&dir);
    let mut bytes = std::fs::read(&path).unwrap();
    let end = bytes.len() as u64;
    bytes.extend(std::iter::repeat_n(0u8, 4096));
    std::fs::write(&path, &bytes).unwrap();

    let store = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(store.len().unwrap(), 4);
    assert_eq!(store.recovery().torn_tail_at, Some(end));
    assert_eq!(store.recovery().bytes_discarded, 4096);
    assert_eq!(store.recovery().torn_records_discarded, 1);

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovery_replays_the_checkpoint_and_the_log_together() {
    let dir = temp_dir("recover-both");
    {
        // A threshold this small forces several checkpoints over the run.
        let store = DurableStore::open(&dir, config().with_checkpoint_after_bytes(2048)).unwrap();
        for i in 0..120 {
            store.put(&format!("key-{i:03}"), record(i)).unwrap();
        }
        for i in 0..40 {
            assert!(store.delete(&format!("key-{i:03}")).unwrap());
        }
        assert!(
            store.stats().checkpoints >= 2,
            "the workload must actually have checkpointed"
        );
    }

    let store = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(store.len().unwrap(), 80);
    for i in 0..40 {
        assert_eq!(store.get(&format!("key-{i:03}")).unwrap(), None);
    }
    for i in 40..120 {
        assert_eq!(store.get(&format!("key-{i:03}")).unwrap(), Some(record(i)));
    }
    let report = store.recovery();
    assert!(
        report.checkpoint_records > 0,
        "state came from a checkpoint"
    );
    assert!(
        report.generation > 0,
        "generations advance with checkpoints"
    );
    assert!(
        report.checkpoint_keys > 0 && report.checkpoint_keys <= 120,
        "the checkpoint carried a real share of the state"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- atomicity --------------------------------------------------------------

#[test]
fn a_transaction_is_all_or_nothing_at_every_truncation_of_its_record() {
    let dir = temp_dir("atomic-truncate");
    let members = ["txn/a", "txn/b", "txn/c", "txn/d"];
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        store.put("committed-earlier", json!("safe")).unwrap();
        let mut batch = WriteBatch::new();
        for (i, key) in members.iter().enumerate() {
            batch = batch.put(*key, record(i));
        }
        assert_eq!(batch.len(), members.len());
        store.commit(batch).unwrap();
    }

    let path = log_path(&dir);
    let complete = std::fs::read(&path).unwrap();
    let extents = frame_extents(&complete);
    assert_eq!(
        extents.len(),
        2,
        "a four-key transaction is one record, not four"
    );
    let (start, end) = extents[1];

    for cut in start..=end {
        std::fs::write(&path, &complete[..cut]).unwrap();
        let store = DurableStore::open(&dir, config())
            .unwrap_or_else(|e| panic!("cut at {cut} must still open: {e}"));

        assert_eq!(
            store.get("committed-earlier").unwrap(),
            Some(json!("safe")),
            "cut at {cut}: an earlier commit is not collateral damage"
        );

        let present = members
            .iter()
            .filter(|key| store.get(key).unwrap().is_some())
            .count();
        assert!(
            present == 0 || present == members.len(),
            "cut at {cut}: {present} of {} keys visible — a transaction was torn in half",
            members.len()
        );
        assert_eq!(
            present == members.len(),
            cut == end,
            "cut at {cut}: the batch is visible exactly when its record is complete"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_batch_becomes_visible_to_readers_all_at_once() {
    let dir = temp_dir("atomic-readers");
    let store = Arc::new(DurableStore::open(&dir, config()).unwrap());
    store
        .commit(
            WriteBatch::new()
                .put("left", json!(0))
                .put("right", json!(0)),
        )
        .unwrap();

    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            for round in 1..=200 {
                store
                    .commit(
                        WriteBatch::new()
                            .put("left", json!(round))
                            .put("right", json!(round)),
                    )
                    .unwrap();
            }
        })
    };

    // One `snapshot` is one read of the index, so it observes a committed
    // state and never a batch in progress. Two separate `get` calls are two
    // reads and a commit may land between them — the engine documents that it
    // has no snapshot isolation across calls, and this test does not pretend
    // otherwise.
    let mut observations = 0u32;
    while observations < 5_000 {
        let view = store.snapshot();
        assert_eq!(
            view.get("left"),
            view.get("right"),
            "a reader saw one half of a batch without the other"
        );
        observations += 1;
    }
    writer.join().unwrap();

    let view = store.snapshot();
    assert_eq!(view.get("left"), Some(&json!(200)));
    assert_eq!(view.get("right"), Some(&json!(200)));

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_reads_are_two_reads_and_the_engine_does_not_claim_otherwise() {
    // The counterpart to the test above, stated as a property rather than
    // discovered as a surprise: a caller who needs several keys observed
    // together must take one snapshot, not several gets.
    let dir = temp_dir("no-snapshot-isolation");
    let store = DurableStore::open(&dir, config()).unwrap();
    store
        .commit(WriteBatch::new().put("a", json!(1)).put("b", json!(1)))
        .unwrap();

    let first = store.get("a").unwrap();
    store
        .commit(WriteBatch::new().put("a", json!(2)).put("b", json!(2)))
        .unwrap();
    let second = store.get("b").unwrap();
    assert_ne!(
        first, second,
        "successive gets see successive states; only a snapshot freezes one"
    );

    let view = store.snapshot();
    assert_eq!(view.get("a"), view.get("b"));

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_empty_batch_costs_nothing() {
    let dir = temp_dir("empty-batch");
    let store = DurableStore::open(&dir, config()).unwrap();
    store.put("k", json!(1)).unwrap();
    let before = store.stats();

    assert!(WriteBatch::new().is_empty());
    let sequence = store.commit(WriteBatch::new()).unwrap();

    let after = store.stats();
    assert_eq!(sequence, before.last_sequence, "no change, no new sequence");
    assert_eq!(after.commits, before.commits);
    assert_eq!(
        after.log_bytes_appended, before.log_bytes_appended,
        "an empty batch must not spend a barrier recording nothing"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- write amplification ----------------------------------------------------

#[test]
fn writes_append_instead_of_rewriting_the_dataset() {
    const WRITES: usize = 300;
    let value = json!({ "payload": "x".repeat(192) });

    // The engine.
    let engine_dir = temp_dir("amplification-engine");
    let store =
        DurableStore::open(&engine_dir, config().with_checkpoint_after_bytes(16 * 1024)).unwrap();
    for i in 0..WRITES {
        store.put(&format!("key-{i:04}"), value.clone()).unwrap();
    }
    let stats = store.stats();

    // The document store this engine replaces, measured the same way: the
    // bytes a put writes are the bytes of the file it leaves behind.
    let document_dir = temp_dir("amplification-document");
    let path = document_dir.join("store.json");
    let document = qip_storage::FileKeyValueStore::open(&path).unwrap();
    let mut document_bytes = 0u64;
    for i in 0..WRITES {
        document.put(&format!("key-{i:04}"), value.clone()).unwrap();
        document_bytes += std::fs::metadata(&path).unwrap().len();
    }

    assert_eq!(stats.commits, WRITES as u64);
    assert_eq!(stats.keys, WRITES as u64);
    assert!(
        stats.checkpoints >= 1,
        "the workload must have crossed the checkpoint threshold at least once"
    );
    assert!(
        stats.checkpoints < WRITES as u64 / 10,
        "{} checkpoints for {WRITES} writes is a rewrite in disguise",
        stats.checkpoints
    );
    assert!(
        stats.total_bytes_written() <= 5 * stats.log_bytes_appended,
        "amplification is {} bytes written for {} bytes logged; the trigger is \
         supposed to hold it to a small constant",
        stats.total_bytes_written(),
        stats.log_bytes_appended
    );
    assert!(
        stats.total_bytes_written() * 10 < document_bytes,
        "engine wrote {} bytes, the rewrite-everything store wrote {document_bytes}; \
         the point of the engine is that this gap widens with N",
        stats.total_bytes_written()
    );

    drop(store);
    drop(document);
    let _ = std::fs::remove_dir_all(&engine_dir);
    let _ = std::fs::remove_dir_all(&document_dir);
}

#[test]
fn a_checkpoint_retires_the_generation_it_replaces() {
    let dir = temp_dir("checkpoint-generations");
    let store = DurableStore::open(&dir, config()).unwrap();
    for i in 0..50 {
        store.put(&format!("key-{i:03}"), record(i)).unwrap();
    }
    let before = store.stats();
    let generation = store.checkpoint().unwrap();
    let after = store.stats();

    assert_eq!(generation, before.generation + 1);
    assert_eq!(after.checkpoints, before.checkpoints + 1);
    assert_eq!(
        after.live_log_bytes, 0,
        "a checkpoint starts an empty log; that is what bounds it"
    );

    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.iter().filter(|n| n.starts_with("wal.")).count(),
        1,
        "the retired log must be deleted, not accumulated: {names:?}"
    );
    assert_eq!(
        names
            .iter()
            .filter(|n| n.starts_with("checkpoint."))
            .count(),
        1,
        "the retired checkpoint must be deleted too: {names:?}"
    );

    drop(store);
    let store = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(store.len().unwrap(), 50);
    assert_eq!(store.recovery().checkpoint_keys, 50);
    assert_eq!(
        store.recovery().log_records_applied,
        0,
        "everything was in the checkpoint"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A commit whose own record is durable must not be reported as failed just
/// because the checkpoint it triggered could not be written.
///
/// The failure this guards against: `commit_locked` used to propagate a
/// checkpoint error with `?`, so a caller of `put` received `Err` for a write
/// whose record had already been appended to the write-ahead log and
/// `fsync`ed — durable and already visible through `get`. A caller that
/// trusts an `Err` to mean "not written" and retries duplicates the write;
/// one that trusts it to mean "lost" may re-derive and store a different
/// value under the belief the first was discarded. Either is exactly the
/// class of bug this crate's durability guarantee exists to make impossible.
///
/// The checkpoint is made to fail without relying on filesystem permissions —
/// this suite may run as root, under which permission bits are not
/// enforced. Instead, `checkpoint.<generation>` (the exact name the next
/// checkpoint will try to publish) is pre-created *as a directory*. The
/// checkpoint's scratch file is written and flushed under a different,
/// counter-suffixed name and only then `rename`d onto the published name —
/// see `checkpoint_locked` — and `rename(2)` refuses to replace a directory
/// with a regular file for anyone, root included. The write-ahead log the
/// triggering commit appends to is a separate, already-open file untouched by
/// any of this.
#[test]
fn a_commit_whose_triggered_checkpoint_fails_still_reports_success_and_keeps_the_write() {
    let dir = temp_dir("checkpoint-failure-does-not-fail-commit");
    // Clamped to one frame's worth of bytes by `with_checkpoint_after_bytes`,
    // which is small enough that the very first commit's own frame already
    // crosses it — the first commit after open already tries to checkpoint.
    let store = DurableStore::open(&dir, config().with_checkpoint_after_bytes(1)).unwrap();

    // Generation 0 is what `open` on a fresh directory always starts at (see
    // `initialise`), so the checkpoint the first commit triggers publishes
    // under generation 1. Blocking that one name is enough regardless of how
    // many commits it takes to cross the trigger.
    let blocked_checkpoint = dir.join(format!("checkpoint.{:020}", 1));
    std::fs::create_dir(&blocked_checkpoint).unwrap();

    let outcome = store.put("key-000", record(0));

    assert!(
        outcome.is_ok(),
        "a write whose own record was already fsynced must not be reported as \
         failed by a checkpoint attempt that came after it: {outcome:?}"
    );
    assert_eq!(
        store.get("key-000").unwrap(),
        Some(record(0)),
        "the write must be visible even though the checkpoint it triggered could not complete"
    );
    assert!(
        store.stats().checkpoint_failures >= 1,
        "the failed checkpoint must be counted somewhere, or an operator has no way to learn \
         the log has stopped being compacted"
    );
    assert_eq!(
        store.stats().generation,
        0,
        "the checkpoint that could not publish must not have advanced the live generation"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- integrity --------------------------------------------------------------

#[test]
fn a_corrupted_record_is_reported_with_its_offset_and_never_returned() {
    let dir = temp_dir("corrupt-record");
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        for i in 0..5 {
            store.put(&format!("key-{i}"), record(i)).unwrap();
        }
    }

    let path = log_path(&dir);
    let mut bytes = std::fs::read(&path).unwrap();
    let extents = frame_extents(&bytes);
    // Flip a bit inside the payload of the second record. The record is whole,
    // so no truncation can explain it: this is a device returning bytes nobody
    // wrote.
    let target = extents[1].0 + 40 + 4;
    bytes[target] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();

    let error = DurableStore::open(&dir, config()).unwrap_err();
    assert_eq!(error.code(), "io", "corruption is a storage failure");
    let message = error.to_string();
    assert!(
        message.contains(&extents[1].0.to_string()),
        "the error must name the byte offset of the damage: {message}"
    );
    assert!(
        message.contains("digest mismatch"),
        "the error must say what failed: {message}"
    );
    assert!(
        message.contains("wal."),
        "the error must name the file: {message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verification_re_reads_every_record_without_a_restart() {
    let dir = temp_dir("verify");
    let store = DurableStore::open(&dir, config()).unwrap();
    for i in 0..6 {
        store.put(&format!("key-{i}"), record(i)).unwrap();
    }

    let report = store.verify().unwrap();
    assert_eq!(report.log_records, 6);
    assert!(report.bytes_verified > 0);

    // Damage the file underneath the running store: verification is what
    // catches bit rot that appeared after the store was opened.
    let path = log_path(&dir);
    let mut bytes = std::fs::read(&path).unwrap();
    let extents = frame_extents(&bytes);
    bytes[extents[0].0 + 45] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();

    let error = store.verify().unwrap_err();
    assert_eq!(error.code(), "io");
    assert!(error.to_string().contains("corrupt record"));

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_manifest_stops_the_store_from_guessing_which_generation_is_live() {
    let dir = temp_dir("corrupt-manifest");
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        store.put("k", json!(1)).unwrap();
    }
    let manifest = dir.join("MANIFEST");
    let text = std::fs::read_to_string(&manifest).unwrap();
    // A plausible-looking edit: the JSON still parses, the generation is a
    // number, and the digest no longer covers it.
    std::fs::write(
        &manifest,
        text.replace("\"generation\": 0", "\"generation\": 9"),
    )
    .unwrap();

    let error = DurableStore::open(&dir, config()).unwrap_err();
    assert_eq!(error.code(), "io");
    assert!(
        error.to_string().contains("checksum"),
        "{}",
        error.to_string()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_with_records_and_no_manifest_is_refused_rather_than_emptied() {
    let dir = temp_dir("missing-manifest");
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        for i in 0..5 {
            store.put(&format!("key-{i}"), record(i)).unwrap();
        }
    }
    std::fs::remove_file(dir.join("MANIFEST")).unwrap();

    let error = DurableStore::open(&dir, config()).unwrap_err();
    assert_eq!(error.code(), "io");
    let message = error.to_string();
    assert!(
        message.contains("refusing to reinitialise over data"),
        "starting over silently would destroy the records: {message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_store_over_the_same_directory_in_this_process_is_refused() {
    let dir = temp_dir("single-writer");
    let first = DurableStore::open(&dir, config()).unwrap();
    let error = DurableStore::open(&dir, config()).unwrap_err();
    assert_eq!(error.code(), "denied");
    assert!(error.to_string().contains("one writer per directory"));

    // The claim is only about this process, and the guard is released when the
    // store is dropped — not held until exit.
    drop(first);
    let reopened = DurableStore::open(&dir, config()).unwrap();
    assert!(reopened.is_empty().unwrap());

    drop(reopened);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_written_by_something_else_is_not_mistaken_for_a_log() {
    let dir = temp_dir("foreign-file");
    {
        let store = DurableStore::open(&dir, config()).unwrap();
        store.put("k", json!(1)).unwrap();
    }
    let path = log_path(&dir);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0..8].copy_from_slice(b"NOTOURS\x00");
    std::fs::write(&path, &bytes).unwrap();

    let error = DurableStore::open(&dir, config()).unwrap_err();
    assert!(
        error.to_string().contains("not written by this engine"),
        "{}",
        error.to_string()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// --- determinism ------------------------------------------------------------

#[test]
fn the_engine_stamps_commits_with_the_injected_clock_and_no_other() {
    let dir = temp_dir("clock");
    let start = Timestamp::from_civil(2026, 1, 1);
    let manual = Arc::new(ManualClock::new(start));
    let store =
        DurableStore::open(&dir, EngineConfig::new(manual.clone() as Arc<dyn Clock>)).unwrap();
    store.put("k", json!(1)).unwrap();
    manual.advance(qip_core::Duration::from_hours(3));
    store.put("k", json!(2)).unwrap();

    // The stamps are inside the log; what the test can assert from outside is
    // that the engine never reached for a wall clock — the store opened,
    // committed and recovered with nothing but the clock it was handed.
    drop(store);
    let reopened = DurableStore::open(&dir, config()).unwrap();
    assert_eq!(reopened.get("k").unwrap(), Some(json!(2)));

    let text = String::from_utf8_lossy(&std::fs::read(log_path(&dir)).unwrap()).into_owned();
    assert!(
        text.contains(&start.to_string()),
        "the first commit must carry the injected start time"
    );

    drop(reopened);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_error_from_the_engine_is_a_platform_error() {
    // The engine reports through the one error type that crosses crate
    // boundaries, so a caller handles storage failure the way it handles any
    // other failure.
    let dir = temp_dir("error-type");
    let store = DurableStore::open(&dir, config()).unwrap();
    let error: Error = store.range("z", "a").unwrap_err();
    assert_eq!(error.code(), "invalid");

    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
