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
use qip_core::error::{Error, Result};
use qip_core::{EventId, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// What one edge pass sized against, as one bounded P2 record per pass
/// (ADR 0132 decision 4, R12-FABRIC-10102): per instrument the pass
/// registered features for, the feature values the cell evaluated and the
/// engine revision each was computed at, with the feed messages the pass
/// was handed counted and the tape events it applied counted, ranged by
/// event time and digested over their source hashes. The values of a
/// windowed feature rest on earlier passes' ticks too; this record names
/// what *this* pass applied, not every tick a value ever saw. Bounded by
/// the instruments the cell's plan registered features for, not by market
/// activity: a busy pass and a quiet one write a record of the same shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureDigest {
    pub cell: String,
    pub session: String,
    pub pass: u64,
    /// The pass's own clock reading, in nanoseconds, as [`PassMarker::now_ns`].
    pub now_ns: i64,
    /// One entry per instrument the cell holds registered features for, in
    /// instrument order.
    pub instruments: Vec<InstrumentDigest>,
    /// Feed messages the pass was handed, counted and not shipped.
    pub ticks: u64,
    /// Tape events the pass applied, counted; their time range in
    /// nanoseconds, zero when none; and a digest over their source hashes
    /// in applied order, so replay can check it read the same bytes.
    pub applied: u64,
    pub applied_from_ns: i64,
    pub applied_to_ns: i64,
    pub applied_digest: String,
}

/// The features one instrument was sized against this pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentDigest {
    /// The instrument, as the feature key names its subject.
    pub subject: String,
    pub features: Vec<FeatureReading>,
}

/// One feature's value and freshness as the pass read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureReading {
    /// The feature's name and its parameters, joined as the key prints them.
    pub name: String,
    pub value: crate::FeatureValue,
    /// The revision the value was computed at: freshness, by the engine's
    /// own count rather than a clock nobody set.
    pub revision: u64,
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
///
/// A manifest crossing between brains is read only through
/// [`StatedManifest`]: one that arrives without its assumptions or its model
/// versions is refused on deserialisation (R12-CONTRACT-7073). Until
/// 2026-10-25 both fields were `#[serde(default)]`, so a manifest stripped of
/// them in transit was read back as one that had never stated them and
/// verified under the marker-only hash — a replay "reproduced" under a fill
/// model nobody could name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StatedManifest")]
pub struct ReplayManifest {
    marker: PassMarker,
    /// What the run assumed about the world it simulated against. Absent
    /// only on a manifest built in memory by [`ReplayManifest::new`], which
    /// never deserialises.
    #[serde(skip_serializing_if = "Option::is_none")]
    assumptions: Option<ReplayAssumptions>,
    /// The model versions the run used, by component: the fill model, the
    /// impact model, the latency model.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    model_versions: BTreeMap<String, String>,
    reproducibility_hash: String,
}

/// The wire form of a [`ReplayManifest`]: every field required, so serde
/// itself refuses a manifest missing one and names it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatedManifest {
    marker: PassMarker,
    assumptions: ReplayAssumptions,
    model_versions: BTreeMap<String, String>,
    reproducibility_hash: String,
}

impl TryFrom<StatedManifest> for ReplayManifest {
    type Error = Error;

    /// Refuses a blank assumption or a missing or blank model version, as
    /// [`ReplayManifest::with_assumptions`] does. The hash is carried as
    /// read, not recomputed, so a tampered one still surfaces through
    /// [`ReplayManifest::is_consistent`] rather than being silently healed.
    fn try_from(stated: StatedManifest) -> Result<Self> {
        stated.assumptions.check()?;
        Self::check_versions(&stated.model_versions)?;
        Ok(Self {
            marker: stated.marker,
            assumptions: Some(stated.assumptions),
            model_versions: stated.model_versions,
            reproducibility_hash: stated.reproducibility_hash,
        })
    }
}

/// What a simulated run assumed about clock, latency, queue position, fills
/// and impact (R12-CONTRACT-7073). A marker proves *which inputs* a pass
/// applied; two runs over identical inputs under different assumptions about
/// how a venue would have filled them are not the same experiment, and a
/// manifest that cannot say so lets a replay "reproduce" a result under a
/// fill model nobody can name.
///
/// Each is a statement in words, and "none" is a statement (a gateway with no
/// latency model says so); blank is not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayAssumptions {
    pub clock: String,
    pub latency: String,
    pub queue: String,
    pub fill: String,
    pub impact: String,
}

impl ReplayAssumptions {
    /// Refuse a blank assumption, naming it.
    pub fn check(&self) -> Result<()> {
        for (name, text) in [
            ("clock", &self.clock),
            ("latency", &self.latency),
            ("queue", &self.queue),
            ("fill", &self.fill),
            ("impact", &self.impact),
        ] {
            if text.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "the replay manifest states no {name} assumption; say what the run assumed, \
                     or that it assumed none, because a blank cannot be told from an omission"
                )));
            }
        }
        Ok(())
    }
}

impl ReplayManifest {
    /// Build a manifest, computing the reproducibility hash from `marker`.
    pub fn new(marker: PassMarker) -> Result<Self> {
        let reproducibility_hash = Self::hash_of(&marker)?;
        Ok(Self {
            marker,
            assumptions: None,
            model_versions: BTreeMap::new(),
            reproducibility_hash,
        })
    }

    /// Build a manifest that also states what the run assumed and which model
    /// versions it used, hashing all three together so changing either
    /// changes the hash. Refused when an assumption is blank, when no model
    /// version is stated, or when a component or version name is blank.
    pub fn with_assumptions(
        marker: PassMarker,
        assumptions: ReplayAssumptions,
        model_versions: BTreeMap<String, String>,
    ) -> Result<Self> {
        assumptions.check()?;
        Self::check_versions(&model_versions)?;
        let reproducibility_hash = Self::hash_with(&marker, Some(&assumptions), &model_versions)?;
        Ok(Self {
            marker,
            assumptions: Some(assumptions),
            model_versions,
            reproducibility_hash,
        })
    }

    fn check_versions(versions: &BTreeMap<String, String>) -> Result<()> {
        if versions.is_empty() {
            return Err(Error::invalid(
                "the replay manifest states no model versions; name each model the run used",
            ));
        }
        if versions
            .iter()
            .any(|(model, version)| model.trim().is_empty() || version.trim().is_empty())
        {
            return Err(Error::invalid(
                "a model version in the replay manifest is blank; name the model and its version",
            ));
        }
        Ok(())
    }

    /// sha256 of the canonical JSON of `marker` — "the manifest minus the
    /// hash field" for a manifest that states no assumptions.
    pub fn hash_of(marker: &PassMarker) -> Result<String> {
        let value = serde_json::to_value(marker)?;
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }

    /// The hash of a marker together with the assumptions and model versions
    /// beside it. With neither stated it is [`Self::hash_of`], so a manifest
    /// recorded before the fields existed still verifies.
    pub fn hash_with(
        marker: &PassMarker,
        assumptions: Option<&ReplayAssumptions>,
        model_versions: &BTreeMap<String, String>,
    ) -> Result<String> {
        if assumptions.is_none() && model_versions.is_empty() {
            return Self::hash_of(marker);
        }
        let value = serde_json::json!({
            "marker": serde_json::to_value(marker)?,
            "assumptions": serde_json::to_value(assumptions)?,
            "model_versions": serde_json::to_value(model_versions)?,
        });
        Ok(sha256_hex(canonical_json(&value).as_bytes()))
    }

    /// What the run assumed, when it said.
    pub fn assumptions(&self) -> Option<&ReplayAssumptions> {
        self.assumptions.as_ref()
    }

    /// The model versions the run used, by component.
    pub fn model_versions(&self) -> &BTreeMap<String, String> {
        &self.model_versions
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
        Ok(Self::hash_with(
            &self.marker,
            self.assumptions.as_ref(),
            &self.model_versions,
        )? == self.reproducibility_hash)
    }
}
