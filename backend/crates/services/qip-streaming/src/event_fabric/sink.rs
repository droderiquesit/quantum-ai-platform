//! The ledger sink: a consumer that never drops a financial outcome
//! (FABRIC-082, ADR 0100 §5's P1 row).
//!
//! # What it promises
//!
//! * **The checkpoint moves only after the write.** [`LedgerSink::step`]
//!   commits an offset through [`Broker::commit_offset`] strictly after
//!   every record in that batch was applied by the [`OutcomeWriter`]. A
//!   writer that is down therefore leaves the checkpoint where it was, and
//!   the batch is read again on the next step.
//! * **A failed write is retried, never skipped.** There is no attempt
//!   limit: [`SinkStep::Retrying`] is returned for as long as the writer
//!   refuses, with a backoff the caller sleeps. A record the writer can
//!   never accept therefore blocks the sink, and the block is visible as
//!   growing [`LedgerSink::lag`] and [`LedgerSink::retries`] — the opposite
//!   of the dead-letter queue that would make the book quietly miss a fill.
//! * **At-least-once into an idempotent writer.** A crash between the write
//!   and the commit re-applies the batch on restart, so the writer must be
//!   idempotent on `event_id`. That is `qip-capital`'s ledger contract, not
//!   something this module can supply by itself.
//!
//! A batch that fails to decode is refused with an error rather than
//! retried or skipped: corruption is not a transient writer outage, and
//! stepping past it would be exactly the dropped outcome this module exists
//! to prevent.

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, Record};

use super::broker::Broker;

/// Where a financial outcome lands. `apply` must be idempotent on
/// `record.event_id`: the sink re-applies a batch after a restart or a
/// partial failure.
pub trait OutcomeWriter {
    fn apply(&mut self, record: &Record) -> Result<()>;
}

/// What one [`LedgerSink::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkStep {
    /// Nothing past the checkpoint exists yet.
    Idle,
    /// Every record at `offset` was applied and the checkpoint now names it.
    Committed { offset: u64 },
    /// The writer refused a record at `offset`; the checkpoint did not move.
    /// The caller waits `backoff_ms` and steps again.
    Retrying {
        offset: u64,
        attempts: u32,
        backoff_ms: u64,
    },
}

/// The wait before retry number `attempt` (1-based): `base_ms` doubling each
/// time, capped at `cap_ms`. Saturating, so an hour of outage cannot
/// overflow into a zero wait that spins the writer.
pub fn backoff_ms(attempt: u32, base_ms: u64, cap_ms: u64) -> u64 {
    let shift = attempt.saturating_sub(1).min(32);
    base_ms.saturating_mul(1u64 << shift).min(cap_ms)
}

/// One consumer group draining one partition into an [`OutcomeWriter`].
#[derive(Debug)]
pub struct LedgerSink<W: OutcomeWriter> {
    group: String,
    stream: String,
    partition: u32,
    writer: W,
    base_ms: u64,
    cap_ms: u64,
    retries: u64,
    attempts: u32,
}

impl<W: OutcomeWriter> LedgerSink<W> {
    /// Refuses an empty group, and a zero backoff base or a cap below it: a
    /// zero wait retries in a tight loop against a store that is already
    /// struggling.
    pub fn new(
        group: &str,
        stream: &str,
        partition: u32,
        writer: W,
        base_ms: u64,
        cap_ms: u64,
    ) -> Result<Self> {
        if group.trim().is_empty() {
            return Err(Error::invalid("a ledger sink must name its consumer group"));
        }
        if base_ms == 0 || cap_ms < base_ms {
            return Err(Error::invalid(
                "a ledger sink's backoff base must be at least one millisecond and its cap no \
                 smaller than the base",
            ));
        }
        Ok(Self {
            group: group.to_string(),
            stream: stream.to_string(),
            partition,
            writer,
            base_ms,
            cap_ms,
            retries: 0,
            attempts: 0,
        })
    }

    /// Total failed writes so far: the sink's retry metric.
    pub fn retries(&self) -> u64 {
        self.retries
    }

    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Batches between the checkpoint and the head: the sink's lag metric.
    pub fn lag(&self, broker: &Broker) -> Result<u64> {
        let head = broker
            .metadata(&self.stream, self.partition)?
            .high_watermark();
        Ok(head.saturating_sub(self.next_offset(broker)?))
    }

    fn next_offset(&self, broker: &Broker) -> Result<u64> {
        Ok(broker
            .committed_offset(&self.group, &self.stream, self.partition)?
            .map_or(0, |committed| committed + 1))
    }

    /// Apply the next uncommitted batch, or report why it did not advance.
    pub fn step(&mut self, broker: &Broker) -> Result<SinkStep> {
        let offset = self.next_offset(broker)?;
        let fetched = broker.fetch(&self.stream, self.partition, offset, 1_048_576)?;
        let hex = fetched.batches();
        if hex.is_empty() {
            return Ok(SinkStep::Idle);
        }
        let bytes = qip_core::hash::from_hex(hex).ok_or_else(|| {
            Error::schema(format!(
                "the broker returned non-hex bytes at offset {offset}"
            ))
        })?;
        let batch = match Batch::decode(&bytes)? {
            DecodeOutcome::Complete(batch) => batch,
            DecodeOutcome::Torn => {
                return Err(Error::io(format!(
                    "the batch at offset {offset} of {}:{} decoded as torn; refusing to step \
                     past an outcome that cannot be read",
                    self.stream, self.partition
                )));
            }
        };
        for record in &batch.records {
            if self.writer.apply(record).is_err() {
                self.retries += 1;
                self.attempts = self.attempts.saturating_add(1);
                return Ok(SinkStep::Retrying {
                    offset,
                    attempts: self.attempts,
                    backoff_ms: backoff_ms(self.attempts, self.base_ms, self.cap_ms),
                });
            }
        }
        broker.commit_offset(&self.group, &self.stream, self.partition, offset)?;
        self.attempts = 0;
        Ok(SinkStep::Committed { offset })
    }
}
