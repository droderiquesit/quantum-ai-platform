//! The write side: one worker per partition of the P1 outcomes stream,
//! reading batches from the fabric broker and applying every record through
//! [`LedgerStore::apply`] (ADR 0100 §1, §4; LEDGER-8134, FABRIC-8092).
//!
//! This module was a placeholder whose `start` always returned an error, and
//! `main` built it against a hard-coded `127.0.0.1:9090` and never called
//! it. The store underneath was real and tested; nothing reached it.
//!
//! # The ledger's offset is the ledger's, not the broker's
//!
//! Where to resume is read from [`LedgerStore::resume_offset`], which the
//! store commits in the same write-ahead record as the postings it booked
//! (LEDGER-8024). The broker's group checkpoint is committed too, after the
//! store has, but only so the broker can see the ledger's lag — and refuse
//! P1 producers when the ledger falls past the stream's lag limit
//! (FABRIC-049). It is never read back to decide what to apply: an offset
//! held apart from the postings it describes is exactly the two-step write
//! the store exists to avoid.
//!
//! # A record's position: batch offset and index
//!
//! The broker assigns one offset per *batch*; the store places, deduplicates
//! and parks by one offset per *record*. A delivery's offset is therefore
//! `batch_offset * RECORD_STRIDE + index` ([`delivery_offset`]), with
//! [`RECORD_STRIDE`] larger than any batch the codec can decode can hold.
//! Two records of one batch never share a delivery offset — sharing one would
//! read to the store as "one offset arriving with two hashes", an integrity
//! break on every multi-record batch.
//!
//! # Never skip, retry for ever
//!
//! FABRIC-8092 asks for a ledger sink that retries indefinitely, because a
//! dropped fill is a ledger short for ever. Nothing here moves past a record
//! it has not applied:
//!
//! * a store error retries the same delivery, with a bounded backoff, until
//!   it commits or the process is stopped (`qip_ledger_store_retries_total`
//!   counts every retry);
//! * a fetch or join error waits and asks again from the same position;
//! * a record whose payload does not decode stalls its partition at that
//!   record, retrying, and says so on `/health` — skipping it would be
//!   skipping a record that may be a fill.
//!
//! A P1 `Gap` record is the one record not applied: it is the producer's own
//! statement that a window of its journal is missing, it books nothing, and
//! the next record of that cell does not chain onto the ledger's tail — so
//! the store parks the cell on the discontinuity, which is the refusal a gap
//! deserves. Records on any other topic are likewise not ledger records and
//! are passed over without moving the store.
//!
//! # The paper fence is the store's
//!
//! This module decodes and delivers; it does not judge a fill. A fill whose
//! `simulated` flag is not true is refused, counted and its cell parked by
//! the store (ADR 0106), and nothing here can route around that.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use qip_contracts::reflex::{ChainSpan, OutcomeRecord};
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, sha256_hex};
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::{
    EVENT_FABRIC_GAP, REFLEX_CHAIN_SPAN, REFLEX_OUTCOME_RECORDED, TopicBinding,
};
use qip_events::event_fabric::codec::Batch;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::consumer::{Consumer, ConsumerConfig};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, Sleeper, ThreadSleeper};
use serde::Serialize;

use crate::store::{Delivery, Disposition, LedgerStore, P1Record};
use crate::telemetry::LedgerTelemetry;

/// Delivery offsets reserved per broker batch. A batch is at most 32 MiB
/// (`codec::MAX_BATCH_LEN`) and every record carries at least its framing,
/// so no decodable batch holds anywhere near 2^24 records; one that claims
/// to is refused by [`delivery_offset`] rather than allowed to collide with
/// the next batch's positions.
pub const RECORD_STRIDE: u64 = 1 << 24;

/// The fetch credit each worker asks with: one generous batch.
pub const FETCH_CREDIT_BYTES: u32 = 4 * 1024 * 1024;

/// The longest a worker waits between retries of one failing step.
const MAXIMUM_BACKOFF_MS: u64 = 5_000;

/// Builds one transport to the broker. Each worker owns its own, because a
/// transport carries its bearer token and is not shared across threads.
pub type Connect = Arc<dyn Fn() -> Result<Box<dyn FabricTransport + Send>> + Send + Sync>;

/// The delivery offset of record `index` of the batch at `batch_offset`.
pub fn delivery_offset(batch_offset: u64, index: usize) -> Result<u64> {
    let index = u64::try_from(index)
        .ok()
        .filter(|i| *i < RECORD_STRIDE)
        .ok_or_else(|| {
            Error::invalid(format!(
                "batch {batch_offset} holds more than {RECORD_STRIDE} records; no batch the codec \
                 decodes can, so this one is refused rather than overlapping the next"
            ))
        })?;
    batch_offset
        .checked_mul(RECORD_STRIDE)
        .and_then(|base| base.checked_add(index))
        .ok_or_else(|| {
            Error::numeric(format!(
                "batch offset {batch_offset} is past what a delivery offset can carry"
            ))
        })
}

/// The batch a delivery offset falls in.
pub fn batch_of(delivery_offset: u64) -> u64 {
    delivery_offset / RECORD_STRIDE
}

/// The store's partition name for `(stream, partition)`.
pub fn partition_label(stream: &str, partition: u32) -> String {
    format!("{stream}:{partition}")
}

fn decode_as<T: for<'de> serde::Deserialize<'de>>(
    event: &AnyEvent,
    binding: TopicBinding,
) -> Result<T> {
    if event.schema_version > binding.schema_version {
        return Err(Error::schema(format!(
            "{} schema version {} is newer than the {} this ledger reads; upgrade the ledger \
             before the producer",
            event.topic, event.schema_version, binding.schema_version
        )));
    }
    serde_json::from_value(event.payload.clone()).map_err(|e| {
        Error::schema(format!(
            "a {} record's payload does not read back: {e}",
            event.topic
        ))
    })
}

/// What one record of a P1 batch is to the ledger.
#[derive(Clone, Debug, PartialEq)]
pub enum Decoded {
    /// A record the store applies.
    Ledger(Box<Delivery>),
    /// A producer-declared gap: books nothing; the chain discontinuity it
    /// leaves parks the cell at the store.
    Gap,
    /// A record on a topic that is not a ledger record.
    Other,
}

/// Decode every record of `batch` at or after `from` into what the store
/// is offered, in order. `Err` for a record whose payload does not decode:
/// the caller stalls there rather than skipping it.
pub fn decode_batch(partition: &str, batch: &Batch, from: u64) -> Result<Vec<(u64, Decoded)>> {
    let mut decoded = Vec::with_capacity(batch.records.len());
    for (index, record) in batch.records.iter().enumerate() {
        let offset = delivery_offset(batch.base_offset, index)?;
        if offset < from {
            continue;
        }
        let event = record.decode_payload(batch.encoding).map_err(|e| {
            let message = format!(
                "{partition} batch {} record {index} does not decode: {}",
                batch.base_offset,
                e.message()
            );
            e.relabelled(message)
        })?;
        let p1 = if event.topic == REFLEX_OUTCOME_RECORDED.topic {
            P1Record::Outcome(Box::new(decode_as::<OutcomeRecord>(
                &event,
                REFLEX_OUTCOME_RECORDED,
            )?))
        } else if event.topic == REFLEX_CHAIN_SPAN.topic {
            P1Record::Span(decode_as::<ChainSpan>(&event, REFLEX_CHAIN_SPAN)?)
        } else if event.topic == EVENT_FABRIC_GAP.topic {
            decoded.push((offset, Decoded::Gap));
            continue;
        } else {
            decoded.push((offset, Decoded::Other));
            continue;
        };
        decoded.push((
            offset,
            Decoded::Ledger(Box::new(Delivery {
                partition: partition.to_string(),
                offset,
                // The hash of the bytes the broker holds at this position:
                // the same position arriving with other bytes is an
                // integrity break the store refuses to choose between.
                record_hash: sha256_hex(&record.payload),
                record: p1,
            })),
        ));
    }
    Ok(decoded)
}

/// One partition worker's state, as `/health` reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PartitionStatus {
    /// Whether the group has assigned this partition to the ledger.
    pub joined: bool,
    /// The next delivery offset this worker will apply.
    pub position: u64,
    /// Records applied (posted, advanced, duplicate, parked or held) since
    /// start.
    pub applied: u64,
    /// Why the worker is not progressing, while it is not.
    pub last_error: Option<String>,
}

/// Every worker's status, by partition.
pub type Statuses = Arc<Mutex<BTreeMap<u32, PartitionStatus>>>;

/// What one [`PartitionWorker::step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The broker had nothing new.
    Idle,
    /// A batch was read and every record in it handed to the store.
    Applied { records: usize },
    /// A record asked for the partition to be re-read from an earlier
    /// position; the next step fetches from there.
    ReRead { from: u64 },
    /// Something failed and the same position will be tried again.
    Retry,
}

/// Everything a worker needs besides its transport.
#[derive(Clone)]
pub struct WorkerConfig {
    pub stream: String,
    pub partition: u32,
    pub group: String,
    pub poll: Duration,
    pub clock: Arc<dyn Clock>,
    pub sleeper: Arc<dyn Sleeper>,
}

impl std::fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerConfig")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("group", &self.group)
            .finish_non_exhaustive()
    }
}

/// Reads one partition and applies it. See the module documentation.
pub struct PartitionWorker {
    consumer: Consumer,
    store: Arc<LedgerStore>,
    telemetry: LedgerTelemetry,
    statuses: Statuses,
    stop: Arc<AtomicBool>,
    config: WorkerConfig,
    label: String,
    /// The next delivery offset to apply.
    position: u64,
    /// Whether the consumer has been moved to `position`'s batch.
    seeked: bool,
    joined: bool,
    failures: u32,
}

impl std::fmt::Debug for PartitionWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartitionWorker")
            .field("label", &self.label)
            .field("position", &self.position)
            .field("joined", &self.joined)
            .finish_non_exhaustive()
    }
}

impl PartitionWorker {
    /// A worker for `config.partition`, resuming where the store says.
    pub fn new(
        config: WorkerConfig,
        transport: Box<dyn FabricTransport + Send>,
        store: Arc<LedgerStore>,
        telemetry: LedgerTelemetry,
        statuses: Statuses,
        stop: Arc<AtomicBool>,
    ) -> Result<Self> {
        let label = partition_label(&config.stream, config.partition);
        let consumer = Consumer::new(ConsumerConfig {
            transport,
            stream: config.stream.clone(),
            partition: config.partition,
            group_id: config.group.clone(),
            retry_policy: RetryPolicy::default(),
            breaker_policy: BreakerPolicy::default(),
            clock: config.clock.clone(),
            sleeper: config.sleeper.clone(),
            retry_seed: u64::from(config.partition),
            breaker_seed: u64::from(config.partition).wrapping_add(1),
            timeouts: Timeouts::default(),
            fetch_credit_bytes: FETCH_CREDIT_BYTES,
        })?;
        let position = store.resume_offset(&label)?;
        let worker = Self {
            consumer,
            store,
            telemetry,
            statuses,
            stop,
            config,
            label,
            position,
            seeked: false,
            joined: false,
            failures: 0,
        };
        worker.report(None);
        Ok(worker)
    }

    /// The next delivery offset this worker will apply.
    pub fn position(&self) -> u64 {
        self.position
    }

    fn report(&self, error: Option<String>) {
        let mut statuses = self.statuses.lock().unwrap_or_else(|e| e.into_inner());
        let status = statuses.entry(self.config.partition).or_default();
        status.joined = self.joined;
        status.position = self.position;
        status.last_error = error;
    }

    fn count_applied(&self, records: u64) {
        let mut statuses = self.statuses.lock().unwrap_or_else(|e| e.into_inner());
        let status = statuses.entry(self.config.partition).or_default();
        status.applied = status.applied.saturating_add(records);
    }

    fn backoff(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let poll_ms = u64::try_from(self.config.poll.as_millis()).unwrap_or(MAXIMUM_BACKOFF_MS);
        let shift = self.failures.min(6);
        let ms = poll_ms
            .saturating_mul(1u64 << shift)
            .min(MAXIMUM_BACKOFF_MS);
        Duration::from_millis(i64::try_from(ms).unwrap_or(5_000))
    }

    fn fail(&mut self, error: &Error) -> Step {
        self.report(Some(error.to_string()));
        let wait = self.backoff();
        self.config.sleeper.sleep(wait);
        Step::Retry
    }

    /// Apply `delivery` until the store commits it or the worker is stopped.
    fn apply_until_committed(&mut self, delivery: &Delivery) -> Result<Disposition> {
        loop {
            let started = self.config.clock.now();
            match self.store.apply(delivery) {
                Ok(disposition) => {
                    // Nanoseconds to milliseconds for a latency histogram: a
                    // statistic, and the one place this crosses to f64.
                    let elapsed = (self.config.clock.now().as_nanos() - started.as_nanos()) as f64
                        / 1_000_000.0;
                    self.telemetry.commit_latency_ms(elapsed);
                    return Ok(disposition);
                }
                Err(error) => {
                    self.telemetry.store_retry();
                    self.report(Some(format!(
                        "the store refused delivery {} of {}; retrying the same record: {error}",
                        delivery.offset, self.label
                    )));
                    if self.stop.load(Ordering::SeqCst) {
                        return Err(error);
                    }
                    let wait = self.backoff();
                    self.config.sleeper.sleep(wait);
                }
            }
        }
    }

    /// One step: join if needed, fetch one batch from the current position,
    /// apply what it holds. Never moves past a record it did not apply.
    pub fn step(&mut self) -> Step {
        if !self.joined {
            if let Err(error) = self.consumer.join() {
                return self.fail(&error);
            }
            self.joined = true;
        }
        if !self.seeked {
            self.consumer.seek(batch_of(self.position));
            self.seeked = true;
        }
        let fetched = match self.consumer.fetch() {
            Ok(Some(fetched)) => fetched,
            Ok(None) => {
                self.failures = 0;
                self.report(None);
                return Step::Idle;
            }
            Err(error) => {
                // Re-seek on the next step: the consumer may have advanced
                // past a batch this step never applied.
                self.seeked = false;
                return self.fail(&error);
            }
        };
        let batch = fetched.batch;
        self.telemetry.lag(
            &self.label,
            fetched
                .high_watermark
                .saturating_sub(batch.base_offset.saturating_add(1)),
        );
        let decoded = match decode_batch(&self.label, &batch, self.position) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.seeked = false;
                return self.fail(&error);
            }
        };
        let mut applied = 0usize;
        for (offset, record) in decoded {
            if let Decoded::Ledger(delivery) = record {
                let disposition = match self.apply_until_committed(&delivery) {
                    Ok(disposition) => disposition,
                    Err(error) => {
                        self.seeked = false;
                        return self.fail(&error);
                    }
                };
                applied += 1;
                self.count_applied(1);
                if let Disposition::ReReadFrom { offset: from } = disposition {
                    self.position = from;
                    self.seeked = false;
                    self.report(None);
                    return Step::ReRead { from };
                }
            }
            self.position = offset.saturating_add(1);
        }
        match batch
            .base_offset
            .checked_add(1)
            .map(|b| delivery_offset(b, 0))
        {
            Some(Ok(next)) => self.position = self.position.max(next),
            _ => {
                return self.fail(&Error::numeric(format!(
                    "batch offset {} of {} has no successor",
                    batch.base_offset, self.label
                )));
            }
        }
        // The broker's view of the ledger's lag. Advisory: a refusal makes
        // the worker rejoin; the store's own offset is what resumes.
        if let Err(error) = self.consumer.commit(batch.base_offset) {
            self.joined = false;
            self.report(Some(format!(
                "applied {} batch {} but the group commit was refused; rejoining: {error}",
                self.label, batch.base_offset
            )));
        } else {
            self.report(None);
        }
        self.failures = 0;
        Step::Applied { records: applied }
    }

    /// Step until stopped, waiting the poll interval when idle and
    /// re-joining (which renews the group lease) every idle poll.
    pub fn run(mut self) {
        while !self.stop.load(Ordering::SeqCst) {
            if self.step() == Step::Idle {
                self.joined = false;
                self.config.sleeper.sleep(self.config.poll);
            }
        }
    }
}

/// A sleeper on the real clock, for `main`.
pub fn thread_sleeper() -> Arc<dyn Sleeper> {
    Arc::new(ThreadSleeper)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two records of one batch must not share a delivery offset, and the
    /// last record of a batch must sit below the next batch's first.
    /// Mutation: return `batch_offset` from `delivery_offset` — every record
    /// of a batch shares one offset and this fails.
    #[test]
    fn every_record_of_every_batch_has_its_own_delivery_offset_in_order() {
        let a = delivery_offset(7, 0).expect("in range");
        let b = delivery_offset(7, 1).expect("in range");
        let last = delivery_offset(7, (RECORD_STRIDE - 1) as usize).expect("in range");
        let next = delivery_offset(8, 0).expect("in range");
        assert!(a < b && b < last && last < next);
        assert_eq!(batch_of(last), 7);
        assert_eq!(batch_of(next), 8);
        assert!(delivery_offset(7, RECORD_STRIDE as usize).is_err());
        assert!(delivery_offset(u64::MAX, 0).is_err());
    }
}
