//! The low-latency local path.
//!
//! An in-process queue and nothing else: no hash, no append, no file. It exists
//! because the durable path costs a SHA-256 and a write per event, and on the
//! venue-critical path that cost is spent in the interval between a quote
//! changing and a decision being made.
//!
//! The queue is **bounded and lossy**, and both of those are deliberate.
//! Bounded, because the event that fills it is a consumer that has stopped
//! draining — exactly the fault it exists to survive — and an unbounded queue
//! would turn a stalled consumer into an out-of-memory kill. Lossy, because
//! everything admitted here is [`crate::routing::RoutingClass::Hot`], and
//! `RoutingClass::check` only allows that for topics
//! `qip_events::Topic::is_lossy_tolerable` calls replaceable. The oldest entry
//! is dropped rather than the newest: a stale quote is worth less than a fresh
//! one, so under overload the queue keeps the half that is still true.
//!
//! Every drop is counted. A silent drop on this path would be a book quietly
//! diverging from the market, which is the failure the whole edge stack is
//! built to make impossible.

use qip_contracts::time::Watermark;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::envelope::StreamEnvelope;
use crate::ports::{PublishReceipt, Publisher, Subscriber, TransportDescriptor};
use crate::routing::{RoutingClass, TransportPath};

/// Counters for one local queue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalQueueStats {
    /// Envelopes accepted.
    pub admitted: u64,
    /// Envelopes handed to a consumer.
    pub drained: u64,
    /// Envelopes dropped because the queue was full when a newer one arrived.
    pub dropped: u64,
    /// The most ever queued at once, for capacity planning.
    pub peak_depth: usize,
}

/// A bounded, in-process, lossy transport.
#[derive(Debug)]
pub struct LocalQueue {
    name: String,
    capacity: usize,
    queue: VecDeque<StreamEnvelope>,
    position: u64,
    cursor: u64,
    cursor_at: Timestamp,
    drained: bool,
    stats: LocalQueueStats,
}

impl LocalQueue {
    /// A queue holding at most `capacity` envelopes.
    ///
    /// A capacity of zero is refused rather than silently treated as one: a
    /// queue that drops everything is a configuration mistake that would
    /// otherwise present as a feed that never arrives.
    pub fn new(name: impl Into<String>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a local queue with zero capacity drops every event it is given",
            ));
        }
        Ok(Self {
            name: name.into(),
            capacity,
            queue: VecDeque::with_capacity(capacity),
            position: 0,
            cursor: 0,
            cursor_at: Timestamp::EPOCH,
            drained: false,
            stats: LocalQueueStats::default(),
        })
    }

    pub fn stats(&self) -> LocalQueueStats {
        self.stats
    }

    pub fn depth(&self) -> usize {
        self.queue.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Publisher for LocalQueue {
    fn descriptor(&self) -> TransportDescriptor {
        TransportDescriptor {
            name: self.name.clone(),
            path: TransportPath::Local,
            durable: false,
            available: true,
            production_requirement: None,
        }
    }

    fn publish(&mut self, envelope: StreamEnvelope, _at: Timestamp) -> Result<PublishReceipt> {
        // The mirror of the durable transport's refusal. An event that cannot
        // be lost must not enter a queue whose overload policy is to lose one.
        if !matches!(envelope.routing_class(), RoutingClass::Hot) {
            return Err(Error::invalid(format!(
                "{} is routed {} and must not take the local path: this queue drops its oldest \
                 entry under load, and nothing routed {} is replaceable",
                envelope.event_id(),
                envelope.routing_class(),
                envelope.routing_class()
            )));
        }
        let accepted_at = envelope.ingest_timestamp();
        if self.queue.len() == self.capacity {
            self.queue.pop_front();
            self.stats.dropped += 1;
        }
        self.position += 1;
        self.queue.push_back(envelope);
        self.stats.admitted += 1;
        self.stats.peak_depth = self.stats.peak_depth.max(self.queue.len());
        Ok(PublishReceipt {
            transport: self.name.clone(),
            path: TransportPath::Local,
            position: self.position,
            record_hash: None,
            accepted_at,
        })
    }
}

impl Subscriber for LocalQueue {
    fn descriptor(&self) -> TransportDescriptor {
        Publisher::descriptor(self)
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<StreamEnvelope>> {
        let mut drained = Vec::new();
        while let Some(front) = self.queue.front() {
            if front.ingest_timestamp() > until {
                break;
            }
            let Some(envelope) = self.queue.pop_front() else {
                break;
            };
            self.cursor += 1;
            self.cursor_at = envelope.ingest_timestamp();
            self.drained = true;
            self.stats.drained += 1;
            drained.push(envelope);
        }
        Ok(drained)
    }

    fn watermark(&self) -> Option<Watermark> {
        self.drained
            .then(|| Watermark::new(self.name.clone(), self.cursor, self.cursor_at))
    }

    fn has_pending(&self, until: Timestamp) -> bool {
        self.queue
            .front()
            .is_some_and(|envelope| envelope.ingest_timestamp() <= until)
    }
}
