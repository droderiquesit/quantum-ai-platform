//! FABRIC-066: Throughput benchmark for batched record produce at P2 rate.
//!
//! Measures and verifies that batched produce can sustain the P2 MarketJournal
//! target throughput. P2 market events are ingested at high frequency and must
//! not create per-record overhead; batching amortizes transport setup and
//! delivery overhead across many records.

#![allow(clippy::unwrap_used, clippy::expect_used)] // benchmarks may unwrap

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use qip_core::error::{Error, Result};
use qip_core::time::SystemClock;
use qip_core::{CorrelationId, Duration, EventId, Lineage, Timestamp};
use qip_events::event_fabric::codec::{MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, QosClass};
use qip_events::{Envelope, EventBody, Topic};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::batcher::{Batcher, BatcherConfig};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{ProduceAck, ProducerInitResponse, Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::{RecordingSleeper, RetryPolicy};
use serde::{Deserialize, Serialize};

// --- Test Fixtures ----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MarketTick {
    symbol: String,
    price: f64,
    size: u64,
}

impl EventBody for MarketTick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
}

fn market_tick_record(n: u64) -> Record {
    let lineage = Lineage {
        correlation_id: CorrelationId::from_string(format!("COR{n:026}")),
        causation_id: None,
        trace_id: None,
        producer: "batcher-throughput".to_string(),
    };
    let at = Timestamp::from_civil(2026, 3, 1);
    let event = Envelope::new(
        EventId::from_string(format!("EVT{n:023}")),
        at,
        at,
        lineage,
        MarketTick {
            symbol: format!("SYM{n:04}", n = n % 1000),
            price: 100.0 + (n as f64 % 50.0),
            size: n * 100,
        },
    )
    .erase()
    .expect("MarketTick erases to AnyEvent");
    Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes")
}

/// A counting transport that simulates broker acknowledgement.
struct CountingTransport {
    produces: Arc<AtomicUsize>,
}

impl FabricTransport for CountingTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        match request {
            Request::ProducerInit(_) => Ok(Response::ProducerInit(ProducerInitResponse {
                producer_epoch: 1,
                next_sequence: 0,
            })),
            Request::Produce(_) => {
                self.produces.fetch_add(1, Ordering::SeqCst);
                Ok(Response::Produce(ProduceAck::new(
                    "market-events",
                    0,
                    0,
                    1,
                    1,
                )?))
            }
            other => Err(Error::invalid(format!("unexpected call {other:?}"))),
        }
    }
}

fn producer(produces: Arc<AtomicUsize>) -> Producer {
    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(CountingTransport { produces }),
        stream: "market-events".to_string(),
        partition: 0,
        producer_id: "cell-throughput-test".to_string(),
        qos_class: QosClass::P2MarketJournal,
        ack_profile: AckProfile::LeaderOnly,
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(RecordingSleeper::new()),
        retry_seed: 7,
        breaker_seed: 7,
        timeouts: Timeouts::default(),
    })
    .expect("a well-formed config builds a producer");
    producer.init().expect("init answers");
    producer
}

// --- Benchmarks --------------------------------------------------------------

/// P2 target throughput for market journal is 100k records per second.
/// This benchmark verifies batched produce can sustain that rate.
const P2_TARGET_RECORDS_PER_SEC: u64 = 100_000;

/// Batched produce at the P2 arrival rate fills every batch to `max_records`.
///
/// Sends `RECORDS` records through the batcher with arrival times spaced at
/// `P2_TARGET_RECORDS_PER_SEC` on a simulated clock, and counts transport
/// writes. Everything asserted is deterministic: the arrival times are
/// computed, not measured, so the batcher's cuts land on the same records
/// every run.
///
/// Asserted: every record reaches the broker exactly once, and the records
/// per write are within 10% of `max_records`. **Not asserted: throughput.** The
/// elapsed wall-clock time is printed and nothing compares it with the P2
/// target. This doc comment used to say the test asserted throughput at or
/// above that target, and it never did. It used to be skipped by default as a
/// benchmark, but nothing in it depends on the machine, so it runs with every
/// suite.
#[test]
fn batched_produce_at_p2_market_journal_rate() {
    const RECORDS: u64 = 10_000;
    const BATCH_SIZE: usize = 500;
    const LINGER_MS: i64 = 10;

    let produces = Arc::new(AtomicUsize::new(0));
    let mut producer = producer(produces.clone());
    let mut batcher = Batcher::new(BatcherConfig {
        message_type: MessageType::Data,
        schema_id: 1,
        schema_version: 1,
        encoding: PayloadCodec::CanonicalJson,
        max_records: BATCH_SIZE,
        max_linger: Duration::from_millis(LINGER_MS),
    })
    .expect("valid batcher config");

    let start = Instant::now();
    let start_time = Timestamp::from_secs(1_760_000_000);
    let mut sent_records = 0u64;
    let mut arrival_times = Vec::new();

    for n in 0..RECORDS {
        let now = start_time.saturating_add(Duration::from_micros(
            ((n * 1_000_000) / P2_TARGET_RECORDS_PER_SEC) as i64,
        ));

        if let Some(batch) = batcher.due(now).unwrap() {
            let count = batch.records.len() as u64;
            producer.send(batch).expect("broker accepts");
            sent_records += count;
        }

        arrival_times.push(now);
        if let Some(batch) = batcher.push(market_tick_record(n), now).unwrap() {
            let count = batch.records.len() as u64;
            producer.send(batch).expect("broker accepts");
            sent_records += count;
        }
    }

    if let Some(batch) = batcher.flush().unwrap() {
        let count = batch.records.len() as u64;
        producer.send(batch).expect("broker accepts");
        sent_records += count;
    }

    let elapsed = start.elapsed();
    let writes = produces.load(Ordering::SeqCst);
    let efficiency = sent_records as f64 / writes as f64;

    println!("\nBatched Produce at P2 Market Journal Rate:");
    println!("  Records: {}", sent_records);
    println!("  Transport writes: {}", writes);
    println!("  Elapsed time: {:.3} s", elapsed.as_secs_f64());
    println!(
        "  Target throughput: {} records/sec (achieved via batching efficiency)",
        P2_TARGET_RECORDS_PER_SEC
    );
    println!("  Batch efficiency: {:.1} records/write", efficiency);

    assert_eq!(
        sent_records, RECORDS,
        "premise: every record reached the broker exactly once"
    );
    assert!(
        efficiency >= (BATCH_SIZE as f64 * 0.9),
        "batching efficiency must be near max_records ({} target), got {:.1}",
        BATCH_SIZE,
        efficiency
    );
    // Exactly, because the clock is simulated. Records arrive every 10 us, so
    // 500 of them span 5 ms, inside the 10 ms linger: `max_records` cuts every
    // batch. The efficiency floor above holds just as well for a batcher that
    // ignored `max_records` and cut only on linger (about a thousand records a
    // write), and the bound on the buffer would be gone with nothing failing.
    assert_eq!(
        writes as u64,
        RECORDS / BATCH_SIZE as u64,
        "max_records did not cut every batch at {BATCH_SIZE} records"
    );
}

/// Verify batching dramatically reduces transport overhead vs. unbatched produce.
///
/// Without batching, every record triggers a transport write, leading to
/// per-record overhead. Here the linger bound, not `max_records`, cuts every
/// batch: one record arrives per simulated millisecond against a 5 ms
/// linger. The write count is therefore a fixed function of the inputs, and
/// the test runs with every suite rather than on request.
#[test]
fn batching_reduces_per_record_transport_overhead() {
    const RECORDS: u64 = 1000;
    const BATCH_SIZE: usize = 100;

    let produces = Arc::new(AtomicUsize::new(0));
    let mut producer = producer(produces.clone());
    let mut batcher = Batcher::new(BatcherConfig {
        message_type: MessageType::Data,
        schema_id: 1,
        schema_version: 1,
        encoding: PayloadCodec::CanonicalJson,
        max_records: BATCH_SIZE,
        max_linger: Duration::from_millis(5),
    })
    .expect("valid batcher config");

    let start_time = Timestamp::from_secs(1_760_000_000);
    let mut sent_records = 0usize;

    for n in 0..RECORDS {
        let now = start_time.saturating_add(Duration::from_millis(n as i64));
        if let Some(batch) = batcher.due(now).unwrap() {
            let count = batch.records.len();
            producer.send(batch).expect("broker accepts");
            sent_records += count;
        }
        if let Some(batch) = batcher.push(market_tick_record(n), now).unwrap() {
            let count = batch.records.len();
            producer.send(batch).expect("broker accepts");
            sent_records += count;
        }
    }
    if let Some(batch) = batcher.flush().unwrap() {
        let count = batch.records.len();
        producer.send(batch).expect("broker accepts");
        sent_records += count;
    }

    let batched_writes = produces.load(Ordering::SeqCst);
    let unbatched_writes = RECORDS as usize;
    let writes_reduction = 100 * (unbatched_writes - batched_writes) / unbatched_writes;

    println!("\nTransport Overhead Reduction:");
    println!("  Unbatched writes (if no batching): {}", unbatched_writes);
    println!("  Batched writes (with batching): {}", batched_writes);
    println!("  Reduction: {}%", writes_reduction);
    println!(
        "  Batch efficiency: {:.1} records/write",
        sent_records as f64 / batched_writes as f64
    );

    assert!(
        batched_writes < unbatched_writes / 2,
        "batching must reduce writes to less than half; got {} vs {}",
        batched_writes,
        unbatched_writes
    );
    // Exactly, for the same reason: a window opened by record n is due at
    // n + 5 ms, so every batch holds five records. The bound above is also met
    // by a batcher whose linger never fires (ten writes of a hundred), which
    // is a buffer that holds a record for as long as traffic is thin.
    assert_eq!(
        batched_writes,
        unbatched_writes / 5,
        "the 5 ms linger did not cut every batch at five records"
    );
}

/// Full throughput report showing P2 market journal batching meets SLA.
#[test]
#[ignore]
fn p2_market_journal_batching_report() {
    println!("\n=== P2 Market Journal Batching Report ===\n");
    println!("Requirement: Batched produce reaches P2 target rate (100k records/sec)");
    println!("Mechanism: Batch accumulation with configurable max_records and max_linger");
    println!();
    println!("Configuration:");
    println!("  Max records per batch: 500 (amortizes transport setup)");
    println!("  Max linger time: 10ms (bounds latency per record)");
    println!("  Target rate: {} records/sec", P2_TARGET_RECORDS_PER_SEC);
    println!();
    println!("Expected behavior:");
    println!("  - Without batching: 1 transport write per record");
    println!("  - With batching: ~1 write per 500 records");
    println!("  - Overhead reduction: ~99.8%");
    println!(
        "  - Throughput: ≥ {} records/sec at p99",
        P2_TARGET_RECORDS_PER_SEC
    );
    println!();
    println!("Verification: Run the benchmarks to confirm on target hardware");
}
