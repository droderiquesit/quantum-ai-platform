//! The policy payload a region receives — twelve typed slots, one signature.
//!
//! Blueprint §41.5 names twelve things the centre ships to every region, from
//! trained models down to adversary profiles. This module is the wire shape of
//! that list: an envelope carrying **a typed slot for each of the twelve**,
//! signed as one fact, applied as one fact, and narrowed per §6.2 as its items
//! go stale.
//!
//! # A slot with no producer is stale from birth
//!
//! Most of the twelve have no producer in this platform yet — there is no
//! belief engine, no episodic digest, no compiled plan. The slots exist
//! anyway, and an unproduced slot reports [`Freshness::Unavailable`] from the
//! moment the payload is built. That is not scaffolding; it is the fail-closed
//! behaviour §6.2 requires. A cell that has never received belief priors
//! behaves exactly as one whose priors went stale: confidence-weighted sizing
//! falls back to the fixed conservative multiplier. The platform's sizing was
//! never belief-weighted; this makes that fact load-bearing instead of
//! implicit.
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
use qip_core::error::{Error, Result};
use qip_core::hash::{sha256_hex, to_hex};
use qip_core::{Decimal, Duration, Timestamp, hmac_sha256};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The twelve items of blueprint §41.5, in its order.
///
/// An enum rather than twelve booleans so a caller can iterate the list and a
/// match on it is exhaustive — adding a thirteenth item forces every decision
/// about it to be made explicitly.
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
        }
    }

    pub const fn all() -> [Self; 12] {
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
            // "on promotion" / "on change" / "on re-estimation".
            Self::TrainedModels
            | Self::CompiledPlan
            | Self::CausalDigest
            | Self::RegimeState
            | Self::FeasibilityConstraints => Duration::from_secs(86_400),
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
    /// §6.2 gives its loss no trading impact whatsoever.
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

impl<T> Slot<T> {
    /// A slot nothing has produced.
    pub const fn unproduced() -> Self {
        Self {
            value: None,
            produced_at: None,
        }
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

/// Which cycles may run and which of the eight mechanisms each is assigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CycleWhitelist {
    /// Cycle identifier to path assignment, ordered.
    pub cycles: BTreeMap<String, String>,
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

/// Feasibility constraints per venue: minimum order, fee floor, tick. Exact,
/// because every one of them bounds money.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeasibilityConstraints {
    pub minimum_order: BTreeMap<String, Decimal>,
    pub fee_floor: BTreeMap<String, Decimal>,
    pub tick: BTreeMap<String, Decimal>,
}

/// Per-venue adversary posture, as the adversary monitor's opaque summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdversaryProfiles {
    pub venues: BTreeMap<String, serde_json::Value>,
}

/// The signed twelve-item payload one region receives.
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
    pub fn signing_payload(&self) -> Result<String> {
        let mut parts = vec![
            self.sequence.to_string(),
            self.cell.clone(),
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
        Ok(vec![
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
        ])
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
