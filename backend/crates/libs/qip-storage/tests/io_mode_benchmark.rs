//! I/O mode benchmark: direct vs. buffered I/O for journal append and fetch.
//!
//! FABRIC-064 requires measuring append and fetch p99 latencies under both I/O
//! modes on the target disk, so the platform can choose the faster mode at
//! deployment time. This benchmark measures buffered I/O (the current
//! implementation) and documents the direct I/O variant.
//!
//! # Buffered I/O (current)
//!
//! Uses standard Rust file I/O (std::fs::File), which goes through the kernel's
//! page cache. Every write is acknowledged only after an explicit fsync(). The
//! kernel handles the actual device writes asynchronously.
//!
//! # Direct I/O (pending)
//!
//! Would use O_DIRECT (fcntl on Unix, FILE_FLAG_NO_BUFFERING on Windows) to
//! bypass the kernel page cache entirely. Writes go directly to the device;
//! fsync() becomes a no-op. Requires buffers aligned to the filesystem block
//! size (typically 4 KiB). The trade-off: lower latency variance (no page cache
//! overhead) but higher per-write cost (direct device I/O).
//!
//! To implement direct I/O in qip-storage:
//! - Add `libc` crate (requires an ADR per CLAUDE.md)
//! - Implement `IoMode::Direct` in `EngineConfig`
//! - Modify `FrameLog::create` and `FrameLog::open_at` to use `libc::open`
//!   with `libc::O_DIRECT` flag
//! - Allocate aligned buffers for writes using `libc::memalign` or similar
//! - Replace fsync() calls with direct I/O semantics (writes are durable
//!   immediately for most modern storage)

#![allow(clippy::unwrap_used, clippy::expect_used)] // benchmarks may unwrap

use qip_core::{Clock, ManualClock, Timestamp};
use qip_storage::engine::{Durability, DurableStore, EngineConfig, WriteBatch};
use qip_storage::kv::KeyValueStore;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// --- fixtures ---------------------------------------------------------------

static BENCHMARK_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = BENCHMARK_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-iobench-{label}-{}-{unique}",
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
    EngineConfig::new(clock()).with_checkpoint_after_bytes(100 * 1024 * 1024)
}

// --- latency measurement utilities ------------------------------------------

/// Measure the p99 latency of an operation over `count` iterations.
///
/// Collects all latencies, sorts them, and returns the 99th percentile.
fn measure_p99<F: FnMut()>(count: usize, mut operation: F) -> Duration {
    let mut latencies = Vec::with_capacity(count);

    for _ in 0..count {
        let start = Instant::now();
        operation();
        let elapsed = start.elapsed();
        latencies.push(elapsed);
    }

    latencies.sort();
    let p99_idx = (count * 99) / 100;
    latencies.get(p99_idx).copied().unwrap_or_default()
}

/// The record an append benchmark writes for `key_index` in batch `batch`.
///
/// The batch number is part of the value, so a key overwritten by every batch
/// still says which batch wrote it last. Without that, a reopened store
/// holding the first batch's value would read the same as one holding the
/// last batch's, and [`assert_every_append_survives_a_reopen`] could not tell
/// a log that took every append from a log that took one.
fn append_record(batch: usize, record_size_bytes: usize) -> serde_json::Value {
    json!({
        "value": "x".repeat(record_size_bytes.saturating_sub(100)),
        "size_bytes": record_size_bytes,
        "batch": batch,
    })
}

/// Measure the p99 latency of append operations.
///
/// Each append writes a `WriteBatch` containing `batch_size` records, each
/// roughly `record_size_bytes` in size, overwriting the same `batch_size`
/// keys every time.
fn measure_append_p99(
    store: &Arc<DurableStore>,
    batch_count: usize,
    batch_size: usize,
    record_size_bytes: usize,
) -> Duration {
    let mut batch_index = 0;
    measure_p99(batch_count, || {
        let mut batch = WriteBatch::new();
        for i in 0..batch_size {
            batch = batch.put(
                format!("append-key-{i}"),
                append_record(batch_index, record_size_bytes),
            );
        }
        store.commit(batch).unwrap();
        batch_index += 1;
    })
}

/// Measure the p99 latency of fetch (get) operations, and count the fetches
/// that returned the record [`write_fetch_dataset`] wrote under their key.
///
/// The count is the premise of the figure: a fetch p99 taken over misses is
/// the cost of looking up nothing, which an in-memory index answers fastest
/// of all.
fn measure_fetch_p99(
    store: &Arc<DurableStore>,
    read_count: usize,
    total_keys: usize,
) -> (Duration, usize) {
    let mut key_index = 0;
    let mut found = 0;
    let p99 = measure_p99(read_count, || {
        let key = format!("append-key-{}", key_index % total_keys);
        if store.get(&key).unwrap() == Some(fetch_record(key_index % total_keys)) {
            found += 1;
        }
        key_index = key_index.wrapping_add(1);
    });
    (p99, found)
}

/// The record the fetch benchmark stores under key `index`. Distinct per key,
/// so a fetch that returned a neighbour's record is not counted as found.
fn fetch_record(index: usize) -> serde_json::Value {
    json!({ "value": "x".repeat(100), "key": index })
}

/// Commit `total_keys` records in batches of a hundred.
fn write_fetch_dataset(store: &DurableStore, total_keys: usize) {
    let mut batch = WriteBatch::new();
    for i in 0..total_keys {
        batch = batch.put(format!("append-key-{i}"), fetch_record(i));
        if batch.len() >= 100 {
            store.commit(batch.clone()).unwrap();
            batch = WriteBatch::new();
        }
    }
    if !batch.is_empty() {
        store.commit(batch).unwrap();
    }
}

/// The deterministic half of an append benchmark: what was timed was a commit
/// the engine acknowledged and wrote to its log, every time.
///
/// The latency printed beside it is wall clock and is asserted against
/// nothing. These tests used to be skipped by default, because a wall-clock
/// ceiling on shared hardware fails for reasons unrelated to the code, and a
/// skipped test is a waived gate. What replaces the ceiling cannot vary with
/// the machine:
///
/// * the engine counted exactly `batch_count` commits, so no timed operation
///   was the empty batch `commit` returns from without writing; and
/// * after the store is dropped and reopened from its directory, every key
///   holds the record the *last* batch wrote. That is a store recovered from
///   its log, not the index the timed calls also updated, so a figure taken
///   over appends that never reached the log would fail here.
///
/// A reopen inside one process proves the bytes reached the file, not that
/// `fsync` returned. Power loss cannot be cut from a test, which is the
/// limit `qip_storage::engine`'s module documentation already names.
fn assert_every_append_survives_a_reopen(
    dir: &Path,
    store: Arc<DurableStore>,
    batch_count: usize,
    batch_size: usize,
    record_size_bytes: usize,
) {
    assert_eq!(
        store.stats().commits,
        batch_count as u64,
        "the engine acknowledged a different number of commits than were timed"
    );
    drop(store);
    let reopened = DurableStore::open(dir, config()).unwrap();
    let last = append_record(batch_count - 1, record_size_bytes);
    for i in 0..batch_size {
        assert_eq!(
            reopened.get(&format!("append-key-{i}")).unwrap(),
            Some(last.clone()),
            "append-key-{i} did not hold the last batch's record after a reopen, so the appends \
             timed above did not all reach the log"
        );
    }
}

// --- buffered I/O benchmark -------------------------------------------------

#[test]
fn small_record_appends_are_timed_and_every_one_reaches_the_log() {
    const BATCHES: usize = 1000;
    const BATCH_SIZE: usize = 10;
    const RECORD_BYTES: usize = 100;
    let dir = temp_dir("buffered-append-small");
    let config = config();
    // The figure below is labelled an fsync-per-commit figure, so the store
    // it is taken on must be the one that fsyncs every commit.
    assert_eq!(
        config.durability(),
        Durability::Synchronous,
        "the append figure is labelled fsync-per-commit and the store does not fsync every commit"
    );
    let store = Arc::new(DurableStore::open(&dir, config).unwrap());

    let p99 = measure_append_p99(&store, BATCHES, BATCH_SIZE, RECORD_BYTES);
    println!(
        "Buffered I/O, fsync per commit — append p99 (small records): {:.3} ms",
        p99.as_secs_f64() * 1000.0
    );

    assert_every_append_survives_a_reopen(&dir, store, BATCHES, BATCH_SIZE, RECORD_BYTES);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn large_record_appends_are_timed_and_every_one_reaches_the_log() {
    const BATCHES: usize = 100;
    const BATCH_SIZE: usize = 1;
    const RECORD_BYTES: usize = 10240;
    let dir = temp_dir("buffered-append-large");
    let config = config();
    assert_eq!(
        config.durability(),
        Durability::Synchronous,
        "the append figure is labelled fsync-per-commit and the store does not fsync every commit"
    );
    let store = Arc::new(DurableStore::open(&dir, config).unwrap());

    let p99 = measure_append_p99(&store, BATCHES, BATCH_SIZE, RECORD_BYTES);
    println!(
        "Buffered I/O, fsync per commit — append p99 (large records): {:.3} ms",
        p99.as_secs_f64() * 1000.0
    );

    assert_every_append_survives_a_reopen(&dir, store, BATCHES, BATCH_SIZE, RECORD_BYTES);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fetches_are_timed_and_every_one_finds_the_record_written_under_its_key() {
    const KEYS: usize = 10_000;
    const READS: usize = 1000;
    let dir = temp_dir("buffered-fetch");
    let store = Arc::new(DurableStore::open(&dir, config()).unwrap());
    write_fetch_dataset(&store, KEYS);
    assert_eq!(
        store.stats().keys,
        KEYS as u64,
        "premise: the dataset the fetches read was written in full"
    );

    let (p99, found) = measure_fetch_p99(&store, READS, KEYS);
    println!(
        "Buffered I/O — fetch p99 (in-memory index): {:.3} us",
        p99.as_secs_f64() * 1_000_000.0
    );

    // The deterministic half, in place of the `p99 < 1ms` ceiling this test
    // used to be skipped with: every timed fetch was a hit on the right record.
    assert_eq!(
        found, READS,
        "only {found} of {READS} timed fetches returned the record written under their key, so \
         the figure above is partly the cost of a miss"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

// --- direct I/O benchmark (documented for future implementation) ----------

#[test]
#[ignore]
fn direct_io_benchmark_not_yet_implemented() {
    // To implement this benchmark:
    //
    // 1. Add `libc` crate dependency (requires ADR):
    //    - This is a breaking change to the dependency policy (CLAUDE.md,
    //      ADR 0002)
    //
    // 2. Extend `IoMode` enum in `EngineConfig`:
    //    ```rust
    //    pub enum IoMode {
    //        Buffered,
    //        Direct, // requires O_DIRECT or FILE_FLAG_NO_BUFFERING
    //    }
    //    ```
    //
    // 3. Modify `FrameLog::create` to accept an `IoMode`:
    //    - Buffered: use `OpenOptions::new().write(true).create(true).open()`
    //    - Direct:  use `libc::open(path, O_WRONLY | O_CREAT | O_DIRECT, mode)`
    //
    // 4. Allocate aligned buffers for direct I/O:
    //    - Use `libc::memalign(4096, size)` to allocate 4 KiB aligned memory
    //    - Write data to aligned buffers only
    //
    // 5. For reads, use `libc::open()` with `O_DIRECT` and read into
    //    aligned buffers.
    //
    // 6. Remove fsync() calls for direct I/O (writes are durable
    //    immediately on most modern storage):
    //    - Buffered I/O: fsync() required
    //    - Direct I/O:   fsync() is typically a no-op for O_DIRECT
    //
    // The benchmark would then compare:
    //
    //   - Buffered append p99: ~5-50 ms (depends on disk + page cache)
    //   - Direct append p99:   ~1-10 ms (direct device I/O, less variance)
    //   - Buffered fetch p99:  ~1-5 us  (in-memory index, unchanged)
    //   - Direct fetch p99:    ~1-5 us  (in-memory index, unchanged)
    //
    // Expected outcome: Direct I/O wins on append latency variance; buffered
    // may win on throughput if the workload allows batching in the page cache.
    // The platform should choose based on the target disk type and measured
    // SLA for append latency.

    println!("Direct I/O benchmark requires libc crate and EngineConfig extension.");
    println!("Tracked as FABRIC-064 follow-on work after initial buffered benchmark.");
}

// --- full I/O mode comparison report ----------------------------------------

/// Run a complete I/O mode benchmark and print a summary.
///
/// This is a documentation test that shows how the two modes would be
/// compared. To run it, use:
/// ```
/// cargo test --test io_mode_benchmark -- --nocapture --ignored io_benchmark_report
/// ```
#[test]
#[ignore]
fn io_benchmark_report() {
    println!("\n=== I/O Mode Benchmark Report ===\n");

    let scenarios = vec![
        ("small-appends", 1000, 10, 100),
        ("large-appends", 100, 1, 10240),
    ];

    println!("Buffered I/O Results:");
    println!("---------------------");
    for (name, batch_count, batch_size, record_size) in scenarios {
        let dir = temp_dir(&format!("report-buffered-{name}"));
        let store = Arc::new(DurableStore::open(&dir, config()).unwrap());

        let p99 = measure_append_p99(&store, batch_count, batch_size, record_size);
        println!(
            "  {}: append p99 = {:.3} ms",
            name,
            p99.as_secs_f64() * 1000.0
        );
    }

    println!("\nDirect I/O Results:");
    println!("-------------------");
    println!("  [not yet implemented — awaiting libc crate addition via ADR]");

    println!("\nConclusion:");
    println!("----------");
    println!("Once direct I/O is implemented, the platform will choose the I/O mode");
    println!("that minimizes append p99 latency on the target disk type.");
    println!("The chosen mode will be recorded in the deployment configuration.");
}
