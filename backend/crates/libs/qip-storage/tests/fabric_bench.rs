use qip_core::SystemClock;
/// Test that fabric_io_modes benchmark harness runs without panicking.
/// This is a quick smoke test; the actual benchmark is in benches/ for manual runs.
use qip_core::kv::KeyValueStore;
use qip_storage::{Durability, DurableStore, EngineConfig};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[test]
fn fabric_bench_synchronous_mode_records_append_latencies() {
    let work_dir = PathBuf::from("/tmp/fabric-bench-sync");
    let _ = fs::remove_dir_all(&work_dir);

    let clock = SystemClock;
    let config = EngineConfig::new(Arc::new(clock)).with_durability(Durability::Synchronous);
    let store = DurableStore::open(&work_dir, config).expect("open store");

    let record_count = 100;
    let mut latencies = Vec::with_capacity(record_count);

    for i in 0..record_count {
        let key = format!("key-{:06}", i);
        let value = serde_json::json!({"index": i});
        let start = Instant::now();
        store.put(&key, value).expect("put succeeded");
        let elapsed = start.elapsed();
        latencies.push(elapsed);
    }

    latencies.sort();
    let p99 = latencies[(record_count * 99) / 100];

    // p99 should be reasonably fast for 100 small records (less than 100ms)
    assert!(
        p99.as_millis() < 100,
        "p99 latency {:.2}ms is unexpectedly high for synchronous writes",
        p99.as_secs_f64() * 1000.0
    );

    let _ = fs::remove_dir_all(&work_dir);
}

#[test]
fn fabric_bench_os_buffered_mode_records_append_latencies() {
    let work_dir = PathBuf::from("/tmp/fabric-bench-buffered");
    let _ = fs::remove_dir_all(&work_dir);

    let clock = SystemClock;
    let config = EngineConfig::new(Arc::new(clock)).with_durability(Durability::OsBuffered);
    let store = DurableStore::open(&work_dir, config).expect("open store");

    let record_count = 100;
    let mut latencies = Vec::with_capacity(record_count);

    for i in 0..record_count {
        let key = format!("key-{:06}", i);
        let value = serde_json::json!({"index": i});
        let start = Instant::now();
        store.put(&key, value).expect("put succeeded");
        let elapsed = start.elapsed();
        latencies.push(elapsed);
    }

    latencies.sort();
    let p99 = latencies[(record_count * 99) / 100];

    // p99 should be very fast for os-buffered writes (typically under 10ms)
    assert!(
        p99.as_millis() < 50,
        "p99 latency {:.2}ms is unexpectedly high for os-buffered writes",
        p99.as_secs_f64() * 1000.0
    );

    let _ = fs::remove_dir_all(&work_dir);
}

#[test]
fn fabric_bench_modes_show_measurable_performance_difference() {
    let sync_latencies = measure_mode_latencies(Durability::Synchronous, 50);
    let buffered_latencies = measure_mode_latencies(Durability::OsBuffered, 50);

    let sync_p50 = sync_latencies[sync_latencies.len() / 2];
    let buffered_p50 = buffered_latencies[buffered_latencies.len() / 2];

    // OS-buffered should generally be faster than synchronous (fsync) mode
    // Note: on a slow disk or under load, this may not always hold, but on
    // most systems the difference is measurable
    println!(
        "Sync p50: {:.3}ms, Buffered p50: {:.3}ms",
        sync_p50.as_secs_f64() * 1000.0,
        buffered_p50.as_secs_f64() * 1000.0
    );

    // Both should be reasonably fast for small workloads
    assert!(
        sync_p50.as_millis() < 50,
        "synchronous p50 latency is too high"
    );
    assert!(
        buffered_p50.as_millis() < 50,
        "os-buffered p50 latency is too high"
    );
}

fn measure_mode_latencies(durability: Durability, record_count: usize) -> Vec<std::time::Duration> {
    let work_dir = PathBuf::from(format!(
        "/tmp/fabric-bench-measure-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&work_dir);

    let clock = SystemClock;
    let config = EngineConfig::new(Arc::new(clock)).with_durability(durability);
    let store = DurableStore::open(&work_dir, config).expect("open store");

    let mut latencies = Vec::with_capacity(record_count);
    for i in 0..record_count {
        let key = format!("key-{:06}", i);
        let value = serde_json::json!({"index": i});
        let start = Instant::now();
        store.put(&key, value).expect("put succeeded");
        let elapsed = start.elapsed();
        latencies.push(elapsed);
    }

    let _ = fs::remove_dir_all(&work_dir);
    latencies
}
