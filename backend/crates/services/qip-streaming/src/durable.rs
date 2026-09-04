//! The in-tree durable transport.
//!
//! Real, working and tested: envelopes are framed into `qip_events::AnyEvent`
//! and appended to `qip_events::EventLog`, which is append-only and
//! hash-chained and can be file-backed. Nothing here is a stand-in — this is
//! the transport a single-node deployment actually runs on, and the one a
//! replay reads from.
//!
//! Reusing `EventLog` rather than growing a second store buys three things that
//! would otherwise have to be rebuilt and kept in step: the chain that makes a
//! truncated or edited history detectable, the correlation and topic indexes a
//! decision reconstruction walks, and `EventFilter`'s point-in-time query. The
//! frame deliberately copies identity, topic, timestamps and lineage out of the
//! envelope so all three keep working on streamed events unchanged.
//!
//! A managed Pub/Sub transport is [`crate::pubsub`], and it is a port. This is
//! not a fallback for it: a deployment configured for Pub/Sub is refused rather
//! than quietly served from a local file, for the reason `qip-storage` gives.

use qip_contracts::time::Watermark;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::log::{EventLog, LogRecord};
use qip_events::{EventFilter, LogStats};

use crate::envelope::StreamEnvelope;
use crate::ports::{PublishReceipt, Publisher, Subscriber, TransportDescriptor};
use crate::routing::{RoutingClass, TransportPath};

/// An append-only, hash-chained transport backed by `qip_events::EventLog`.
#[derive(Debug)]
pub struct DurableLogTransport {
    name: String,
    log: EventLog,
    /// Log position already handed to the subscriber. Contiguous: the log
    /// assigns its own ordering, so there is no hole to run past.
    cursor: u64,
    /// Known-time of the record at `cursor`, which the watermark publishes.
    cursor_at: Timestamp,
    drained: bool,
}

impl DurableLogTransport {
    /// A transport whose log lives in process memory.
    pub fn in_memory(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            log: EventLog::in_memory(),
            cursor: 0,
            cursor_at: Timestamp::EPOCH,
            drained: false,
        }
    }

    /// A transport whose log is mirrored to a JSONL file, loading whatever is
    /// already there.
    pub fn open(name: impl Into<String>, path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            name: name.into(),
            log: EventLog::open(path)?,
            cursor: 0,
            cursor_at: Timestamp::EPOCH,
            drained: false,
        })
    }

    /// The underlying log, for queries this port does not expose.
    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// Verify the hash chain. `Err(sequence)` names the first broken link.
    ///
    /// Delegated rather than reimplemented: the chain is `EventLog`'s
    /// invariant, and a second verifier that agreed with it would be untested
    /// duplication while one that disagreed would be a bug in whichever is
    /// wrong.
    pub fn verify_chain(&self) -> std::result::Result<(), u64> {
        self.log.verify_chain()
    }

    pub fn stats(&self) -> LogStats {
        self.log.stats()
    }

    pub fn len(&self) -> usize {
        self.log.len()
    }

    pub fn is_empty(&self) -> bool {
        self.log.is_empty()
    }

    /// Every envelope matching a filter, ignoring the subscription cursor.
    ///
    /// The replay entry point, in known-time as well as in valid-time.
    ///
    /// `EventFilter::until` — what `EventFilter::as_of` sets — bounds
    /// `occurred_at`, which is when the fact was *true*. A record that became
    /// true before an instant and knowable after it satisfies that filter and
    /// must still not be returned: serving it is point-in-time leakage, and a
    /// backtest reading a filing hours before the platform received it is
    /// profitable for exactly that reason. So the known-time bound is applied
    /// here, on the same instant, and a replay returns only what the platform
    /// could actually have acted on by then.
    pub fn replay(&self, filter: &EventFilter) -> Result<Vec<StreamEnvelope>> {
        self.log
            .query(filter)
            .into_iter()
            .filter(|frame| {
                filter
                    .until
                    .is_none_or(|knowable_by| frame.recorded_at < knowable_by)
            })
            .map(StreamEnvelope::from_frame)
            .collect()
    }

    /// Rewind the subscription so a consumer can re-read from a position.
    ///
    /// Explicit, and named for what it does. A durable transport whose cursor
    /// could only move forward would make a crashed consumer's recovery
    /// impossible without a second copy of the log.
    pub fn seek(&mut self, position: u64, at: Timestamp) {
        self.cursor = position;
        self.cursor_at = at;
        self.drained = position > 0;
    }

    /// The records this transport holds, in log order.
    pub fn records(&self) -> &[LogRecord] {
        self.log.records()
    }
}

impl Publisher for DurableLogTransport {
    fn descriptor(&self) -> TransportDescriptor {
        TransportDescriptor {
            name: self.name.clone(),
            path: TransportPath::Durable,
            durable: true,
            available: true,
            production_requirement: None,
        }
    }

    fn publish(&mut self, envelope: StreamEnvelope, at: Timestamp) -> Result<PublishReceipt> {
        // The refusal is the enforcement of the routing rule at the transport
        // rather than only at the envelope: a hot event reaching here means a
        // router sent it to the wrong place, and the append it was about to
        // make is exactly the latency the class exists to avoid.
        if matches!(envelope.routing_class(), RoutingClass::Hot) {
            return Err(Error::invalid(format!(
                "{} is routed hot and must not traverse the durable buffer: an append and a hash \
                 on the venue-critical path is the latency the class exists to avoid",
                envelope.event_id()
            )));
        }
        let frame = envelope.to_frame()?;
        let position = self.log.append(&frame)?;
        let record_hash = self
            .log
            .records()
            .last()
            .map(|record| record.record_hash.clone());
        Ok(PublishReceipt {
            transport: self.name.clone(),
            path: TransportPath::Durable,
            position,
            record_hash,
            accepted_at: at,
        })
    }
}

impl Subscriber for DurableLogTransport {
    fn descriptor(&self) -> TransportDescriptor {
        Publisher::descriptor(self)
    }

    /// Drain the contiguous run of records at or before `until`.
    ///
    /// Stops at the first record past `until` rather than skipping it, so the
    /// cursor stays contiguous and a record appended late but stamped early is
    /// picked up on a later poll instead of being lost behind a cursor that
    /// jumped over it.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<StreamEnvelope>> {
        let mut drained = Vec::new();
        for record in self.log.records() {
            if record.sequence <= self.cursor {
                continue;
            }
            if record.event.recorded_at > until {
                break;
            }
            drained.push(StreamEnvelope::from_frame(&record.event)?);
            self.cursor = record.sequence;
            self.cursor_at = record.event.recorded_at;
            self.drained = true;
        }
        Ok(drained)
    }

    fn watermark(&self) -> Option<Watermark> {
        self.drained
            .then(|| Watermark::new(self.name.clone(), self.cursor, self.cursor_at))
    }

    fn has_pending(&self, until: Timestamp) -> bool {
        self.log
            .records()
            .iter()
            .any(|record| record.sequence > self.cursor && record.event.recorded_at <= until)
    }
}
