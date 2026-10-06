use qip_core::SystemClock;
/// Benchmark comparing buffered (with fsync) vs direct I/O modes on storage engine.
/// Measures append and fetch p99 latencies at varying throughput levels.
///
/// This harness is run manually to establish I/O performance characteristics on
/// the target disk type and is not part of the standard test suite.
use qip_core::kv::KeyValueStore;
use qip_storage::{Durability, DurableStore, EngineConfig};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let work_dir = PathBuf::from("/tmp/fabric-io-bench");
    let _ = fs::remove_dir_all(&work_dir);
    fs::create_dir_all(&work_dir).expect("create bench dir");

    println!("Fabric I/O Mode Benchmark");
    println!("=========================\n");

    // Warm up with a small store
    {
        println!("Warming up...");
        let store_dir = work_dir.join("warmup");
        benchmark_mode("warmup", &store_dir, Durability::Synchronous, 10);
        let _ = fs::remove_dir_all(&store_dir);
    }

    // Small batch workload (1K records)
    println!("\nSmall workload (1K records):");
    benchmark_workload(&work_dir, "small", 1000);

    // Medium batch workload (10K records)
    println!("\nMedium workload (10K records):");
    benchmark_workload(&work_dir, "medium", 10_000);

    let _ = fs::remove_dir_all(&work_dir);
    println!("\nBenchmark complete.");
}

fn benchmark_workload(base_dir: &PathBuf, name: &str, record_count: usize) {
    let store_dir = base_dir.join(name);
    println!("  Buffered I/O (fsync on every commit):");
    benchmark_mode(name, &store_dir, Durability::Synchronous, record_count);
    let _ = fs::remove_dir_all(&store_dir);

    println!("  OS-buffered (no fsync guarantee):");
    benchmark_mode(name, &store_dir, Durability::OsBuffered, record_count);
    let _ = fs::remove_dir_all(&store_dir);
}

fn benchmark_mode(_name: &str, store_dir: &PathBuf, durability: Durability, record_count: usize) {
    let _ = fs::remove_dir_all(store_dir);
    fs::create_dir_all(store_dir).expect("create store dir");

    let clock = SystemClock;
    let config = EngineConfig::new(Arc::new(clock)).with_durability(durability);
    let store = DurableStore::open(store_dir, config).expect("open store");

    // Append benchmark: measure latency of individual puts
    let mut append_latencies = Vec::with_capacity(record_count);
    for i in 0..record_count {
        let key = format!("key-{:06}", i);
        let value = serde_json::json!({"index": i, "data": "x".repeat(1024)});
        let start = Instant::now();
        store.put(&key, value).expect("put");
        let elapsed = start.elapsed();
        append_latencies.push(elapsed);
    }

    append_latencies.sort();
    let p50_append = append_latencies[record_count / 2];
    let p99_append = append_latencies[(record_count * 99) / 100];
    let p999_append = if record_count > 1000 {
        append_latencies[(record_count * 999) / 1000]
    } else {
        append_latencies[record_count - 1]
    };

    // Fetch benchmark: measure latency of individual gets after all appends
    let mut fetch_latencies = Vec::with_capacity(record_count / 10);
    for i in (0..record_count).step_by(10) {
        let key = format!("key-{:06}", i);
        let start = Instant::now();
        let _ = store.get(&key).expect("get");
        let elapsed = start.elapsed();
        fetch_latencies.push(elapsed);
    }

    fetch_latencies.sort();
    let p50_fetch = fetch_latencies[fetch_latencies.len() / 2];
    let p99_fetch = fetch_latencies[(fetch_latencies.len() * 99) / 100];

    println!(
        "    Append latency: p50={:.2}ms, p99={:.2}ms, p999={:.2}ms",
        p50_append.as_secs_f64() * 1000.0,
        p99_append.as_secs_f64() * 1000.0,
        p999_append.as_secs_f64() * 1000.0
    );
    println!(
        "    Fetch latency:  p50={:.2}ms, p99={:.2}ms",
        p50_fetch.as_secs_f64() * 1000.0,
        p99_fetch.as_secs_f64() * 1000.0
    );
}
