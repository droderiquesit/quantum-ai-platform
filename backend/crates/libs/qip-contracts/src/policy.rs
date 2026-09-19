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
        }
    }

    pub const fn all() -> [Self; 13] {
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
            | Self::Dispositions => Duration::from_secs(86_400),
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
            _ => None,
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
    /// Hex MAC over [`Self::signing_payload`]. Empty until signed.
    pub signature: String,
}

impl PolicyPayload {
    /// A payload with every slot unproduced — the shape the platform can
    /// honestly ship today, which narrows a cell to its conservative floor.
    pub fn unproduced(sequence: u64, cell: impl Into<String>, issued_at: Timestamp) -> Self {
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
            signature: String::new(),
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
