//! FABRIC-7043: `cargo bench -p qip-fabricd --bench fabric [-- <batches>
//! <records per batch> <payload bytes>]` prints produce and fetch latency
//! percentiles and produce throughput for one broker on loopback. See
//! `qip_fabricd::bench` for what the numbers do and do not describe.

use qip_fabricd::bench::{BenchSpec, run};

fn argument(position: usize, default: u32) -> Result<u32, String> {
    match std::env::args()
        .filter(|a| !a.starts_with('-'))
        .nth(position)
    {
        None => Ok(default),
        Some(text) => text
            .parse::<u32>()
            .map_err(|_| format!("argument {position} must be a whole number, not '{text}'")),
    }
}

fn main() {
    let spec = match (argument(1, 2_000), argument(2, 4), argument(3, 256)) {
        (Ok(batches), Ok(records_per_batch), Ok(payload_bytes)) => BenchSpec {
            batches,
            records_per_batch,
            payload_bytes,
        },
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            eprintln!("fabric-bench: {error}");
            std::process::exit(2);
        }
    };
    let root = std::env::temp_dir().join(format!("qip-fabric-bench-{}", std::process::id()));
    let outcome = run(&root, spec);
    let _ = std::fs::remove_dir_all(&root);
    match outcome {
        Ok(report) => print!("{report}"),
        Err(error) => {
            eprintln!("fabric-bench: {error}");
            std::process::exit(1);
        }
    }
}
