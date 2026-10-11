//! A market event as the Tick Lake stores it, with the clock corrections
//! that took its venue time to the capturing clock.
//!
//! Plain data and validation, no I/O. It lives here and not in `qip-storage`
//! because both sides of the capture seam need it: the edge capture path in
//! `qip-sequencing` builds it, and the lake writer in `qip-storage` seals it.
//! Putting it in storage made a Lane 0 crate link the crate that holds the
//! Redis, warehouse and HTTP clients, which is the reach the architecture
//! suite exists to refuse.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};

/// A lake timestamp as exact integer nanoseconds.
///
/// `Timestamp`'s own serde form is RFC 3339 for a human-read event log, and
/// it truncates to milliseconds. The lake exists for *exact* replay: two
/// ticks a few microseconds apart sealed through that form come back sharing
/// an instant, in an order the venue never printed, and a normalized time no
/// longer equals its event time plus its corrections. Reading accepts either
/// form, so nothing written the other way is stranded.
mod exact_nanos {
    use qip_core::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        value: &Timestamp,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(value.as_nanos())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Timestamp, D::Error> {
        Timestamp::deserialize(deserializer)
    }
}

/// One clock correction, as stored: what kind, by how much, applied when,
/// from how many observations, with what published uncertainty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockCorrectionLineage {
    /// `offset_estimate` or `monotonic_floor`, as the discipline names it.
    pub kind: String,
    pub delta_nanos: i64,
    #[serde(with = "exact_nanos")]
    pub applied_at: Timestamp,
    pub samples: usize,
    /// A statistic, hence `f64`.
    pub uncertainty_nanos: f64,
}

/// A market event as the lake stores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapturedMarketEvent {
    pub venue: String,
    pub feed: String,
    pub instrument: String,
    /// The feed's sequence number, where it numbers its messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// When the venue says it happened.
    #[serde(with = "exact_nanos")]
    pub event_time: Timestamp,
    /// When this platform received it — when it became knowable here.
    #[serde(with = "exact_nanos")]
    pub receive_time: Timestamp,
    /// The event time in the capturing clock, after the corrections below.
    #[serde(with = "exact_nanos")]
    pub normalized_time: Timestamp,
    /// Half-width of the interval the true instant lies in.
    pub uncertainty: Duration,
    /// Every correction between `event_time` and `normalized_time`, in order.
    #[serde(default)]
    pub corrections: Vec<ClockCorrectionLineage>,
    /// Where this event fell against the previous one on its stream, when
    /// the capture path aligned them (`before`, `after`, `unordered`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_order: Option<String>,
    /// The entitlement it was obtained under — the lake partition's.
    pub entitlement: String,
    /// The canonical record.
    pub canonical: serde_json::Value,
    /// SHA-256 of the raw venue payload, kept whether or not the bytes are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_sha256: Option<String>,
    /// The raw venue payload. Never serialised into the canonical segment:
    /// it goes to the raw object, and only where the right permits.
    #[serde(skip)]
    pub raw: Option<Vec<u8>>,
}

impl CapturedMarketEvent {
    /// Refuse an event the lake could not partition or the time model could
    /// not trust: blank venue, instrument or entitlement, a negative
    /// uncertainty, or a normalized time that the corrections do not
    /// account for.
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("venue", &self.venue),
            ("instrument", &self.instrument),
            ("entitlement", &self.entitlement),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a captured market event has no {name}; the lake partitions by it and \
                     cannot place an event without one"
                )));
            }
        }
        if self.uncertainty < Duration::ZERO {
            return Err(Error::invalid(
                "a captured market event's clock uncertainty is negative; it is a half-width, \
                 zero or more",
            ));
        }
        let applied: i64 = self.corrections.iter().map(|c| c.delta_nanos).sum();
        let expected = self.event_time.as_nanos().saturating_add(applied);
        if expected != self.normalized_time.as_nanos() {
            return Err(Error::invalid(format!(
                "the normalized time of a {} event is {} ns from its event time and its \
                 corrections account for {applied} ns; a normalized time the lineage cannot \
                 reproduce is a correction nobody recorded",
                self.venue,
                self.normalized_time.as_nanos() - self.event_time.as_nanos()
            )));
        }
        Ok(())
    }
}
