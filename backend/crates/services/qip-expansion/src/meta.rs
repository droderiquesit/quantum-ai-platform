//! The Meta-Intelligence Brain's attention and compute market (blueprint
//! §9.12; R12-AMBIENT-7202, 3110, 3111, 3112, 3113, 3114).
//!
//! # The failure this prevents
//!
//! Every research item the desk started was charged its declared cost and
//! nothing asked whether the *next* unit of compute spent on it would buy
//! anything. A question that had stopped yielding kept its slot until its
//! sweep count ran out; a quantum experiment and a CPU simulation were the
//! same "compute unit"; and a model's track record entered no allocation at
//! all. Scaling followed the number of open items, not their value — which
//! is §9.12's whole objection.
//!
//! # What this decides
//!
//! [`MetaIntelligence::allocate`] runs one market round over [`WorkBid`]s:
//!
//! * Every unit of every bid is priced at its **marginal** value: the bid's
//!   value in basis points of decision value — expected uncertainty
//!   reduction *or* expected economic value, whichever is larger, as §9.12
//!   states it — weighted by the reputation of what does the work, scaled by
//!   how much of what this work promised it has actually delivered, and
//!   divided by `k + 1` for its `k`-th unit this round (diminishing returns:
//!   the second simulation of one question resolves less than the first).
//! * Units are granted greedily by marginal value per unit of cost, per
//!   resource type (CPU, GPU, TPU, QPU), until the resource's budget is
//!   spent or the next unit's value per cost falls below the floor.
//! * A QPU unit is never granted alone: its classical baseline is charged
//!   to the CPU budget at the same instant (ADR 0006), and a QPU grant whose
//!   baseline the CPU budget cannot pay is declined. Every QPU grant carries
//!   a validated [`ComputePlan`] naming the baseline.
//! * Bids that share a subject are redundant: only the most valuable is
//!   priced, and the rest are declined naming it.
//! * Work retired for persistent unproductivity is declined.
//!
//! Each grant is a [`ComputeAllocation`] whose rationale states the expected
//! value it was granted on (R12-AMBIENT-3110).
//!
//! [`MetaIntelligence::record_outcome`] is the other half: what a window of
//! granted units actually realised, per unit (R12-AMBIENT-3112). A work item
//! whose realised value per unit stays at or below the retirement floor for
//! `retire_after` consecutive windows is retired (R12-AMBIENT-3114), and its
//! realisation ratio scales every later bid it makes.
//!
//! # Statistics, not money
//!
//! Values here are basis points of a decision's value and compute units, and
//! the market arithmetic is `f64`: these are estimates, not balances. Budgets
//! and unit costs arrive as [`Decimal`] compute units and cross into `f64`
//! at exactly one place, [`to_f64_units`]; nothing here moves money.

use qip_contracts::ambient::{
    ComputeAllocation, ComputeResourceType, MAX_SEVERITY_BP, ModelReputation,
};
use qip_contracts::control::{ComputeBackend, ComputeCandidate, ComputePlan, ComputePlanFields};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The most bids one round prices. A round fed more is refused rather than
/// truncated: which bids were dropped would be an unrecorded decision.
pub const MAX_BIDS: usize = 512;
/// The most units one bid may ask for in a round.
pub const MAX_UNITS_PER_BID: u32 = 64;
/// Work items whose history is kept. The oldest-updated is forgotten first.
pub const MAX_TRACKED: usize = 1_024;
/// The weight an uninformed reputation cell carries: an explicit prior, not
/// the global average — a model's record in one regime says nothing about a
/// regime it has never traded (R12-AMBIENT-3111).
pub const UNINFORMED_REPUTATION_BP: u32 = 5_000;
/// One notional window of full delivery added to both sides of a work
/// item's realisation ratio, in basis points: an empty window lowers the
/// next bid without zeroing it, so the market keeps measuring until the
/// retirement rule can decide. See `WorkHistory::realisation`.
pub const REALISATION_PRIOR_BP: f64 = 1_000.0;

/// What a bid's compute would be spent on (§9.12: "questions, models,
/// agents, simulations and solvers").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    Question,
    Model,
    Agent,
    Simulation,
    Solver,
    DataSource,
}

impl WorkKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Model => "model",
            Self::Agent => "agent",
            Self::Simulation => "simulation",
            Self::Solver => "solver",
            Self::DataSource => "data_source",
        }
    }
}

pub const fn resource_str(resource: ComputeResourceType) -> &'static str {
    match resource {
        ComputeResourceType::Cpu => "cpu",
        ComputeResourceType::Gpu => "gpu",
        ComputeResourceType::Tpu => "tpu",
        ComputeResourceType::Qpu => "qpu",
    }
}

/// The cell a reputation is held in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReputationKey {
    pub domain: String,
    pub horizon: String,
    pub regime: String,
}

impl ReputationKey {
    pub fn new(domain: &str, horizon: &str, regime: &str) -> Result<Self> {
        for (name, value) in [("domain", domain), ("horizon", horizon), ("regime", regime)] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a reputation key needs a {name}; a blank cell would pool every record into one"
                )));
            }
        }
        Ok(Self {
            domain: domain.trim().to_string(),
            horizon: horizon.trim().to_string(),
            regime: regime.trim().to_string(),
        })
    }
}

/// What the book knows about one cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing<'a> {
    Informed(&'a ModelReputation),
    /// Nothing recorded for this exact cell. Deliberately not the average of
    /// the domain's other cells.
    Uninformed,
}

impl Standing<'_> {
    /// The weight the allocator applies, in basis points.
    pub fn weight_bp(&self) -> u32 {
        match self {
            Self::Informed(reputation) => reputation.overall_bp(),
            Self::Uninformed => UNINFORMED_REPUTATION_BP,
        }
    }
}

/// Model reputation by domain, horizon and regime (R12-AMBIENT-3111).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReputationBook {
    cells: BTreeMap<ReputationKey, ModelReputation>,
}

impl ReputationBook {
    /// Record the latest reputation for its own cell. Bounded by
    /// [`MAX_TRACKED`]; past it the cell is refused rather than an old one
    /// silently replaced.
    pub fn record(&mut self, reputation: ModelReputation) -> Result<()> {
        let key = ReputationKey::new(
            reputation.domain(),
            reputation.horizon(),
            reputation.regime(),
        )?;
        if !self.cells.contains_key(&key) && self.cells.len() >= MAX_TRACKED {
            return Err(Error::invalid(format!(
                "the reputation book holds {MAX_TRACKED} cells; retire one before recording `{}/{}/{}`",
                key.domain, key.horizon, key.regime
            )));
        }
        self.cells.insert(key, reputation);
        Ok(())
    }

    pub fn standing(&self, key: &ReputationKey) -> Standing<'_> {
        self.cells
            .get(key)
            .map_or(Standing::Uninformed, Standing::Informed)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// One work item asking for compute this round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkBid {
    pub work_id: String,
    pub kind: WorkKind,
    /// What the work serves. Two bids with one subject are redundant.
    pub subject: String,
    pub resource: ComputeResourceType,
    /// Expected uncertainty reduction of the first unit, in basis points of
    /// the decision's uncertainty.
    pub information_value_bp: u32,
    /// Expected economic value of resolving the question, in basis points of
    /// the decision's value. A stated zero is zero.
    pub economic_value_bp: u32,
    /// Compute units one unit of this work costs on its resource.
    pub unit_cost: Decimal,
    pub max_units: u32,
    /// Whose record weights the bid.
    pub reputation: ReputationKey,
    /// The classical baseline's cost per unit, charged to the CPU budget.
    /// Required for a QPU bid (ADR 0006); ignored otherwise.
    pub classical_baseline_cost: Option<Decimal>,
}

impl WorkBid {
    fn validate(&self) -> Result<()> {
        if self.work_id.trim().is_empty() || self.subject.trim().is_empty() {
            return Err(Error::invalid(
                "a work bid names its work and its subject; neither may be blank",
            ));
        }
        if self.information_value_bp > MAX_SEVERITY_BP || self.economic_value_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "bid `{}` states a value above {MAX_SEVERITY_BP} bp; refuse it at the source rather than clamp it",
                self.work_id
            )));
        }
        if !self.unit_cost.is_positive() {
            return Err(Error::invalid(format!(
                "bid `{}` costs {} a unit; a free unit would be granted without limit",
                self.work_id, self.unit_cost
            )));
        }
        if self.max_units == 0 || self.max_units > MAX_UNITS_PER_BID {
            return Err(Error::invalid(format!(
                "bid `{}` asks for {} unit(s); ask for 1..={MAX_UNITS_PER_BID}",
                self.work_id, self.max_units
            )));
        }
        if self.resource == ComputeResourceType::Qpu
            && self
                .classical_baseline_cost
                .is_none_or(|cost| cost.is_negative())
        {
            return Err(Error::invalid(format!(
                "QPU bid `{}` names no classical baseline cost; ADR 0006 computes a classical \
                 baseline every time a quantum path runs",
                self.work_id
            )));
        }
        Ok(())
    }

    /// The first unit's value before reputation and realisation: §9.12's
    /// "expected uncertainty reduction or expected economic value".
    pub fn base_value_bp(&self) -> u32 {
        self.information_value_bp.max(self.economic_value_bp)
    }
}

/// The market's floors and memory.
#[derive(Clone, Debug, PartialEq)]
pub struct MetaPolicy {
    /// The least marginal value, in basis points per compute unit, a unit
    /// is granted at.
    pub value_floor_bp_per_unit_cost: f64,
    /// Realised basis points per unit at or below which a window counts as
    /// unproductive.
    pub retire_floor_bp_per_unit: f64,
    /// Consecutive unproductive windows before work is retired.
    pub retire_after: u32,
    /// Windows of realised history kept per work item.
    pub window: usize,
    /// Residual economic weight, in the caller's money unit, at or above
    /// which a specialist population is proposed for a domain.
    pub specialist_threshold: Decimal,
}

impl MetaPolicy {
    pub fn validate(&self) -> Result<()> {
        if !self.value_floor_bp_per_unit_cost.is_finite()
            || self.value_floor_bp_per_unit_cost < 0.0
            || !self.retire_floor_bp_per_unit.is_finite()
            || self.retire_floor_bp_per_unit < 0.0
        {
            return Err(Error::invalid(
                "the market's floors are non-negative, finite basis points",
            ));
        }
        if self.retire_after == 0 || self.window == 0 {
            return Err(Error::invalid(
                "retire_after and window are at least one; zero would retire work on its first grant",
            ));
        }
        if !self.specialist_threshold.is_positive() {
            return Err(Error::invalid(
                "the specialist threshold is positive; at zero every residual would spawn a population",
            ));
        }
        Ok(())
    }
}

impl Default for MetaPolicy {
    fn default() -> Self {
        Self {
            value_floor_bp_per_unit_cost: 50.0,
            retire_floor_bp_per_unit: 1.0,
            retire_after: 3,
            window: 8,
            specialist_threshold: Decimal::from_int(1_000),
        }
    }
}

/// One measured window: what `units` of granted compute realised
/// (R12-AMBIENT-3112).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginalValue {
    pub work_id: String,
    pub kind: WorkKind,
    pub units: u32,
    /// What the grant was made on, in basis points.
    pub expected_bp: f64,
    /// What it delivered, in basis points.
    pub realised_bp: f64,
    /// `realised_bp / units`: the marginal value of one added unit.
    pub per_unit_bp: f64,
    pub at: Timestamp,
}

#[derive(Clone, Debug, Default)]
struct WorkHistory {
    windows: VecDeque<MarginalValue>,
    unproductive_streak: u32,
    last_update: Option<Timestamp>,
}

impl WorkHistory {
    /// Realised over expected across the kept windows, each side seeded
    /// with [`REALISATION_PRIOR_BP`] of full delivery; one with no history.
    ///
    /// The prior is what lets retirement fire. Without it one empty window
    /// scored the work at zero, the market stopped granting it, no further
    /// window was ever measured, and the `retire_after` streak stalled at
    /// one — work starved in place, holding its started slot, and the
    /// retirement rule read as a control while being unable to trigger.
    fn realisation(&self) -> f64 {
        let expected: f64 = self.windows.iter().map(|w| w.expected_bp).sum();
        let realised: f64 = self.windows.iter().map(|w| w.realised_bp).sum();
        (realised + REALISATION_PRIOR_BP) / (expected + REALISATION_PRIOR_BP)
    }
}

/// Why a bid got nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Decline {
    Retired {
        windows: u32,
    },
    Redundant {
        with: String,
    },
    /// Its first unit's value per cost was below the floor.
    BelowFloor,
    /// The resource's budget was spent on better work.
    Outbid,
    /// A QPU bid whose classical baseline the CPU budget could not pay.
    NoClassicalBaseline,
    /// The next unit would take the agent whose record weights the bid past
    /// the compute budget the plan gave that agent (R12-AMBIENT-9034).
    OverAgentBudget,
}

impl Decline {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Retired { .. } => "retired",
            Self::Redundant { .. } => "redundant",
            Self::BelowFloor => "below_floor",
            Self::Outbid => "outbid",
            Self::NoClassicalBaseline => "no_classical_baseline",
            Self::OverAgentBudget => "over_agent_budget",
        }
    }
}

/// What one bid was granted.
#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub allocation: ComputeAllocation,
    pub kind: WorkKind,
    pub subject: String,
    /// The value, in basis points, the units were granted on: the sum of each
    /// granted unit's marginal value.
    pub expected_bp: f64,
    /// The last granted unit's marginal value per compute unit — what the
    /// market cleared at for this work.
    pub clearing_bp_per_unit_cost: f64,
    /// Compute units charged, including a QPU grant's baseline.
    pub charged: BTreeMap<ComputeResourceType, Decimal>,
    /// The hybrid plan, for a QPU grant (ADR 0006).
    pub plan: Option<ComputePlan>,
}

/// One market round.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AllocationRound {
    pub grants: Vec<Grant>,
    pub declined: Vec<(String, Decline)>,
    pub spent: BTreeMap<ComputeResourceType, Decimal>,
}

impl AllocationRound {
    pub fn grant(&self, work_id: &str) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.allocation.work_id() == work_id)
    }

    pub fn granted_ids(&self) -> BTreeSet<String> {
        self.grants
            .iter()
            .map(|g| g.allocation.work_id().to_string())
            .collect()
    }
}

/// A domain whose residual error is economically meaningful
/// (R12-AMBIENT-3113).
#[derive(Clone, Debug, PartialEq)]
pub struct DomainResidual {
    pub domain: String,
    /// The error left after the current population, as a fraction in [0, 1].
    pub residual: f64,
    /// The money that error is applied to, in the caller's unit.
    pub exposure: Decimal,
}

/// A proposal to create a specialist population for one domain.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecialistProposal {
    pub domain: String,
    pub residual: f64,
    /// `residual × exposure`: what the residual costs, in the exposure's unit.
    pub economic_weight: Decimal,
    pub rationale: String,
}

/// The allocator and its memory.
#[derive(Debug)]
pub struct MetaIntelligence {
    policy: MetaPolicy,
    reputation: ReputationBook,
    history: BTreeMap<String, WorkHistory>,
    retired: BTreeMap<String, u32>,
    plans_issued: u64,
}

/// The one crossing from compute units to the market's arithmetic.
pub fn to_f64_units(units: Decimal) -> f64 {
    units.to_f64()
}

impl MetaIntelligence {
    pub fn new(policy: MetaPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            reputation: ReputationBook::default(),
            history: BTreeMap::new(),
            retired: BTreeMap::new(),
            plans_issued: 0,
        })
    }

    pub const fn policy(&self) -> &MetaPolicy {
        &self.policy
    }

    pub const fn reputation(&self) -> &ReputationBook {
        &self.reputation
    }

    pub fn reputation_mut(&mut self) -> &mut ReputationBook {
        &mut self.reputation
    }

    pub fn is_retired(&self, work_id: &str) -> bool {
        self.retired.contains_key(work_id)
    }

    /// The retired work and the unproductive windows that retired each.
    pub fn retired(&self) -> impl Iterator<Item = (&String, &u32)> {
        self.retired.iter()
    }

    /// The kept windows for one work item, oldest first.
    pub fn marginal_values(&self, work_id: &str) -> impl Iterator<Item = &MarginalValue> {
        self.history
            .get(work_id)
            .into_iter()
            .flat_map(|h| h.windows.iter())
    }

    /// Forget work that no longer exists, so a closed item's retirement does
    /// not outlive it and the memory stays bounded by what is open.
    pub fn forget(&mut self, work_id: &str) {
        self.history.remove(work_id);
        self.retired.remove(work_id);
    }

    /// The marginal value of the `k`-th unit (zero-based) of `bid` this
    /// round, in basis points.
    pub fn marginal_value_bp(&self, bid: &WorkBid, k: u32) -> f64 {
        let reputation = f64::from(self.reputation.standing(&bid.reputation).weight_bp())
            / f64::from(MAX_SEVERITY_BP);
        let realisation = self
            .history
            .get(&bid.work_id)
            .map_or(1.0, WorkHistory::realisation);
        f64::from(bid.base_value_bp()) * reputation * realisation / f64::from(k + 1)
    }

    /// One market round. See the module doc for the rules. No agent holds
    /// a budget of its own; [`Self::allocate_within`] states them.
    pub fn allocate(
        &mut self,
        bids: &[WorkBid],
        budget: &BTreeMap<ComputeResourceType, Decimal>,
        now: Timestamp,
    ) -> Result<AllocationRound> {
        self.allocate_within(bids, budget, &BTreeMap::new(), now)
    }

    /// One market round in which each agent named in `agent_budgets` — the
    /// [`ReputationKey`] whose record weights its bids — may be charged at
    /// most its stated compute units across every resource, the QPU
    /// baseline included. A unit that would take an agent past its budget is
    /// refused, and a bid refused before its first unit is declined
    /// [`Decline::OverAgentBudget`].
    ///
    /// The failure it prevents: one agent with a high reputation outbids
    /// every other for the whole round, so the per-resource budget bounds
    /// the platform and nothing bounds the agent. An agent the map does not
    /// name is bounded by the resource budgets only, as in [`Self::allocate`].
    pub fn allocate_within(
        &mut self,
        bids: &[WorkBid],
        budget: &BTreeMap<ComputeResourceType, Decimal>,
        agent_budgets: &BTreeMap<ReputationKey, Decimal>,
        now: Timestamp,
    ) -> Result<AllocationRound> {
        for (agent, amount) in agent_budgets {
            if amount.is_negative() {
                return Err(Error::invalid(format!(
                    "the budget for agent {}/{}/{} is {amount}; state zero to grant it nothing",
                    agent.domain, agent.horizon, agent.regime
                )));
            }
        }
        let mut agent_left: BTreeMap<&ReputationKey, f64> = agent_budgets
            .iter()
            .map(|(agent, amount)| (agent, to_f64_units(*amount)))
            .collect();
        if bids.len() > MAX_BIDS {
            return Err(Error::invalid(format!(
                "{} bids in one round; the market prices at most {MAX_BIDS}, and dropping some \
                 silently would be an allocation nobody recorded",
                bids.len()
            )));
        }
        let mut ids = BTreeSet::new();
        for bid in bids {
            bid.validate()?;
            if !ids.insert(bid.work_id.as_str()) {
                return Err(Error::invalid(format!(
                    "`{}` bids twice in one round; one work item makes one bid",
                    bid.work_id
                )));
            }
        }
        for (resource, amount) in budget {
            if amount.is_negative() {
                return Err(Error::invalid(format!(
                    "the {} budget is {amount}; state zero to grant nothing",
                    resource_str(*resource)
                )));
            }
        }

        let mut round = AllocationRound::default();
        // Retired, then redundant: of bids sharing a subject only the most
        // valuable first unit is priced; ties go to the lower work id so a
        // replay declines the same bid.
        let mut eligible: Vec<&WorkBid> = Vec::new();
        let mut best_by_subject: BTreeMap<&str, &WorkBid> = BTreeMap::new();
        for bid in bids {
            if let Some(windows) = self.retired.get(&bid.work_id) {
                round
                    .declined
                    .push((bid.work_id.clone(), Decline::Retired { windows: *windows }));
                continue;
            }
            let value = self.marginal_value_bp(bid, 0);
            match best_by_subject.get(bid.subject.as_str()) {
                Some(best)
                    if self
                        .marginal_value_bp(best, 0)
                        .total_cmp(&value)
                        .then_with(|| bid.work_id.cmp(&best.work_id))
                        .is_gt() => {}
                _ => {
                    best_by_subject.insert(bid.subject.as_str(), bid);
                }
            }
        }
        for bid in bids {
            if self.retired.contains_key(&bid.work_id) {
                continue;
            }
            match best_by_subject.get(bid.subject.as_str()) {
                Some(best) if best.work_id == bid.work_id => eligible.push(bid),
                Some(best) => round.declined.push((
                    bid.work_id.clone(),
                    Decline::Redundant {
                        with: best.work_id.clone(),
                    },
                )),
                None => {}
            }
        }

        let mut remaining: BTreeMap<ComputeResourceType, f64> = ComputeResourceType::ALL
            .into_iter()
            .map(|r| (r, budget.get(&r).copied().map_or(0.0, to_f64_units)))
            .collect();
        // Units granted, expected value, last clearing price, per bid index.
        let mut units: Vec<u32> = vec![0; eligible.len()];
        let mut expected: Vec<f64> = vec![0.0; eligible.len()];
        let mut clearing: Vec<f64> = vec![0.0; eligible.len()];
        let mut stopped: Vec<Option<Decline>> = vec![None; eligible.len()];
        let floor = self.policy.value_floor_bp_per_unit_cost;
        loop {
            // The best next unit across every open bid: value per compute
            // unit, the baseline included in a QPU unit's cost.
            let mut best: Option<(usize, f64, f64)> = None;
            for (i, bid) in eligible.iter().enumerate() {
                if stopped[i].is_some() || units[i] >= bid.max_units {
                    continue;
                }
                let value = self.marginal_value_bp(bid, units[i]);
                let cost = unit_cost_with_baseline(bid);
                let per_cost = value / cost;
                if per_cost < floor {
                    stopped[i] = Some(if units[i] == 0 {
                        Decline::BelowFloor
                    } else {
                        Decline::Outbid
                    });
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((j, _, best_per_cost)) => per_cost
                        .total_cmp(&best_per_cost)
                        .then_with(|| eligible[j].work_id.cmp(&bid.work_id))
                        .is_gt(),
                };
                if better {
                    best = Some((i, value, per_cost));
                }
            }
            let Some((i, value, per_cost)) = best else {
                break;
            };
            let bid = eligible[i];
            let own = to_f64_units(bid.unit_cost);
            let baseline = if bid.resource == ComputeResourceType::Qpu {
                bid.classical_baseline_cost.map_or(0.0, to_f64_units)
            } else {
                0.0
            };
            let own_left = remaining.get(&bid.resource).copied().unwrap_or(0.0);
            let cpu_left = remaining
                .get(&ComputeResourceType::Cpu)
                .copied()
                .unwrap_or(0.0);
            if own_left < own {
                stopped[i] = Some(Decline::Outbid);
                continue;
            }
            if bid.resource == ComputeResourceType::Qpu && cpu_left < baseline {
                stopped[i] = Some(Decline::NoClassicalBaseline);
                continue;
            }
            if let Some(left) = agent_left.get_mut(&bid.reputation) {
                if *left < own + baseline {
                    stopped[i] = Some(Decline::OverAgentBudget);
                    continue;
                }
                *left -= own + baseline;
            }
            if let Some(left) = remaining.get_mut(&bid.resource) {
                *left -= own;
            }
            if baseline > 0.0
                && let Some(left) = remaining.get_mut(&ComputeResourceType::Cpu)
            {
                *left -= baseline;
            }
            units[i] += 1;
            expected[i] += value;
            clearing[i] = per_cost;
        }

        for (i, bid) in eligible.iter().enumerate() {
            if units[i] == 0 {
                round.declined.push((
                    bid.work_id.clone(),
                    stopped[i].clone().unwrap_or(Decline::Outbid),
                ));
                continue;
            }
            let count = Decimal::from_int(i64::from(units[i]));
            let mut charged = BTreeMap::new();
            let own = bid
                .unit_cost
                .checked_mul(count)
                .ok_or_else(|| Error::numeric("a grant's cost overflowed"))?;
            charged.insert(bid.resource, own);
            let mut plan = None;
            if bid.resource == ComputeResourceType::Qpu {
                let baseline = bid
                    .classical_baseline_cost
                    .unwrap_or(Decimal::ZERO)
                    .checked_mul(count)
                    .ok_or_else(|| Error::numeric("a baseline's cost overflowed"))?;
                // A QPU bid's own resource is the QPU, so the CPU slot is
                // the baseline's alone.
                charged.insert(ComputeResourceType::Cpu, baseline);
                plan = Some(self.plan_for(bid, own, baseline)?);
            }
            for (resource, amount) in &charged {
                let slot = round.spent.entry(*resource).or_insert(Decimal::ZERO);
                *slot = slot
                    .checked_add(*amount)
                    .ok_or_else(|| Error::numeric("round spend overflowed"))?;
            }
            let standing = self.reputation.standing(&bid.reputation);
            let reputation = match &standing {
                Standing::Informed(reputation) => (*reputation).clone(),
                Standing::Uninformed => ModelReputation::new(
                    &bid.reputation.domain,
                    &bid.reputation.horizon,
                    &bid.reputation.regime,
                    UNINFORMED_REPUTATION_BP,
                    UNINFORMED_REPUTATION_BP,
                    UNINFORMED_REPUTATION_BP,
                )?,
            };
            let rationale = format!(
                "{} unit(s) of {} for {} `{}` on an expected {:.1} bp (first unit {:.1} bp from \
                 max(information {} bp, economic {} bp) × reputation {} bp{} × realisation \
                 {:.2}); cleared at {:.2} bp per compute unit against a floor of {:.2}",
                units[i],
                resource_str(bid.resource),
                bid.kind.as_str(),
                bid.subject,
                expected[i],
                self.marginal_value_bp(bid, 0),
                bid.information_value_bp,
                bid.economic_value_bp,
                standing.weight_bp(),
                if matches!(standing, Standing::Uninformed) {
                    " (uninformed)"
                } else {
                    ""
                },
                self.history
                    .get(&bid.work_id)
                    .map_or(1.0, WorkHistory::realisation),
                clearing[i],
                floor,
            );
            round.grants.push(Grant {
                allocation: ComputeAllocation::new(
                    bid.work_id.clone(),
                    bid.resource,
                    units[i],
                    now,
                    reputation,
                    rationale,
                )?,
                kind: bid.kind,
                subject: bid.subject.clone(),
                expected_bp: expected[i],
                clearing_bp_per_unit_cost: clearing[i],
                charged,
                plan,
            });
        }
        Ok(round)
    }

    fn plan_for(&mut self, bid: &WorkBid, qpu: Decimal, baseline: Decimal) -> Result<ComputePlan> {
        self.plans_issued = self.plans_issued.saturating_add(1);
        let budget = qpu
            .checked_add(baseline)
            .ok_or_else(|| Error::numeric("a plan's budget overflowed"))?;
        ComputePlan::new(ComputePlanFields {
            schema_version: 1,
            plan_id: format!("meta-plan-{:06}", self.plans_issued),
            task: bid.work_id.clone(),
            classical_baseline: format!("{}:classical", bid.work_id),
            candidates: vec![
                ComputeCandidate {
                    id: format!("{}:classical", bid.work_id),
                    backend: ComputeBackend::Cpu,
                    solver: "classical-baseline".to_string(),
                    estimated_cost: baseline,
                    estimated_latency: Duration::ZERO,
                },
                ComputeCandidate {
                    id: format!("{}:qpu", bid.work_id),
                    backend: ComputeBackend::Qpu,
                    solver: "qpu".to_string(),
                    estimated_cost: qpu,
                    estimated_latency: Duration::ZERO,
                },
            ],
            // A zero-cost baseline still needs a positive budget to validate;
            // the QPU's own cost is positive by `WorkBid::validate`.
            budget,
            verifier: "the objective re-evaluated classically against the baseline".to_string(),
        })
    }

    /// Record what a window of granted units realised, and retire work that
    /// has stayed unproductive for `retire_after` windows. Returns the
    /// window's measurement.
    pub fn record_outcome(
        &mut self,
        work_id: &str,
        kind: WorkKind,
        units: u32,
        expected_bp: f64,
        realised_bp: f64,
        at: Timestamp,
    ) -> Result<MarginalValue> {
        if units == 0 {
            return Err(Error::invalid(format!(
                "`{work_id}` realised something on zero units; record the window a grant paid for"
            )));
        }
        for (name, value) in [("expected", expected_bp), ("realised", realised_bp)] {
            if !value.is_finite() || value < 0.0 {
                return Err(Error::invalid(format!(
                    "`{work_id}`'s {name} value is {value}; a window's value is a finite, \
                     non-negative number of basis points"
                )));
            }
        }
        if !self.history.contains_key(work_id) && self.history.len() >= MAX_TRACKED {
            let oldest = self
                .history
                .iter()
                .min_by(|a, b| a.1.last_update.cmp(&b.1.last_update).then(a.0.cmp(b.0)))
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.history.remove(&oldest);
            }
        }
        let window = self.policy.window;
        let retire_after = self.policy.retire_after;
        let floor = self.policy.retire_floor_bp_per_unit;
        let measured = MarginalValue {
            work_id: work_id.to_string(),
            kind,
            units,
            expected_bp,
            realised_bp,
            per_unit_bp: realised_bp / f64::from(units),
            at,
        };
        let history = self.history.entry(work_id.to_string()).or_default();
        if history.windows.len() == window {
            history.windows.pop_front();
        }
        history.windows.push_back(measured.clone());
        history.last_update = Some(at);
        if measured.per_unit_bp <= floor {
            history.unproductive_streak = history.unproductive_streak.saturating_add(1);
        } else {
            history.unproductive_streak = 0;
        }
        if history.unproductive_streak >= retire_after {
            self.retired
                .insert(work_id.to_string(), history.unproductive_streak);
        }
        Ok(measured)
    }

    /// Domains whose residual error, applied to its exposure, is worth a new
    /// specialist population. Ordered by weight, heaviest first, then by
    /// domain.
    pub fn propose_specialists(
        &self,
        residuals: &[DomainResidual],
    ) -> Result<Vec<SpecialistProposal>> {
        let mut proposals = Vec::new();
        for r in residuals {
            if !r.residual.is_finite() || !(0.0..=1.0).contains(&r.residual) {
                return Err(Error::invalid(format!(
                    "the residual for `{}` is {}; a residual error is a fraction in [0, 1]",
                    r.domain, r.residual
                )));
            }
            if r.exposure.is_negative() {
                return Err(Error::invalid(format!(
                    "the exposure for `{}` is {}; state its magnitude",
                    r.domain, r.exposure
                )));
            }
            // Statistic into money: the residual is an error rate, the
            // exposure an amount; their product is the money the error is
            // applied to, rounded to the Decimal's scale.
            let residual = Decimal::from_f64(r.residual)
                .ok_or_else(|| Error::numeric("a residual has no decimal form"))?;
            let weight = r
                .exposure
                .checked_mul(residual)
                .ok_or_else(|| Error::numeric("a residual's economic weight overflowed"))?;
            if weight >= self.policy.specialist_threshold {
                proposals.push(SpecialistProposal {
                    domain: r.domain.clone(),
                    residual: r.residual,
                    economic_weight: weight,
                    rationale: format!(
                        "residual error {:.3} on an exposure of {} costs {weight}, at or past the \
                         {} at which a specialist population is worth creating",
                        r.residual, r.exposure, self.policy.specialist_threshold
                    ),
                });
            }
        }
        proposals.sort_by(|a, b| {
            b.economic_weight
                .cmp(&a.economic_weight)
                .then_with(|| a.domain.cmp(&b.domain))
        });
        Ok(proposals)
    }
}

fn unit_cost_with_baseline(bid: &WorkBid) -> f64 {
    let own = to_f64_units(bid.unit_cost);
    if bid.resource == ComputeResourceType::Qpu {
        own + bid.classical_baseline_cost.map_or(0.0, to_f64_units)
    } else {
        own
    }
}
