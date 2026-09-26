//! Market event contract. See ADR 0100 §8.
//!
//! Red-team finding B1 was that nothing on the path produced a value of this
//! type at all: a decoded [`crate::message::MarketMessage`] moved straight
//! into feature computation, and every check this type exists to force —
//! that the payload has not drifted from what it was hashed against, that the
//! platform is actually licensed to use it — never ran, because there was no
//! seam for it to run at.
//!
//! [`MarketEvent`] is that seam. It carries three clocks rather than one
//! because a message can sit in a queue between arrival and normalisation,
//! and collapsing the three into "when we saw it" hides exactly the backlog
//! an operator most needs visible; a payload hash that is recomputed rather
//! than trusted, so a caller cannot attach a hash for a payload other than
//! the one it is handing over; and a mandatory entitlement, so admitting a
//! message and licensing it for use are one act instead of two acts that can
//! silently fall out of step.

use crate::governance::Entitlement;
use crate::message::MarketMessage;
use crate::venue::Origin;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};

/// A decoded market message, admitted onto the platform's own timeline.
///
/// Three timestamps are kept as three fields rather than derived from one
/// another: `event_time` is the venue's own claim of when the fact became
/// true, `receive_time` is when this cell's clock saw the message arrive, and
/// `normalized_time` is when the normalisation pipeline finished turning the
/// wire message into this record. A queue between receipt and normalisation
/// is ordinary; a type that only kept one of the three would make that queue
/// invisible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketEvent {
    payload: MarketMessage,
    event_time: Timestamp,
    receive_time: Timestamp,
    normalized_time: Timestamp,
    /// How much slop this cell allows around `event_time`, e.g. from a venue
    /// clock synchronised only to a documented tolerance. Never negative — a
    /// negative uncertainty is not a tighter bound than zero, it is the sign
    /// that a subtraction ran the wrong way.
    uncertainty: Duration,
    /// sha256 of the canonical JSON of `payload`. Recomputed at construction
    /// rather than trusted, so this field can never name a hash for a payload
    /// other than the one it travels with.
    source_hash: String,
    /// The licence this event is admitted under. There is exactly one
    /// constructor for this type and it refuses an entitlement that is not
    /// granted at `event_time`, so a `MarketEvent` carrying data this
    /// platform is not licensed to use cannot be built.
    entitlement: Entitlement,
}

impl MarketEvent {
    /// Build a market event, recomputing the source hash from `payload` and
    /// refusing anything that does not hold together.
    ///
    /// Order of checks: uncertainty's sign is a property of the value handed
    /// in and is cheapest to reject first; the entitlement is checked before
    /// the hash is recomputed so an unlicensed payload is refused for the
    /// reason that actually matters rather than for an incidental hash
    /// mismatch a caller might "fix" by copying in the recomputed value.
    pub fn new(
        payload: MarketMessage,
        event_time: Timestamp,
        receive_time: Timestamp,
        normalized_time: Timestamp,
        uncertainty: Duration,
        source_hash: impl Into<String>,
        entitlement: Entitlement,
    ) -> Result<Self> {
        if uncertainty.as_nanos() < 0 {
            return Err(Error::invalid(
                "market event uncertainty must not be negative; a negative value is not a \
                 tighter bound, it is a subtraction that ran the wrong way",
            ));
        }
        if !entitlement.is_granted(event_time) {
            return Err(Error::denied(format!(
                "{} carries no active entitlement at this event's time; a market event may not \
                 be built from data this platform is not licensed to use",
                entitlement.dataset()
            )));
        }
        let source_hash = source_hash.into();
        let recomputed = Self::hash_payload(&payload)?;
        if recomputed != source_hash {
            return Err(Error::invalid(format!(
                "the recorded source hash {source_hash} does not match the payload's own hash \
                 {recomputed}; the payload was altered after the hash was taken"
            )));
        }
        Ok(Self {
            payload,
            event_time,
            receive_time,
            normalized_time,
            uncertainty,
            source_hash: recomputed,
            entitlement,
        })
    }

    /// sha256 of the canonical JSON of `payload` — the figure a
    /// [`MarketEvent`]'s own `source_hash` must equal.
    pub fn hash_payload(payload: &MarketMessage) -> Result<String> {
        let value = serde_json::to_value(payload)?;
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }

    pub fn payload(&self) -> &MarketMessage {
        &self.payload
    }

    /// Where this event's payload came from — the venue, feed, partition and
    /// sequence a gap or an audit is measured against.
    pub fn origin(&self) -> &Origin {
        &self.payload.origin
    }

    pub const fn event_time(&self) -> Timestamp {
        self.event_time
    }

    pub const fn receive_time(&self) -> Timestamp {
        self.receive_time
    }

    pub const fn normalized_time(&self) -> Timestamp {
        self.normalized_time
    }

    pub const fn uncertainty(&self) -> Duration {
        self.uncertainty
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn entitlement(&self) -> &Entitlement {
        &self.entitlement
    }

    /// The number of messages missing between `previous` and `self` on the
    /// same venue/feed/partition stream, or `None` if the two are not on the
    /// same stream or `self` does not sit after `previous`.
    ///
    /// Judged from [`Origin::sequence`] alone, never from any of the three
    /// timestamps: two ticks on a busy book can share a nanosecond with
    /// nothing lost between them, and two on a quiet one can be seconds apart
    /// with nothing lost either. Time is not evidence of a gap in either
    /// direction; only a hole in the venue's own sequence is.
    pub fn sequence_gap(&self, previous: &Self) -> Option<u64> {
        let earlier = &previous.payload.origin;
        let later = &self.payload.origin;
        if later.stream_key() != earlier.stream_key() || later.sequence <= earlier.sequence {
            return None;
        }
        let gap = later.sequence - earlier.sequence - 1;
        (gap > 0).then_some(gap)
    }
}
