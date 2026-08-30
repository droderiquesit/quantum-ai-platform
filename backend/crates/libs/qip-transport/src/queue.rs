//! The bounded outbound queue, which refuses rather than grows.
//!
//! # Why this one refuses where `qip_streaming::LocalQueue` drops
//!
//! Both queues are bounded and neither is optional, but they overflow in
//! opposite directions, and the difference is what they carry.
//!
//! `LocalQueue` holds venue-critical ticks, which are replaceable within
//! milliseconds, so under overload it drops its oldest entry and keeps the half
//! that is still true. This queue holds regional state deltas and signed
//! capital envelopes going between cells and the central plane. None of that is
//! replaceable, so dropping the oldest would mean losing a delta with no record
//! that it existed. It therefore **refuses the newest** with
//! [`crate::TransportError::QueueFull`], and the caller finds out at the call
//! site, synchronously, while it still holds the thing it wanted to send.
//!
//! An unbounded queue is the third option and it is the worst one: a slow peer
//! turns into memory growth, memory growth turns into an out-of-memory kill,
//! and the kill takes the queue's whole contents with it. A named refusal is a
//! bad afternoon; an OOM kill mid-rebalance is an incident.
//!
//! # The refusal is a signal, not just an error
//!
//! A full outbound queue means the peer is slower than this cell is producing.
//! [`QueueStats::refused`] counts how often that happened, which is the number
//! that tells an operator whether to widen the queue, add a peer replica, or
//! slow the producer down. A queue that silently absorbed the difference would
//! report nothing until it could no longer absorb it.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::error::TransportError;
use qip_core::{Error, Result};

/// Counters for one outbound queue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStats {
    /// Items accepted into the queue.
    pub admitted: u64,
    /// Items refused because the queue was full. Non-zero means the peer is
    /// slower than this cell produces.
    pub refused: u64,
    /// Items taken out for sending.
    pub drained: u64,
    /// The most ever queued at once, for capacity planning.
    pub peak_depth: usize,
}

/// A bounded FIFO queue that refuses at capacity.
#[derive(Debug)]
pub struct OutboundQueue<T> {
    name: String,
    capacity: usize,
    items: VecDeque<T>,
    stats: QueueStats,
}

impl<T> OutboundQueue<T> {
    /// A queue holding at most `capacity` items.
    ///
    /// Zero is refused rather than treated as one: a queue that refuses
    /// everything is a configuration mistake that would otherwise present as a
    /// transport that has never delivered anything.
    pub fn new(name: impl Into<String>, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an outbound queue with zero capacity refuses every message it is given",
            ));
        }
        Ok(Self {
            name: name.into(),
            capacity,
            items: VecDeque::with_capacity(capacity.min(1024)),
            stats: QueueStats::default(),
        })
    }

    /// Admit one item, or refuse it by name.
    pub fn push(&mut self, item: T) -> std::result::Result<(), TransportError> {
        if self.items.len() >= self.capacity {
            self.stats.refused += 1;
            return Err(TransportError::QueueFull {
                queue: self.name.clone(),
                capacity: self.capacity,
            });
        }
        self.items.push_back(item);
        self.stats.admitted += 1;
        self.stats.peak_depth = self.stats.peak_depth.max(self.items.len());
        Ok(())
    }

    /// Check that `count` more items would fit, before admitting any of them.
    ///
    /// For batch admission. A batch that is accepted half-way and then refused
    /// leaves the caller unable to say which half was taken, which is the
    /// failure `qip_streaming::Publisher::publish_batch` documents in its
    /// default implementation. Checking first makes the batch all-or-nothing.
    pub fn check_capacity(&mut self, count: usize) -> std::result::Result<(), TransportError> {
        if self.items.len() + count > self.capacity {
            self.stats.refused += 1;
            return Err(TransportError::QueueFull {
                queue: self.name.clone(),
                capacity: self.capacity,
            });
        }
        Ok(())
    }

    /// Take the oldest item. FIFO is the whole ordering guarantee this
    /// transport offers, and it starts here.
    pub fn pop(&mut self) -> Option<T> {
        let item = self.items.pop_front();
        if item.is_some() {
            self.stats.drained += 1;
        }
        item
    }

    pub fn front(&self) -> Option<&T> {
        self.items.front()
    }

    pub fn depth(&self) -> usize {
        self.items.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn remaining(&self) -> usize {
        self.capacity - self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn stats(&self) -> QueueStats {
        self.stats
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}
