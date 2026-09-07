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
//!
//! # Why the window survives the process
//!
//! Every sentence above is about one process. A deployment that streams for a
//! week is not one process: it is a revision rollout, an eviction, a restart
//! after an out-of-memory kill, and a window that lives only in memory is empty
//! after each of them. The sources this platform reads make that immediately
//! visible — `FrankfurterRatesConnector::decode` ignores the cursor entirely
//! and re-decodes the whole rate table on every poll, so the *only* thing
//! standing between a restart and the same three reference rates being
//! published a second time as new observations is this window. It held across a
//! re-poll and not across a restart.
//!
//! [`DedupWindow::recent`] hands a bounded tail of the window to a checkpoint
//! and [`DedupWindow::restore`] takes it back. The carry is bounded and
//! ordinarily far smaller than the capacity, so the honest statement is
//! narrower than the in-process one: a redelivery within the carry is
//! recognised across a restart, and one older than it is admitted again exactly
//! as it would be after an eviction. A bound that is stated is a bound an
//! operator can size; a window silently emptied by a restart is not.

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

    /// The number of characters [`Self::of`] produces: SHA-256 in lower-case
    /// hex.
    const HEX_LEN: usize = 64;

    /// A fingerprint read back from a checkpoint, or a refusal.
    ///
    /// Validated rather than trusted. A checkpoint is a file on a durable
    /// store, and a value that reached this window without being a fingerprint
    /// — a truncation, a stray key, a hand edit — would occupy a slot in a
    /// bounded window and match nothing, quietly shrinking the very window a
    /// restart is relying on. Refusing names the file to look at; accepting
    /// would degrade dedup by an amount nobody could see.
    pub fn from_hex(text: &str) -> Result<Self> {
        if text.len() != Self::HEX_LEN
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::invalid(format!(
                "{text:?} is not an event fingerprint: {} lower-case hex characters are expected \
                 and {} were given. A checkpoint carrying anything else has been truncated or \
                 edited, and restoring from it would silently shrink the dedup window",
                Self::HEX_LEN,
                text.len()
            )));
        }
        Ok(Self(text.to_string()))
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

/// Which session a fingerprint entering the window belongs to.
///
/// An argument to [`DedupWindow::take_in`] and nothing else: it decides which
/// counters move, which is the whole difference between a poll and a resume.
/// It is deliberately *not* stored on the entry. It was, for one day, to make a
/// `carried()` accessor exact rather than derived — and that accessor had no
/// caller outside tests, so the bookkeeping was state maintained at two seams
/// to answer a question nothing in this platform asked. Storing a field to make
/// an unused accessor honest is the same defect as the accessor, one level
/// down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// Seeded by [`DedupWindow::restore`] from the last session's checkpoint.
    Carried,
    /// Observed by this process, from a poll.
    Session,
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
        self.take_in(fingerprint, Origin::Session)
    }

    /// The one path by which a fingerprint enters or is recognised, whichever
    /// session it belongs to.
    ///
    /// `origin` decides which counters move, and that is the whole difference
    /// between a poll and a resume: a carried fingerprint is not traffic this
    /// process saw, so it moves neither `admitted` nor `duplicates`. Not
    /// counting it is what [`Self::restore`] used to achieve by counting it and
    /// then zeroing the counters afterwards — the same result reached by never
    /// making the claim, which leaves nothing for a later reader to wonder
    /// about having been cleared or not.
    fn take_in(&mut self, fingerprint: &EventFingerprint, origin: Origin) -> Novelty {
        if self.seen.contains(fingerprint) {
            if origin == Origin::Session {
                self.duplicates = self.duplicates.saturating_add(1);
            }
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
        if origin == Origin::Session {
            self.admitted = self.admitted.saturating_add(1);
        }
        Novelty::New
    }

    /// Whether the fingerprint is in the window, without recording it.
    pub fn contains(&self, fingerprint: &EventFingerprint) -> bool {
        self.seen.contains(fingerprint)
    }

    /// The most recently admitted fingerprints, oldest first, at most `limit`.
    ///
    /// The tail rather than the head: a restart is about to re-read the newest
    /// page of an at-least-once source, so the fingerprints worth carrying are
    /// the ones that page will contain. `limit` is the caller's bound and is
    /// what keeps a checkpoint a fixed size whatever the window's capacity is.
    pub fn recent(&self, limit: usize) -> Vec<EventFingerprint> {
        let skip = self.order.len().saturating_sub(limit);
        self.order.iter().skip(skip).cloned().collect()
    }

    /// Seed an unused window from a checkpoint's carry, oldest first.
    ///
    /// Returns how many were taken, which is not always how many were given:
    /// a carry longer than the capacity fills the window with its newest and
    /// the surplus is counted as evicted, because the alternative — growing
    /// past the capacity — is the unbounded working set this window exists to
    /// refuse.
    ///
    /// Refuses a window that has already observed something. Restoring over a
    /// running window would evict fingerprints from *this* session to make room
    /// for older ones from the last, which is a dedup window that gets worse
    /// the longer it runs; and there is no legitimate caller, because a resume
    /// happens before the first poll or not at all.
    pub fn restore(
        &mut self,
        carried: impl IntoIterator<Item = EventFingerprint>,
    ) -> Result<usize> {
        if self.admitted != 0 || self.duplicates != 0 {
            return Err(Error::invalid(format!(
                "this dedup window has already observed {} fingerprint(s), so it cannot be \
                 restored from a checkpoint: the carry would evict what this session has seen to \
                 make room for what the last one saw. Resume before the first poll, or not at all",
                self.admitted.saturating_add(self.duplicates)
            )));
        }
        let mut taken: usize = 0;
        for fingerprint in carried {
            // Taken in as the last session's, not this one's. `admitted` keeps
            // meaning "events this process took in" — which is what the
            // ledger's duplicate ratio divides by — because the carry never
            // enters it, rather than because a statement afterwards subtracts
            // it out again.
            if self.take_in(&fingerprint, Origin::Carried).is_new() {
                taken = taken.saturating_add(1);
            }
        }
        // `evicted` is deliberately *not* reset, and is the one counter here
        // that does not describe this session's traffic. It counts capacity
        // pressure whatever caused it, and a carry longer than the capacity is
        // capacity pressure a deployment needs to see: it means the last
        // session handed over more than this window can hold, so the oldest of
        // it was dropped at start-up. Zeroing it would hide a sizing mistake at
        // the one moment it is visible.
        Ok(taken)
    }
}
