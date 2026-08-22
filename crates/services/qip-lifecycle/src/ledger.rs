//! The record of how every strategy got where it is.
//!
//! The ledger's job is reconstruction. A strategy holding capital today should
//! be traceable back to the candidate it was, through every rung, with the
//! evidence each gate saw and the names of the people who signed. That is what
//! makes a post-mortem possible: not "the model degraded" but "the model was
//! promoted on a holdout of 260 observations after 4,000 trials, and here is
//! the outcome the gate recorded at the time".
//!
//! The asymmetry between the two directions is the design:
//!
//! * **Up** goes through [`AuthorisedPromotion`], whose only constructor
//!   refuses a capital-holding rung without a dual [`Approval`] and computes
//!   the target rung from [`GateStage::next`] rather than accepting one. A
//!   caller cannot skip a rung or forget an approval, because it cannot
//!   construct the value that would let it.
//! * **Down** goes through [`LifecycleLedger::demote`], which takes no
//!   approver, no credential and no evidence. Anyone or anything may push a
//!   strategy down. This mirrors the kill switch in
//!   `qip_risk_engine::autonomy`, and for the same reason: a false demotion
//!   costs a day of missed opportunity and a missed one costs the book.

use crate::evidence::StrategyEvidence;
use qip_contracts::gate::{GateOutcome, GateStage, Promotion};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A promotion that has already cleared its authority check.
///
/// Fields are private and there is one constructor, so the two rules that
/// matter cannot be forgotten at a call site: the target rung is
/// [`GateStage::next`] of the current one, and a rung that
/// [`GateStage::requires_human_approval`] carries a dual [`Approval`]. A
/// promotion to pilot without two names is not refused at runtime — it has no
/// representation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthorisedPromotion {
    from: GateStage,
    to: GateStage,
    approval: Option<Approval>,
    at: Timestamp,
}

impl AuthorisedPromotion {
    /// Advance one rung, refusing anything that would let capital move without
    /// two people having decided that it should.
    pub fn advance(from: GateStage, approval: Option<Approval>, at: Timestamp) -> Result<Self> {
        let to = from.next().ok_or_else(|| {
            Error::denied(format!(
                "{} is terminal; there is no rung above it",
                from.as_str()
            ))
        })?;

        if to.requires_human_approval() {
            let recorded = approval.as_ref().ok_or_else(|| {
                Error::denied(format!(
                    "promotion to {} needs a recorded human approval",
                    to.as_str()
                ))
            })?;
            if !recorded.is_dual() {
                return Err(Error::denied(format!(
                    "promotion to {} needs two approvers; {} approved alone",
                    to.as_str(),
                    recorded.approver
                )));
            }
            // An approval dated after the promotion it authorises was written
            // to fit the record rather than to make the decision.
            if recorded.at > at {
                return Err(Error::invalid(format!(
                    "the approval is dated {} but the promotion is dated {}",
                    recorded.at.to_rfc3339(),
                    at.to_rfc3339()
                )));
            }
        }

        Ok(Self {
            from,
            to,
            approval,
            at,
        })
    }

    pub fn from(&self) -> GateStage {
        self.from
    }

    pub fn to(&self) -> GateStage {
        self.to
    }

    pub fn approval(&self) -> Option<&Approval> {
        self.approval.as_ref()
    }

    pub fn at(&self) -> Timestamp {
        self.at
    }
}

/// One recorded move, with whatever the gate saw at the time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub promotion: Promotion,
    /// The gate outcome that admitted the strategy. `None` for demotions,
    /// which pass no gate.
    pub outcome: Option<GateOutcome>,
    /// The approval, kept whole rather than reduced to a name, so a review can
    /// read the rationale the approvers gave at the time.
    pub approval: Option<Approval>,
}

impl LedgerEntry {
    pub fn is_escalation(&self) -> bool {
        self.promotion.is_escalation()
    }
}

/// Every move every strategy has made.
#[derive(Clone, Debug, Default)]
pub struct LifecycleLedger {
    entries: BTreeMap<StrategyId, Vec<LedgerEntry>>,
}

impl LifecycleLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where a strategy stands. An unknown strategy is a candidate: there is
    /// no registration step, because a strategy nobody has promoted has
    /// exactly the permissions of one that was just proposed.
    pub fn stage_of(&self, strategy: &StrategyId) -> GateStage {
        self.entries
            .get(strategy)
            .and_then(|entries| entries.last())
            .map_or(GateStage::Candidate, |entry| entry.promotion.to)
    }

    pub fn history(&self, strategy: &StrategyId) -> &[LedgerEntry] {
        self.entries.get(strategy).map_or(&[], Vec::as_slice)
    }

    pub fn strategies(&self) -> impl Iterator<Item = &StrategyId> {
        self.entries.keys()
    }

    /// The rungs a strategy has stood on, in order, starting at candidate.
    ///
    /// This is the reconstruction the ledger exists for. A path that reaches
    /// pilot without passing through shadow is a bug in this crate, and the
    /// test suite asserts it cannot happen.
    pub fn path(&self, strategy: &StrategyId) -> Vec<GateStage> {
        let mut path = vec![GateStage::Candidate];
        for entry in self.history(strategy) {
            path.push(entry.promotion.to);
        }
        path
    }

    /// Whether a strategy has ever stood at a rung.
    pub fn reached(&self, strategy: &StrategyId, stage: GateStage) -> bool {
        self.path(strategy).contains(&stage)
    }

    /// The gate outcome that admitted a strategy to a rung, most recent first.
    pub fn admission_evidence(
        &self,
        strategy: &StrategyId,
        stage: GateStage,
    ) -> Option<&GateOutcome> {
        self.history(strategy)
            .iter()
            .rev()
            .filter(|entry| entry.promotion.to == stage)
            .find_map(|entry| entry.outcome.as_ref())
    }

    /// Record a promotion that a gate has passed.
    ///
    /// Four things are checked here that [`AuthorisedPromotion`] cannot check
    /// on its own, because they are properties of this ledger rather than of
    /// the promotion: the strategy is where the promotion says it is, it has
    /// not been retired, the outcome belongs to the rung being entered, and
    /// the outcome passed.
    pub fn record_promotion(
        &mut self,
        strategy: &StrategyId,
        promotion: AuthorisedPromotion,
        outcome: GateOutcome,
        rationale: impl Into<String>,
    ) -> Result<Promotion> {
        let current = self.stage_of(strategy);
        if current == GateStage::Retired {
            return Err(Error::denied(format!(
                "{strategy} is retired; a retired strategy is re-proposed as a new candidate \
                 under a new identity rather than resurrected"
            )));
        }
        if current != promotion.from() {
            return Err(Error::denied(format!(
                "{strategy} is at {} but the promotion starts from {}",
                current.as_str(),
                promotion.from().as_str()
            )));
        }
        if outcome.stage != promotion.to() {
            return Err(Error::invalid(format!(
                "the {} gate's outcome cannot admit a strategy to {}",
                outcome.stage.as_str(),
                promotion.to().as_str()
            )));
        }
        if !outcome.passed {
            let failures: Vec<String> = outcome
                .failures()
                .iter()
                .map(|(name, _, detail)| format!("{name}: {detail}"))
                .collect();
            return Err(Error::guard(format!(
                "{strategy} did not pass the {} gate: {}",
                promotion.to().as_str(),
                failures.join("; ")
            )));
        }

        let record = Promotion {
            from: promotion.from(),
            to: promotion.to(),
            at: promotion.at(),
            approver: promotion.approval().map(|a| a.approver.clone()),
            rationale: rationale.into(),
            evidence: outcome
                .findings
                .iter()
                .map(|(name, passed, detail)| {
                    format!(
                        "{name}={} ({detail})",
                        if *passed { "pass" } else { "fail" }
                    )
                })
                .collect(),
        };
        self.entries
            .entry(strategy.clone())
            .or_default()
            .push(LedgerEntry {
                promotion: record.clone(),
                outcome: Some(outcome),
                approval: promotion.approval().cloned(),
            });
        Ok(record)
    }

    /// Push a strategy down, or retire it.
    ///
    /// Takes no approver and no credential, deliberately. `raised_by` is
    /// recorded and never checked — an unattributed demotion is still
    /// honoured, because requiring attribution would mean a component that
    /// cannot name itself cannot stop a strategy, and the components most
    /// likely to notice trouble are exactly the automatic ones.
    ///
    /// The only refusals are structural: a retired strategy has nowhere lower
    /// to go, and a move that is not downward is not a demotion.
    pub fn demote(
        &mut self,
        strategy: &StrategyId,
        to: GateStage,
        raised_by: impl Into<String>,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<Promotion> {
        let current = self.stage_of(strategy);
        if current == GateStage::Retired {
            return Err(Error::denied(format!("{strategy} is already retired")));
        }
        if to != GateStage::Retired && to >= current {
            return Err(Error::invalid(format!(
                "{} is not below {}; use a promotion to move a strategy up",
                to.as_str(),
                current.as_str()
            )));
        }

        let raised_by = raised_by.into();
        let raised_by = if raised_by.trim().is_empty() {
            "unattributed".to_string()
        } else {
            raised_by
        };
        let record = Promotion {
            from: current,
            to,
            at,
            // No approver, ever. The `None` here is the asymmetry made
            // material in the record: every escalation names someone, every
            // demotion names nobody, and both are correct.
            approver: None,
            rationale: format!("{}: {}", raised_by, reason.into()),
            evidence: Vec::new(),
        };
        self.entries
            .entry(strategy.clone())
            .or_default()
            .push(LedgerEntry {
                promotion: record.clone(),
                outcome: None,
                approval: None,
            });
        Ok(record)
    }

    /// Withdraw a strategy for good.
    ///
    /// Terminal. [`GateStage::next`] returns `None` from retired, so no
    /// promotion can leave it, and [`Self::demote`] refuses to move it
    /// further. Bringing the idea back means a new [`StrategyId`] and a fresh
    /// walk from candidate, so its evidence is re-earned rather than inherited
    /// from whatever state it was in when it was withdrawn.
    pub fn retire(
        &mut self,
        strategy: &StrategyId,
        raised_by: impl Into<String>,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<Promotion> {
        self.demote(strategy, GateStage::Retired, raised_by, reason, at)
    }

    /// Strategies currently holding capital.
    pub fn holding_capital(&self) -> Vec<&StrategyId> {
        self.entries
            .keys()
            .filter(|strategy| self.stage_of(strategy).holds_capital())
            .collect()
    }

    /// Reconstruct a strategy's walk as lines a reviewer can read.
    pub fn narrate(&self, strategy: &StrategyId) -> Vec<String> {
        self.history(strategy)
            .iter()
            .map(|entry| {
                let approver = entry
                    .promotion
                    .approver
                    .as_deref()
                    .unwrap_or("no approver (demotion needs none)");
                format!(
                    "{} {} -> {} by {} : {}",
                    entry.promotion.at.to_rfc3339(),
                    entry.promotion.from.as_str(),
                    entry.promotion.to.as_str(),
                    approver,
                    entry.promotion.rationale
                )
            })
            .collect()
    }
}

/// Run the gate for the rung above `from` and record the result.
///
/// The ordinary path: it is a free function rather than a method so the
/// ledger does not have to own a set of gates, and so a caller that wants a
/// non-default policy can build its own gate and call
/// [`LifecycleLedger::record_promotion`] directly.
pub fn attempt_promotion(
    ledger: &mut LifecycleLedger,
    strategy: &StrategyId,
    evidence: &StrategyEvidence,
    approval: Option<Approval>,
    rationale: impl Into<String>,
    now: Timestamp,
) -> Result<Promotion> {
    let from = ledger.stage_of(strategy);
    let promotion = AuthorisedPromotion::advance(from, approval, now)?;
    let gate = crate::gates::gate_for(promotion.to()).ok_or_else(|| {
        Error::invalid(format!(
            "there is no gate admitting a strategy to {}",
            promotion.to().as_str()
        ))
    })?;
    let outcome = gate.evaluate(evidence, now);
    ledger.record_promotion(strategy, promotion, outcome, rationale)
}
