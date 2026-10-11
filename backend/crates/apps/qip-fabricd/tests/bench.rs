//! FABRIC-7043: the benchmark harness measures what it says — one sample
//! per produce and per fetch, against a real broker, ordered percentiles.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_fabricd::bench::{BenchSpec, run};

/// Mutation: in `bench::measure`, push one fetch sample after the loop
/// instead of one per call — `fetch.samples` is 1, not 24, and this fails.
/// Mutation: fetch from `offset + 1` — the last fetch returns no batch, the
/// harness refuses to report it, and the `expect` fires.
#[test]
fn the_bench_harness_reports_a_sample_per_produce_and_fetch_against_a_real_broker() {
    let root = std::env::temp_dir().join(format!("qip-fabric-bench-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let spec = BenchSpec {
        batches: 24,
        records_per_batch: 2,
        payload_bytes: 64,
    };
    let report = run(&root, spec).expect("the harness runs against a real broker");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(report.spec, spec);
    for summary in [report.produce, report.fetch] {
        assert_eq!(summary.samples, 24);
        assert!(summary.p50_us <= summary.p90_us);
        assert!(summary.p90_us <= summary.p99_us);
        assert!(summary.p99_us <= summary.max_us);
        assert!(summary.max_us > 0, "a round trip over loopback takes time");
    }
    assert!(report.produce_batches_per_second > 0.0);
    let printed = report.to_string();
    assert!(printed.contains("produce  n=24"), "{printed}");
    assert!(printed.contains("fetch    n=24"), "{printed}");
}
