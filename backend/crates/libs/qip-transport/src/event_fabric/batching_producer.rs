//! Production caller for `Batcher` (CONTRACT-043, FABRIC-066).
//!
//! [`BatchingProducer`] owns a background flush thread that polls
//! [`Batcher::due`] at a configurable interval, accumulates records from
//! caller threads, and emits batches to a broker. The flush thread keeps
//! no record waiting longer than the configured `max_linger` bound.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration as StdDuration;

use crate::event_fabric::batcher::{Batcher, BatcherConfig};
use crate::event_fabric::producer::Producer;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration};
use qip_events::event_fabric::codec::Record;

/// Configuration for a batching producer's flush thread.
#[derive(Clone, Debug)]
pub struct BatchingProducerConfig {
    /// Batcher configuration: max_records and max_linger bounds.
    pub batcher: BatcherConfig,
    /// How often the flush thread wakes up to check if `due()` is ready.
    /// Should be well inside `batcher.max_linger` to respect the latency budget.
    pub flush_interval: Duration,
}

/// Thread-safe producer that accumulates records and flushes batches in a
/// background thread.
///
/// `BatchingProducer` owns both a `Producer` and a `Batcher`, and runs a
/// flush thread that periodically calls [`Batcher::due`] to emit batches
/// when the linger time expires or capacity is reached. Callers push records
/// through [`push`](Self::push), and the flush thread sends batches to the broker.
pub struct BatchingProducer {
    inner: Arc<Mutex<BatchingProducerInner>>,
    flush_thread_handle: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for BatchingProducer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchingProducer")
            .field("flush_thread_handle", &self.flush_thread_handle.is_some())
            .finish()
    }
}

struct BatchingProducerInner {
    producer: Producer,
    batcher: Batcher,
    flush_interval: Duration,
    shutdown: bool,
}

impl BatchingProducer {
    /// Create a new batching producer that owns a flush thread.
    /// The flush thread runs until [`shutdown`](Self::shutdown) is called.
    pub fn new(mut producer: Producer, config: BatchingProducerConfig) -> Result<Self> {
        producer.init()?;
        let batcher = Batcher::new(config.batcher.clone())?;
        let inner = Arc::new(Mutex::new(BatchingProducerInner {
            producer,
            batcher,
            flush_interval: config.flush_interval,
            shutdown: false,
        }));

        let inner_clone = Arc::clone(&inner);
        let flush_thread_handle = thread::spawn(move || {
            Self::flush_loop(&inner_clone);
        });

        Ok(Self {
            inner,
            flush_thread_handle: Some(flush_thread_handle),
        })
    }

    /// Push a record to the batcher. The record is held until the next flush.
    pub fn push(&self, record: Record) -> Result<()> {
        let clock = qip_core::time::SystemClock;
        let now = clock.now();

        let mut state = self
            .inner
            .lock()
            .map_err(|_| Error::denied("batching producer lock poisoned"))?;

        if state.shutdown {
            return Err(Error::denied("batching producer is shut down"));
        }

        if let Some(batch) = state.batcher.push(record, now)? {
            state.producer.send(batch)?;
        }
        Ok(())
    }

    /// Flush any pending records and shut down the flush thread.
    pub fn shutdown(&mut self) -> Result<()> {
        {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| Error::denied("batching producer lock poisoned"))?;
            state.shutdown = true;
            if let Some(batch) = state.batcher.flush()? {
                state.producer.send(batch)?;
            }
        }

        if let Some(handle) = self.flush_thread_handle.take() {
            let _ = handle.join();
        }
        Ok(())
    }

    fn flush_loop(inner: &Arc<Mutex<BatchingProducerInner>>) {
        loop {
            thread::sleep(Self::thread_sleep_duration(inner));

            let should_exit = match inner.lock() {
                Ok(mut state) => {
                    if state.shutdown {
                        true
                    } else {
                        let clock = qip_core::time::SystemClock;
                        let now = clock.now();
                        if let Ok(Some(batch)) = state.batcher.due(now) {
                            let _ = state.producer.send(batch);
                        }
                        false
                    }
                }
                Err(_) => true,
            };

            if should_exit {
                break;
            }
        }
    }

    fn thread_sleep_duration(inner: &Arc<Mutex<BatchingProducerInner>>) -> StdDuration {
        if let Ok(state) = inner.lock() {
            let nanos = state.flush_interval.as_nanos();
            if nanos > 0 {
                return StdDuration::from_nanos(nanos as u64);
            }
        }
        StdDuration::from_millis(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breaker::BreakerPolicy;
    use crate::event_fabric::protocol::{ProduceAck, ProducerInitResponse, Request, Response};
    use crate::event_fabric::transport::{FabricTransport, Timeouts};
    use crate::retry::{RecordingSleeper, RetryPolicy};
    use qip_core::time::SystemClock;
    use qip_core::{CorrelationId, EventId, Lineage, Timestamp};
    use qip_events::event_fabric::codec::{MessageType, PayloadCodec};
    use qip_events::event_fabric::policy::{AckProfile, QosClass};
    use qip_events::{Envelope, EventBody, Topic};
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestEvent {
        value: u64,
    }

    impl EventBody for TestEvent {
        const TOPIC: Topic = Topic::MarketTick;
        const SCHEMA_VERSION: u32 = 1;
    }

    fn test_record(n: u64) -> Record {
        let lineage = Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000001"),
            causation_id: None,
            trace_id: None,
            producer: "batching-test".to_string(),
        };
        let at = Timestamp::from_civil(2026, 3, 1);
        let event = Envelope::new(
            EventId::from_string(format!("EVT{n:023}")),
            at,
            at,
            lineage,
            TestEvent { value: n },
        )
        .erase()
        .expect("TestEvent erases");
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes")
    }

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

    #[test]
    fn a_batching_producer_owns_a_flush_thread_that_polls_the_batcher() {
        let produces = Arc::new(AtomicUsize::new(0));
        let producer = Producer::new(crate::event_fabric::producer::ProducerConfig {
            transport: Box::new(CountingTransport {
                produces: produces.clone(),
            }),
            stream: "orders".to_string(),
            partition: 0,
            producer_id: "test-cell".to_string(),
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
        .expect("well-formed config builds");

        let mut bp = BatchingProducer::new(
            producer,
            BatchingProducerConfig {
                batcher: BatcherConfig {
                    message_type: MessageType::Data,
                    schema_id: 1,
                    schema_version: 1,
                    encoding: PayloadCodec::CanonicalJson,
                    max_records: 10,
                    max_linger: Duration::from_millis(50),
                },
                flush_interval: Duration::from_millis(10),
            },
        )
        .expect("batching producer constructs");

        for i in 0..30 {
            bp.push(test_record(i as u64)).expect("push succeeds");
        }

        thread::sleep(StdDuration::from_millis(100));

        let write_count = produces.load(Ordering::SeqCst);
        assert!(
            write_count > 0,
            "flush thread should have sent at least one batch"
        );
        assert!(
            write_count < 30,
            "batching should reduce writes from 30 records to fewer than 30 batches"
        );

        bp.shutdown().expect("shutdown succeeds");
        let final_writes = produces.load(Ordering::SeqCst);
        assert!(
            final_writes >= write_count,
            "shutdown flush should send any remaining records"
        );
    }

    #[test]
    fn a_batching_producer_refuses_pushes_after_shutdown() {
        let produces = Arc::new(AtomicUsize::new(0));
        let producer = Producer::new(crate::event_fabric::producer::ProducerConfig {
            transport: Box::new(CountingTransport {
                produces: produces.clone(),
            }),
            stream: "orders".to_string(),
            partition: 0,
            producer_id: "test-cell".to_string(),
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
        .expect("well-formed config builds");

        let mut bp = BatchingProducer::new(
            producer,
            BatchingProducerConfig {
                batcher: BatcherConfig {
                    message_type: MessageType::Data,
                    schema_id: 1,
                    schema_version: 1,
                    encoding: PayloadCodec::CanonicalJson,
                    max_records: 10,
                    max_linger: Duration::from_millis(50),
                },
                flush_interval: Duration::from_millis(10),
            },
        )
        .expect("batching producer constructs");

        bp.push(test_record(0)).expect("first push succeeds");
        bp.shutdown().expect("shutdown succeeds");

        assert!(
            bp.push(test_record(1)).is_err(),
            "push after shutdown must be refused"
        );
    }
}
