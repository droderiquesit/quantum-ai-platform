//! Where a connector got to, and refusing to resume from the wrong place.
//!
//! A checkpoint is written after a batch is accepted and read back when the
//! process restarts. The interesting part is not the writing — it is the two
//! resumes that must fail:
//!
//! * A checkpoint from a **different source**. Two connectors sharing a
//!   checkpoint store and one typo'd key is a feed silently resuming from
//!   another feed's cursor, which reads as a gap on one side and a replay on
//!   the other.
//! * A checkpoint written under an **incompatible schema major version**. The
//!   cursor's meaning is part of the schema: a token the source no longer
//!   understands, or an event time in a field that has been retyped, resumes
//!   from a position that does not exist.
//!
//! Both are refused by [`Checkpoint::resume_into`] rather than being fixed up,
//! because the only honest repair is to re-read from a position a human chose.
//!
//! # A cursor alone does not survive a restart
//!
//! A checkpoint that carried only a cursor was enough for a source that reads
//! forward from one, and not for the sources this platform actually has.
//! `FrankfurterRatesConnector::decode` takes no cursor at all: it re-decodes
//! the whole rate table every poll, and the dedup window is the only reason
//! the same three reference rates are not republished each hour. That window
//! lived in memory, so the second process of a week-long stream republished
//! everything the first had already absorbed — a duplicate an operator would
//! read as a new observation and a backtest would count twice.
//!
//! So the checkpoint carries a **bounded tail of the dedup window** as well:
//! [`Checkpoint::CARRIED_FINGERPRINTS`] of them, no more, refused if more
//! arrive. That bound is the whole difficulty. Carrying the entire window would
//! make a checkpoint grow with a deployment's memory sizing, and carrying none
//! is where this started.

use super::dedup::EventFingerprint;
use super::manifest::{SchemaVersion, SourceManifest};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A cursor instant, exact to the nanosecond.
///
/// Deserialisation goes through `Timestamp`'s own implementation, which
/// already accepts both an integer and an RFC 3339 string — so a checkpoint
/// written before this module existed still reads.
mod cursor_nanos {
    use qip_core::Timestamp;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        value: &Timestamp,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(value.as_nanos())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Timestamp, D::Error> {
        Timestamp::deserialize(deserializer)
    }
}

/// Where in a source's stream a connector is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "position", rename_all = "snake_case")]
pub enum CursorPosition {
    /// Nothing has been read. The starting position for a new source.
    #[default]
    Beginning,
    /// Everything up to and including this event time has been read.
    ///
    /// Written as integer nanoseconds rather than through `Timestamp`'s own
    /// RFC 3339 form. That form is right for an event log a human reads and it
    /// truncates to milliseconds, and a cursor is not a log line: a source
    /// whose event times carry microseconds — Coinbase's ticker does — would
    /// resume a fraction of a millisecond early on every restart, re-reading
    /// events that the dedup window would then absorb in silence.
    EventTime {
        #[serde(with = "cursor_nanos")]
        at: Timestamp,
    },
    /// An opaque continuation token the source issued. Never interpreted here:
    /// a token this code parsed would be a token this code could get wrong.
    Token { token: String },
}

impl CursorPosition {
    pub const fn event_time(&self) -> Option<Timestamp> {
        match self {
            Self::EventTime { at } => Some(*at),
            Self::Beginning | Self::Token { .. } => None,
        }
    }
}

/// A cursor plus how much has gone past it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub position: CursorPosition,
    /// Events accepted since the beginning of time for this source. Monotone,
    /// and the number a reconciliation against the provider joins on.
    pub events_seen: u64,
}

impl Cursor {
    pub fn beginning() -> Self {
        Self::default()
    }

    pub fn at_event_time(at: Timestamp) -> Self {
        Self {
            position: CursorPosition::EventTime { at },
            events_seen: 0,
        }
    }

    /// Move to a new position, counting `accepted` more events.
    ///
    /// The position only moves forward in event time. A source that re-serves
    /// an older page must not be able to rewind a cursor, because the next
    /// fetch would then re-read everything between and the dedup window would
    /// absorb it silently — a feed doing twice the work with nothing to show
    /// for it.
    pub fn advanced_to(&self, position: CursorPosition, accepted: u64) -> Self {
        let position = match (&self.position, &position) {
            (CursorPosition::EventTime { at: current }, CursorPosition::EventTime { at: next })
                if next < current =>
            {
                self.position.clone()
            }
            _ => position,
        };
        Self {
            position,
            events_seen: self.events_seen.saturating_add(accepted),
        }
    }
}

/// A cursor, bound to the source and schema it means something under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub source_id: String,
    pub schema_version: SchemaVersion,
    pub cursor: Cursor,
    pub taken_at: Timestamp,
    /// The fingerprint of the last event committed, so a resume can tell a
    /// re-delivery of the boundary event from a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fingerprint: Option<String>,
    /// A bounded tail of the dedup window, oldest first, so a restart
    /// recognises the redeliveries the next poll is about to make.
    ///
    /// Empty is legitimate and means exactly one thing: nothing has been
    /// admitted yet under this checkpoint. It is *not* how a checkpoint written
    /// before this field existed reads differently — such a checkpoint also
    /// deserialises to empty, and the first poll after it republishes its
    /// window's worth. That is the pre-existing behaviour and it stops
    /// happening once one checkpoint has been written by this code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_fingerprints: Vec<String>,
}

impl Checkpoint {
    /// Fingerprints a checkpoint carries forward, at most.
    ///
    /// Two hundred and fifty-six. Sized against the widest page any shipped
    /// connector decodes rather than against the dedup capacity: Frankfurter
    /// serves 29 currencies on its health path and 3 on its poll, Coinbase one
    /// print, Alpaca one bar per symbol. A carry of 256 therefore spans many
    /// polls of every source this build has, while a checkpoint stays under
    /// 20 kB whatever `RuntimeConfig::dedup_capacity` a deployment chooses —
    /// and the shipped capacity is 8,192, which would be half a megabyte
    /// written to a durable store on every poll.
    ///
    /// The consequence is stated rather than hidden: a redelivery older than
    /// the newest 256 events is admitted again after a restart. That is the
    /// same trade the window itself makes on eviction, at a tighter bound.
    pub const CARRIED_FINGERPRINTS: usize = 256;

    pub fn new(manifest: &SourceManifest, cursor: Cursor, taken_at: Timestamp) -> Self {
        Self {
            source_id: manifest.source_id.clone(),
            schema_version: manifest.schema.version,
            cursor,
            taken_at,
            last_fingerprint: None,
            recent_fingerprints: Vec::new(),
        }
    }

    /// The same checkpoint carrying a dedup window's tail.
    ///
    /// `carried` is truncated to [`Self::CARRIED_FINGERPRINTS`] here rather
    /// than being refused, because this is the writing side and the caller
    /// handing over a window larger than the bound is the ordinary case. The
    /// *reading* side refuses, because there the surplus came off a durable
    /// store and means something is wrong with the file.
    pub fn carrying(mut self, carried: &[EventFingerprint]) -> Self {
        let skip = carried.len().saturating_sub(Self::CARRIED_FINGERPRINTS);
        self.recent_fingerprints = carried
            .iter()
            .skip(skip)
            .map(|fingerprint| fingerprint.as_str().to_string())
            .collect();
        self.last_fingerprint = carried
            .last()
            .map(|fingerprint| fingerprint.as_str().to_string());
        self
    }

    /// The carried fingerprints, validated, or a refusal naming the file.
    ///
    /// Refuses a carry past the bound. An unbounded list read off a durable
    /// store is an unbounded working set on restore, and the process would die
    /// of memory at start-up — during a restart, which is when a deployment can
    /// least afford a second one.
    pub fn carried(&self) -> Result<Vec<EventFingerprint>> {
        if self.recent_fingerprints.len() > Self::CARRIED_FINGERPRINTS {
            return Err(Error::invalid(format!(
                "the checkpoint for `{}` carries {} fingerprints and at most {} may be carried. \
                 A carry past the bound was not written by this platform; re-read from a position \
                 a human chose rather than restoring an unbounded window at start-up",
                self.source_id,
                self.recent_fingerprints.len(),
                Self::CARRIED_FINGERPRINTS
            )));
        }
        self.recent_fingerprints
            .iter()
            .map(|text| EventFingerprint::from_hex(text))
            .collect()
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|error| Error::schema(format!("the checkpoint could not be written: {error}")))
    }

    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text)
            .map_err(|error| Error::schema(format!("this is not a checkpoint: {error}")))
    }

    /// The cursor this checkpoint holds, if it belongs to `manifest`.
    pub fn resume_into(&self, manifest: &SourceManifest) -> Result<Cursor> {
        if self.source_id != manifest.source_id {
            return Err(Error::invalid(format!(
                "this checkpoint belongs to `{}` and the connector is `{}`. Resuming from it \
                 would read one source from another's position: a gap on one side and a replay \
                 on the other, both silent",
                self.source_id, manifest.source_id
            )));
        }
        if !manifest.schema.version.admits(self.schema_version) {
            return Err(Error::schema(format!(
                "the checkpoint for `{}` was written under schema {} and this connector reads \
                 {}. A cursor means something only under the schema that produced it, so it is \
                 refused rather than reinterpreted; re-read from a position a human chose",
                self.source_id, self.schema_version, manifest.schema.version
            )));
        }
        Ok(self.cursor.clone())
    }
}
