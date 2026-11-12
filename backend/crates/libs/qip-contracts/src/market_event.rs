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

use crate::admission::Admission;
use crate::governance::Entitlement;
use crate::message::MarketMessage;
use crate::venue::Origin;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};

/// A decoded market message, admitted onto the platform's own timeline.
///
/// Three timestamps are kept rather than derived from one another:
/// `event_time` is the venue's own claim of when the fact became true,
/// `receive_time` is when this cell's clock saw the message arrive, and
/// `normalized_time` is when the normalisation pipeline finished turning the
/// wire message into this record. A queue between receipt and normalisation
/// is ordinary; a type that only kept one of the three would make that queue
/// invisible.
///
/// The three times, the clock's doubt, the source hash and the entitlement
/// are not this type's own fields: they are its [`Admission`], held as one
/// value (ADR 0132, R12-TICK-2053). Until 2026-11-12 they were six fields
/// copied out of an `Admission` the constructor built and threw away, and
/// `admission()` rebuilt a header from them on every call — two accounts of
/// one header, the second assembled by hand. Now there is one, and it is the
/// one the constructor checked.
///
/// The wire form is unchanged — the lake and the spool hold millions of
/// lines of it — and decoding it goes back through the refusals rather than
/// around them ([`MarketEventWire`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(into = "MarketEventWire", try_from = "MarketEventWire")]
pub struct MarketEvent {
    payload: MarketMessage,
    admission: Admission,
    /// sha256 of the bytes the venue put on the wire for this message, where
    /// the producer still holds them. The admission's source hash is over the
    /// canonical re-encoding of the decoded message, so it cannot answer "is
    /// this the venue's frame?" — two different wire encodings of one fact
    /// hash to the same source hash, and a re-encoding bug hides inside it.
    /// Absent (not zero) on an event whose producer never held the wire
    /// bytes, and on events recorded before the field existed.
    native_hash: Option<String>,
}

/// [`MarketEvent`]'s wire form, field for field and in the order the lake
/// has always held it.
///
/// A derived `Deserialize` on `MarketEvent` admitted any line that parsed:
/// a payload altered after its hash was taken, a negative uncertainty, an
/// entitlement that was never granted — every refusal of the one
/// constructor bypassed by the decoder (the R12-TICK-2053 skeptic, 2026-10-31).
/// Decoding now re-runs them: the hash is recomputed from the payload and
/// must match, the uncertainty may not be negative, and the entitlement must
/// be granted at the event's time.
///
/// One refusal is deliberately *not* re-run on decode: an event received
/// before it happened. A lake line of that shape is the contamination the
/// replay's knowability exclusion exists to find and count
/// (`qip-deepbrain`'s `tick_desk`); refusing it at the decoder would turn a
/// counted leak into an unparsed line, which is a quieter answer to the same
/// question. The writer refuses it ([`MarketEvent::new`]); the reader
/// measures it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MarketEventWire {
    payload: MarketMessage,
    event_time: Timestamp,
    receive_time: Timestamp,
    normalized_time: Timestamp,
    uncertainty: Duration,
    source_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_hash: Option<String>,
    entitlement: Entitlement,
}

impl From<MarketEvent> for MarketEventWire {
    fn from(event: MarketEvent) -> Self {
        let Admission {
            event_time,
            receive_time,
            normalized_time,
            uncertainty,
            source_hash,
            entitlement,
        } = event.admission;
        Self {
            payload: event.payload,
            event_time,
            receive_time,
            normalized_time,
            uncertainty,
            source_hash,
            native_hash: event.native_hash,
            entitlement,
        }
    }
}

impl TryFrom<MarketEventWire> for MarketEvent {
    type Error = Error;

    fn try_from(wire: MarketEventWire) -> Result<Self> {
        let recomputed = Self::hash_payload(&wire.payload)?;
        if recomputed != wire.source_hash {
            return Err(Error::invalid(format!(
                "a market event line names source hash {} and its payload hashes to \
                 {recomputed}; the payload was altered after the hash was taken",
                wire.source_hash
            )));
        }
        if wire.uncertainty.as_nanos() < 0 {
            return Err(Error::invalid(
                "a market event line carries a negative uncertainty; a negative value is not a \
                 tighter bound, it is a subtraction that ran the wrong way",
            ));
        }
        if !wire.entitlement.is_granted(wire.event_time) {
            return Err(Error::denied(format!(
                "a market event line under {} carries no entitlement granted at its event \
                 time; it may not be read back as data this platform was licensed to use",
                wire.entitlement.dataset()
            )));
        }
        Ok(Self {
            payload: wire.payload,
            admission: Admission {
                event_time: wire.event_time,
                receive_time: wire.receive_time,
                normalized_time: wire.normalized_time,
                uncertainty: wire.uncertainty,
                source_hash: wire.source_hash,
                entitlement: wire.entitlement,
            },
            native_hash: wire.native_hash,
        })
    }
}

impl MarketEvent {
    /// Build a market event, recomputing the source hash from `payload` and
    /// refusing anything that does not hold together.
    ///
    /// The hash is checked first so an altered payload is refused for that
    /// reason; the admission refusals (uncertainty, entitlement, order) are
    /// [`Admission::new`]'s, the one place they are written.
    pub fn new(
        payload: MarketMessage,
        event_time: Timestamp,
        receive_time: Timestamp,
        normalized_time: Timestamp,
        uncertainty: Duration,
        source_hash: impl Into<String>,
        entitlement: Entitlement,
    ) -> Result<Self> {
        let source_hash = source_hash.into();
        let recomputed = Self::hash_payload(&payload)?;
        if recomputed != source_hash {
            return Err(Error::invalid(format!(
                "the recorded source hash {source_hash} does not match the payload's own hash \
                 {recomputed}; the payload was altered after the hash was taken"
            )));
        }
        let admission = Admission::new(
            event_time,
            receive_time,
            normalized_time,
            uncertainty,
            recomputed,
            entitlement,
        )?;
        // Not `Admitted::admit(payload, admission)`: producers across the
        // tree stamp a payload's `capture_time` and the header's receive
        // time from two clocks (the venue's line and the node's receipt), so
        // the own-times rule would refuse ordinary events. Named in
        // R12-TICK-2053's gap.
        Ok(Self {
            payload,
            admission,
            native_hash: None,
        })
    }

    /// The admission header this event was admitted under (ADR 0132): the
    /// one the constructor, or the decoder, checked — held, not rebuilt.
    pub fn admission(&self) -> &Admission {
        &self.admission
    }

    /// Record the venue's own bytes for this message, hashing them here so a
    /// caller cannot attach a digest of bytes it is not handing over.
    #[must_use]
    pub fn with_native_bytes(mut self, bytes: &[u8]) -> Self {
        self.native_hash = Some(sha256_hex(bytes));
        self
    }

    /// sha256 of the source-native wire bytes, when they were retained.
    pub fn native_hash(&self) -> Option<&str> {
        self.native_hash.as_deref()
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
        self.admission.event_time
    }

    pub const fn receive_time(&self) -> Timestamp {
        self.admission.receive_time
    }

    pub const fn normalized_time(&self) -> Timestamp {
        self.admission.normalized_time
    }

    pub const fn uncertainty(&self) -> Duration {
        self.admission.uncertainty
    }

    pub fn source_hash(&self) -> &str {
        &self.admission.source_hash
    }

    pub fn entitlement(&self) -> &Entitlement {
        &self.admission.entitlement
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
