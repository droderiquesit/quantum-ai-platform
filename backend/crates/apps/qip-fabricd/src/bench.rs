//! FABRIC-7043's benchmark harness: produce and fetch latency percentiles
//! and throughput, measured against a real `qip-fabricd` on loopback through
//! the client SDK's production transport.
//!
//! The harness is a library function so that one test can prove it measures
//! what it says (`tests/bench.rs`) and `benches/fabric.rs` can run it at a
//! size worth reading. It is not a load test of a deployment: it measures one
//! broker on one machine, over loopback, with the archive in memory, and the
//! numbers it prints are evidence about this code path and nothing wider.
//! The platform has made latency claims about the fabric before it had a way
//! to measure them; this is the measurement, not a claim.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use qip_core::SystemClock;
use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_storage::MemoryBlobStore;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{FetchRequest, Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

use crate::config::{self, Config};

const STREAM: &str = "bench.journal";
const PRODUCER: &str = "bench-producer";
const CONSUMER: &str = "bench-consumer";

/// What one run measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchSpec {
    pub batches: u32,
    pub records_per_batch: u32,
    pub payload_bytes: u32,
}

/// Nearest-rank percentiles of a set of latencies, in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatencySummary {
    pub samples: usize,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

/// Nearest-rank percentiles of `samples`. Refuses an empty set rather than
/// reporting zeros nobody measured.
pub fn summarize(samples: &[u64]) -> Result<LatencySummary> {
    if samples.is_empty() {
        return Err(Error::invalid(
            "no latency was measured; a percentile of nothing is not zero",
        ));
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = |percent: usize| -> u64 {
        // Nearest rank: the smallest value with at least `percent`% of the
        // samples at or below it.
        let index = (percent * sorted.len()).div_ceil(100).max(1) - 1;
        sorted[index.min(sorted.len() - 1)]
    };
    Ok(LatencySummary {
        samples: sorted.len(),
        p50_us: rank(50),
        p90_us: rank(90),
        p99_us: rank(99),
        max_us: sorted[sorted.len() - 1],
    })
}

/// One run's results.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchReport {
    pub spec: BenchSpec,
    pub produce: LatencySummary,
    pub fetch: LatencySummary,
    /// Batches produced per second over the produce phase. A statistic, so
    /// an `f64`.
    pub produce_batches_per_second: f64,
}

impl std::fmt::Display for BenchReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let line = |f: &mut std::fmt::Formatter<'_>, what: &str, s: &LatencySummary| {
            writeln!(
                f,
                "{what:<8} n={:<6} p50={}us p90={}us p99={}us max={}us",
                s.samples, s.p50_us, s.p90_us, s.p99_us, s.max_us
            )
        };
        writeln!(
            f,
            "fabric-bench: {} batches x {} records x {} bytes, one broker on loopback",
            self.spec.batches, self.spec.records_per_batch, self.spec.payload_bytes
        )?;
        line(f, "produce", &self.produce)?;
        line(f, "fetch", &self.fetch)?;
        writeln!(
            f,
            "produce throughput {:.1} batches/s",
            self.produce_batches_per_second
        )
    }
}

fn token(identity: &str) -> String {
    format!("bench-fixture-token-for-{identity}-0000000000000000000000")
}

fn timeouts() -> Timeouts {
    Timeouts::new(
        StdDuration::from_secs(5),
        StdDuration::from_secs(30),
        StdDuration::from_secs(30),
    )
}

fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Start a broker rooted at `root` (which must be empty or absent), produce
/// `spec.batches` batches to one P2 partition, fetch each back one at a time,
/// and report both phases' latencies.
pub fn run(root: &Path, spec: BenchSpec) -> Result<BenchReport> {
    if spec.batches == 0 || spec.records_per_batch == 0 {
        return Err(Error::invalid(
            "a benchmark needs at least one batch of at least one record",
        ));
    }
    std::fs::create_dir_all(root)?;
    let catalogue = format!(
        r#"{{"streams":[{{"name":"{STREAM}","qos_class":"p2_market_journal","partition_key":"cell",
            "ordering":"per_partition","retention":"event_anchored","replication_factor":1,
            "mirroring":"none","overload_policy":"throttle_with_gap","ack_profile":"leader_only",
            "byte_quota_per_producer":1073741824,"message_quota_per_producer":100000000,
            "lag_limit":100000000,"entitlement_dataset":"qip-bench","entitlement_usage":"research",
            "seal_age_ms":1000,"peak_bytes_per_second":1073741824,"topics":[]}}],
          "grants":[
            {{"identity":"{PRODUCER}","stream":"{STREAM}","permission":"produce","key_scope":"any"}},
            {{"identity":"{CONSUMER}","stream":"{STREAM}","permission":"consume","key_scope":"any"}}]}}"#
    );
    std::fs::write(root.join("streams.json"), catalogue)?;
    let identities = [PRODUCER, CONSUMER]
        .iter()
        .map(|id| {
            format!(
                "{id} {}\n",
                qip_core::hash::sha256_hex(token(id).as_bytes())
            )
        })
        .collect::<String>();
    std::fs::write(root.join("identities"), identities)?;

    let path = |name: &str| root.join(name).display().to_string();
    let variables: BTreeMap<String, String> = [
        (config::DATA_DIR, path("data")),
        (config::CATALOGUE, path("streams.json")),
        (config::IDENTITIES_FILE, path("identities")),
        (config::ARCHIVE_DIR, path("archive")),
        (config::LISTEN, "127.0.0.1:0".to_string()),
        (config::HEALTH_LISTEN, "127.0.0.1:0".to_string()),
        (config::PARTITIONS, "1".to_string()),
        // ADR 0112: the harness owns both ends of a loopback socket and holds
        // no workload certificate, so the broker it starts is a `local`
        // plaintext one by construction. It is a measurement tool run on a
        // workstation, not a deployment.
        (config::DEPLOYMENT, "local".to_string()),
        (
            qip_transport::tls::TLS_MODE_VARIABLE,
            "plaintext".to_string(),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect();
    let config = Config::parse(&variables)?;
    let running = crate::start_with_archive(
        &config,
        Arc::new(SystemClock),
        Some(Arc::new(MemoryBlobStore::new())),
    )?;
    let measured = measure(running.address(), spec);
    running.stop();
    measured
}

fn measure(address: &str, spec: BenchSpec) -> Result<BenchReport> {
    let base = format!("http://{address}");
    let transport = |identity: &str| -> Result<HttpTransport> {
        Ok(HttpTransport::new(
            base.clone(),
            BearerToken::new(token(identity))?,
        ))
    };
    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(transport(PRODUCER)?),
        stream: STREAM.to_string(),
        partition: 0,
        producer_id: PRODUCER.to_string(),
        qos_class: QosClass::P2MarketJournal,
        ack_profile: QosClass::P2MarketJournal.ack_floor(),
        retry_policy: RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        },
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(ThreadSleeper),
        retry_seed: 1,
        breaker_seed: 1,
        timeouts: timeouts(),
    })?;
    producer.init()?;

    let payload = vec![b'x'; spec.payload_bytes as usize];
    let mut produce = Vec::with_capacity(spec.batches as usize);
    let produce_started = Instant::now();
    for batch_number in 0..spec.batches {
        let records = (0..spec.records_per_batch)
            .map(|record| Record {
                event_id: format!("bench-{batch_number}-{record}"),
                trace_id: None,
                source_timestamp_ns: i64::from(batch_number),
                payload: payload.clone(),
            })
            .collect();
        let batch = Batch::new(
            MessageType::Data,
            QosClass::P2MarketJournal.batch_schema_id(),
            BATCH_SCHEMA_VERSION,
            PayloadCodec::CanonicalJson,
            records,
        )?
        .with_provenance(qip_events::event_fabric::codec::Provenance::Fabricd);
        let started = Instant::now();
        producer.send(batch)?;
        produce.push(micros(started));
    }
    let produce_elapsed = produce_started.elapsed().as_secs_f64();

    let mut consumer = transport(CONSUMER)?;
    let mut fetch = Vec::with_capacity(spec.batches as usize);
    for offset in 0..u64::from(spec.batches) {
        let started = Instant::now();
        let answer = consumer.call(
            Request::Fetch(FetchRequest {
                stream: STREAM.to_string(),
                partition: 0,
                offset,
                // One byte of credit: the broker always serves one batch, so
                // every fetch is one batch and every sample one round trip.
                max_bytes: 1,
            }),
            timeouts(),
        )?;
        fetch.push(micros(started));
        match answer {
            Response::Fetch(fetched) if !fetched.batches().is_empty() => {}
            other => {
                return Err(Error::io(format!(
                    "fetch at offset {offset} returned no batch: {other:?}"
                )));
            }
        }
    }

    Ok(BenchReport {
        spec,
        produce: summarize(&produce)?,
        fetch: summarize(&fetch)?,
        produce_batches_per_second: if produce_elapsed > 0.0 {
            f64::from(spec.batches) / produce_elapsed
        } else {
            f64::INFINITY
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nearest-rank on a set whose answer is known by hand.
    ///
    /// Mutation: replace `.div_ceil(100)` with `/ 100` — p99 of ten samples
    /// rounds down to the ninth value, 9, and the summary assertion fails.
    #[test]
    fn percentiles_are_nearest_rank_and_an_empty_set_is_refused() {
        let samples: Vec<u64> = (1..=10).rev().collect();
        let summary = summarize(&samples).expect("non-empty");
        assert_eq!(
            summary,
            LatencySummary {
                samples: 10,
                p50_us: 5,
                p90_us: 9,
                p99_us: 10,
                max_us: 10,
            }
        );
        assert_eq!(summarize(&[7]).expect("one").p99_us, 7);
        assert!(summarize(&[]).is_err());
    }
}
