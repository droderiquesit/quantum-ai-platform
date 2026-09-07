//! The seam between the lifecycle ledger's capital veto and the §23.4 pool
//! arithmetic.
//!
//! Two services need to know about each other here, so they meet in the
//! runtime, which is the only layer permitted to introduce them
//! (`.claude/rules/architecture/00-boundaries.md`).
//!
//! # Why this is a module rather than a dependency edge
//!
//! `qip-lifecycle` refuses promotions to rungs that hold capital. The blueprint
//! §23.4 pool arithmetic — four pools summing to one total exactly once, the
//! unfunded commitment liability charged to the reserved pool whether or not
//! anyone budgeted against it, the multi-source horizon register — lives in
//! `qip_optimization_engine::horizons`, and that crate reaches `qip-quantum`.
//!
//! A veto crate that can reach a solver is a veto whose input is an optimiser's
//! answer: the thing meant to constrain the optimiser becomes the thing asking
//! it. `qip-acceptance`'s
//! `nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver`
//! refuses that transitively, and its own comment predicts the exact route —
//! not a direct edge to the solver, which a reviewer would question, but an
//! edge to the optimiser, which looks entirely reasonable on a risk crate until
//! you notice what it drags behind it. `qip-lifecycle` took that edge and the
//! test caught it.
//!
//! The answer is not a second implementation of the pool arithmetic. It is this
//! adapter: the arithmetic stays where it is, called from the runtime, and
//! `qip-lifecycle` names only the figures it refuses on, through
//! [`qip_lifecycle::horizon::HorizonReconciler`].
//!
//! # What this side decides, and what it does not
//!
//! This side **measures**. It settles the register, reconciles the candidate
//! against every strategy already drawing on a pool, and reports each bucket's
//! pool and commitment. It deliberately does not report a verdict, and in
//! particular it does not turn a standing disagreement into an error: an
//! unsettled register is returned as the disagreement it is, so that the
//! refusal is taken by the gate on figures rather than relayed from here. A
//! control whose whole content is forwarding somebody else's answer reads as
//! protection and is not.
//!
//! # What is wired, and what is still stated by a person
//!
//! A [`PoolReconciler`] is built and attached to the lifecycle ledger by
//! [`super::CentralPlane::arm_horizons`], which the LEARN stage calls every
//! cycle through [`crate::Platform::arm_horizon_gate`]. So the gate fires on
//! figures this platform computed: the budgets are the allocator's own sizing
//! of the whole proposal book at the cycle's drawdown, and the unfunded
//! commitment liability is `CommitmentBook::unfunded_total` at the cycle's
//! instant. Neither is restated anywhere and neither can go stale by more than
//! one cycle.
//!
//! What no code can supply is the two facts nobody measures: how the risk
//! budget divides four ways, and which horizon a strategy sits at. Both are
//! desk statements, and [`HorizonPolicy`] on
//! [`super::plane::CentralConfig`] is where the desk states them — the same
//! shape and for the same reason as `CentralConfig::arbitrage`. With no policy
//! stated the plane arms nothing and says so on the cycle; a policy that does
//! not sum to the configured budget stops the process at start-up rather than
//! reconciling promotions against capital nobody has.
//!
//! The honest remainder, so nobody reads more into this than is here: no
//! deployment states a policy yet, because `CentralConfig::horizons` is read
//! from configuration in a composition root and no root loads it; and nothing
//! in this tree promotes a strategy outside a test, so the gate has no
//! production *event* to refuse either. Both gaps are outside this module and
//! neither is closed by pretending in here.

use qip_contracts::signal::StrategyId;
use qip_core::decimal::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::StrategyId as NumericStrategyId;
use qip_lifecycle::horizon::{
    HorizonBucket, HorizonDisagreement, HorizonFunding, HorizonReconciler, HorizonStanding,
};
use qip_optimization_engine::families::FamilyId;
use qip_optimization_engine::horizons::{
    CapitalPools, FamilyBudget, Horizon, HorizonDispute, HorizonReconciliation, HorizonRegister,
    HorizonSource, SettledHorizons, reconcile,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One source's statement that one strategy sits at one §23.4 horizon.
///
/// Attributed, because the whole value of the register behind it is that when
/// two claims disagree an operator can see which two and repair the one that is
/// wrong. Configuration rather than a measurement because the platform measures
/// no strategy's horizon anywhere: asserting a strategy attribute nobody
/// measured is how a backtest becomes a story.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HorizonClaim {
    pub strategy: StrategyId,
    /// The component or operator making the claim.
    pub source: String,
    pub horizon: Horizon,
}

/// The desk's §23.4 statement: how the risk budget divides four ways, and which
/// horizon each strategy sits at.
///
/// Stated rather than derived, and the two halves are stated for different
/// reasons. The split is a treasury decision — capital that belongs to no
/// horizon is capital two horizons will both spend, and there is no arithmetic
/// that can invent the division. The claims are stated because nothing in this
/// platform records how long a strategy holds; deriving one from a program's
/// bar interval would be an inference wearing a measurement's clothes.
///
/// The total is deliberately absent: it is
/// [`super::plane::CentralConfig::total_budget`], which the desk has already
/// stated once. A second total here would be a second claim on the same fact,
/// and [`CapitalPools::new`] refusing the sum is the cross-check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HorizonPolicy {
    pub available_inventory: Decimal,
    pub deployable_capital: Decimal,
    pub capital_not_reserved_for_calls: Decimal,
    pub reserved_capital: Decimal,
    /// Every source's claim about every strategy. Two sources disagreeing is
    /// recorded and refused at the gate rather than refused here; one source
    /// contradicting itself is refused here, at start-up.
    pub claims: Vec<HorizonClaim>,
    /// The reason a desk is deciding over an unresolved disagreement, if it
    /// has taken one. `None` means a disputed strategy cannot be promoted to a
    /// rung that holds capital.
    #[serde(default)]
    pub despite: Option<String>,
}

impl HorizonPolicy {
    /// The four pools of this policy against a stated total and a measured
    /// liability.
    ///
    /// `unfunded_commitments` is a figure the platform computes — it is not on
    /// this policy — because a liability an operator restates in configuration
    /// is a second claim about a number the commitment book already holds, and
    /// the louder one would be wrong.
    pub fn pools(&self, total: Decimal, unfunded_commitments: Decimal) -> Result<CapitalPools> {
        CapitalPools::new(
            total,
            self.available_inventory,
            self.deployable_capital,
            self.capital_not_reserved_for_calls,
            self.reserved_capital,
            unfunded_commitments,
        )
    }

    /// Refuse a policy the plane could not act on, at start-up.
    ///
    /// Checked where the configuration is read rather than where a promotion is
    /// attempted, because a split that does not sum reaches the gate as a
    /// refusal of every promotion — indistinguishable, months later, from a
    /// book that is genuinely over-committed.
    ///
    /// A policy claiming nothing is refused for the opposite reason: it would
    /// refuse every promotion to a capital-holding rung with "no source has
    /// said which horizon this sits at", which reads as a control working and
    /// is a control with no subject.
    pub fn validate(&self, total: Decimal) -> Result<()> {
        if self.claims.is_empty() {
            return Err(Error::invalid(
                "the §23.4 horizon policy names no strategy, so every promotion to a rung that \
                 holds capital would be refused for want of a claim; state which horizon each \
                 strategy sits at, or state no policy at all",
            ));
        }
        // A zero liability here, not the book's: this runs before any cycle has
        // read the commitment book, and what is being checked is the split
        // against the total, which the liability does not enter.
        self.pools(total, Decimal::ZERO)?;
        // Builds the register the plane will build, so a source contradicting
        // itself stops the process rather than surfacing at the first
        // promotion.
        let mut register = HorizonRegister::new();
        for claim in &self.claims {
            register.claim(
                &numeric(&claim.strategy),
                HorizonSource::new(claim.source.clone())?,
                claim.horizon,
            )?;
        }
        if self.despite.as_ref().is_some_and(|r| r.trim().is_empty()) {
            return Err(Error::invalid(
                "the §23.4 horizon policy records a blank reason for deciding over a \
                 disagreement; write down why the disagreement is being decided over rather \
                 than repaired, or remove the field",
            ));
        }
        Ok(())
    }
}

/// What one arming of the horizon gate found, for the cycle's journal.
///
/// The standings are the point: an operator reading the entry can see which
/// bucket had how much charged against it at the moment the gate was armed,
/// which is what a refusal months later has to be reconcilable against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonArming {
    /// Strategies the allocator sized, and whose sizing became a claim on a
    /// pool.
    pub strategies_budgeted: usize,
    /// The sum of those budgets.
    pub budgeted: Decimal,
    /// The unfunded commitment liability the years pool was charged, as the
    /// commitment book held it at this instant.
    pub liability: Decimal,
    /// Every bucket's pool and commitment. Empty when the register is disputed
    /// and no desk has recorded a decision, in which case `unsettled` says so.
    pub standings: Vec<HorizonStanding>,
    /// Why no standing could be computed, when none could. The gate is armed
    /// either way — a disputed register refuses at the gate, and leaving it
    /// unarmed because it could not be *described* would be failing open at
    /// exactly the moment the platform's own sources disagree.
    pub unsettled: Option<String>,
    /// Proposals the allocator gave nothing, with its reason. These carry no
    /// budget, so a promotion of one is refused by the gate for want of a
    /// stated claim rather than admitted as a claim of zero.
    pub unbudgeted: Vec<String>,
}

impl HorizonArming {
    /// Whether any bucket is already over its pool before a promotion is even
    /// attempted.
    pub fn is_breached(&self) -> bool {
        self.standings.iter().any(HorizonStanding::is_breached)
    }

    /// The line an operator reads in the stage's account of itself.
    pub fn describe(&self) -> String {
        let buckets = self
            .standings
            .iter()
            .map(|standing| {
                format!(
                    "{}={}/{}",
                    standing.bucket, standing.committed, standing.pool
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut line = format!(
            "the §23.4 horizon gate is armed on {} strategy budget(s) totalling {} against an \
             unfunded commitment liability of {}",
            self.strategies_budgeted, self.budgeted, self.liability
        );
        if !buckets.is_empty() {
            line.push_str(&format!(" (committed/pool {buckets})"));
        }
        if let Some(unsettled) = &self.unsettled {
            line.push_str(&format!("; no standing could be computed: {unsettled}"));
        }
        if !self.unbudgeted.is_empty() {
            line.push_str(&format!(
                "; {} proposal(s) the allocator sized at nothing carry no claim",
                self.unbudgeted.len()
            ));
        }
        line
    }
}

/// The horizon claims, the pools and the capital each strategy is asking for.
///
/// Everything in it is stated by whoever composes it; nothing here infers a
/// pool split or a horizon, because the platform measures neither and asserting
/// a strategy attribute nobody measured is how a backtest becomes a story.
#[derive(Clone, Debug, Default)]
pub struct PoolReconciler {
    register: HorizonRegister,
    pools: Option<CapitalPools>,
    budget_of: BTreeMap<StrategyId, Decimal>,
    despite: Option<String>,
}

impl PoolReconciler {
    /// Start from the four pools. Claims and budgets are added afterwards.
    pub fn new(pools: CapitalPools) -> Self {
        Self {
            register: HorizonRegister::new(),
            pools: Some(pools),
            budget_of: BTreeMap::new(),
            despite: None,
        }
    }

    /// Build from the desk's stated policy, a stated total and a **measured**
    /// liability.
    ///
    /// The budgets are deliberately not here. They are the allocator's, added
    /// by [`super::CentralPlane::arm_horizons`] from the plan it computed this
    /// cycle, so that what the gate reconciles is what the platform would
    /// actually size rather than a figure somebody typed beside the split.
    pub fn from_policy(
        policy: &HorizonPolicy,
        total: Decimal,
        unfunded_commitments: Decimal,
    ) -> Result<Self> {
        let mut reconciler = Self::new(policy.pools(total, unfunded_commitments)?);
        for claim in &policy.claims {
            reconciler.claim(&claim.strategy, claim.source.clone(), claim.horizon)?;
        }
        if let Some(reason) = &policy.despite {
            reconciler = reconciler.deciding_despite(reason.clone());
        }
        Ok(reconciler)
    }

    /// Every bucket's pool and commitment over everything budgeted, with no
    /// candidate.
    ///
    /// The book as it stands, for the cycle journal. It runs the same
    /// arithmetic through the same [`reconcile`] call as [`Self::fund`] does,
    /// rather than summing the budgets a second way: two derivations of one
    /// figure disagree, and the one in the journal would be the one nobody
    /// checked.
    pub fn standing(&self) -> Result<Vec<HorizonStanding>> {
        let settled = self.settled()?;
        let subjects: BTreeSet<StrategyId> = self.budget_of.keys().cloned().collect();
        standings_of(&self.reconcile_over(&settled, &subjects)?)
    }

    /// Charge every subject's budget to its settled bucket and reconcile.
    ///
    /// Refuses — rather than assuming — a subject with no claimed bucket or no
    /// stated budget. A claim of zero and an unknown claim are not the same
    /// thing, and treating the second as the first lets a strategy hold capital
    /// no pool was ever charged for.
    fn reconcile_over(
        &self,
        settled: &SettledHorizons,
        subjects: &BTreeSet<StrategyId>,
    ) -> Result<HorizonReconciliation> {
        let pools = self.pools()?;
        let mut budgets: Vec<FamilyBudget> = Vec::new();
        for (index, strategy) in subjects.iter().enumerate() {
            let horizon = settled.horizon_of(&numeric(strategy)).ok_or_else(|| {
                Error::invalid(format!(
                    "no source has said which horizon {strategy} sits at, and it is about to draw \
                     on a capital pool; record a claim naming its source before promoting it"
                ))
            })?;
            let money = self.budget_of.get(strategy).copied().ok_or_else(|| {
                Error::invalid(format!(
                    "{strategy} draws on the {horizon} pool with no stated budget; state the \
                     capital it is asking for, because a claim of zero and an unknown claim are \
                     not the same thing"
                ))
            })?;
            // A family of one per strategy. Arithmetically identical for a pool
            // sum, and deliberately *not* a correlation family: these
            // `FamilyId`s are positional indices over the subject set and must
            // never be read as the families `FamilyClustering` produces.
            // Reconciling on the family axis needs a `FamilyAssignment`, which
            // has no constructor but the clustering, and no correlation matrix
            // is in hand at a promotion.
            budgets.push(FamilyBudget::from_money(
                FamilyId::new(index),
                horizon,
                money,
            )?);
        }
        reconcile(pools, &budgets)
    }

    /// Record one source's claim about where a strategy sits.
    ///
    /// Refuses a source that contradicts itself; two *different* sources
    /// disagreeing is recorded rather than refused, because that is the fact
    /// the register exists to surface.
    pub fn claim(
        &mut self,
        strategy: &StrategyId,
        source: impl Into<String>,
        horizon: Horizon,
    ) -> Result<()> {
        let source = HorizonSource::new(source)?;
        self.register.claim(&numeric(strategy), source, horizon)
    }

    /// State the capital a strategy is asking for. Refuses a negative figure
    /// rather than netting it against another strategy's claim: a negative
    /// budget nets a real breach away and the reconciliation then reports
    /// balanced.
    pub fn budget(&mut self, strategy: &StrategyId, money: Decimal) -> Result<()> {
        if money.is_negative() {
            return Err(Error::invalid(format!(
                "{strategy} is budgeted {money}; a negative capital claim is a short position, \
                 which is sized as an exposure and not as a claim on a capital pool"
            )));
        }
        self.budget_of.insert(strategy.clone(), money);
        Ok(())
    }

    /// Record the reason a desk is deciding over an unresolved disagreement.
    ///
    /// Without this, a disputed register is reported as disputed and the gate
    /// refuses. With it, the settlement takes the least liquid claim — the only
    /// defensible reading, since funding a position against a *more* liquid
    /// pool than it deserves is the failure §23.4 exists to prevent — and the
    /// reason and the overruled claims travel through to the ledger entry.
    pub fn deciding_despite(mut self, reason: impl Into<String>) -> Self {
        self.despite = Some(reason.into());
        self
    }

    /// The pools, or a refusal naming what was never supplied.
    ///
    /// `Option` internally because [`Default`] exists for the builder's
    /// convenience and [`CapitalPools`] has no defensible default — a zero
    /// total is refused by its own constructor, and a fabricated one would let
    /// a promotion be reconciled against capital nobody has.
    pub fn pools(&self) -> Result<&CapitalPools> {
        self.pools.as_ref().ok_or_else(|| {
            Error::invalid(
                "no capital pools were given to the horizon reconciler, so there is nothing to \
                 reconcile against; build it with `PoolReconciler::new`",
            )
        })
    }

    /// The settlement as it stands, disagreements and all.
    ///
    /// Public so an operator surface can show the argument before a promotion
    /// is attempted rather than only in the refusal that stops one.
    pub fn settled(&self) -> Result<SettledHorizons> {
        match self.register.settle() {
            Ok(settled) => Ok(settled),
            Err(refusal) => match &self.despite {
                Some(reason) => self.register.settle_despite(reason),
                None => Err(refusal),
            },
        }
    }
}

impl HorizonReconciler for PoolReconciler {
    fn fund(
        &self,
        candidate: &StrategyId,
        alongside: &BTreeSet<StrategyId>,
    ) -> Result<HorizonFunding> {
        // Reported rather than raised. The gate refuses on this, and it can
        // only do so if it is handed the argument instead of an error that has
        // already made the decision.
        let disputes = self.register.disputes();
        let settled = match (disputes.is_empty(), &self.despite) {
            (true, _) => self.register.settle()?,
            (false, Some(reason)) => self.register.settle_despite(reason)?,
            (false, None) => {
                return Ok(HorizonFunding {
                    bucket: None,
                    standings: Vec::new(),
                    disagreements: translate_disputes(&disputes)?,
                    despite: None,
                });
            }
        };

        // The candidate plus everything already drawing on a pool. A promotion
        // reconciled against the candidate alone would pass every time: the
        // pool it is charged to is only ever breached by the sum.
        let mut subjects: BTreeSet<StrategyId> = alongside.clone();
        subjects.insert(candidate.clone());

        let reconciliation = self.reconcile_over(&settled, &subjects)?;
        let horizon = settled.horizon_of(&numeric(candidate)).ok_or_else(|| {
            Error::invalid(format!(
                "{candidate} was not among the strategies reconciled; this is a defect in the \
                 seam rather than in the submission"
            ))
        })?;
        let standings = standings_of(&reconciliation)?;

        Ok(HorizonFunding {
            bucket: Some(HorizonBucket::new(horizon.as_str())?),
            standings,
            disagreements: translate_disputes(settled.disputes())?,
            despite: settled.despite().map(str::to_string),
        })
    }
}

/// Every bucket the reconciliation reported, in the §23.4 table's order.
///
/// Every bucket, not only the candidate's and not only the breached ones. The
/// gate reads `committed > pool` itself, so handing it the breaches alone would
/// move the decision back to this side.
fn standings_of(reconciliation: &HorizonReconciliation) -> Result<Vec<HorizonStanding>> {
    reconciliation
        .positions()
        .values()
        .map(|position| {
            Ok(HorizonStanding {
                bucket: HorizonBucket::new(position.horizon.as_str())?,
                treatment: position.horizon.treatment().to_string(),
                pool: position.pool,
                committed: position.committed,
            })
        })
        .collect()
}

/// Carry the optimiser's disputes across the seam.
///
/// Both sides of every argument travel, not only the claim that won: a record
/// keeping only the winner cannot be told apart afterwards from one where
/// nobody disagreed.
fn translate_disputes(disputes: &[HorizonDispute]) -> Result<Vec<HorizonDisagreement>> {
    disputes
        .iter()
        .map(|dispute| {
            let mut claims: BTreeMap<HorizonBucket, BTreeSet<String>> = BTreeMap::new();
            for (horizon, sources) in dispute.claims() {
                claims.insert(
                    HorizonBucket::new(horizon.as_str())?,
                    sources.iter().map(|s| s.as_str().to_string()).collect(),
                );
            }
            Ok(HorizonDisagreement {
                strategy: StrategyId::new(dispute.strategy().as_str()),
                claims,
            })
        })
        .collect()
}

/// Bridge the core crate's `StrategyId` to the contract crate's and back.
///
/// The two are newtypes over the same string in two namespaces, and
/// `super::structure` bridges them the same way at the same kind of seam.
/// Lossless, and explicit rather than hidden behind a `From`, so the crossing
/// is visible where it happens.
fn numeric(strategy: &StrategyId) -> NumericStrategyId {
    NumericStrategyId::from_string(strategy.as_str())
}

/// The reconciler a failed arming leaves in place: it refuses everything and
/// says why.
///
/// # Why this type exists rather than a detach
///
/// `LifecycleLedger` holds its assurance as an `Option`, and `None` is the
/// **ungated** state — the legitimate one a plane with no stated
/// [`HorizonPolicy`] is in. So detaching on error would not refuse promotions,
/// it would admit every one of them, which is the opposite of what a failure
/// to measure the pools should mean.
///
/// Leaving the previous cycle's assurance attached is no better and was the
/// original defect: [`super::plane::CentralPlane::arm_horizons`] attached only
/// on its success path, so a cycle whose `CommitmentBook::unfunded_total`
/// began returning an error went on reconciling promotions against pool bounds
/// computed when the unfunded liability was some other number. The gate stayed
/// green by being out of date, which is the failure mode a stale control has
/// and a refusing one does not. Found by an independent security review of
/// `27da0c5`, before the gate had a production producer.
///
/// So the third option, and the one CLAUDE.md's third principle asks for:
/// every promotion to a capital-holding rung is refused until an arming
/// succeeds. Refusing a promotion is recoverable — the next cycle arms and the
/// promotion proceeds. Admitting one against a liability nobody measured this
/// cycle is not.
#[derive(Debug)]
pub struct UnarmedHorizons {
    reason: String,
}

impl UnarmedHorizons {
    /// Refuse every promotion, naming `reason` — the arming failure — in each
    /// refusal, so an operator reads why the gate is closed rather than
    /// discovering that it is.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl HorizonReconciler for UnarmedHorizons {
    fn fund(
        &self,
        candidate: &StrategyId,
        _alongside: &BTreeSet<StrategyId>,
    ) -> Result<HorizonFunding> {
        Err(Error::denied(format!(
            "{candidate} cannot take a rung that holds capital: the §23.4 pool gate could not be \
             armed this cycle, so no pool bound has been measured against today's liability. The \
             arming failed because: {}. This refusal clears itself as soon as one cycle arms the \
             gate; it is not a judgement about {candidate}",
            self.reason
        )))
    }
}
