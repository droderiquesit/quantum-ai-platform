//! Four of the seven §24.9 superintelligence control contracts:
//! `InformationRequest` (R12-CONTRACT-6111), `ModelVote` (6112),
//! `ComputePlan` (6113) and `HedgePlan` (6114). The other three are
//! [`crate::forecast::ForecastState`] (6109),
//! [`crate::world_model::WorldBranch`] (6110) and
//! [`crate::capital::CapitalGrant`] (6115).
//!
//! Each existed as a fragment in a service — an information request with a
//! proposition and nothing else, a scored view with no version or cost, a
//! routing decision with no budget or verifier, a hedge proposal with no
//! rollback. These are the whole records, refused on decode when any field
//! is missing, so a control loop reading one cannot mistake a fragment for a
//! decision somebody finished making.

use crate::validated::{
    Contract, Validated, after, basis_points, listed, named, names, non_negative, positive, refuse,
};
use crate::venue::VenueId;
use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// ---- InformationRequest ------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InformationRequestFields {
    pub schema_version: u32,
    pub request_id: String,
    /// The question the information should answer.
    pub question: String,
    /// The expected reduction in decision uncertainty, in the account
    /// currency's value of information. Strictly positive: a request that
    /// expects to learn nothing is not worth its budget.
    pub expected_information_value: Decimal,
    pub issued_at: Timestamp,
    /// After this the answer is no longer useful.
    pub deadline: Timestamp,
    /// Source Registry ids the request may draw on; at least one, each one a
    /// source whose licence has already been evaluated.
    pub allowed_sources: Vec<String>,
    /// The most the request may spend, in the account currency.
    pub budget: Decimal,
}

/// The §24.9 active-sensing request. Built only through [`Validated::new`].
pub type InformationRequest = Validated<InformationRequestFields>;

impl Contract for InformationRequestFields {
    const NAME: &'static str = "InformationRequest";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a blank id or question, a non-positive information value or
    /// budget, a deadline not after issue, and no allowed source — an empty
    /// list is no sources, never all of them.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "request_id", &self.request_id)?;
        named(c, "question", &self.question)?;
        positive(
            c,
            "expected_information_value",
            self.expected_information_value,
        )?;
        after(c, "deadline", self.deadline, "issued_at", self.issued_at)?;
        names(c, "allowed_sources", &self.allowed_sources)?;
        positive(c, "budget", self.budget)
    }
}

// ---- ModelVote ---------------------------------------------------------

/// One past score of the voting model, oldest first in a history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorePoint {
    pub at: Timestamp,
    /// A proper score (Brier, log loss); lower is better, never negative.
    pub score: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelVoteFields {
    pub schema_version: u32,
    pub model: String,
    pub version: String,
    /// The proposition voted on.
    pub proposition: String,
    pub probability_bp: u32,
    /// The regime the model believes is in force.
    pub regime: String,
    /// At least one point, strictly increasing in time. A model with no
    /// history has no standing in a tournament.
    pub score_history: Vec<ScorePoint>,
    /// What producing the vote cost, in the account currency.
    pub compute_cost: Decimal,
    pub cast_at: Timestamp,
}

/// The §24.9 model-tournament vote. Built only through [`Validated::new`].
pub type ModelVote = Validated<ModelVoteFields>;

impl Contract for ModelVoteFields {
    const NAME: &'static str = "ModelVote";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names, a probability above one, an empty or unordered
    /// score history, a negative score or cost, and a history point after
    /// the vote was cast — a score from the future is leakage.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "model", &self.model)?;
        named(c, "version", &self.version)?;
        named(c, "proposition", &self.proposition)?;
        basis_points(c, "probability_bp", self.probability_bp)?;
        named(c, "regime", &self.regime)?;
        listed(c, "score_history", &self.score_history)?;
        for p in &self.score_history {
            non_negative(c, "score_history.score", p.score)?;
        }
        if self.score_history.windows(2).any(|w| w[1].at <= w[0].at) {
            return refuse(
                c,
                "has a score history that is not strictly increasing in time",
            );
        }
        if self.score_history.iter().any(|p| p.at > self.cast_at) {
            return refuse(c, "cites a score recorded after the vote was cast");
        }
        non_negative(c, "compute_cost", self.compute_cost)
    }
}

// ---- ForecastConsensus -------------------------------------------------

/// One voter's share of a [`ForecastConsensus`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsensusWeight {
    pub model: String,
    /// Its share of the consensus, in `[0, 1]`; the shares sum to one.
    pub weight: Decimal,
}

/// The consensus a tournament formed from its [`ModelVote`]s and the weight
/// each voter carried in it (R12-MODEL-9031).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastConsensusFields {
    pub schema_version: u32,
    /// The proposition voted on, as the votes name it.
    pub proposition: String,
    pub regime: String,
    /// The consensus probability of the proposition.
    pub probability_bp: u32,
    /// Every voter's share, by distinct model name.
    pub weights: Vec<ConsensusWeight>,
    /// Whether the shares were earned by reputation. `false` only where no
    /// voter had earned any, and the shares are then equal — stated, so an
    /// equal split cannot pass for an earned one.
    pub reputation_weighted: bool,
    pub formed_at: Timestamp,
}

/// The §24.9 consensus beside the votes. Built only through
/// [`Validated::new`].
pub type ForecastConsensus = Validated<ForecastConsensusFields>;

/// How far the shares may sum from one: they are rounded from statistics, and
/// a tolerance this tight still refuses a share that was dropped.
const CONSENSUS_SUM_TOLERANCE: Decimal = Decimal::from_raw(1_000);

impl Contract for ForecastConsensusFields {
    const NAME: &'static str = "ForecastConsensus";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names, a probability above one, no voters, a voter
    /// named twice, a negative share or one above one, shares that do not sum
    /// to one, and unequal shares claimed as unearned.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "proposition", &self.proposition)?;
        named(c, "regime", &self.regime)?;
        basis_points(c, "probability_bp", self.probability_bp)?;
        listed(c, "weights", &self.weights)?;
        let mut seen = BTreeSet::new();
        let mut sum = Decimal::ZERO;
        for w in &self.weights {
            named(c, "weights.model", &w.model)?;
            non_negative(c, "weights.weight", w.weight)?;
            if w.weight > Decimal::ONE {
                return refuse(c, format!("gives {} a share above one", w.model));
            }
            if !seen.insert(w.model.as_str()) {
                return refuse(c, format!("names {} twice", w.model));
            }
            sum += w.weight;
        }
        if (sum - Decimal::ONE).abs() > CONSENSUS_SUM_TOLERANCE {
            return refuse(c, format!("has shares summing to {sum}, not one"));
        }
        if !self.reputation_weighted
            && self
                .weights
                .windows(2)
                .any(|pair| pair[0].weight != pair[1].weight)
        {
            return refuse(c, "claims unearned shares that are not equal");
        }
        Ok(())
    }
}

// ---- ComputePlan -------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeBackend {
    Cpu,
    Gpu,
    Tpu,
    Qpu,
}

impl ComputeBackend {
    /// Whether a result from this backend can serve as the ADR 0006
    /// classical baseline.
    pub const fn is_classical(self) -> bool {
        !matches!(self, Self::Qpu)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeCandidate {
    pub id: String,
    pub backend: ComputeBackend,
    pub solver: String,
    pub estimated_cost: Decimal,
    pub estimated_latency: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputePlanFields {
    pub schema_version: u32,
    pub plan_id: String,
    /// The task to route, e.g. a QUBO's id.
    pub task: String,
    /// The candidate id that is the classical baseline; it always runs.
    pub classical_baseline: String,
    /// At least one.
    pub candidates: Vec<ComputeCandidate>,
    /// The most the whole plan may spend, in the account currency.
    pub budget: Decimal,
    /// What checks a result before it is used (a feasibility checker, the
    /// objective re-evaluated classically).
    pub verifier: String,
}

/// The §24.9 hybrid compute plan. Built only through [`Validated::new`].
pub type ComputePlan = Validated<ComputePlanFields>;

impl Contract for ComputePlanFields {
    const NAME: &'static str = "ComputePlan";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names, no candidate or one listed twice, a negative cost
    /// or latency, a non-positive budget, and — ADR 0006 made structural — a
    /// baseline that is not one of the candidates, that runs on a quantum
    /// backend, or that the budget cannot pay for. A plan whose baseline
    /// would be skipped for want of money is a quantum result with nothing
    /// to compare it to.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "plan_id", &self.plan_id)?;
        named(c, "task", &self.task)?;
        named(c, "verifier", &self.verifier)?;
        positive(c, "budget", self.budget)?;
        listed(c, "candidates", &self.candidates)?;
        let mut ids = BTreeSet::new();
        for k in &self.candidates {
            named(c, "candidates.id", &k.id)?;
            named(c, "candidates.solver", &k.solver)?;
            if !ids.insert(k.id.as_str()) {
                return refuse(c, format!("lists candidate `{}` twice", k.id));
            }
            non_negative(c, "candidates.estimated_cost", k.estimated_cost)?;
            if k.estimated_latency.as_nanos() < 0 {
                return refuse(c, "has a candidate with a negative latency");
            }
        }
        let Some(baseline) = self
            .candidates
            .iter()
            .find(|k| k.id == self.classical_baseline)
        else {
            return refuse(
                c,
                format!(
                    "names baseline `{}`, which is not a candidate; ADR 0006 requires a classical \
                     baseline every time",
                    self.classical_baseline
                ),
            );
        };
        if !baseline.backend.is_classical() {
            return refuse(c, "names a quantum candidate as its classical baseline");
        }
        if baseline.estimated_cost > self.budget {
            return refuse(
                c,
                "cannot afford its own classical baseline; raise the budget or drop the plan",
            );
        }
        Ok(())
    }
}

// ---- HedgePlan ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    pub risk_factor: String,
    /// Signed exposure in the account currency; never zero.
    pub amount: Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HedgeSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HedgeInstrument {
    pub instrument: String,
    pub venue: VenueId,
    pub side: HedgeSide,
    pub quantity: Decimal,
}

/// Whether the hedge can be put on and taken off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HedgeLiquidity {
    /// The hedge's largest share of average daily volume, in basis points.
    pub max_adv_share_bp: u32,
    /// How long unwinding the hedge would take at that share.
    pub time_to_exit: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HedgeRollback {
    /// The condition that triggers taking the hedge off.
    pub trigger: String,
    /// The ordered steps; at least one.
    pub steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HedgePlanFields {
    pub schema_version: u32,
    pub plan_id: String,
    pub exposures: Vec<Exposure>,
    /// Scenario ids the hedge protects against; at least one.
    pub scenarios_protected: Vec<String>,
    pub instruments: Vec<HedgeInstrument>,
    /// Expected cost of carrying the hedge, in the account currency.
    pub cost: Decimal,
    pub liquidity: HedgeLiquidity,
    pub rollback: HedgeRollback,
}

/// The §24.9 portfolio protection plan. A plan, not an order: nothing here
/// reaches a venue. Built only through [`Validated::new`].
pub type HedgePlan = Validated<HedgePlanFields>;

impl Contract for HedgePlanFields {
    const NAME: &'static str = "HedgePlan";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses no exposure or a zero one, no scenario, no instrument or a
    /// non-positive quantity, a negative cost, an ADV share above 100% or a
    /// non-positive exit time, and a rollback with no trigger or no step — a
    /// hedge nobody knows how to take off is a second position.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "plan_id", &self.plan_id)?;
        listed(c, "exposures", &self.exposures)?;
        for e in &self.exposures {
            named(c, "exposures.risk_factor", &e.risk_factor)?;
            if e.amount.is_zero() {
                return refuse(c, format!("hedges a zero exposure to `{}`", e.risk_factor));
            }
        }
        names(c, "scenarios_protected", &self.scenarios_protected)?;
        listed(c, "instruments", &self.instruments)?;
        for i in &self.instruments {
            named(c, "instruments.instrument", &i.instrument)?;
            named(c, "instruments.venue", i.venue.as_str())?;
            positive(c, "instruments.quantity", i.quantity)?;
        }
        non_negative(c, "cost", self.cost)?;
        basis_points(
            c,
            "liquidity.max_adv_share_bp",
            self.liquidity.max_adv_share_bp,
        )?;
        if self.liquidity.time_to_exit.as_nanos() <= 0 {
            return refuse(
                c,
                "states a non-positive time to exit; say how long unwinding takes",
            );
        }
        named(c, "rollback.trigger", &self.rollback.trigger)?;
        names(c, "rollback.steps", &self.rollback.steps)
    }
}
