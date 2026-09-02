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
}

impl Checkpoint {
    pub fn new(manifest: &SourceManifest, cursor: Cursor, taken_at: Timestamp) -> Self {
        Self {
            source_id: manifest.source_id.clone(),
            schema_version: manifest.schema.version,
            cursor,
            taken_at,
            last_fingerprint: None,
        }
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
