//! Producer-side batching (CONTRACT-043).
//!
//! One network write per record spends a round trip and a fsync on every
//! event. [`Batcher`] accumulates records and hands back one [`Batch`] when
//! either bound is hit: `max_records` (so the buffer is bounded) or
//! `max_linger` (so no record waits longer than the topic's latency budget).
//! Both bounds are required constructor arguments with no default, for the
//! reason `AckProfile` has none: a batcher that picked its own linger would
//! be choosing the stream's latency budget for it.
//!
//! The batcher owns no thread and no timer. The caller supplies `now` and
//! polls [`Batcher::due`] from its own loop, so a flush is a function of the
//! clock it was given and replays identically. ponytail: a poll-driven
//! linger, not a background flusher; add one only if a caller cannot poll at
//! a period well inside `max_linger`.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};

/// What a batcher stamps on every batch it emits and when it emits one.
#[derive(Clone, Debug)]
pub struct BatcherConfig {
    pub message_type: MessageType,
    pub schema_id: u32,
    pub schema_version: u32,
    pub encoding: PayloadCodec,
    /// Flush as soon as this many records are pending. At least one.
    pub max_records: usize,
    /// Flush once the oldest pending record has waited this long. Positive.
    pub max_linger: Duration,
}

/// Accumulates records into batches; see the module documentation.
#[derive(Debug)]
pub struct Batcher {
    config: BatcherConfig,
    pending: Vec<Record>,
    opened_at: Option<Timestamp>,
}

impl Batcher {
    pub fn new(config: BatcherConfig) -> Result<Self> {
        if config.max_records == 0 {
            return Err(Error::invalid(
                "a batcher needs max_records of at least one; zero would never hold a record",
            ));
        }
        if config.max_linger.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a batcher needs a positive max_linger: state the latency budget the stream allows",
            ));
        }
        Ok(Self {
            config,
            pending: Vec::new(),
            opened_at: None,
        })
    }

    /// Records waiting for the next flush.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Add `record` at `now`. Returns a batch when this record fills it, or
    /// when the oldest pending record has already waited `max_linger`.
    pub fn push(&mut self, record: Record, now: Timestamp) -> Result<Option<Batch>> {
        // A due batch goes out first so this record starts a fresh window
        // rather than inheriting an age it did not earn.
        let overdue = self.due(now)?;
        if self.pending.is_empty() {
            self.opened_at = Some(now);
        }
        self.pending.push(record);
        if overdue.is_some() {
            // Two batches cannot be returned from one call; the overdue one
            // wins and the new record waits in a window that starts now.
            return Ok(overdue);
        }
        if self.pending.len() >= self.config.max_records {
            return self.flush();
        }
        Ok(None)
    }

    /// Flush if the oldest pending record has waited `max_linger` at `now`.
    pub fn due(&mut self, now: Timestamp) -> Result<Option<Batch>> {
        match self.opened_at {
            Some(opened) if now >= opened.saturating_add(self.config.max_linger) => self.flush(),
            _ => Ok(None),
        }
    }

    /// Emit whatever is pending as one batch, or `None` if nothing is.
    pub fn flush(&mut self) -> Result<Option<Batch>> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        self.opened_at = None;
        let records = std::mem::take(&mut self.pending);
        Batch::new(
            self.config.message_type,
            self.config.schema_id,
            self.config.schema_version,
            self.config.encoding,
            records,
        )
        .map(Some)
    }
}
