//! Replay contract: what one edge pass recorded about itself, and the
//! artifact that lets a later run prove it did the same thing. See ADR 0100
//! §8.
//!
//! Red-team finding B5: replay is defined to carry only exogenous inputs —
//! tape events, control frames and clock ticks. Two more facts reach the
//! digest chain without going through any of those, because they are read
//! from this node's own local state rather than delivered to it: the
//! journal's own back-pressure, and the halt flag this node polls from its
//! own filesystem (`apply_polled_halt` journals `HaltChanged`,
//! `qip-edge/src/cell.rs:1985`). A [`PassMarker`] that did not record what
//! those two read leaves replay to guess, and the guess replay makes when it
//! has no better information is "normal every pass" — indistinguishable from
//! the pass that actually halted on one of them. `readings` is how the guess
//! is replaced by the reading the live pass actually took.

use qip_core::canonical::canonical_json;
use qip_core::error::Result;
use qip_core::{EventId, sha256_hex};
use serde::{Deserialize, Serialize};

/// One control frame's position in its own stream, as a pass applied it.
///
/// A position rather than the frame itself: replay needs to know where a
/// pass stopped reading each control stream, not to re-carry a payload the
/// control stream already holds under this position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPosition {
    pub stream: String,
    pub partition: u32,
    pub offset: u64,
    pub event_id: EventId,
}

impl ControlPosition {
    pub fn new(stream: impl Into<String>, partition: u32, offset: u64, event_id: EventId) -> Self {
        Self {
            stream: stream.into(),
            partition,
            offset,
            event_id,
        }
    }
}

/// A lossless recording of one edge-side state word plus whatever detail text
/// the edge type carried alongside it.
///
/// `qip-contracts` cannot name `qip-edge`'s `JournalPressure`: edge depends on
/// contracts, and not the other way round. So the edge type is flattened to
/// its own discriminant name (`state`) and its detail text (`detail`) here,
/// and the conversion back to the rich edge type lives at the node
/// (SLICE-55). `state` is written only where the edge type itself was
/// actually read — never invented to fill the field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PressureRecord {
    pub state: String,
    pub detail: String,
}

impl PressureRecord {
    pub fn new(state: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            detail: detail.into(),
        }
    }
}

/// A lossless recording of a polled wire's state — the same shape as
/// [`PressureRecord`] because both are "a state word plus detail text", and
/// this one carries `PolledHalt` and `RegionOutlook` alike, since
/// `qip-contracts` cannot name either.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRecord {
    pub state: String,
    pub detail: String,
}

impl WireRecord {
    pub fn new(state: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            detail: detail.into(),
        }
    }
}

/// What a pass actually applied from each of the readings that reach the
/// digest chain without being one of the exogenous inputs replay already
/// tracks.
///
/// `None` means the wire was not armed or not configured on this node —
/// **never** "normal". A pass on a node with no region-share wire configured
/// at all and a pass on a node whose wire read a healthy outlook must not
/// collapse to the same recorded value, or replay could not tell "this input
/// never existed here" from "this input was read and was fine".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedReadings {
    pub journal_pressure: Option<PressureRecord>,
    pub halt_flag: Option<WireRecord>,
    pub region_wire: Option<WireRecord>,
}

/// What one edge pass applied, recorded at the seam where it applied it.
///
/// Every field is either an exogenous input (the tape span, the control
/// positions) or a reading the pass took of its own local state (`readings`)
/// or its own build (`config_digest`, `plan_digest`, `binary_version`,
/// `gateway_seed`) — nothing here is computed from anything else in the
/// struct, so replaying the same inputs against the same build must reach
/// the same marker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PassMarker {
    pub cell: String,
    pub session: String,
    pub pass: u64,
    /// The pass's own clock reading, in nanoseconds since the epoch.
    /// Deliberately a raw integer rather than [`qip_core::Timestamp`]: this
    /// marker's wire form is a plain nanosecond count, not an RFC 3339
    /// string, so two independently-built markers for the same pass hash
    /// identically.
    pub now_ns: i64,
    pub tape_digest: String,
    pub tape_from: u64,
    pub tape_to: u64,
    pub control: Vec<ControlPosition>,
    pub readings: AppliedReadings,
    pub config_digest: String,
    pub plan_digest: String,
    pub gateway_seed: u64,
    pub binary_version: String,
}

/// A [`PassMarker`] with its reproducibility hash computed over it.
///
/// The hash is the whole point of this type: two nodes — or the same node,
/// replayed later — that produced the same marker must produce the same
/// hash, and a change to *any* field the marker carries, including one
/// nobody thought to compare by eye, must change it. That is why the hash is
/// taken over the canonical JSON of the entire marker rather than over a
/// hand-picked subset of its fields: a hand-picked list is exactly the shape
/// of bug that leaves one field — `gateway_seed`, say — silently free to vary
/// between two passes a caller believes were "reproduced".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayManifest {
    marker: PassMarker,
    reproducibility_hash: String,
}

impl ReplayManifest {
    /// Build a manifest, computing the reproducibility hash from `marker`.
    pub fn new(marker: PassMarker) -> Result<Self> {
        let reproducibility_hash = Self::hash_of(&marker)?;
        Ok(Self {
            marker,
            reproducibility_hash,
        })
    }

    /// sha256 of the canonical JSON of `marker` — "the manifest minus the
    /// hash field": a [`ReplayManifest`] carries nothing beyond `marker` and
    /// this hash itself, so hashing the marker alone and hashing the
    /// manifest with its hash field removed are the same computation.
    pub fn hash_of(marker: &PassMarker) -> Result<String> {
        let value = serde_json::to_value(marker)?;
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }

    pub fn marker(&self) -> &PassMarker {
        &self.marker
    }

    pub fn reproducibility_hash(&self) -> &str {
        &self.reproducibility_hash
    }

    /// Whether the stored hash still matches the marker it was computed
    /// from.
    ///
    /// Within this crate's own API a mismatch cannot arise — the fields are
    /// private and [`ReplayManifest::new`] is the only constructor — so this
    /// exists for the deserialised case: a manifest read back off a wire or a
    /// journal whose hash was altered, or whose marker was, after the fact.
    pub fn is_consistent(&self) -> Result<bool> {
        Ok(Self::hash_of(&self.marker)? == self.reproducibility_hash)
    }
}
