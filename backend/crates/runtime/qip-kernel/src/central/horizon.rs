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
//! Nothing constructs a [`PoolReconciler`] in a composition root yet, which is
//! the same gap `qip_lifecycle::horizon` states about the gate it feeds. The
//! seam being in the right layer does not make it wired.

use qip_contracts::signal::StrategyId;
use qip_core::decimal::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::StrategyId as NumericStrategyId;
use qip_lifecycle::horizon::{
    HorizonBucket, HorizonDisagreement, HorizonFunding, HorizonReconciler, HorizonStanding,
};
use qip_optimization_engine::families::FamilyId;
use qip_optimization_engine::horizons::{
    CapitalPools, FamilyBudget, Horizon, HorizonDispute, HorizonRegister, HorizonSource,
    SettledHorizons, reconcile,
};
use std::collections::{BTreeMap, BTreeSet};

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

        let pools = self.pools()?;

        // The candidate plus everything already drawing on a pool. A promotion
        // reconciled against the candidate alone would pass every time: the
        // pool it is charged to is only ever breached by the sum.
        let mut subjects: BTreeSet<StrategyId> = alongside.clone();
        subjects.insert(candidate.clone());

        let mut budgets: Vec<FamilyBudget> = Vec::new();
        let mut candidate_horizon: Option<Horizon> = None;
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
            if strategy == candidate {
                candidate_horizon = Some(horizon);
            }
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

        let horizon = candidate_horizon.ok_or_else(|| {
            Error::invalid(format!(
                "{candidate} was not among the strategies reconciled; this is a defect in the \
                 seam rather than in the submission"
            ))
        })?;

        let reconciliation = reconcile(pools, &budgets)?;
        // Every bucket, not only the candidate's and not only the breached
        // ones. The gate reads `committed > pool` itself, so handing it the
        // breaches alone would move the decision back to this side.
        let standings = reconciliation
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
            .collect::<Result<Vec<_>>>()?;

        Ok(HorizonFunding {
            bucket: Some(HorizonBucket::new(horizon.as_str())?),
            standings,
            disagreements: translate_disputes(settled.disputes())?,
            despite: settled.despite().map(str::to_string),
        })
    }
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
