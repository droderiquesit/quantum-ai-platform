//! The policy payload a region receives — twelve typed slots, one signature,
//! and a thirteenth slot beside them (ADR 0080).
//!
//! Blueprint §41.5 names twelve things the centre ships to every region, from
//! trained models down to adversary profiles. This module is the wire shape of
//! that list: an envelope carrying **a typed slot for each of the twelve**,
//! signed as one fact, applied as one fact, and narrowed per §6.2 as its items
//! go stale. ADR 0080 adds a thirteenth, [`Dispositions`], which is not in
//! §41.5's table and is a stated deviation from it: the retired strategies'
//! open lots the centre wants a cell to unwind. It rides this payload rather
//! than a topic of its own so it inherits the one sequence, signature and
//! replay discipline the twelve already have.
//!
//! # A slot with no producer is stale from birth
//!
//! Most of the twelve have no producer in this platform yet — there is no
//! compiled plan, no causal digest, no regime estimate. **Do not read that
//! list as fixed, and do not quote a count from here**: the sentence named
//! belief priors and the episodic digest too until each gained one, and a
//! reader who trusted it after that was told a closed gap was still open.
//! `grep -n 'Slot::produced\|= episodic\|= belief' backend/crates/apps/qip-api/src/mesh.rs`
//! is the enumeration.
//!
//! The slots exist anyway, and an unproduced slot reports
//! [`Freshness::Unavailable`] from the moment the payload is built. That is
//! not scaffolding; it is the fail-closed behaviour §6.2 requires. A cell that
//! has never received belief priors behaves exactly as one whose priors went
//! stale: confidence-weighted sizing falls back to the fixed conservative
//! multiplier. That remains the behaviour on every cycle in which the centre
//! has formed no belief, or none inside this item's five-minute window —
//! `qip_kernel::central::belief` ships nothing at all in either case rather
//! than a default that would read like a real prior.
//!
//! # What this deliberately reuses
//!
//! The envelope generalises [`crate::capital::CapitalEnvelope`]'s proven
//! pattern rather than inventing a second mechanism: a canonical signing
//! string covering every field that matters, a keyed MAC over it with the same
//! trust root the capital channel already uses, verification at the cell into
//! a type whose only constructor recomputes the signature, and refusal —
//! never repair — on any mismatch. One fabric, one signing pattern, one key
//! rotation.
//!
//! # What this structurally cannot carry
//!
//! An autonomy ceiling. Two guarantees, one from each direction:
//! `AutonomyLevel` lives in `qip-risk-engine`, a service, and this crate is a
//! library below every service — the workspace's layering (enforced by
//! `architecture.rs`) means no type here *can* name it. And the payload
//! refuses unknown fields on deserialisation, so a ceiling cannot ride in as
//! an extra key either. Policy travels here; permission does not.

use crate::degradation::{Capability, DegradationState, Freshness};
use crate::message::BookSide;
use crate::signal::StrategyId;
use crate::venue::VenueClass;
use qip_core::error::{Error, Result};
use qip_core::hash::{sha256_hex, to_hex};
use qip_core::lineage::Lineage;
use qip_core::{Decimal, Duration, Timestamp, hmac_sha256};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The twelve items of blueprint §41.5, in its order, and ADR 0080's
/// thirteenth after them.
///
/// An enum rather than thirteen booleans so a caller can iterate the list and
/// a match on it is exhaustive — adding an item forces every decision about
/// it to be made explicitly. The thirteenth did exactly that: its time to
/// live, its capability mapping and its place in the signing string were each
/// decided by a compiler error rather than a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyItem {
    TrainedModels,
    CompiledPlan,
    BeliefPriors,
    EpisodicDigest,
    CausalDigest,
    RegimeState,
    CapitalGrants,
    CycleWhitelist,
    RiskEnvelope,
    InventoryTargets,
    FeasibilityConstraints,
    AdversaryProfiles,
    /// ADR 0080: the retired strategies' lots this cell is to unwind. Not in
    /// §41.5's table; see [`Dispositions`].
    Dispositions,
    /// Opportunity definitions that the strategy runtime evaluates against
    /// live local state (REFLEX-026).
    OpportunityDefinitions,
}

impl PolicyItem {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TrainedModels => "trained_models",
            Self::CompiledPlan => "compiled_plan",
            Self::BeliefPriors => "belief_priors",
            Self::EpisodicDigest => "episodic_digest",
            Self::CausalDigest => "causal_digest",
            Self::RegimeState => "regime_state",
            Self::CapitalGrants => "capital_grants",
            Self::CycleWhitelist => "cycle_whitelist",
            Self::RiskEnvelope => "risk_envelope",
            Self::InventoryTargets => "inventory_targets",
            Self::FeasibilityConstraints => "feasibility_constraints",
            Self::AdversaryProfiles => "adversary_profiles",
            Self::Dispositions => "dispositions",
            Self::OpportunityDefinitions => "opportunity_definitions",
        }
    }

    pub const fn all() -> [Self; 14] {
        [
            Self::TrainedModels,
            Self::CompiledPlan,
            Self::BeliefPriors,
            Self::EpisodicDigest,
            Self::CausalDigest,
            Self::RegimeState,
            Self::CapitalGrants,
            Self::CycleWhitelist,
            Self::RiskEnvelope,
            Self::InventoryTargets,
            Self::FeasibilityConstraints,
            Self::AdversaryProfiles,
            Self::Dispositions,
            Self::OpportunityDefinitions,
        ]
    }

    /// How long this item stays fresh, taken from §41.5's cadence column at
    /// the conservative end of each stated range.
    ///
    /// "On change" and "on promotion" items get a day: they are republished
    /// with every payload, so the TTL only matters when payloads themselves
    /// stop arriving — at which point a day is how long the item outlives the
    /// silence before narrowing.
    pub const fn time_to_live(&self) -> Duration {
        match self {
            // "on promotion" / "on change" / "on re-estimation". A
            // disposition changes on a retirement or a fill, so it takes the
            // "on change" day; its consumer reads it whatever its freshness
            // (a stale instruction to reduce is still an instruction to
            // reduce), so the figure bounds nothing at the cell and is here
            // so the item is not the one with no stated cadence.
            Self::TrainedModels
            | Self::CompiledPlan
            | Self::CausalDigest
            | Self::RegimeState
            | Self::FeasibilityConstraints
            | Self::Dispositions
            | Self::OpportunityDefinitions => Duration::from_secs(86_400),
            // "seconds to minutes" — the conservative end is minutes.
            Self::BeliefPriors => Duration::from_secs(300),
            // "minutes".
            Self::EpisodicDigest => Duration::from_secs(600),
            // "hourly, adaptive" / "hourly".
            Self::CapitalGrants | Self::AdversaryProfiles => Duration::from_secs(3_600),
            // "1–5 min" — one minute.
            Self::CycleWhitelist => Duration::from_secs(60),
            // "30 s – 5 min" — thirty seconds.
            Self::RiskEnvelope => Duration::from_secs(30),
            // "fast clock" — the whitelist's cadence is the fastest stated
            // number in the table, so the fast clock gets the same.
            Self::InventoryTargets => Duration::from_secs(60),
        }
    }

    /// The §6.2 capability this item's staleness narrows, where one exists.
    ///
    /// Three items map; the rest go stale without a cognitive consequence
    /// (their consequence is operational — an old whitelist, an old envelope —
    /// and belongs to the consumer of that slot, not to the degradation
    /// table). Ingestion and counterfactual scoring are deliberately absent:
    /// ingestion staleness is the cell's own feed watermark, not something the
    /// centre ships, and counterfactual scoring never ships at all because
    /// §6.2 gives its loss no trading impact whatsoever. Dispositions map to
    /// nothing for the reason ADR 0080 gives: a stale instruction to reduce
    /// is still an instruction to reduce, and it narrows nothing that was
    /// not already narrowed.
    pub const fn capability(&self) -> Option<Capability> {
        match self {
            Self::BeliefPriors => Some(Capability::BeliefState),
            Self::EpisodicDigest => Some(Capability::EpisodicMemory),
            Self::CausalDigest => Some(Capability::CausalGraph),
            Self::TrainedModels
            | Self::CompiledPlan
            | Self::RegimeState
            | Self::CapitalGrants
            | Self::CycleWhitelist
            | Self::RiskEnvelope
            | Self::InventoryTargets
            | Self::FeasibilityConstraints
            | Self::AdversaryProfiles
            | Self::Dispositions
            | Self::OpportunityDefinitions => None,
        }
    }
}

/// One slot of the payload: a value, if anything produced one, and when.
///
/// `produced_at: None` is the stale-from-birth case. It is not an error and
/// not a default to be papered over — it is the honest wire representation of
/// "this platform does not have that capability yet", and it narrows exactly
/// like staleness does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot<T> {
    value: Option<T>,
    produced_at: Option<Timestamp>,
}

/// The default slot is the unproduced one, so a payload field that is absent
/// from the wire (`#[serde(default)]`) reads as "nothing produced" — the same
/// fail-closed value every slot starts at — and never as a value.
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self::unproduced()
    }
}

impl<T> Slot<T> {
    /// A slot nothing has produced.
    pub const fn unproduced() -> Self {
        Self {
            value: None,
            produced_at: None,
        }
    }

    /// Whether nothing produced this slot.
    pub const fn is_unproduced(&self) -> bool {
        self.value.is_none()
    }

    /// A produced slot. The timestamp is the producer's, not the shipper's:
    /// freshness measures the fact, not the envelope.
    pub fn produced(value: T, produced_at: Timestamp) -> Self {
        Self {
            value: Some(value),
            produced_at: Some(produced_at),
        }
    }

    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn produced_at(&self) -> Option<Timestamp> {
        self.produced_at
    }

    /// Freshness against an item's TTL.
    ///
    /// A value with no production instant is refused rather than guessed at:
    /// it reads as `Unavailable`, because a fact whose age cannot be
    /// established must narrow further, never less. The same rule covers the
    /// converse corruption — a timestamp with no value.
    pub fn freshness(&self, item: PolicyItem, now: Timestamp) -> Freshness {
        match (&self.value, self.produced_at) {
            (Some(_), Some(produced_at)) => {
                if now < produced_at {
                    // A fact from the future is a clock fault, and a clock
                    // fault narrows rather than flattering the reading.
                    Freshness::Stale
                } else if now <= produced_at.saturating_add(item.time_to_live()) {
                    Freshness::Fresh
                } else {
                    Freshness::Stale
                }
            }
            _ => Freshness::Unavailable,
        }
    }
}

/// A model named by digest. Weights never travel this fabric — a payload is
/// policy, and ten ONNX artifacts are an artifact store's business.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelManifest {
    /// Model name to content digest, ordered so the wire form is stable.
    pub models: BTreeMap<String, String>,
}

/// A complete model pack carrying signed artifacts, calibration, and deployment constraints.
/// Carries signed model artifacts; the features they consume; calibration; the universes
/// they are allowed to be applied to; a resource budget; an expiry; and the rollback parent
/// it replaces (CONTRACT-011).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPack {
    /// Artifact digest of the signed model, hex-encoded SHA-256.
    pub artifact_digest: String,
    /// Signature over the digest, HMAC-SHA256 keyed with the region key.
    pub signature: String,
    /// Feature names this model consumes, in canonical order.
    pub features: Vec<String>,
    /// Calibration confidence (0.0 to 1.0) of the model on the training set.
    pub calibration: f64,
    /// Allowed universes (instruments) this model can be applied to.
    pub allowed_universes: BTreeSet<String>,
    /// Resource budget for execution: worst-case microseconds.
    pub budget_microseconds: u64,
    /// Expiry timestamp in seconds since epoch.
    pub expires_at: u64,
    /// Digest of the model pack this one replaces, empty string if no rollback parent.
    pub rollback_parent: String,
}

impl ModelPack {
    /// Refuse a pack with any empty required field, out-of-range calibration, or empty universe list.
    pub fn validate(&self) -> Result<()> {
        if self.artifact_digest.is_empty() {
            return Err(Error::invalid("model_pack: artifact_digest is empty"));
        }
        if self.signature.is_empty() {
            return Err(Error::invalid("model_pack: signature is empty"));
        }
        if self.features.is_empty() {
            return Err(Error::invalid("model_pack: features is empty"));
        }
        if !(0.0..=1.0).contains(&self.calibration) {
            return Err(Error::invalid(
                "model_pack: calibration is out of range [0.0, 1.0]",
            ));
        }
        if self.allowed_universes.is_empty() {
            return Err(Error::invalid("model_pack: allowed_universes is empty"));
        }
        if self.budget_microseconds == 0 {
            return Err(Error::invalid("model_pack: budget_microseconds is zero"));
        }
        if self.expires_at == 0 {
            return Err(Error::invalid("model_pack: expires_at is zero"));
        }
        Ok(())
    }
}

/// The type of uncertainty held by a belief: aleatoric (inherent randomness),
/// epistemic (knowledge gap), or model (model limitation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyType {
    /// Inherent randomness in the phenomenon.
    Aleatoric,
    /// Knowledge gap that could be reduced with more information.
    Epistemic,
    /// Model limitation or approximation error.
    Model,
}

impl UncertaintyType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Aleatoric => "aleatoric",
            Self::Epistemic => "epistemic",
            Self::Model => "model",
        }
    }
}

/// A belief state carrying compressed knowledge (CONTRACT-010).
/// Carries the proposition believed; supporting evidence; the causal path
/// deriving it; confidence; the type of uncertainty; and a TTL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefState {
    /// The proposition or distribution believed.
    pub proposition: String,
    /// Evidence references supporting this belief, non-empty.
    pub evidence_set: Vec<String>,
    /// The causal path deriving this belief.
    pub causal_path: Vec<String>,
    /// Confidence level (0.0 to 1.0).
    pub confidence: f64,
    /// The type of uncertainty held.
    pub uncertainty_type: UncertaintyType,
    /// Expiry timestamp in seconds since epoch; belief is refused after this.
    pub expires_at: u64,
}

impl BeliefState {
    /// Refuse a belief with any empty required field, out-of-range confidence,
    /// empty evidence set, or expiry in the past (at construction).
    pub fn validate(&self, now: u64) -> Result<()> {
        if self.proposition.is_empty() {
            return Err(Error::invalid("belief_state: proposition is empty"));
        }
        if self.evidence_set.is_empty() {
            return Err(Error::invalid("belief_state: evidence_set is empty"));
        }
        if self.causal_path.is_empty() {
            return Err(Error::invalid("belief_state: causal_path is empty"));
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(Error::invalid(
                "belief_state: confidence is out of range [0.0, 1.0]",
            ));
        }
        if self.expires_at <= now {
            return Err(Error::invalid("belief_state: expires_at is in the past"));
        }
        Ok(())
    }

    /// Refuse reading this belief if it has expired.
    pub fn check_expired(&self, now: u64) -> Result<()> {
        if self.expires_at <= now {
            return Err(Error::invalid("belief_state: belief has expired"));
        }
        Ok(())
    }
}

/// Opportunity definitions keyed by opportunity ID, ordered for stable wire form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpportunityManifest {
    /// Opportunity ID to its definition digest, ordered so the wire form is stable.
    pub opportunities: BTreeMap<String, String>,
}

/// The compiled plan, by digest and size. The plan itself ships elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDigest {
    pub digest: String,
    pub strategies: u64,
}

/// Belief priors keyed by subject. Confidence is a statistic, so `f64` is the
/// correct type here; the *sizing* it drives stays `Decimal` in the cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeliefPriors {
    pub priors: BTreeMap<String, f64>,
}

/// The compact episodic digest for the current neighbourhood.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodicDigest {
    pub digest: String,
    pub episodes: u64,
}

/// Which causal edges are active, by identifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalDigest {
    pub active_edges: Vec<String>,
}

/// The regime and how confidently it is held. Confidence is a statistic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegimeState {
    pub regime: String,
    pub confidence: f64,
}

/// The grant signatures the centre believes are live for this cell.
///
/// **A manifest, not a delivery path.** Grants travel their own verified
/// channel exactly as before; this slot exists so the cell can reconcile what
/// it holds against what the centre believes it holds, making a dropped grant
/// visible instead of silent. Carrying the grants themselves here as well
/// would be a second source of truth for the same fact, and two independent
/// claims about one fact will disagree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantManifest {
    /// The signatures of the live grants, ordered.
    pub live_grants: Vec<String>,
}

/// One conversion the centre permits a cell's arbitrage desk to price: a
/// trade edge of blueprint §30's graph, as the whitelist (§41.5 item 8) can
/// carry it.
///
/// Added because the whitelist's string map cannot carry what a graph needs.
/// A cycle identifier names a cycle; a desk needs the *edges* — which book
/// at which venue, consumed on which side, at what proportional cost — and
/// encoding those into a string would be a second grammar inside a signed
/// payload, parsed at the cell with no schema to refuse against. This is
/// the structured form instead, typed so a malformed edge is refused by
/// deserialisation and `deny_unknown_fields`, and the venue is re-checked
/// against the cell's own list before a graph is built from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhitelistedConversion {
    /// The venue whose book this conversion trades against. A venue the
    /// receiving cell may not reach makes the whole whitelist unusable there.
    pub venue: String,
    /// What the venue is, for the planner's settlement assumptions.
    pub venue_class: VenueClass,
    /// The book, named by its own instrument id — not either of the
    /// instruments on it, since a venue quoting one against several has
    /// several books and no single one for it.
    pub market: String,
    /// The instrument held before the conversion.
    pub from: String,
    /// The instrument held after it.
    pub to: String,
    /// The side of the book consumed: `Ask` buys `to`, `Bid` sells `from`.
    pub side: BookSide,
    /// Proportional cost of taking the conversion, as a fraction in `[0, 1)`.
    /// Exact, because it is charged against money.
    pub cost_fraction: Decimal,
}

/// Which cycles may run and which of the eight mechanisms each is assigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleWhitelist {
    /// Cycle identifier to path assignment, ordered.
    pub cycles: BTreeMap<String, String>,
    /// The trade edges the desk may price, in the order the centre listed
    /// them. Empty is "no graph", and a cell installs no desk from it.
    ///
    /// Additive to the signed shape. The slot's digest is taken over its
    /// serialised bytes, so this is skipped when empty: a payload signed
    /// before the field existed deserialises with it empty, serialises
    /// without it, and produces the digest — and the signature — it always
    /// did. A cell built before this field refuses a payload that carries
    /// it, by `deny_unknown_fields`, which is the safe direction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversions: Vec<WhitelistedConversion>,
    /// How much of each starting instrument a cycle may commit, by
    /// instrument id. Exact, because it sizes a position. Skipped when
    /// empty for the same reason as `conversions`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub start_sizes: BTreeMap<String, Decimal>,
}

/// The risk envelope as shipped.
///
/// The blueprint says "at ten levels"; nothing in this platform produces ten
/// levels, and inventing an enum to satisfy the phrase would be a control that
/// cannot fire. What exists is a limit set, so that is what ships, as opaque
/// JSON the risk engine owns the schema of. The traceability matrix records
/// the shape conflict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskEnvelopeSnapshot {
    pub limits: serde_json::Value,
    /// Lineage tracking: correlation id, causation id, trace id, and producer.
    pub lineage: Lineage,
}

/// Inventory targets and mirror bands per instrument, with reference prices.
/// Money and quantities are exact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryTargets {
    pub targets: BTreeMap<String, Decimal>,
    pub reference_prices: BTreeMap<String, Decimal>,
}

/// Feasibility constraints per venue: minimum order, fee floor, tick, the
/// venues the centre has withdrawn, and the regions it has derived as dark.
/// The three grids are exact, because every one of them bounds money.
///
/// # `withdrawn_venues` only ever subtracts
///
/// A name here is a venue the centre has already withdrawn on feasibility
/// evidence and journaled (ADR 0062), and the only thing a cell may do with
/// it is refuse. There is deliberately no "permitted venues" field beside it:
/// a cell's venue list comes from its own configuration, checked at
/// `Cell::install_arbitrage` and again at the node's `graph_from_whitelist`,
/// and a payload that could name a venue *into* either would move the
/// paper-trading and capital boundaries onto a wire that authenticates
/// nobody. Subtraction is safe in the direction this travels for exactly
/// that reason — the worst a forged or replayed payload can do with this set
/// is stop a cell trading somewhere it was configured to trade.
///
/// Read whatever its freshness at the cell: a stale withdrawal is still the
/// last thing the centre said, and re-admitting a venue because the centre
/// went quiet is the one direction this field must never move in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeasibilityConstraints {
    pub minimum_order: BTreeMap<String, Decimal>,
    pub fee_floor: BTreeMap<String, Decimal>,
    pub tick: BTreeMap<String, Decimal>,
    /// Venue ids the centre has withdrawn. `BTreeSet` because the slot is
    /// digested and signed, and a set that serialised in two orders would
    /// sign as two payloads.
    ///
    /// **Additive on the wire, and the deploy order that follows from it.**
    /// This field is skipped when empty and defaults to empty when absent,
    /// exactly as [`CycleWhitelist::conversions`] is and for the same
    /// reasons — a payload signed before the field existed deserialises with
    /// it empty, serialises without it, and produces the digest, and so the
    /// signature, it always did. Without that, a code review found, the
    /// field was a breaking change to a signed cross-process contract that
    /// said nothing about being one: `qip-api` and `qip-edge-node` deploy
    /// separately (Cloud Run and Compute Engine), and a new centre shipping
    /// `withdrawn_venues` to a cell built before it fails the whole
    /// [`PolicyPayload`] under `deny_unknown_fields` — all twelve slots
    /// degraded, not one.
    ///
    /// The skew that remains is deliberate and is the fail-closed half. Once
    /// the centre has actually withdrawn something the field is present, and
    /// an old cell refuses the payload whole rather than applying eleven
    /// slots and silently keeping a venue the centre stopped using. So
    /// **cells upgrade before the centre**; between the two an old cell
    /// applies no new policy, narrows on staleness per §6.2, and keeps the
    /// last payload it applied. That is the safe direction, and it is the
    /// direction `conversions` chose first.
    ///
    /// Defaulting to *empty* is not fail-open, because it is what the absent
    /// field means: a centre that predates ADR 0062 could not have withdrawn
    /// a venue on feasibility evidence, and a cell that reads no withdrawal
    /// from it behaves as the platform did before the slot had a consumer —
    /// the withdrawal still reaches the cell by the `CycleWhitelist` the
    /// centre already omits the venue from. An empty set here removes
    /// nothing; it can never *add* a venue, because there is no field on
    /// this struct by which a payload could.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub withdrawn_venues: BTreeSet<String>,
    /// Regions the centre has derived as dark — heard from once, and from
    /// none of their cells within `CentralConfig::region_dark_after` (ADR
    /// 0079). A cell refuses at `check_extension_for` any mirrored leg whose
    /// counterpart venue sits in one, read against its own
    /// `CellConfig::venue_regions`.
    ///
    /// **Subtract-only, for the same reason `withdrawn_venues` is.** A name
    /// here suspends a mirror; there is no field by which a payload could
    /// declare a region *lit*, because the cell's own reading of its peers
    /// (`RegionOutlook`, off a wire the node polls) is not the centre's to
    /// clear, and a region that has come back is one the centre simply stops
    /// naming. The worst a forged or replayed payload can do with this set
    /// is stop a cell mirroring into a region it was configured to reach.
    ///
    /// Not `withdrawn_venues`: a venue is not a region. A global venue
    /// traded from two regions would be withdrawn at the healthy one, and
    /// `withdrawn_venues` is ADR 0062's evidence window whose reinstatement
    /// takes two signatures — a region that comes back must not need two
    /// humans to say so.
    ///
    /// Same serde discipline as the field above, and the same deploy order
    /// follows: skipped when empty so every payload signed before the field
    /// existed keeps its digest, present once a region is dark so an old
    /// cell refuses the payload whole rather than mirroring into a region
    /// the centre has stopped hearing from. Cells upgrade before the centre.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub dark_regions: BTreeSet<String>,
}

/// Per-venue adversary posture, as the adversary monitor's opaque summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdversaryProfiles {
    pub venues: BTreeMap<String, serde_json::Value>,
}

/// ADR 0080's thirteenth slot: the lots this cell holds for strategies the
/// centre has retired, and the signed quantity that flattens each.
///
/// Keyed strategy, then instrument, to a signed `flatten_by` — exactly
/// `CentralPlane::scheduled_unwinds`' shape filtered to the one cell the
/// payload is for, with the cell prefix dropped from the instrument key
/// because the payload already names its cell. Negative flattens a long,
/// positive a short. `Decimal`, because it is a quantity of a position.
///
/// # What the cell may do with it, and what it may not
///
/// A cell reads this to build **reduce-only** intents against the lot *it*
/// holds for that strategy in that instrument, and refuses — never trades —
/// when it holds nothing, or when the sign would increase the lot or carry
/// it through flat. That sign check is what lets this slot ride a wire that
/// authenticates only the centre: the worst a forged or replayed payload can
/// do with it is close a position the platform holds, which is a loss of
/// edge and never a widening of risk (ADR 0062, ADR 0073: the policy wire
/// may subtract and never add). A retired strategy can never again receive
/// a capital envelope (ADR 0075), so the one intent this slot produces is
/// the one intent in a cell that passes with none, and it is safe for
/// exactly as long as the sign check holds.
///
/// # Absent from the wire when it says nothing
///
/// Skipped when unproduced or empty and defaulted to unproduced when absent,
/// exactly as [`FeasibilityConstraints::withdrawn_venues`] is and for the
/// same reason: a payload signed before the slot existed decodes, serialises
/// and digests as it always did, and so keeps its signature. The skew is the
/// same too — once the centre has something to say the field is present and
/// a cell built before it refuses the whole payload under
/// `deny_unknown_fields`, so **cells upgrade before the centre**. That is the
/// fail-closed direction: an old cell applies no new policy and keeps the
/// last it applied, rather than applying twelve slots and silently ignoring
/// an instruction to reduce.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispositions {
    /// Strategy, then instrument, to the signed quantity that brings the lot
    /// the cell holds for that strategy to zero. `BTreeMap` twice because the
    /// slot is digested and signed, and a map that serialised in two orders
    /// would sign as two payloads.
    pub unwinds: BTreeMap<StrategyId, BTreeMap<String, Decimal>>,
}

impl Dispositions {
    /// Whether this names no lot at all.
    pub fn is_empty(&self) -> bool {
        self.unwinds.values().all(BTreeMap::is_empty)
    }

    /// How many lots this names, across every strategy.
    pub fn len(&self) -> usize {
        self.unwinds.values().map(BTreeMap::len).sum()
    }
}

/// Whether the dispositions slot says nothing — unproduced, or produced with
/// no lot in it — and so stays off the wire and out of the signing string.
///
/// One predicate for both, deliberately: the signer and the verifier each
/// derive the signing string from their own copy, so the rule that decides
/// what the wire carries and the rule that decides what is signed have to be
/// the same function or a payload could sign on one side and not verify on
/// the other.
fn dispositions_unstated(slot: &Slot<Dispositions>) -> bool {
    slot.value().is_none_or(Dispositions::is_empty)
}

/// Whether a slot is unproduced and so stays off the wire and out of the
/// signing string, allowing payloads signed before the slot existed to keep
/// their digest.
fn is_unproduced_slot<T>(slot: &Slot<T>) -> bool {
    slot.is_unproduced()
}

/// The signed twelve-item payload one region receives, plus ADR 0080's
/// thirteenth slot.
///
/// Unknown fields are refused on deserialisation. That is half of the
/// guarantee that this cannot carry an autonomy ceiling — the other half is
/// that no type here can name one, because the layering keeps this crate below
/// the service that defines it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyPayload {
    /// Strictly increasing per cell. The idempotency and anti-replay key: a
    /// cell refuses any sequence at or below the last it applied.
    pub sequence: u64,
    /// The one cell this payload is for. A payload for another cell is a
    /// replay however genuine its signature.
    pub cell: String,
    pub issued_at: Timestamp,
    /// How long the payload as a whole may serve before the cell treats every
    /// slot as stale regardless of its own instant.
    pub valid_for: Duration,
    /// Whether the centre has halted this cell. Redundant with the halt
    /// command on its own topic, deliberately: a cell that missed the
    /// broadcast converges at the next payload. A stale payload can never
    /// *clear* a halt, because clearing requires a fresh sequence.
    pub halted: bool,
    pub trained_models: Slot<ModelManifest>,
    pub compiled_plan: Slot<PlanDigest>,
    pub belief_priors: Slot<BeliefPriors>,
    pub episodic_digest: Slot<EpisodicDigest>,
    pub causal_digest: Slot<CausalDigest>,
    pub regime_state: Slot<RegimeState>,
    pub capital_grants: Slot<GrantManifest>,
    pub cycle_whitelist: Slot<CycleWhitelist>,
    pub risk_envelope: Slot<RiskEnvelopeSnapshot>,
    pub inventory_targets: Slot<InventoryTargets>,
    pub feasibility_constraints: Slot<FeasibilityConstraints>,
    pub adversary_profiles: Slot<AdversaryProfiles>,
    /// ADR 0080's thirteenth slot. Off the wire and out of the signing
    /// string while it says nothing, so every payload signed before it
    /// existed keeps its digest; see [`Dispositions`] for the deploy order
    /// that follows.
    #[serde(default, skip_serializing_if = "dispositions_unstated")]
    pub dispositions: Slot<Dispositions>,
    /// Opportunity definitions that the strategy runtime evaluates against
    /// live state. Off the wire and out of the signing string while unproduced,
    /// so payloads signed before this field existed keep their digest.
    #[serde(default, skip_serializing_if = "is_unproduced_slot")]
    pub opportunity_definitions: Slot<OpportunityManifest>,
    /// Hex MAC over [`Self::signing_payload`]. Empty until signed.
    pub signature: String,
    /// Lineage tracking: correlation id, causation id, trace id, and producer.
    pub lineage: Lineage,
}

impl PolicyPayload {
    /// A payload with every slot unproduced — the shape the platform can
    /// honestly ship today, which narrows a cell to its conservative floor.
    pub fn unproduced(
        sequence: u64,
        cell: impl Into<String>,
        issued_at: Timestamp,
        lineage: Lineage,
    ) -> Self {
        Self {
            sequence,
            cell: cell.into(),
            issued_at,
            valid_for: Duration::from_secs(300),
            halted: false,
            trained_models: Slot::unproduced(),
            compiled_plan: Slot::unproduced(),
            belief_priors: Slot::unproduced(),
            episodic_digest: Slot::unproduced(),
            causal_digest: Slot::unproduced(),
            regime_state: Slot::unproduced(),
            capital_grants: Slot::unproduced(),
            cycle_whitelist: Slot::unproduced(),
            risk_envelope: Slot::unproduced(),
            inventory_targets: Slot::unproduced(),
            feasibility_constraints: Slot::unproduced(),
            adversary_profiles: Slot::unproduced(),
            dispositions: Slot::unproduced(),
            opportunity_definitions: Slot::unproduced(),
            signature: String::new(),
            lineage,
        }
    }

    /// The freshness of one item at `now`.
    ///
    /// The payload's own age caps every slot: past `valid_for`, everything is
    /// at best stale, whatever its own instant says. An old envelope carrying
    /// a "fresh" fact is how a replayed payload would smuggle confidence.
    pub fn freshness(&self, item: PolicyItem, now: Timestamp) -> Freshness {
        let own = match item {
            PolicyItem::TrainedModels => self.trained_models.freshness(item, now),
            PolicyItem::CompiledPlan => self.compiled_plan.freshness(item, now),
            PolicyItem::BeliefPriors => self.belief_priors.freshness(item, now),
            PolicyItem::EpisodicDigest => self.episodic_digest.freshness(item, now),
            PolicyItem::CausalDigest => self.causal_digest.freshness(item, now),
            PolicyItem::RegimeState => self.regime_state.freshness(item, now),
            PolicyItem::CapitalGrants => self.capital_grants.freshness(item, now),
            PolicyItem::CycleWhitelist => self.cycle_whitelist.freshness(item, now),
            PolicyItem::RiskEnvelope => self.risk_envelope.freshness(item, now),
            PolicyItem::InventoryTargets => self.inventory_targets.freshness(item, now),
            PolicyItem::FeasibilityConstraints => self.feasibility_constraints.freshness(item, now),
            PolicyItem::AdversaryProfiles => self.adversary_profiles.freshness(item, now),
            PolicyItem::Dispositions => self.dispositions.freshness(item, now),
            PolicyItem::OpportunityDefinitions => self.opportunity_definitions.freshness(item, now),
        };
        let expired = now > self.issued_at.saturating_add(self.valid_for) || now < self.issued_at;
        if expired && own == Freshness::Fresh {
            Freshness::Stale
        } else {
            own
        }
    }

    /// The §6.2 narrowing this payload implies at `now`.
    ///
    /// This is [`DegradationState`]'s consumer — the mapping from what the
    /// centre shipped, and how long ago, to what the cell may still do.
    /// Ingestion is deliberately not set here: it is the cell's own feed
    /// watermark, observed locally by the caller, and a payload cannot vouch
    /// for a feed it does not carry.
    pub fn narrowing(&self, now: Timestamp) -> DegradationState {
        let mut state = DegradationState::nothing_known();
        for item in PolicyItem::all() {
            if let Some(capability) = item.capability() {
                state.observe(capability, self.freshness(item, now));
            }
        }
        state
    }

    /// The long-horizon knowledge items whose value in this copy differs from
    /// `global`'s (ARCH-009).
    ///
    /// The items are the three with a §6.2 capability — belief priors, the
    /// episodic digest and the causal digest — which are exactly the slots
    /// that carry the centre's knowledge rather than an operating bound. The
    /// comparison is on the value and never on `produced_at`: the same
    /// knowledge re-published a minute later is a fresher copy, not a
    /// different one, and counting it would make every payload a divergence
    /// and the record of one worthless.
    ///
    /// It reports and decides nothing. Which copy wins is not this function's
    /// to say: the centre's does, at the cell's application seam, by the swap
    /// that already replaces a payload whole.
    ///
    /// The match has no wildcard, for the reason [`PolicyItem`] gives: a
    /// fourteenth item does not compile here until somebody says whether it
    /// is knowledge, rather than being silently left out of the record.
    pub fn knowledge_divergence(&self, global: &Self) -> Vec<PolicyItem> {
        PolicyItem::all()
            .into_iter()
            .filter(|item| match item {
                PolicyItem::BeliefPriors => {
                    self.belief_priors.value() != global.belief_priors.value()
                }
                PolicyItem::EpisodicDigest => {
                    self.episodic_digest.value() != global.episodic_digest.value()
                }
                PolicyItem::CausalDigest => {
                    self.causal_digest.value() != global.causal_digest.value()
                }
                PolicyItem::TrainedModels
                | PolicyItem::CompiledPlan
                | PolicyItem::RegimeState
                | PolicyItem::CapitalGrants
                | PolicyItem::CycleWhitelist
                | PolicyItem::RiskEnvelope
                | PolicyItem::InventoryTargets
                | PolicyItem::FeasibilityConstraints
                | PolicyItem::AdversaryProfiles
                | PolicyItem::Dispositions
                | PolicyItem::OpportunityDefinitions => false,
            })
            .collect()
    }

    /// The bytes the signature is taken over.
    ///
    /// Sequence, cell, window, halt flag, and a digest of every slot — so a
    /// payload cannot be re-addressed, re-sequenced, un-halted, or have one
    /// slot swapped while the rest still verify. Slot digests are over the
    /// serialised slot, and every map inside a slot is a `BTreeMap`, so the
    /// serialisation is deterministic and a digest names exactly one value.
    ///
    /// The thirteenth slot's digest is appended only when the slot says
    /// something, by the same predicate that keeps it off the wire. A
    /// payload with nothing to unwind therefore signs to the byte as it did
    /// before the slot existed, which is what lets the centre and the cells
    /// upgrade separately; and a payload with something to unwind signs over
    /// it, so a disposition cannot be added to, removed from or altered on a
    /// payload that still verifies.
    pub fn signing_payload(&self) -> Result<String> {
        let mut parts = vec![
            self.sequence.to_string(),
            // Length-prefixed because it is the one free-text field in this
            // string. See `length_prefixed` for the collision the prefix
            // closes; every other part is numeric, boolean, or a fixed-length
            // digest under a fixed enum name, none of which can absorb a
            // delimiter.
            length_prefixed(&self.cell),
            self.issued_at.as_secs().to_string(),
            self.valid_for.as_nanos().to_string(),
            self.halted.to_string(),
        ];
        for (name, digest) in self.slot_digests()? {
            parts.push(format!("{name}={digest}"));
        }
        Ok(parts.join("|"))
    }

    fn slot_digests(&self) -> Result<Vec<(&'static str, String)>> {
        fn digest<T: Serialize>(
            item: PolicyItem,
            slot: &Slot<T>,
        ) -> Result<(&'static str, String)> {
            let bytes = serde_json::to_vec(slot).map_err(|error| {
                Error::invalid(format!(
                    "the {} slot cannot be serialised, so it cannot be signed: {error}",
                    item.as_str()
                ))
            })?;
            Ok((item.as_str(), sha256_hex(&bytes)))
        }
        let mut digests = vec![
            digest(PolicyItem::TrainedModels, &self.trained_models)?,
            digest(PolicyItem::CompiledPlan, &self.compiled_plan)?,
            digest(PolicyItem::BeliefPriors, &self.belief_priors)?,
            digest(PolicyItem::EpisodicDigest, &self.episodic_digest)?,
            digest(PolicyItem::CausalDigest, &self.causal_digest)?,
            digest(PolicyItem::RegimeState, &self.regime_state)?,
            digest(PolicyItem::CapitalGrants, &self.capital_grants)?,
            digest(PolicyItem::CycleWhitelist, &self.cycle_whitelist)?,
            digest(PolicyItem::RiskEnvelope, &self.risk_envelope)?,
            digest(PolicyItem::InventoryTargets, &self.inventory_targets)?,
            digest(
                PolicyItem::FeasibilityConstraints,
                &self.feasibility_constraints,
            )?,
            digest(PolicyItem::AdversaryProfiles, &self.adversary_profiles)?,
        ];
        if !dispositions_unstated(&self.dispositions) {
            digests.push(digest(PolicyItem::Dispositions, &self.dispositions)?);
        }
        if !is_unproduced_slot(&self.opportunity_definitions) {
            digests.push(digest(
                PolicyItem::OpportunityDefinitions,
                &self.opportunity_definitions,
            )?);
        }
        Ok(digests)
    }

    /// Sign with the shared trust root — the same key, and the same keyed MAC,
    /// that already guards capital envelopes. A payload deserves exactly the
    /// guard capital has, and one key means one rotation.
    ///
    /// Refuses an empty key: a signature anyone can recompute from nothing is
    /// not a signature, and the capital channel refuses the same way.
    pub fn signed(mut self, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a policy payload cannot be signed with an empty key; the trust root is missing",
            ));
        }
        let payload = self.signing_payload()?;
        self.signature = to_hex(&hmac_sha256(key, payload.as_bytes()));
        Ok(self)
    }
}

/// A free-text field, made safe to join with delimiters.
///
/// A signing string built by joining fields with `|` is not injective when a
/// field can itself contain `|`: the pair `{cell: "a", reason: "100|b|c"}`
/// and `{cell: "a|100", reason: "b|c"}` serialise to the same bytes and so
/// share one MAC — two different commands wearing one signature. Not
/// exploitable today, because every free-text field here is centre-controlled
/// and the cell is independently re-checked at verification, but a signing
/// scheme that is only injective while its inputs stay polite is a defect
/// waiting for the field that makes it reachable.
///
/// The prefix is the field's byte length and a colon, so the parser of the
/// string — and more importantly the signer of it — cannot be confused about
/// where a field ends, whatever the field contains.
fn length_prefixed(field: &str) -> String {
    format!("{}:{}", field.len(), field)
}

/// A halt, as a command rather than as staleness.
///
/// §6.2 is about decay: stale policy *narrows* a cell. A halt is not decay —
/// it is a decision, and a decision must not be expressible only as the
/// absence of something else. This command travels the same fabric as the
/// payload on its own topic, engage-only: there is deliberately no
/// release command, because release is a fresh policy decision and rides a
/// newer signed payload. Stopping is one small frame; resuming requires the
/// centre to affirmatively republish policy — the same asymmetry the operator
/// kill switch keeps.
///
/// Idempotent by construction: applying the same halt twice is one halt, so
/// no sequence is needed and a redelivered frame is harmless.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaltCommand {
    /// The one cell this halt is for.
    pub cell: String,
    /// When the centre decided. Doubles as the release barrier: a payload can
    /// only release a halt if it was issued *after* this instant, so a
    /// pre-halt payload still in flight cannot un-halt the cell it was racing.
    pub issued_at: Timestamp,
    /// Why, for the journal. An unexplained halt is cleared with less care
    /// than an explained one.
    pub reason: String,
    /// Hex MAC over [`Self::signing_payload`]. Empty until signed.
    pub signature: String,
}

impl HaltCommand {
    pub fn new(cell: impl Into<String>, issued_at: Timestamp, reason: impl Into<String>) -> Self {
        Self {
            cell: cell.into(),
            issued_at,
            reason: reason.into(),
            signature: String::new(),
        }
    }

    /// The bytes the signature is taken over: every field. A halt that could
    /// be re-addressed to a different cell, re-dated past a release barrier,
    /// or re-worded would be a different command wearing this one's signature.
    pub fn signing_payload(&self) -> String {
        // Both free-text fields are length-prefixed; see `length_prefixed`
        // for the collision this closes. The instant between them is numeric
        // and cannot absorb a delimiter, so it needs none.
        format!(
            "halt|{}|{}|{}",
            length_prefixed(&self.cell),
            self.issued_at.as_secs(),
            length_prefixed(&self.reason)
        )
    }

    /// Sign with the shared trust root — verified, because the failure
    /// directions were weighed: accepting a forged halt stops trading (safe,
    /// but a denial-of-service lever for anyone who can inject frames), and an
    /// unauthenticated stop-lever on a polled inbox is the worse trade. Lost
    /// connectivity is covered separately, by payload TTLs narrowing the cell.
    pub fn signed(mut self, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a halt cannot be signed with an empty key; the trust root is missing",
            ));
        }
        self.signature = to_hex(&hmac_sha256(key, self.signing_payload().as_bytes()));
        Ok(self)
    }
}

/// A policy frame for deterministic enforcement. One of seventeen packets in the
/// policy broadcast carrying a specific gate's decision logic and veto conditions.
///
/// Each frame is independently signed and verified, carried at P0 priority, and
/// applies deterministically at the cell with no model consultation. The seventeen
/// frames combine into a complete risk policy: four risk gates, four feasibility
/// gates, four regime gates, four settlement gates, and one master frame governing
/// the whole (roughly; the exact distribution emerges from implementation).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFrame {
    /// Which frame this is, for ordering and deduplication.
    pub frame_id: u32,
    /// The cell this policy is for.
    pub cell: String,
    /// When the centre issued this frame.
    pub issued_at: Timestamp,
    /// How long this frame remains fresh before the cell reads it as stale.
    pub valid_for: Duration,
    /// The frame's payload, opaque to the fabric (JSON so different frame types
    /// can carry different schemas without mutual knowledge).
    pub payload: serde_json::Value,
    /// Hex MAC over [`Self::signing_payload`].
    pub signature: String,
}

impl PolicyFrame {
    pub fn new(
        frame_id: u32,
        cell: impl Into<String>,
        issued_at: Timestamp,
        valid_for: Duration,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            frame_id,
            cell: cell.into(),
            issued_at,
            valid_for,
            payload,
            signature: String::new(),
        }
    }

    /// Whether this frame is still fresh at `now`.
    pub fn is_fresh(&self, now: Timestamp) -> bool {
        now >= self.issued_at && now <= self.issued_at.saturating_add(self.valid_for)
    }

    /// The bytes the signature is taken over.
    pub fn signing_payload(&self) -> Result<String> {
        let payload_json = serde_json::to_string(&self.payload).map_err(|error| {
            Error::invalid(format!(
                "the policy frame payload cannot be serialised, so it cannot be signed: {error}"
            ))
        })?;
        Ok(format!(
            "frame|{}|{}|{}|{}|{}",
            self.frame_id,
            length_prefixed(&self.cell),
            self.issued_at.as_secs(),
            self.valid_for.as_nanos(),
            payload_json
        ))
    }

    /// Sign with the shared trust root.
    pub fn signed(mut self, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a policy frame cannot be signed with an empty key; the trust root is missing",
            ));
        }
        let payload = self.signing_payload()?;
        self.signature = to_hex(&hmac_sha256(key, payload.as_bytes()));
        Ok(self)
    }
}

/// A deterministic risk gate that refuses to call a model and enforces a
/// control path at the type level through [`Determinism::Required`].
///
/// A [`RiskGate`] is a decision whose answer is derived from inputs and control
/// logic alone — no ML model, no heuristic, no estimation. Its routing decision
/// is [`Determinism::Required`], so the cost router's type system forces every
/// consumption point to assume the gate was pre-computed and never model-routed.
/// That enforcement is structural rather than a convention, so a bug that tried
/// to route it through a model would not compile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskGate {
    /// Which gate this is, for identification and logging.
    pub gate_id: String,
    /// The cell this gate applies to.
    pub cell: String,
    /// When evaluated.
    pub evaluated_at: Timestamp,
    /// Whether the gate permits the proposed action. A gate that fires is a veto.
    pub permit: bool,
    /// Why the gate reached its decision, for the journal.
    pub rationale: String,
    /// Hex MAC over [`Self::signing_payload`].
    pub signature: String,
}

impl RiskGate {
    pub fn new(
        gate_id: impl Into<String>,
        cell: impl Into<String>,
        evaluated_at: Timestamp,
        permit: bool,
        rationale: impl Into<String>,
    ) -> Self {
        Self {
            gate_id: gate_id.into(),
            cell: cell.into(),
            evaluated_at,
            permit,
            rationale: rationale.into(),
            signature: String::new(),
        }
    }

    /// The bytes the signature is taken over.
    pub fn signing_payload(&self) -> Result<String> {
        Ok(format!(
            "gate|{}|{}|{}|{}|{}",
            length_prefixed(&self.gate_id),
            length_prefixed(&self.cell),
            self.evaluated_at.as_secs(),
            self.permit,
            length_prefixed(&self.rationale)
        ))
    }

    /// Sign with the shared trust root.
    pub fn signed(mut self, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a risk gate cannot be signed with an empty key; the trust root is missing",
            ));
        }
        let payload = self.signing_payload()?;
        self.signature = to_hex(&hmac_sha256(key, payload.as_bytes()));
        Ok(self)
    }
}

/// A regime change, requiring dual authorization from two operators.
///
/// A regime change is a high-consequence decision that moves the market regime
/// estimate, which drives allocation mode, sizing, and strategy selection. It
/// requires two independent signatures — not a consensus between machines, but
/// explicit human consent from two different roles (e.g., CRO and Portfolio
/// Manager), so neither can unilaterally move the regime.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegimeChange {
    /// The regime being entered.
    pub regime: String,
    /// Confidence in the estimate (0.0 to 1.0).
    pub confidence: f64,
    /// When the decision was made.
    pub decided_at: Timestamp,
    /// First signer's identity (role or user id).
    pub signer_one: String,
    /// First signature (HMAC-SHA256).
    pub signature_one: String,
    /// Second signer's identity.
    pub signer_two: String,
    /// Second signature (HMAC-SHA256).
    pub signature_two: String,
}

impl RegimeChange {
    pub fn new(
        regime: impl Into<String>,
        confidence: f64,
        decided_at: Timestamp,
        signer_one: impl Into<String>,
    ) -> Result<Self> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(Error::invalid(format!(
                "regime change confidence {} is not a probability",
                confidence
            )));
        }
        Ok(Self {
            regime: regime.into(),
            confidence,
            decided_at,
            signer_one: signer_one.into(),
            signature_one: String::new(),
            signer_two: String::new(),
            signature_two: String::new(),
        })
    }

    /// The bytes the first signer commits to.
    pub fn signing_payload_one(&self) -> Result<String> {
        Ok(format!(
            "regime1|{}|{}|{}|{}",
            length_prefixed(&self.regime),
            self.confidence,
            self.decided_at.as_secs(),
            length_prefixed(&self.signer_one)
        ))
    }

    /// The bytes the second signer commits to — includes the first signature
    /// so the second signer cannot be captured without invalidating the first.
    pub fn signing_payload_two(&self) -> Result<String> {
        Ok(format!(
            "regime2|{}|{}|{}|{}|{}|{}",
            length_prefixed(&self.regime),
            self.confidence,
            self.decided_at.as_secs(),
            length_prefixed(&self.signer_one),
            length_prefixed(&self.signature_one),
            length_prefixed(&self.signer_two)
        ))
    }

    /// Whether this regime change has both signatures and is complete.
    pub fn is_complete(&self) -> bool {
        !self.signature_one.is_empty() && !self.signature_two.is_empty()
    }

    /// Apply the first signature. Returns `Err` if it is already signed by
    /// someone, to prevent the same signer from claiming both roles.
    pub fn signed_one(mut self, signer: impl Into<String>, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a regime change cannot be signed with an empty key",
            ));
        }
        self.signer_one = signer.into();
        let payload = self.signing_payload_one()?;
        self.signature_one = to_hex(&hmac_sha256(key, payload.as_bytes()));
        Ok(self)
    }

    /// Apply the second signature. Returns `Err` if only one signature is needed
    /// or if the regime change is already complete. The second signer may not
    /// be the same as the first (enforced by caller).
    pub fn signed_two(mut self, signer: impl Into<String>, key: &[u8]) -> Result<Self> {
        if key.is_empty() {
            return Err(Error::denied(
                "a regime change cannot be signed with an empty key",
            ));
        }
        if self.signature_one.is_empty() {
            return Err(Error::invalid(
                "the regime change must be signed by the first signer before the second",
            ));
        }
        self.signer_two = signer.into();
        let payload = self.signing_payload_two()?;
        self.signature_two = to_hex(&hmac_sha256(key, payload.as_bytes()));
        Ok(self)
    }

    /// Verify both signatures with their respective keys, returning `Err` if
    /// any check fails. This is called at the edge cell or composition root to
    /// confirm the regime change is authentic before applying it.
    pub fn verify(&self, key_one: &[u8], key_two: &[u8]) -> Result<()> {
        if !self.is_complete() {
            return Err(Error::denied("regime change is not fully signed"));
        }
        if key_one.is_empty() || key_two.is_empty() {
            return Err(Error::denied(
                "regime change verification requires both signing keys",
            ));
        }
        let payload_one = self.signing_payload_one()?;
        let expected_one = to_hex(&hmac_sha256(key_one, payload_one.as_bytes()));
        if expected_one != self.signature_one {
            return Err(Error::denied(
                "regime change first signature does not verify",
            ));
        }
        let payload_two = self.signing_payload_two()?;
        let expected_two = to_hex(&hmac_sha256(key_two, payload_two.as_bytes()));
        if expected_two != self.signature_two {
            return Err(Error::denied(
                "regime change second signature does not verify",
            ));
        }
        Ok(())
    }
}

/// A distilled model contract specifies bounds on an ML model's runtime
/// resource consumption and latency requirements.
///
/// Distilled models are small, fixed-size functions embedded in the hot path.
/// This contract enforces upper bounds on memory, worst-case latency, and
/// specifies whether the model is compatible with reflex lanes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistilledModelContract {
    /// Name of the model, non-empty.
    pub name: String,
    /// Maximum memory in bytes the model uses.
    pub memory_bytes: u64,
    /// Maximum latency in microseconds.
    pub latency_micros: u64,
    /// Whether this model is compatible with reflex decision lanes.
    pub reflex_compatible: bool,
    /// Worst-case evaluation steps charged against a strategy's budget.
    pub worst_case_cost: u64,
}

impl DistilledModelContract {
    /// Validate that all bounds are sensible and consistent.
    ///
    /// Checks:
    /// - `name` is non-empty.
    /// - `memory_bytes`, `latency_micros`, and `worst_case_cost` are all
    ///   non-zero.
    /// - `latency_micros` >= `worst_case_cost` (latency must accommodate cost).
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(Error::invalid("distilled model name must be non-empty"));
        }
        if self.memory_bytes == 0 {
            return Err(Error::invalid(
                "distilled model memory_bytes must be non-zero",
            ));
        }
        if self.latency_micros == 0 {
            return Err(Error::invalid(
                "distilled model latency_micros must be non-zero",
            ));
        }
        if self.worst_case_cost == 0 {
            return Err(Error::invalid(
                "distilled model worst_case_cost must be non-zero",
            ));
        }
        if self.latency_micros < self.worst_case_cost {
            return Err(Error::invalid(format!(
                "distilled model latency_micros ({}) must be >= worst_case_cost ({})",
                self.latency_micros, self.worst_case_cost
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
mod policy_frame_tests {
    use super::*;
    use serde_json::json;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    fn policy_key() -> Vec<u8> {
        b"m6-stage-a3-deterministic-policy".to_vec()
    }

    fn secondary_key() -> Vec<u8> {
        b"m6-secondary-signer-authority".to_vec()
    }

    // --- Policy Frame Tests (SLICE-49-29 through SLICE-49-33) ----------------

    #[test]
    fn a_policy_frame_round_trips_with_deterministic_serialization() -> Result<()> {
        let payload = json!({
            "gate_class": "risk",
            "decision": "permit",
            "confidence": 0.95
        });

        let frame = PolicyFrame::new(
            1,
            "cell-us-1",
            t(0),
            Duration::from_secs(300),
            payload.clone(),
        );
        let json_str = serde_json::to_string(&frame).expect("serializable");
        let decoded: PolicyFrame = serde_json::from_str(&json_str).expect("own wire form decodes");

        assert_eq!(decoded, frame);
        assert_eq!(decoded.frame_id, 1);
        assert_eq!(decoded.cell, "cell-us-1");
        Ok(())
    }

    #[test]
    fn the_policy_frame_signature_covers_all_decision_affecting_fields() -> Result<()> {
        let payload = json!({"gate_id": "risk_limit", "permit": true});
        let base = PolicyFrame::new(
            1,
            "cell-1",
            t(100),
            Duration::from_secs(300),
            payload.clone(),
        );
        let reference = base.signing_payload()?;

        let mut reframed = base.clone();
        reframed.frame_id = 2;
        assert_ne!(
            reframed.signing_payload()?,
            reference,
            "frame_id change did not change signature"
        );

        let mut readdressed = base.clone();
        readdressed.cell = "cell-2".to_string();
        assert_ne!(
            readdressed.signing_payload()?,
            reference,
            "cell change did not change signature"
        );

        let mut redated = base.clone();
        redated.issued_at = t(200);
        assert_ne!(
            redated.signing_payload()?,
            reference,
            "timestamp change did not change signature"
        );

        let mut rewindowed = base.clone();
        rewindowed.valid_for = Duration::from_secs(600);
        assert_ne!(
            rewindowed.signing_payload()?,
            reference,
            "validity window change did not change signature"
        );

        let mut repayloaded = base.clone();
        repayloaded.payload = json!({"gate_id": "risk_limit", "permit": false});
        assert_ne!(
            repayloaded.signing_payload()?,
            reference,
            "payload change did not change signature"
        );

        Ok(())
    }

    #[test]
    fn a_policy_frame_refuses_an_empty_signing_key() -> Result<()> {
        let frame = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), json!({}));
        let result = frame.signed(&[]);

        assert!(result.is_err(), "empty key was accepted");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("trust root is missing")
        );
        Ok(())
    }

    #[test]
    fn a_policy_frame_is_fresh_within_its_window_and_stale_beyond_it() -> Result<()> {
        let frame = PolicyFrame::new(1, "cell-1", t(100), Duration::from_secs(300), json!({}));

        assert!(!frame.is_fresh(t(99)));
        assert!(frame.is_fresh(t(100)));
        assert!(frame.is_fresh(t(250)));
        assert!(frame.is_fresh(t(400)));
        assert!(!frame.is_fresh(t(401)));
        assert!(!frame.is_fresh(t(1000)));

        Ok(())
    }

    #[test]
    fn a_signed_policy_frame_carries_a_deterministic_signature() -> Result<()> {
        let payload = json!({"test": "data"});
        let frame = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), payload.clone());

        assert!(frame.signature.is_empty(), "unsigned frame has a signature");

        let signed = frame.signed(&policy_key())?;
        assert!(
            !signed.signature.is_empty(),
            "signed frame has no signature"
        );

        let frame2 = PolicyFrame::new(1, "cell-1", t(0), Duration::from_secs(300), payload);
        let signed2 = frame2.signed(&policy_key())?;

        assert_eq!(
            signed.signature, signed2.signature,
            "signature is not deterministic"
        );

        Ok(())
    }

    // --- Risk Gate Tests (SLICE-49-34 through SLICE-49-38) ------------------

    #[test]
    fn a_risk_gate_is_constructed_with_all_required_fields() -> Result<()> {
        let gate = RiskGate::new(
            "limit_capital",
            "cell-1",
            t(0),
            true,
            "capital available within limit",
        );

        assert_eq!(gate.gate_id, "limit_capital");
        assert_eq!(gate.cell, "cell-1");
        assert_eq!(gate.evaluated_at, t(0));
        assert!(gate.permit);
        assert_eq!(gate.rationale, "capital available within limit");
        assert!(gate.signature.is_empty());

        Ok(())
    }

    #[test]
    fn a_risk_gate_signature_covers_gate_id_cell_time_permit_and_rationale() -> Result<()> {
        let base = RiskGate::new("gate_1", "cell-1", t(100), true, "permit reason");
        let reference = base.signing_payload()?;

        let mut regated = base.clone();
        regated.gate_id = "gate_2".to_string();
        assert_ne!(
            regated.signing_payload()?,
            reference,
            "gate_id change did not affect signature"
        );

        let mut readdressed = base.clone();
        readdressed.cell = "cell-2".to_string();
        assert_ne!(
            readdressed.signing_payload()?,
            reference,
            "cell change did not affect signature"
        );

        let mut redated = base.clone();
        redated.evaluated_at = t(200);
        assert_ne!(
            redated.signing_payload()?,
            reference,
            "timestamp change did not affect signature"
        );

        let mut reproven = base.clone();
        reproven.permit = false;
        assert_ne!(
            reproven.signing_payload()?,
            reference,
            "permit change did not affect signature"
        );

        let mut reexplained = base.clone();
        reexplained.rationale = "different reason".to_string();
        assert_ne!(
            reexplained.signing_payload()?,
            reference,
            "rationale change did not affect signature"
        );

        Ok(())
    }

    #[test]
    fn a_risk_gate_refuses_an_empty_signing_key() -> Result<()> {
        let gate = RiskGate::new("gate_1", "cell-1", t(0), true, "reason");
        let result = gate.signed(&[]);

        assert!(result.is_err(), "empty key was accepted");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("trust root is missing")
        );
        Ok(())
    }

    #[test]
    fn a_risk_gate_that_refuses_is_marked_as_a_veto() -> Result<()> {
        let permit_gate = RiskGate::new("gate_1", "cell-1", t(0), true, "allowed");
        let veto_gate = RiskGate::new("gate_1", "cell-1", t(0), false, "refused: limit exceeded");

        assert!(permit_gate.permit);
        assert!(!veto_gate.permit);

        let signed_permit = permit_gate.signed(&policy_key())?;
        let signed_veto = veto_gate.signed(&policy_key())?;

        assert!(!signed_permit.signature.is_empty());
        assert!(!signed_veto.signature.is_empty());

        Ok(())
    }

    #[test]
    fn a_signed_risk_gate_produces_deterministic_signatures() -> Result<()> {
        let gate1 = RiskGate::new("gate_1", "cell-1", t(100), true, "permit");
        let gate2 = RiskGate::new("gate_1", "cell-1", t(100), true, "permit");

        let signed1 = gate1.signed(&policy_key())?;
        let signed2 = gate2.signed(&policy_key())?;

        assert_eq!(signed1.signature, signed2.signature);

        Ok(())
    }

    // --- Multi-Signature Regime Change Tests (SLICE-49-39 through SLICE-49-45)

    #[test]
    fn a_regime_change_starts_unsigned_and_requires_two_signatures() -> Result<()> {
        let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

        assert_eq!(change.regime, "crisis");
        assert_eq!(change.confidence.to_bits(), 0.85_f64.to_bits());
        assert_eq!(change.signer_one, "officer_1");
        assert!(change.signature_one.is_empty());
        assert!(change.signature_two.is_empty());
        assert!(!change.is_complete(), "unsigned change was marked complete");

        Ok(())
    }

    #[test]
    fn a_regime_change_refuses_invalid_confidence() -> Result<()> {
        let neg = RegimeChange::new("crisis", -0.1, t(0), "officer");
        assert!(
            neg.is_err(),
            "negative confidence was accepted as a probability"
        );

        let over = RegimeChange::new("crisis", 1.1, t(0), "officer");
        assert!(
            over.is_err(),
            "confidence over 1.0 was accepted as a probability"
        );

        let zero = RegimeChange::new("quiet", 0.0, t(0), "officer")?;
        assert_eq!(zero.confidence.to_bits(), 0.0_f64.to_bits());

        let one = RegimeChange::new("trending", 1.0, t(0), "officer")?;
        assert_eq!(one.confidence.to_bits(), 1.0_f64.to_bits());

        Ok(())
    }

    #[test]
    fn a_regime_change_refuses_to_sign_with_an_empty_key() -> Result<()> {
        let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

        let result = change.signed_one("officer_1", &[]);
        assert!(
            result.is_err(),
            "empty key was accepted for first signature"
        );

        Ok(())
    }

    #[test]
    fn a_regime_change_refuses_second_signature_before_first() -> Result<()> {
        let change = RegimeChange::new("crisis", 0.85, t(0), "officer_1")?;

        let result = change.signed_two("officer_2", &policy_key());
        assert!(result.is_err(), "second signature applied before first");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("first signer before the second")
        );

        Ok(())
    }

    #[test]
    fn a_fully_signed_regime_change_carries_both_signatures_and_verifies() -> Result<()> {
        let change = RegimeChange::new("mean_reverting", 0.75, t(100), "risk_officer")?;

        let with_one = change.signed_one("risk_officer", &policy_key())?;
        assert!(!with_one.signature_one.is_empty());
        assert!(with_one.signature_two.is_empty());
        assert!(!with_one.is_complete());

        let complete = with_one.signed_two("portfolio_manager", &secondary_key())?;
        assert!(!complete.signature_one.is_empty());
        assert!(!complete.signature_two.is_empty());
        assert!(complete.is_complete());

        let verified = complete.verify(&policy_key(), &secondary_key());
        assert!(
            verified.is_ok(),
            "complete regime change failed verification"
        );

        Ok(())
    }

    #[test]
    fn a_regime_change_verification_fails_on_signature_mismatch() -> Result<()> {
        let change = RegimeChange::new("crisis", 0.9, t(200), "cro")?;

        let with_one = change.signed_one("cro", &policy_key())?;
        let complete = with_one.signed_two("pm", &secondary_key())?;

        assert!(complete.verify(&policy_key(), &secondary_key()).is_ok());

        let wrong_key = b"wrong-key-for-testing".to_vec();
        assert!(complete.verify(&wrong_key, &secondary_key()).is_err());
        assert!(complete.verify(&policy_key(), &wrong_key).is_err());
        assert!(complete.verify(&secondary_key(), &policy_key()).is_err());

        Ok(())
    }

    #[test]
    fn regime_change_signatures_are_deterministic_per_signer() -> Result<()> {
        let change1 = RegimeChange::new("trending", 0.88, t(300), "analyst_a")?;
        let change2 = RegimeChange::new("trending", 0.88, t(300), "analyst_a")?;

        let s1_one = change1.signed_one("analyst_a", &policy_key())?;
        let s2_one = change2.signed_one("analyst_a", &policy_key())?;
        assert_eq!(s1_one.signature_one, s2_one.signature_one);

        let s1_complete = s1_one.signed_two("analyst_b", &secondary_key())?;
        let s2_complete = s2_one.signed_two("analyst_b", &secondary_key())?;

        assert_eq!(s1_complete.signature_one, s2_complete.signature_one);
        assert_eq!(s1_complete.signature_two, s2_complete.signature_two);

        Ok(())
    }
}
