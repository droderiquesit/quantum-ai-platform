//! `qip_transport::event_fabric::batcher::Batcher` (CONTRACT-043): many
//! records, few transport writes, and no record held past the latency budget.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use qip_core::error::{Error, Result};
use qip_core::time::SystemClock;
use qip_core::{CorrelationId, Duration, EventId, Lineage, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, QosClass};
use qip_events::{Envelope, EventBody, Topic};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::batcher::{Batcher, BatcherConfig};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::{ProduceAck, ProducerInitResponse, Request, Response};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::{RecordingSleeper, RetryPolicy};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tick {
    symbol: String,
}

impl EventBody for Tick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
}

fn record(n: u64) -> Record {
    let lineage = Lineage {
        correlation_id: CorrelationId::from_string("COR00000000000000000000001"),
        causation_id: None,
        trace_id: None,
        producer: "batcher-test".to_string(),
    };
    let at = Timestamp::from_civil(2026, 3, 1);
    let event = Envelope::new(
        EventId::from_string(format!("EVT{n:023}")),
        at,
        at,
        lineage,
        Tick {
            symbol: "SOLO".to_string(),
        },
    )
    .erase()
    .expect("Tick erases to AnyEvent");
    Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes")
}

fn config(max_records: usize, linger_ms: i64) -> BatcherConfig {
    BatcherConfig {
        message_type: MessageType::Data,
        schema_id: 1,
        schema_version: 1,
        encoding: PayloadCodec::CanonicalJson,
        max_records,
        max_linger: Duration::from_millis(linger_ms),
    }
}

/// Counts every produce the producer sends; answers each as accepted.
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
                Ok(Response::Produce(ProduceAck::new("orders", 0, 0, 1, 1)?))
            }
            other => Err(Error::invalid(format!("unexpected call {other:?}"))),
        }
    }
}

fn producer(produces: Arc<AtomicUsize>) -> Producer {
    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(CountingTransport { produces }),
        stream: "orders".to_string(),
        partition: 0,
        producer_id: "cell-eu-1".to_string(),
        qos_class: QosClass::P2MarketJournal,
        ack_profile: AckProfile::LeaderOnly,
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(RecordingSleeper::new()),
        retry_seed: 3,
        breaker_seed: 3,
        timeouts: Timeouts::default(),
    })
    .expect("a well-formed config builds a producer");
    producer.init().expect("init answers");
    producer
}

fn at(ms: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_millis(ms))
}

/// Mutation: make `Batcher::push` flush on every record (return
/// `self.flush()` unconditionally) — the transport then sees one write per
/// record and the `writes < records` assertion fails.
#[test]
fn a_fixed_rate_of_records_makes_fewer_transport_writes_than_records_and_none_waits_past_the_linger()
 {
    const RECORDS: u64 = 100;
    const LINGER_MS: i64 = 20;
    let produces = Arc::new(AtomicUsize::new(0));
    let mut producer = producer(produces.clone());
    let mut batcher = Batcher::new(config(10, LINGER_MS)).expect("a stated budget builds");

    // One record per millisecond; poll the linger at each tick before the push.
    let mut arrival = Vec::new();
    let mut sent_records = 0usize;
    let mut worst_wait_ms = 0i64;
    let mut send = |batch: Batch, now_ms: i64, arrival: &mut Vec<i64>| {
        let n = batch.records.len();
        for waited_since in arrival.drain(..n) {
            worst_wait_ms = worst_wait_ms.max(now_ms - waited_since);
        }
        producer.send(batch).expect("the scripted broker accepts");
        n
    };
    for n in 0..RECORDS {
        let now_ms = n as i64;
        if let Some(batch) = batcher.due(at(now_ms)).unwrap() {
            sent_records += send(batch, now_ms, &mut arrival);
        }
        arrival.push(now_ms);
        if let Some(batch) = batcher.push(record(n), at(now_ms)).unwrap() {
            sent_records += send(batch, now_ms, &mut arrival);
        }
    }
    if let Some(batch) = batcher.flush().unwrap() {
        sent_records += send(batch, RECORDS as i64, &mut arrival);
    }

    let writes = produces.load(Ordering::SeqCst);
    assert_eq!(
        sent_records, RECORDS as usize,
        "premise: every record reached the transport exactly once"
    );
    assert_eq!(writes, 10, "100 records in batches of 10 is ten writes");
    assert!(writes < RECORDS as usize, "batching must save writes");
    assert!(
        worst_wait_ms <= LINGER_MS,
        "a record waited {worst_wait_ms} ms against a {LINGER_MS} ms budget"
    );
}

/// Mutation: make `Batcher::due` always return `Ok(None)` — the two records
/// below are then never released by the clock and the second assertion fails.
#[test]
fn a_slow_trickle_is_released_by_the_clock_and_a_partial_batch_waits_until_then() {
    let mut batcher = Batcher::new(config(100, 5)).unwrap();
    assert!(batcher.push(record(0), at(0)).unwrap().is_none());
    assert!(batcher.push(record(1), at(1)).unwrap().is_none());
    assert_eq!(batcher.pending(), 2, "premise: both records are held");

    assert!(batcher.due(at(4)).unwrap().is_none(), "inside the budget");
    let batch = batcher
        .due(at(5))
        .unwrap()
        .expect("the oldest record has now waited the whole linger");
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batcher.pending(), 0);
}

#[test]
fn a_batcher_without_a_positive_linger_or_any_capacity_is_refused() {
    assert!(Batcher::new(config(0, 5)).is_err(), "zero capacity");
    assert!(Batcher::new(config(10, 0)).is_err(), "zero linger");
    assert!(
        Batcher::new(config(10, 5)).is_ok(),
        "premise: a stated budget builds"
    );
}
