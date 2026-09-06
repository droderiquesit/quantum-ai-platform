//! Which capital pool funds a rung, and what happens when the platform's own
//! sources disagree about it.
//!
//! Blueprint §23.4 splits capital four ways — inventory recycled continuously,
//! deployable capital, capital not reserved for calls, and reserved capital
//! carrying the unfunded commitment liability — so that a multi-year position
//! is never funded out of the inventory a market maker needs by the close.
//!
//! # What is here and what is deliberately not
//!
//! The arithmetic that holds a desk to §23.4 — four pools summing to one total
//! exactly once, the unfunded commitment liability charged to the reserved
//! pool whether or not anybody budgeted against it, and the settlement of a
//! multi-source horizon register — lives in `qip_optimization_engine::horizons`
//! and is **not** restated here. Neither is it *called* from here, and that is
//! the difference this module exists to hold.
//!
//! `qip-lifecycle` vetoes promotions to rungs that hold capital. The
//! optimisation engine reaches a quantum solver. A veto crate that can reach a
//! solver is a veto whose input is an optimiser's answer, which is the failure
//! `nothing_that_vetoes_executes_or_moves_money_can_reach_a_quantum_solver` in
//! the acceptance suite refuses — and it refuses it transitively, because the
//! route in is never a direct edge to the solver but a reasonable-looking edge
//! to the optimiser. That test caught exactly this crate taking exactly that
//! edge.
//!
//! So the pool arithmetic stays in one place and this crate reaches it through
//! [`HorizonReconciler`], a port whose adapter is composed in `qip-kernel` —
//! the only layer permitted to introduce two services to each other. Nothing
//! is re-implemented; what changed is which crate owns the types crossing the
//! seam.
//!
//! # The refusal, and why it is on this side of the port
//!
//! The reconciler *measures*: it reports each bucket's pool and what is charged
//! against it, which bucket the candidate settled at, and which sources are
//! still arguing. [`HorizonAssurance::admit`] *decides*, on that data:
//!
//! 1. **A standing disagreement refuses.** While two sources place the
//!    candidate at different buckets and no desk has recorded a reason for
//!    deciding over them, the promotion is refused and the refusal names both
//!    sides and who took them. Before a register existed, a strategy's horizon
//!    was one map entry, so the last writer won silently and the position was
//!    funded out of whichever pool that writer happened to name.
//! 2. **A decision taken anyway is recorded.** The stated reason and every
//!    overruled claim travel into [`HorizonVerdict`] and from there onto the
//!    [`crate::LedgerEntry`], permanently. A promotion taken over an unresolved
//!    argument is otherwise indistinguishable afterwards from one taken on
//!    agreement, and a post-mortem that cannot tell them apart draws the wrong
//!    lesson from whichever one went wrong.
//! 3. **A breached pool refuses, and is not trimmed.** Any bucket whose
//!    commitment exceeds its pool stops the promotion. Trimming would be the
//!    platform quietly choosing which strategy goes unfunded at the moment the
//!    desk most needs to make that choice.
//!
//! Note which way round steps 1 and 3 are. The gate does not ask the reconciler
//! whether the promotion is allowed and relay the answer; it asks for the
//! figures and refuses on them itself. An adapter that computed a breach and
//! reported "fine" would still be refused here, because `committed > pool` is
//! read on this side. A control whose whole content is forwarding somebody
//! else's verdict is the `MaxExpectedShortfall` defect this repository names:
//! it reads as protection and is not.
//!
//! # The honest limit
//!
//! The assurance is **optional**. A [`crate::LifecycleLedger`] with none
//! attached promotes exactly as it did before this module existed, and no
//! composition root attaches one yet, so in a deployment the gate is wired and
//! unfed. That is a real gap and it is not dressed up as anything else.

use crate::ledger::LifecycleLedger;
use qip_contracts::signal::StrategyId;
use qip_core::decimal::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The name of one §23.4 horizon bucket, as the reconciler named it.
///
/// A name rather than a copy of the optimisation engine's four-arm enum, on
/// purpose. This crate never orders buckets, never asks which is less liquid
/// and never maps one to a pool — settlement and allocation are the
/// reconciler's — so an enum here would buy no guarantee and would be a second
/// statement of the §23.4 table, free to drift out of step with the first the
/// day a fifth bucket is added. What it carries through, it carries verbatim.
///
/// Blank names are refused rather than accepted and rendered as nothing: a
/// refusal that says a promotion breached the `` pool names no pool.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct HorizonBucket(String);

impl HorizonBucket {
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a horizon bucket with no name cannot be reported in a refusal or matched \
                 against a standing; name the §23.4 bucket the capital is charged to",
            ));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HorizonBucket {
    type Error = Error;

    fn try_from(name: String) -> Result<Self> {
        Self::new(name)
    }
}

impl std::fmt::Display for HorizonBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One bucket's pool and what is charged against it, once the candidate is
/// included.
///
/// `committed` is the whole charge, including any liability nobody budgeted
/// for — at the years horizon the reserved pool must meet the unfunded
/// commitments as well as the positions, and a commitment nobody allocated
/// against is exactly the one that surprises a desk when it is called.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonStanding {
    pub bucket: HorizonBucket,
    /// How the blueprint says this bucket is funded, so a refusal can say
    /// which pool was meant and not only which one was short.
    pub treatment: String,
    pub pool: Decimal,
    pub committed: Decimal,
}

impl HorizonStanding {
    /// Pool less commitment.
    ///
    /// `Result` rather than a saturating floor: a subtraction of two
    /// `Decimal`s that overflows means the figures reaching this gate are not
    /// the figures anybody intended, and a headroom of `Decimal::MIN` recorded
    /// on a ledger entry would read afterwards as a measurement.
    pub fn headroom(&self) -> Result<Decimal> {
        self.pool.checked_sub(self.committed).ok_or_else(|| {
            Error::numeric(format!(
                "the {} pool of {} less its commitment of {} overflows; check the units the \
                 reconciler was given",
                self.bucket, self.pool, self.committed
            ))
        })
    }

    /// Whether more is charged to this bucket than its pool holds.
    pub fn is_breached(&self) -> bool {
        self.committed > self.pool
    }
}

/// Two or more sources placing one strategy at different buckets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonDisagreement {
    pub strategy: StrategyId,
    /// Bucket to the sources that claimed it. A `BTreeMap` of `BTreeSet`s
    /// because this reaches a refusal message and a ledger entry, and a replay
    /// that reorders the sides of an argument is not a replay.
    pub claims: BTreeMap<HorizonBucket, BTreeSet<String>>,
}

impl HorizonDisagreement {
    /// The argument as one line a reviewer can read.
    pub fn narrate(&self) -> String {
        let sides: Vec<String> = self
            .claims
            .iter()
            .map(|(bucket, sources)| {
                format!(
                    "{bucket} claimed by {}",
                    sources.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        format!("{}: {}", self.strategy, sides.join(" against "))
    }
}

/// What the reconciler measured, for the gate to decide on.
///
/// Every field is a measurement, not a verdict. There is deliberately no
/// "allowed" flag: see the module note on why the refusal is taken on this
/// side of the port.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonFunding {
    /// The bucket the candidate settled at, or `None` while nothing has
    /// settled it. `None` refuses — an unknown bucket and a bucket of zero
    /// claim are not the same thing.
    pub bucket: Option<HorizonBucket>,
    /// Every bucket's standing once the candidate's budget is included.
    pub standings: Vec<HorizonStanding>,
    /// Disagreements standing at the moment of the measurement. Empty when the
    /// sources agreed.
    pub disagreements: Vec<HorizonDisagreement>,
    /// The stated reason a desk decided over those disagreements, if one was
    /// recorded.
    pub despite: Option<String>,
}

/// The port through which this crate reaches the §23.4 pool arithmetic.
///
/// Implemented in `qip-kernel` over `qip_optimization_engine::horizons`. It is
/// a port rather than a direct call because that crate reaches a quantum
/// solver and this one holds a veto over capital; see the module note.
///
/// `Send + Sync` because a [`LifecycleLedger`] is shared, and `Debug` because
/// the ledger is `Debug` and a field that erases its own type from a dump is a
/// field nobody can diagnose from a log.
pub trait HorizonReconciler: std::fmt::Debug + Send + Sync {
    /// Measure `candidate`'s funding alongside everything already drawing on a
    /// pool.
    ///
    /// `alongside` matters as much as `candidate`: a promotion reconciled
    /// against the candidate alone passes every time, because a pool is only
    /// ever breached by the sum. Refuses — rather than assuming — when a
    /// subject has no claimed bucket or no stated budget.
    fn fund(
        &self,
        candidate: &StrategyId,
        alongside: &BTreeSet<StrategyId>,
    ) -> Result<HorizonFunding>;
}

/// What the horizon gate found, kept on the ledger entry it admitted.
///
/// `disputes` and `despite` are the half that matters after the fact. A
/// promotion decided over an unresolved disagreement is indistinguishable in
/// the record from one decided on agreement unless the record says so, and a
/// post-mortem that cannot tell them apart will read the wrong lesson out of
/// whichever one went wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizonVerdict {
    /// The bucket the candidate was funded at.
    pub horizon: HorizonBucket,
    /// How the blueprint funds that bucket.
    pub treatment: String,
    /// Pool less commitment at that bucket once the candidate is included.
    /// Never negative: a negative headroom is a breach and a breach refuses.
    pub headroom: Decimal,
    /// Every disagreement standing at the moment of the decision, with the
    /// sides that made it. Empty when the sources agreed.
    pub disputes: Vec<HorizonDisagreement>,
    /// The stated reason the desk decided over those disagreements. `None`
    /// when there were none to decide over.
    pub despite: Option<String>,
}

impl HorizonVerdict {
    /// Whether this promotion was taken over an unresolved disagreement.
    pub fn decided_over_disagreement(&self) -> bool {
        !self.disputes.is_empty()
    }

    /// The verdict as lines a reviewer can read.
    pub fn narrate(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "funded at the {} horizon ({}), leaving {} of headroom",
            self.horizon, self.treatment, self.headroom
        )];
        if let Some(reason) = &self.despite {
            lines.push(format!(
                "decided over {} unresolved horizon disagreement(s): {reason}",
                self.disputes.len()
            ));
        }
        lines.extend(self.disputes.iter().map(HorizonDisagreement::narrate));
        lines
    }
}

/// The gate a [`LifecycleLedger`] puts every capital-holding promotion through.
///
/// Hand one to a ledger and every promotion to a rung that holds capital is
/// measured by the reconciler and refused here on what it measured.
#[derive(Clone, Debug)]
pub struct HorizonAssurance {
    reconciler: Arc<dyn HorizonReconciler>,
}

impl HorizonAssurance {
    /// Take the pool arithmetic from `reconciler`.
    ///
    /// `Arc` rather than `Box` because [`LifecycleLedger`] is `Clone` and two
    /// clones of a ledger must not disagree about the pools: a second
    /// reconciler is a second claim on the same fact, and CLAUDE.md's sixth
    /// principle says which of the two would be wrong.
    pub fn new(reconciler: Arc<dyn HorizonReconciler>) -> Self {
        Self { reconciler }
    }

    /// Measure and refuse `candidate` alongside everything `ledger` funds.
    ///
    /// Called by [`LifecycleLedger::record_promotion`] when the rung being
    /// entered holds capital.
    pub fn admit(
        &self,
        ledger: &LifecycleLedger,
        candidate: &StrategyId,
    ) -> Result<HorizonVerdict> {
        // Everything already drawing on a pool, less the candidate itself: a
        // strategy submitted twice is budgeted twice and one of the two claims
        // is not the record.
        let mut alongside: BTreeSet<StrategyId> = ledger
            .holding_capital()
            .into_iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        alongside.remove(candidate);

        let funding = self.reconciler.fund(candidate, &alongside)?;

        // Refusal one: nobody has settled the argument, and no desk has taken
        // responsibility for deciding over it.
        if !funding.disagreements.is_empty() && funding.despite.is_none() {
            let sides: Vec<String> = funding
                .disagreements
                .iter()
                .map(HorizonDisagreement::narrate)
                .collect();
            return Err(Error::denied(format!(
                "{candidate} cannot take a rung that holds capital while its horizon is \
                 disputed: {}. Repair the source that is wrong, or record the reason a desk is \
                 deciding over the disagreement",
                sides.join("; ")
            )));
        }

        let bucket = funding.bucket.clone().ok_or_else(|| {
            Error::invalid(format!(
                "no source has said which horizon {candidate} sits at, and it is about to draw \
                 on a capital pool; record a claim naming its source before promoting it"
            ))
        })?;

        // Refusal two: a bucket is over its pool. Every bucket is checked, not
        // only the candidate's — the candidate's own budget can be what pushes
        // a different bucket over once the liability is charged.
        let breaches: Vec<String> = funding
            .standings
            .iter()
            .filter(|standing| standing.is_breached())
            .map(|standing| {
                let over = standing
                    .committed
                    .checked_sub(standing.pool)
                    .unwrap_or(Decimal::ZERO);
                format!(
                    "{} is over its pool of {} by {} ({})",
                    standing.bucket, standing.pool, over, standing.treatment
                )
            })
            .collect();
        if !breaches.is_empty() {
            return Err(Error::denied(format!(
                "{candidate} cannot take a rung that holds capital: the allocation is not \
                 reconciled: {}. Lower the budgets at that horizon or move capital into its \
                 pool — this gate will not trim them for you",
                breaches.join("; ")
            )));
        }

        let standing = funding
            .standings
            .iter()
            .find(|standing| standing.bucket == bucket)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "the reconciliation reported no standing at the {bucket} horizon, which is \
                     the one {candidate} settled at; every bucket is reported whether or not \
                     anything is budgeted at it"
                ))
            })?;

        Ok(HorizonVerdict {
            horizon: bucket,
            treatment: standing.treatment.clone(),
            headroom: standing.headroom()?,
            disputes: funding.disagreements,
            despite: funding.despite,
        })
    }
}
