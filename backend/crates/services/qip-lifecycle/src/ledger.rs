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

use crate::band::{BandVerdict, HoldoutBand};
use crate::corridor::{CorridorPolicy, CorridorSubject};
use crate::evidence::StrategyEvidence;
use crate::gates::Admission;
use crate::horizon::{HorizonAssurance, HorizonVerdict};
use crate::trials::TrialBook;
use qip_contracts::gate::{GateOutcome, GateStage, Promotion};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_observability::metrics::{Metrics, labels, names};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

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
    /// The band the holdout validation defined. Present on every holdout
    /// admission and nothing else; the interval the strategy's live
    /// performance is held inside of, from the evidence that admitted it.
    #[serde(default)]
    pub band: Option<HoldoutBand>,
    /// Which capital pool funded this rung, and what the platform's own
    /// sources disagreed about at the moment it was taken. Present on every
    /// admission to a rung that holds capital where the ledger carries a
    /// [`HorizonAssurance`], and on nothing else.
    ///
    /// `serde(default)` so a record written before this field existed still
    /// loads. A promotion taken over an unresolved horizon disagreement is
    /// indistinguishable afterwards from one taken on agreement unless the
    /// record says which it was, and a post-mortem that cannot tell them apart
    /// draws the wrong lesson from whichever went wrong.
    #[serde(default)]
    pub horizon: Option<HorizonVerdict>,
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
    /// Where each move is counted, if whoever composed the ledger gave it a
    /// registry. Handed in rather than constructed, because a ledger that
    /// made its own registry would count into a series nothing scrapes; and
    /// optional rather than required, because the ledger's job is the record
    /// and a missing registry must not stop a demotion.
    metrics: Option<Arc<Metrics>>,
    /// Where every holdout evaluation is charged, per family, for life.
    /// Optional for the same reason as the registry — a ledger is built
    /// before its composition root can hand anything in — but with the
    /// opposite consequence: a missing registry loses a count, a missing
    /// book refuses every promotion to holdout, because without it the
    /// lifetime trial count is unknown and unknown is not zero.
    trials: Option<TrialBook>,
    /// Which capital pool funds each rung that holds capital, and the
    /// disagreements between the platform's own sources about it. Optional for
    /// the same reason as the trial book — a ledger exists before its
    /// composition root can hand anything in — and with the trial book's
    /// consequence rather than the registry's, but scoped: a ledger with none
    /// promotes exactly as it did before this field existed, and one that has
    /// been given an assurance refuses every promotion to a capital-holding
    /// rung the assurance will not admit. `qip-kernel`'s
    /// `CentralPlane::arm_horizons` attaches one each cycle from the LEARN
    /// stage; see `crate::horizon` for the two stated inputs no deployment
    /// supplies yet.
    horizons: Option<HorizonAssurance>,
    /// The corridors this desk has declared, and the policy last emitted for
    /// them. Re-derived on every recorded move, so the policy is never older
    /// than the standings it is derived from — a stale corridor cap is one
    /// that keeps funding a strategy the ledger has already retired.
    corridors: Vec<CorridorSubject>,
    corridor_policy: Option<CorridorPolicy>,
}

impl LifecycleLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge holdout evaluations to `book` from now on.
    pub fn with_trial_book(mut self, book: TrialBook) -> Self {
        self.attach_trial_book(book);
        self
    }

    /// Charge holdout evaluations to `book` from now on, for a ledger that
    /// already exists.
    pub fn attach_trial_book(&mut self, book: TrialBook) {
        self.trials = Some(book);
    }

    pub fn trial_book(&self) -> Option<&TrialBook> {
        self.trials.as_ref()
    }

    pub fn trial_book_mut(&mut self) -> Option<&mut TrialBook> {
        self.trials.as_mut()
    }

    /// Hold every promotion to a capital-holding rung to the §23.4 pools from
    /// now on.
    pub fn with_horizons(mut self, assurance: HorizonAssurance) -> Self {
        self.attach_horizons(assurance);
        self
    }

    /// As [`Self::with_horizons`], for a ledger that already exists.
    pub fn attach_horizons(&mut self, assurance: HorizonAssurance) {
        self.horizons = Some(assurance);
    }

    pub fn horizons(&self) -> Option<&HorizonAssurance> {
        self.horizons.as_ref()
    }

    /// Declare the corridors whose policy this ledger sets, and emit it once
    /// against the standings as they are now.
    ///
    /// Emitting immediately rather than waiting for the next promotion matters:
    /// a desk that declares a corridor funding a strategy that was retired last
    /// week must not be told the corridor is open until something unrelated
    /// happens to move the ledger.
    pub fn declare_corridors(
        &mut self,
        subjects: Vec<CorridorSubject>,
        now: Timestamp,
    ) -> Result<&CorridorPolicy> {
        let policy = crate::corridor::emit(self, &subjects, now)?;
        self.corridors = subjects;
        self.corridor_policy = Some(policy);
        self.corridor_policy
            .as_ref()
            .ok_or_else(|| Error::invalid("the corridor policy was not retained after emission"))
    }

    /// The corridor policy as it stands. `None` until corridors are declared —
    /// the capability's subject, without which there is nothing to rule on.
    pub fn corridor_policy(&self) -> Option<&CorridorPolicy> {
        self.corridor_policy.as_ref()
    }

    pub fn corridors(&self) -> &[CorridorSubject] {
        &self.corridors
    }

    /// Re-derive the corridor policy after a standing changed.
    ///
    /// Deliberately infallible at the call sites that recorded the move: the
    /// subjects were validated when they were declared, and a re-emission that
    /// refused would leave the ledger holding a policy older than the standing
    /// it describes while the move itself had already been recorded. If it
    /// somehow cannot be re-derived the previous policy is dropped rather than
    /// kept, because a stale corridor cap is the one failure this re-emission
    /// exists to prevent.
    fn reemit_corridor_policy(&mut self, now: Timestamp) {
        if self.corridors.is_empty() {
            return;
        }
        self.corridor_policy = crate::corridor::emit(self, &self.corridors.clone(), now).ok();
    }

    /// Count moves into `metrics` from now on.
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.attach_metrics(metrics);
        self
    }

    /// Count moves into `metrics` from now on, for a ledger that already
    /// exists — which is every ledger a composition root reaches, since the
    /// plane builds its own before the root can hand anything in.
    pub fn attach_metrics(&mut self, metrics: Arc<Metrics>) {
        self.metrics = Some(metrics);
    }

    /// Count one recorded move. Keyed on the rungs alone: seven, closed, and
    /// what an alert wants to filter on. Never the strategy, whose number is
    /// whatever the foundry proposes.
    fn record_move(&self, series: &str, from: GateStage, to: GateStage) {
        if let Some(metrics) = &self.metrics {
            metrics.count(
                series,
                labels([("from", from.as_str()), ("to", to.as_str())]),
            );
        }
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

    /// The holdout band a strategy is held inside of: the one its most recent
    /// holdout admission produced. `None` for a strategy never admitted to
    /// holdout, which is a strategy with nothing to be inside of.
    pub fn holdout_band(&self, strategy: &StrategyId) -> Option<&HoldoutBand> {
        self.history(strategy)
            .iter()
            .rev()
            .filter(|entry| entry.promotion.to == GateStage::Holdout)
            .find_map(|entry| entry.band.as_ref())
    }

    /// Judge live returns against the strategy's holdout band.
    ///
    /// Refuses when there is no band on record. A strategy with no band
    /// cannot be "inside" anything, and answering "inside" for it would be
    /// the Phase 3 gate passing on a criterion nobody defined.
    pub fn band_verdict(&self, strategy: &StrategyId, live_returns: &[f64]) -> Result<BandVerdict> {
        let band = self.holdout_band(strategy).ok_or_else(|| {
            Error::denied(format!(
                "no holdout band is on record for {strategy}; live performance is judged \
                 against the band the holdout gate produced, and there is none to be inside \
                 of. Promote {strategy} through the holdout gate before judging it live"
            ))
        })?;
        band.judge(live_returns)
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
    /// Five things are checked here that [`AuthorisedPromotion`] cannot check
    /// on its own, because they are properties of this ledger rather than of
    /// the promotion: the strategy is where the promotion says it is, it has
    /// not been retired, the outcome belongs to the rung being entered, the
    /// outcome passed, and a holdout admission carries the band its
    /// validation defined — a strategy admitted without one would later be
    /// judged live against nothing.
    pub fn record_promotion(
        &mut self,
        strategy: &StrategyId,
        promotion: AuthorisedPromotion,
        admission: Admission,
        rationale: impl Into<String>,
    ) -> Result<Promotion> {
        let Admission { outcome, band } = admission;
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
        match (promotion.to(), band.as_ref()) {
            (GateStage::Holdout, None) => {
                return Err(Error::denied(format!(
                    "the holdout admission for {strategy} carries no band; a strategy is \
                     admitted to holdout only with the band its validation produced, because \
                     live performance is judged against it and a band that does not exist \
                     cannot be fallen outside of. Admit through `HoldoutGate::admit`"
                )));
            }
            (to, Some(_)) if to != GateStage::Holdout => {
                return Err(Error::invalid(format!(
                    "a holdout band belongs to the holdout admission; the {} admission for \
                     {strategy} must not carry one",
                    to.as_str()
                )));
            }
            _ => {}
        }

        // Which pool funds this rung, and what the platform's own sources
        // disagreed about at the moment it was taken. Checked here rather than
        // in `attempt_promotion` so a caller that builds its own gate and
        // records directly cannot route around it — the same reason the rung,
        // the retirement and the band are checked here.
        let horizon = match (&self.horizons, promotion.to().holds_capital()) {
            (Some(assurance), true) => Some(assurance.admit(self, strategy)?),
            _ => None,
        };

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
                band,
                horizon,
            });
        self.record_move(names::STRATEGY_PROMOTIONS, record.from, record.to);
        self.reemit_corridor_policy(record.at);
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
                band: None,
                // A demotion passes no gate, so it reconciles no horizon. The
                // pool it stops drawing on is a fact about the promotions that
                // remain, and the corridor re-emission below is where that
                // becomes consequential.
                horizon: None,
            });
        self.record_move(names::STRATEGY_DEMOTIONS, record.from, record.to);
        self.reemit_corridor_policy(record.at);
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
    let evidence = charge_holdout_trials(ledger, strategy, evidence, promotion.to(), now)?;
    let admission = gate.admit(strategy, &evidence, now);
    ledger.record_promotion(strategy, promotion, admission, rationale)
}

/// Charge this evaluation to the strategy's family before the holdout gate
/// reads the count, and hand the gate evidence carrying the account.
///
/// Charged before the gate runs and whether or not it passes, because a
/// candidate that failed was still a trial — leaving failures uncounted is
/// the sweep counting only its winners. Only the holdout rung is charged: it
/// is the rung that deflates a Sharpe, and the count is meaningless anywhere
/// else. Whatever account the submitted evidence carried is replaced, so the
/// number the gate reads is the book's and not the researcher's.
///
/// Refuses when the ledger has no book, naming what to attach. The book's own
/// refusal covers a strategy no family has enrolled.
fn charge_holdout_trials<'e>(
    ledger: &mut LifecycleLedger,
    strategy: &StrategyId,
    evidence: &'e StrategyEvidence,
    to: GateStage,
    now: Timestamp,
) -> Result<Cow<'e, StrategyEvidence>> {
    if to != GateStage::Holdout {
        return Ok(Cow::Borrowed(evidence));
    }
    // Without holdout evidence there is nothing to charge and the gate fails
    // on its first check; charging nothing here keeps that refusal the one
    // the operator sees.
    let Some(holdout) = evidence.holdout.as_ref() else {
        return Ok(Cow::Borrowed(evidence));
    };
    let book = ledger.trial_book_mut().ok_or_else(|| {
        Error::denied(format!(
            "the lifetime trial count for {strategy} is unknown: this ledger has no trial book. \
             Attach one with `LifecycleLedger::with_trial_book` — opened on a durable store with \
             `TrialBook::open` — open the family with `TrialBook::open_family` and enrol \
             {strategy} with `TrialBook::enrol`; an unknown count is not zero"
        ))
    })?;
    let account = book.charge(strategy, holdout.trials, now)?;
    Ok(Cow::Owned(evidence.clone().with_trial_account(account)))
}
