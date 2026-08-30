//! Durable outbound spool: the half of delivery that survives the process.
//!
//! [`MeshPublisher::REQUIREMENTS`] names what production must add, and two of
//! the four are here:
//!
//! > a durable outbound spool and a durable dead-letter sink, because the queue
//! > and the default sink are in memory and a pod restart loses both
//!
//! [`crate::queue`] is bounded, refuses rather than drops, and is entirely in
//! memory: a pod restart loses whatever it held. For a state delta that is
//! replaceable within milliseconds that is the right trade. For a signed
//! capital envelope it is not, and this module is the version that keeps them.
//!
//! # Persist, then send, then forget — in that order
//!
//! The order is the whole design, and reversing any two of the three is a
//! different failure:
//!
//! * **Send, then persist.** A crash in between loses the message with no
//!   record it ever existed. This is the ordering that looks efficient and
//!   silently drops capital instructions.
//! * **Persist, then forget, then send.** A crash in between loses it just as
//!   completely, having paid for the write.
//! * **Persist, send, then forget** — this one. A crash after the send and
//!   before the delete re-sends the message on restart. That is a *duplicate*,
//!   which this transport already promises and the peer's inbox already
//!   detects by idempotency key.
//!
//! So the spool converts a restart from a lossy event into a duplicating one.
//! That is not a smaller failure in general, but it is the smaller one here,
//! for the reason the crate documentation gives about refusing to retry: for a
//! capital envelope, at-most-once is worse than at-least-once. Every consumer
//! of this transport must already be idempotent; the spool does not add that
//! requirement, it relies on the one that is already stated.
//!
//! # What it does not do
//!
//! * **It is not a queue that can be shared between processes.** Two publishers
//!   pointed at one namespace will both claim the same entries. The lease this
//!   would need is a distributed lock, and a hand-rolled one guarding capital
//!   instructions is the mistake ADR 0009 names about in-tree crypto, wearing
//!   different clothes. One publisher per namespace, enforced by deployment.
//! * **It does not fsync per message.** Durability is the
//!   [`KeyValueStore`]'s, and the file-backed adapter in `qip-storage` writes
//!   through the platform's file I/O. A host that loses its page cache loses
//!   the tail. Surviving *that* is what the managed bus was for, and ADR 0011
//!   accepted its loss knowingly.
//! * **It does not bound how long an entry may sit.** An entry whose peer never
//!   returns stays until it is dead-lettered by the publisher's retry policy or
//!   removed by an operator. A spool that expired its own entries would be
//!   dropping capital instructions on a timer.

use crate::deadletter::{DeadLetter, DeadLetterSink};
use crate::error::TransportError;
use qip_core::error::{Error, Result};
use qip_core::kv::{KeyValueStore, KeyValueStoreExt};
use qip_events::AnyEvent;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How wide the zero-padded sequence in a key is.
///
/// Keys are ordered lexicographically by [`KeyValueStore::keys_with_prefix`],
/// so the padding is what makes that ordering numeric. Twenty digits holds
/// `u64::MAX`, which means the ordering never breaks — a spool that wrapped at
/// ten digits would silently start replaying its oldest entries first after
/// ten billion messages.
const SEQUENCE_WIDTH: usize = 20;

/// One message waiting to be sent, with everything needed to send it again
/// after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpoolEntry {
    /// Position in this spool's own ordering. Monotonic, never reused.
    pub sequence: u64,
    /// The idempotency key, duplicated out of the frame so a recovering
    /// publisher can report what it is about to re-send without deserialising
    /// every entry.
    pub key: String,
    /// How many sends have been attempted across all lifetimes of the process.
    ///
    /// Persisted with the entry rather than held in memory, so a publisher that
    /// crashes does not silently reset a message's attempt count and retry it
    /// forever. This is the field that makes the retry budget survive a
    /// restart, which is the point of the spool.
    pub attempts: u32,
    /// The last error seen, for an operator reading the backlog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The message itself.
    pub frame: AnyEvent,
}

/// Counters an operator needs, and which the requirement asks be alerted on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoolStats {
    /// Entries admitted across this process's lifetime.
    pub admitted: u64,
    /// Entries refused because the spool was at capacity. One of the two
    /// numbers that say a delta never arrived.
    pub refused: u64,
    /// Entries committed — sent and acknowledged, then removed.
    pub committed: u64,
    /// Entries found already present when the spool was opened. Non-zero means
    /// this process inherited work from one that did not finish.
    pub recovered: u64,
}

/// A bounded, persistent outbound queue.
///
/// Refuses at capacity exactly as [`crate::queue::OutboundQueue`] does, and for
/// the same reason: a spool that grew without bound would trade a lost message
/// for an exhausted disk, which fails everything rather than one thing.
#[derive(Debug)]
pub struct DurableSpool {
    store: Arc<dyn KeyValueStore>,
    name: String,
    namespace: String,
    capacity: usize,
    next_sequence: u64,
    stats: SpoolStats,
}

impl DurableSpool {
    /// Open a spool over `store`, adopting whatever is already there.
    ///
    /// Opening is a recovery: entries left by a previous process are counted
    /// and the sequence continues past the highest of them. A spool that reset
    /// its sequence on open would write new entries that sort *before* the
    /// unsent backlog, and the backlog would then never be reached.
    pub fn open(
        store: Arc<dyn KeyValueStore>,
        name: impl Into<String>,
        capacity: usize,
    ) -> Result<Self> {
        let name = name.into();
        if capacity == 0 {
            return Err(Error::invalid(
                "a spool that can hold nothing refuses every message; it is not a smaller \
                 spool, it is a disabled transport",
            ));
        }
        let namespace = format!("spool/{name}/");
        let pending = store.keys_with_prefix(&namespace)?;
        let recovered = pending.len() as u64;
        let highest = pending
            .iter()
            .filter_map(|key| sequence_of(key, &namespace))
            .max();

        Ok(Self {
            store,
            name,
            namespace,
            capacity,
            next_sequence: highest.map_or(0, |seq| seq + 1),
            stats: SpoolStats {
                recovered,
                ..SpoolStats::default()
            },
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub const fn stats(&self) -> SpoolStats {
        self.stats
    }

    /// How many entries are waiting.
    pub fn depth(&self) -> Result<usize> {
        Ok(self.store.keys_with_prefix(&self.namespace)?.len())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.depth()? == 0)
    }

    /// Persist a message, before any attempt to send it.
    ///
    /// Returns the assigned sequence. The caller sends *after* this returns and
    /// calls [`Self::commit`] only once the peer has acknowledged.
    pub fn push(&mut self, frame: AnyEvent) -> std::result::Result<u64, TransportError> {
        let depth = self.depth().map_err(|error| TransportError::Inadmissible {
            detail: format!("the spool could not be read: {error}"),
        })?;
        if depth >= self.capacity {
            self.stats.refused += 1;
            return Err(TransportError::QueueFull {
                queue: self.name.clone(),
                capacity: self.capacity,
            });
        }

        let sequence = self.next_sequence;
        let entry = SpoolEntry {
            sequence,
            key: frame.dedup_key(),
            attempts: 0,
            last_error: None,
            frame,
        };
        self.store
            .put_as(&self.key_for(sequence), &entry)
            .map_err(|error| TransportError::Inadmissible {
                detail: format!("the spool could not be written: {error}"),
            })?;

        self.next_sequence += 1;
        self.stats.admitted += 1;
        Ok(sequence)
    }

    /// The oldest entry still waiting, without removing it.
    ///
    /// FIFO, like the in-memory queue: per-publisher order is the only ordering
    /// this transport offers and it starts here.
    pub fn front(&self) -> Result<Option<SpoolEntry>> {
        let Some(key) = self
            .store
            .keys_with_prefix(&self.namespace)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        self.store.get_as(&key)
    }

    /// Every entry waiting, oldest first. For an operator, and for a test that
    /// asserts a restart recovered the right backlog.
    pub fn backlog(&self) -> Result<Vec<SpoolEntry>> {
        let mut entries = Vec::new();
        for key in self.store.keys_with_prefix(&self.namespace)? {
            if let Some(entry) = self.store.get_as::<SpoolEntry>(&key)? {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Record that a send was attempted and failed, keeping the entry.
    ///
    /// The attempt count is persisted rather than held in memory precisely so
    /// that a crash does not reset it. Without this, a message that fails
    /// permanently would be retried from zero after every restart and never
    /// reach the dead-letter sink.
    pub fn record_attempt(&self, sequence: u64, error: &impl std::fmt::Display) -> Result<()> {
        let key = self.key_for(sequence);
        let Some(mut entry) = self.store.get_as::<SpoolEntry>(&key)? else {
            return Err(Error::not_found(format!(
                "spool entry {sequence} is not in {}; it was committed or removed already",
                self.name
            )));
        };
        entry.attempts += 1;
        entry.last_error = Some(error.to_string());
        self.store.put_as(&key, &entry)
    }

    /// Remove an entry the peer has acknowledged.
    ///
    /// Called *after* the send, never before — see the module documentation on
    /// why that ordering is the design rather than an implementation detail.
    pub fn commit(&mut self, sequence: u64) -> Result<bool> {
        let removed = self.store.delete(&self.key_for(sequence))?;
        if removed {
            self.stats.committed += 1;
        }
        Ok(removed)
    }

    fn key_for(&self, sequence: u64) -> String {
        format!("{}{sequence:0SEQUENCE_WIDTH$}", self.namespace)
    }
}

fn sequence_of(key: &str, namespace: &str) -> Option<u64> {
    key.strip_prefix(namespace)?.parse().ok()
}

/// A dead-letter sink that survives the process.
///
/// The other half of the requirement. [`crate::deadletter::MemoryDeadLetters`]
/// is bounded and in memory, so the record of what never arrived is itself lost
/// in the restart that often follows the incident that produced it.
#[derive(Debug)]
pub struct DurableDeadLetters {
    store: Arc<dyn KeyValueStore>,
    namespace: String,
    recorded: u64,
    /// Letters this sink could not write down.
    ///
    /// [`DeadLetterSink::record`] is infallible by contract — a sink that could
    /// fail would need a dead-letter path of its own, and the recursion has to
    /// stop somewhere. So a failed write is *counted* instead, and a non-zero
    /// count means the record of undelivered messages is itself incomplete,
    /// which is strictly worse news than a dead letter and has to be visible.
    unrecordable: u64,
}

impl DurableDeadLetters {
    pub fn open(store: Arc<dyn KeyValueStore>, name: impl AsRef<str>) -> Result<Self> {
        let namespace = format!("deadletter/{}/", name.as_ref());
        let existing = store.keys_with_prefix(&namespace)?.len() as u64;
        Ok(Self {
            store,
            namespace,
            recorded: existing,
            unrecordable: 0,
        })
    }

    /// How many letters could not be written down. Non-zero is an incident.
    pub const fn unrecordable(&self) -> u64 {
        self.unrecordable
    }

    /// Every letter held, oldest first.
    pub fn letters(&self) -> Result<Vec<DeadLetter>> {
        let mut letters = Vec::new();
        for key in self.store.keys_with_prefix(&self.namespace)? {
            if let Some(letter) = self.store.get_as::<DeadLetter>(&key)? {
                letters.push(letter);
            }
        }
        Ok(letters)
    }

    /// Remove a letter an operator has dealt with — re-sent it, or decided it
    /// must not be re-sent. Returns whether it was there.
    pub fn release(&self, key: &str) -> Result<bool> {
        for stored in self.store.keys_with_prefix(&self.namespace)? {
            if let Some(letter) = self.store.get_as::<DeadLetter>(&stored)?
                && letter.key == key
            {
                return self.store.delete(&stored);
            }
        }
        Ok(false)
    }
}

impl DeadLetterSink for DurableDeadLetters {
    fn record(&mut self, letter: DeadLetter) {
        // Keyed by arrival order, not by idempotency key: two failures of the
        // same message are two facts an operator needs, and keying by the
        // message would keep only the most recent.
        let key = format!("{}{:0SEQUENCE_WIDTH$}", self.namespace, self.recorded);
        match self.store.put_as(&key, &letter) {
            Ok(()) => self.recorded += 1,
            Err(_) => self.unrecordable += 1,
        }
    }

    fn len(&self) -> usize {
        self.store
            .keys_with_prefix(&self.namespace)
            .map_or(0, |keys| keys.len())
    }

    fn recorded(&self) -> u64 {
        self.recorded
    }

    fn evicted(&self) -> u64 {
        // Nothing is evicted: the store is not bounded by this type. What is
        // lost is what could not be written at all, which is a different fact
        // and is counted separately by `unrecordable`.
        0
    }
}
