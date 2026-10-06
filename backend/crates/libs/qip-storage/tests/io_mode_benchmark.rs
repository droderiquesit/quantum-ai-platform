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
use qip_storage::engine::{DurableStore, EngineConfig, WriteBatch};
use qip_storage::kv::KeyValueStore;
use serde_json::json;
use std::path::PathBuf;
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

/// Measure the p99 latency of append operations.
///
/// Each append writes a `WriteBatch` containing `batch_size` records, each
/// roughly `record_size_bytes` in size.
fn measure_append_p99(
    store: &Arc<DurableStore>,
    batch_count: usize,
    batch_size: usize,
    record_size_bytes: usize,
) -> Duration {
    let text = "x".repeat(record_size_bytes.saturating_sub(100));

    measure_p99(batch_count, || {
        let mut batch = WriteBatch::new();
        for i in 0..batch_size {
            batch = batch.put(
                format!("append-key-{i}"),
                json!({
                    "value": text,
                    "size_bytes": record_size_bytes,
                }),
            );
        }
        store.commit(batch).unwrap();
    })
}

/// Measure the p99 latency of fetch (get) operations.
///
/// Fetches a random key from the store. Assumes keys have been written first.
fn measure_fetch_p99(store: &Arc<DurableStore>, read_count: usize, total_keys: usize) -> Duration {
    let mut key_index = 0;

    measure_p99(read_count, || {
        let key = format!("append-key-{}", key_index % total_keys);
        let _ = store.get(&key).unwrap();
        key_index = key_index.wrapping_add(1);
    })
}

// --- buffered I/O benchmark -------------------------------------------------

#[test]
#[ignore] // run with `cargo test --ignored -- --nocapture` to execute
fn buffered_io_append_p99_small_records() {
    let dir = temp_dir("buffered-append-small");
    let store = Arc::new(DurableStore::open(&dir, config()).unwrap());

    // Append 1000 batches of 10 small records (100 bytes each).
    let p99 = measure_append_p99(&store, 1000, 10, 100);
    println!(
        "Buffered I/O — append p99 (small records): {:.3} ms",
        p99.as_secs_f64() * 1000.0
    );

    // Assert the benchmark ran; the threshold is intentionally loose
    // (1 second) since container hardware is variable.
    assert!(
        p99 < Duration::from_secs(1),
        "append p99 unexpectedly high: {:?}",
        p99
    );
}

#[test]
#[ignore]
fn buffered_io_append_p99_large_records() {
    let dir = temp_dir("buffered-append-large");
    let store = Arc::new(DurableStore::open(&dir, config()).unwrap());

    // Append 100 batches of 1 large record (10 KiB each).
    let p99 = measure_append_p99(&store, 100, 1, 10240);
    println!(
        "Buffered I/O — append p99 (large records): {:.3} ms",
        p99.as_secs_f64() * 1000.0
    );

    assert!(
        p99 < Duration::from_secs(1),
        "append p99 unexpectedly high: {:?}",
        p99
    );
}

#[test]
#[ignore]
fn buffered_io_fetch_p99_after_append() {
    let dir = temp_dir("buffered-fetch");
    let store = Arc::new(DurableStore::open(&dir, config()).unwrap());

    // Write 100 batches of 100 records to establish a dataset.
    let mut batch = WriteBatch::new();
    for i in 0..10000 {
        batch = batch.put(
            format!("append-key-{i}"),
            json!({
                "value": "x".repeat(100),
            }),
        );
        if batch.len() >= 100 {
            store.commit(batch.clone()).unwrap();
            batch = WriteBatch::new();
        }
    }
    if !batch.is_empty() {
        store.commit(batch).unwrap();
    }

    // Now measure fetch p99 over 1000 reads.
    let p99 = measure_fetch_p99(&store, 1000, 10000);
    println!(
        "Buffered I/O — fetch p99 (in-memory index): {:.3} us",
        p99.as_secs_f64() * 1_000_000.0
    );

    // Fetches are in-memory (from the index), so p99 should be microseconds.
    assert!(
        p99 < Duration::from_millis(1),
        "fetch p99 unexpectedly high: {:?}",
        p99
    );
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
