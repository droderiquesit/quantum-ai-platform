//! Idempotency by event fingerprint.
//!
//! Every source this platform reads is at-least-once. A poll window overlaps
//! the last one on purpose (losing a trade is worse than seeing one twice), a
//! retried request whose first response was lost delivers the same page again,
//! and a resume from a checkpoint re-reads the boundary event. So duplicates
//! are not an anomaly to be prevented — they are normal, and the job is to
//! make them *detectable* cheaply and exactly once.
//!
//! # What is in a fingerprint, and why each part
//!
//! `source_id` — two sources reporting the same trade are two facts, and
//! collapsing them would hide a disagreement between providers that is worth
//! seeing. `schema_version` — the same bytes under a new major version may not
//! mean the same event. The source's own key — the provider's identity for the
//! record, which is what a reconciliation joins on. The event time — a source
//! that reuses ids across days would otherwise have its second day swallowed.
//! And the canonical body — a *corrected* record carries the same key and time
//! as the original and must not be mistaken for a redelivery of it.
//!
//! # Why the parts are length-prefixed
//!
//! Concatenating `source|key|time` lets a key containing the separator forge a
//! different event's fingerprint. Length prefixes make the encoding
//! unambiguous, so two distinct events cannot collide by construction rather
//! than by luck.
//!
//! # Why the window is bounded
//!
//! An unbounded set of every fingerprint ever seen is a process that dies of
//! memory during the incident where the source starts replaying its history.
//! The window holds the most recent [`DedupWindow::capacity`] fingerprints and
//! evicts the oldest, so a duplicate older than the window is admitted again.
//! That is the honest trade and it is why the bus deduplicates too: past this
//! window, `qip_events::EventBody::idempotency_key` is the next line.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

/// A content-addressed identity for one event from one source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventFingerprint(String);

impl EventFingerprint {
    /// Fingerprint one event.
    ///
    /// `key` is the source's own identity for the record — a trade id, a
    /// series code, a currency pair. `body` is the decoded payload, whose
    /// JSON object keys `serde_json` orders, so the same event fingerprints
    /// the same however the source ordered its fields.
    pub fn of(
        source_id: &str,
        schema_version: &str,
        key: &str,
        event_time: Timestamp,
        body: &serde_json::Value,
    ) -> Self {
        let canonical_body = body.to_string();
        let event_nanos = event_time.as_nanos().to_string();
        let mut material = String::new();
        for part in [
            source_id,
            schema_version,
            key,
            event_nanos.as_str(),
            canonical_body.as_str(),
        ] {
            // Length-prefixed, so no value can be split across two fields.
            material.push_str(&part.len().to_string());
            material.push(':');
            material.push_str(part);
        }
        Self(sha256_hex(material.as_bytes()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first bytes, for a log line that has to be readable.
    pub fn short(&self) -> &str {
        let end = self
            .0
            .char_indices()
            .nth(12)
            .map_or(self.0.len(), |(i, _)| i);
        &self.0[..end]
    }
}

impl std::fmt::Display for EventFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether an event has been seen inside the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Novelty {
    New,
    Duplicate,
}

impl Novelty {
    pub const fn is_new(self) -> bool {
        matches!(self, Self::New)
    }
}

/// A bounded set of recently seen fingerprints.
#[derive(Clone, Debug)]
pub struct DedupWindow {
    capacity: usize,
    seen: BTreeSet<EventFingerprint>,
    order: VecDeque<EventFingerprint>,
    duplicates: u64,
    admitted: u64,
    evicted: u64,
}

impl DedupWindow {
    pub fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a dedup window of zero remembers nothing, so every redelivery would be published \
                 as a new event",
            ));
        }
        Ok(Self {
            capacity,
            seen: BTreeSet::new(),
            order: VecDeque::with_capacity(capacity),
            duplicates: 0,
            admitted: 0,
            evicted: 0,
        })
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub const fn duplicates(&self) -> u64 {
        self.duplicates
    }

    pub const fn admitted(&self) -> u64 {
        self.admitted
    }

    /// Fingerprints dropped to stay inside the capacity. A non-zero count is
    /// how a deployment learns its window is too small for the source's replay
    /// behaviour, rather than learning it from duplicated records downstream.
    pub const fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Whether this fingerprint has been seen, recording it if not.
    pub fn observe(&mut self, fingerprint: &EventFingerprint) -> Novelty {
        if self.seen.contains(fingerprint) {
            self.duplicates = self.duplicates.saturating_add(1);
            return Novelty::Duplicate;
        }
        if self.order.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
            self.evicted = self.evicted.saturating_add(1);
        }
        self.seen.insert(fingerprint.clone());
        self.order.push_back(fingerprint.clone());
        self.admitted = self.admitted.saturating_add(1);
        Novelty::New
    }

    /// Whether the fingerprint is in the window, without recording it.
    pub fn contains(&self, fingerprint: &EventFingerprint) -> bool {
        self.seen.contains(fingerprint)
    }
}
