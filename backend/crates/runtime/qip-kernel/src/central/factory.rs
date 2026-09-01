//! Registering candidates and walking them up the ladder.
//!
//! The factory holds three things and owns none of the rules: the compiled
//! programs and evidence a researcher submitted, the
//! [`LifecycleLedger`] that records every move, and a [`DemotionMonitor`] that
//! runs on a review tick. Every decision it makes is delegated:
//!
//! * whether a promotion may happen at all —
//!   [`qip_lifecycle::AuthorisedPromotion`], via [`attempt_promotion`];
//! * whether the evidence is good enough — the gate for the rung being
//!   entered, in `qip_lifecycle::gates`;
//! * whether a live strategy should be pushed back down —
//!   [`DemotionMonitor::enforce`].
//!
//! The reason there is nothing to skip here is structural rather than
//! disciplinary. `AuthorisedPromotion::advance` computes the target rung from
//! [`qip_contracts::gate::GateStage::next`] and refuses a capital-holding rung
//! without a dual approval, so this module cannot offer a "promote straight to
//! pilot" call: there is no value it could construct to represent one.

use qip_ai::registry::ModelRegistry;
use qip_contracts::gate::{GateStage, Promotion};
use qip_contracts::governance::Approval;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_lifecycle::demotion::{
    DemotionMonitor, DemotionPolicy, DemotionTrigger, LiveObservation, PilotBaseline,
};
use qip_lifecycle::evidence::{KillCondition, StrategyEvidence};
use qip_lifecycle::ledger::{LifecycleLedger, attempt_promotion};
use qip_observability::metrics::Metrics;
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::program::Program;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A strategy the centre is considering, with everything a gate will ask for.
///
/// The compiled program travels with the candidate rather than being fetched
/// later, because the thing a gate admits and the thing a cell runs must be
/// the same artefact. A promotion recorded against one program and a DNA
/// shipped from another is exactly the divergence the shadow gate exists to
/// catch, arriving through the back door.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyCandidate {
    compiled: CompiledStrategy,
    program: Program,
    cell: String,
    venue: VenueId,
    evidence: StrategyEvidence,
    model_reference: Option<String>,
    evidence_artifacts: Vec<String>,
    registered_at: Timestamp,
}

impl StrategyCandidate {
    /// Register a compiled strategy and the arena it points into.
    ///
    /// Refuses a program that does not validate, and a compiled strategy whose
    /// plan points outside it. Both would produce a DNA that a cell could
    /// verify and then fail to run, which is the worst of the two failure
    /// modes: the signature says the payload is intact and the payload is
    /// unusable.
    pub fn new(
        compiled: CompiledStrategy,
        program: Program,
        cell: impl Into<String>,
        venue: VenueId,
        registered_at: Timestamp,
    ) -> Result<Self> {
        program.validate()?;
        for node in compiled.plan() {
            if program.node(*node).is_none() {
                return Err(Error::invalid(format!(
                    "the compiled strategy {} plans a node the program does not contain; the \
                     compiled form and the arena it indexes came from different compilations",
                    compiled.id()
                )));
            }
        }
        let cell = cell.into();
        if cell.trim().is_empty() {
            return Err(Error::invalid(
                "a candidate must name the cell that would run it; capital is granted per cell \
                 and an unnamed cell cannot be recalled",
            ));
        }
        Ok(Self {
            compiled,
            program,
            cell,
            venue,
            evidence: StrategyEvidence::new(),
            model_reference: None,
            evidence_artifacts: Vec::new(),
            registered_at,
        })
    }

    /// Attach the evidence the gates will read.
    pub fn with_evidence(mut self, evidence: StrategyEvidence) -> Self {
        self.evidence = evidence;
        self
    }

    /// Name the model driving the strategy, as `name@version`.
    ///
    /// Carried so the demotion monitor can ask
    /// [`qip_ai::registry::ModelRegistry`] whether that model may still drive a
    /// decision. A strategy whose model is overdue for review is demoted
    /// without anyone having to notice the connection.
    pub fn with_model(mut self, reference: impl Into<String>) -> Self {
        self.model_reference = Some(reference.into());
        self
    }

    /// References to the artifacts the evidence consists of — digests of the
    /// backtest, the feature definitions, the model card.
    ///
    /// These become the `inputs` of the [`qip_contracts::governance::Provenance`]
    /// over a shipped [`super::StrategyDna`], which is what makes the chain
    /// from a live order back to the data it was fitted on walkable.
    pub fn with_evidence_artifacts(mut self, references: Vec<String>) -> Self {
        self.evidence_artifacts = references;
        self
    }

    pub fn strategy(&self) -> &StrategyId {
        self.compiled.id()
    }

    pub fn compiled(&self) -> &CompiledStrategy {
        &self.compiled
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub fn evidence(&self) -> &StrategyEvidence {
        &self.evidence
    }

    pub fn model_reference(&self) -> Option<&str> {
        self.model_reference.as_deref()
    }

    pub fn evidence_artifacts(&self) -> &[String] {
        &self.evidence_artifacts
    }

    pub fn registered_at(&self) -> Timestamp {
        self.registered_at
    }

    /// The kill conditions stated at the pilot gate, if any were.
    pub fn kill_conditions(&self) -> &[KillCondition] {
        self.evidence
            .pilot
            .as_ref()
            .map_or(&[], |pilot| pilot.kill_conditions.as_slice())
    }
}

/// What one review tick concluded about one strategy.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyReview {
    pub strategy: StrategyId,
    pub stage_before: GateStage,
    pub stage_after: GateStage,
    /// Every trigger that fired, not only the first. An incident review needs
    /// to know the model was also overdue, not merely that the loss limit was
    /// hit first.
    pub triggers: Vec<DemotionTrigger>,
    /// The demotion the triggers caused, or `None` where the strategy was
    /// already at or below where they would have put it.
    pub demotion: Option<Promotion>,
}

impl StrategyReview {
    /// Whether this tick moved the strategy.
    pub fn moved(&self) -> bool {
        self.demotion.is_some()
    }
}

/// Candidates, their walk up the ladder, and the triggers that push them down.
#[derive(Debug, Default)]
pub struct StrategyFactory {
    candidates: BTreeMap<StrategyId, StrategyCandidate>,
    ledger: LifecycleLedger,
    monitor: DemotionMonitor,
    baselines: BTreeMap<StrategyId, PilotBaseline>,
}

impl StrategyFactory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a non-default demotion policy.
    pub fn with_demotion_policy(mut self, policy: DemotionPolicy) -> Self {
        self.monitor = DemotionMonitor::new(policy);
        self
    }

    /// Count every rung the ledger records into `metrics`.
    pub fn attach_metrics(&mut self, metrics: Arc<Metrics>) {
        self.ledger.attach_metrics(metrics);
    }

    /// Register a candidate.
    ///
    /// Refuses a second registration under the same identity. A strategy id is
    /// what the ledger, the envelope and the exposure aggregate all key on, so
    /// two different programs sharing one is two different things wearing the
    /// same audit trail.
    pub fn register(&mut self, candidate: StrategyCandidate) -> Result<()> {
        let strategy = candidate.strategy().clone();
        if self.candidates.contains_key(&strategy) {
            return Err(Error::denied(format!(
                "{strategy} is already registered; a revised strategy is a new candidate under a \
                 new identity, so its evidence is re-earned rather than inherited"
            )));
        }
        self.candidates.insert(strategy, candidate);
        Ok(())
    }

    /// Replace a candidate's evidence as later rungs produce it.
    pub fn submit_evidence(
        &mut self,
        strategy: &StrategyId,
        evidence: StrategyEvidence,
    ) -> Result<()> {
        let candidate = self.require_mut(strategy)?;
        candidate.evidence = evidence;
        Ok(())
    }

    pub fn candidate(&self, strategy: &StrategyId) -> Option<&StrategyCandidate> {
        self.candidates.get(strategy)
    }

    pub fn candidates(&self) -> impl Iterator<Item = &StrategyCandidate> {
        self.candidates.values()
    }

    pub fn ledger(&self) -> &LifecycleLedger {
        &self.ledger
    }

    pub fn monitor(&self) -> DemotionMonitor {
        self.monitor
    }

    /// Where a strategy stands. An unregistered strategy is a candidate, which
    /// is what the ledger says too: nobody has promoted it, so it has the
    /// permissions of one that was just proposed.
    pub fn stage_of(&self, strategy: &StrategyId) -> GateStage {
        self.ledger.stage_of(strategy)
    }

    /// Whether a strategy may hold real capital right now.
    ///
    /// The question [`super::CentralPlane::issue`] asks before it sizes
    /// anything, and the reason a shadow-stage strategy cannot be given an
    /// envelope however good its evidence looks.
    pub fn holds_capital(&self, strategy: &StrategyId) -> bool {
        self.stage_of(strategy).holds_capital()
    }

    /// Every strategy currently at a capital-holding rung.
    pub fn holding_capital(&self) -> Vec<StrategyId> {
        self.ledger.holding_capital().into_iter().cloned().collect()
    }

    /// Attempt one step up the ladder.
    ///
    /// The approval is `Option` because most rungs do not need one, and the
    /// two that do cannot be reached without a dual one — that check lives in
    /// [`qip_lifecycle::AuthorisedPromotion::advance`] and is not repeated
    /// here, because a second copy of a rule is a second place for it to
    /// drift.
    pub fn promote(
        &mut self,
        strategy: &StrategyId,
        approval: Option<Approval>,
        rationale: impl Into<String>,
        now: Timestamp,
    ) -> Result<Promotion> {
        let candidate = self.candidates.get(strategy).ok_or_else(|| {
            Error::not_found(format!(
                "{strategy} is not registered, so there is no evidence to put in front of a gate"
            ))
        })?;
        let promotion = attempt_promotion(
            &mut self.ledger,
            strategy,
            &candidate.evidence,
            approval,
            rationale,
            now,
        )?;
        if let Some(baseline) = self.baseline_for(strategy, promotion.to, promotion.at) {
            self.baselines.insert(strategy.clone(), baseline);
        }
        Ok(promotion)
    }

    /// Push a strategy down. Takes no approver, deliberately — see
    /// [`LifecycleLedger::demote`].
    pub fn demote(
        &mut self,
        strategy: &StrategyId,
        to: GateStage,
        raised_by: impl Into<String>,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<Promotion> {
        self.ledger.demote(strategy, to, raised_by, reason, at)
    }

    /// Withdraw a strategy for good.
    pub fn retire(
        &mut self,
        strategy: &StrategyId,
        raised_by: impl Into<String>,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<Promotion> {
        self.ledger.retire(strategy, raised_by, reason, at)
    }

    /// The baseline live performance is judged against.
    pub fn baseline(&self, strategy: &StrategyId) -> Option<&PilotBaseline> {
        self.baselines.get(strategy)
    }

    /// Replace a baseline with one established from realised pilot returns.
    ///
    /// Exposed because the baseline seeded at promotion is the research
    /// expectation, and the pilot's own returns are the better comparison once
    /// they exist. Refreshing it from *live* data would make decay
    /// undetectable — a baseline that follows the strategy down is not a
    /// baseline — so this is a deliberate call, not something the review tick
    /// does on its own.
    pub fn set_baseline(&mut self, baseline: PilotBaseline) {
        self.baselines.insert(baseline.strategy.clone(), baseline);
    }

    /// Run the automatic demotion triggers over live observations.
    ///
    /// One entry per observation that has a baseline to be judged against.
    /// Observations for strategies with no baseline are skipped rather than
    /// erroring: a strategy that never reached pilot has no pilot to have
    /// decayed from, and treating that as a failure would make the tick fail
    /// whenever a cell reported on something it also runs in shadow.
    pub fn review(
        &mut self,
        observations: &[LiveObservation],
        models: Option<&ModelRegistry>,
        now: Timestamp,
    ) -> Result<Vec<StrategyReview>> {
        // Copied out so the ledger can be borrowed mutably below; the monitor
        // is a pair of thresholds, not state.
        let monitor = self.monitor;
        let mut reviews = Vec::new();
        for observation in observations {
            let Some(baseline) = self.baselines.get(&observation.strategy) else {
                continue;
            };
            let stage_before = self.ledger.stage_of(&observation.strategy);
            let (triggers, demotion) =
                monitor.enforce(&mut self.ledger, baseline, observation, models, now)?;
            reviews.push(StrategyReview {
                strategy: observation.strategy.clone(),
                stage_before,
                stage_after: self.ledger.stage_of(&observation.strategy),
                triggers,
                demotion,
            });
        }
        Ok(reviews)
    }

    /// The rungs a strategy has stood on, starting at candidate.
    pub fn path(&self, strategy: &StrategyId) -> Vec<GateStage> {
        self.ledger.path(strategy)
    }

    /// The whole walk, as lines a reviewer can read.
    pub fn narrate(&self, strategy: &StrategyId) -> Vec<String> {
        self.ledger.narrate(strategy)
    }

    fn require_mut(&mut self, strategy: &StrategyId) -> Result<&mut StrategyCandidate> {
        self.candidates
            .get_mut(strategy)
            .ok_or_else(|| Error::not_found(format!("{strategy} is not registered")))
    }

    /// The baseline a strategy entering `stage` should be judged against.
    ///
    /// Entering pilot there is no live history yet, so the research
    /// expectation — the held-out returns the holdout gate read — stands in.
    /// That is precisely the expected-versus-actual comparison the learn edge
    /// needs: the first thing live data can falsify is the number the
    /// promotion was granted on. Entering scaled the pilot's own returns
    /// replace it, because by then there is something realised to compare to.
    fn baseline_for(
        &self,
        strategy: &StrategyId,
        stage: GateStage,
        at: Timestamp,
    ) -> Option<PilotBaseline> {
        if !stage.holds_capital() {
            return None;
        }
        let candidate = self.candidates.get(strategy)?;
        let returns = match stage {
            GateStage::Scaled => candidate
                .evidence
                .scaled
                .as_ref()
                .map(|scaled| scaled.pilot_returns.clone()),
            _ => candidate
                .evidence
                .holdout
                .as_ref()
                .map(|holdout| holdout.holdout_returns.clone()),
        }?;
        Some(PilotBaseline {
            strategy: strategy.clone(),
            established_at: at,
            returns,
            kill_conditions: candidate.kill_conditions().to_vec(),
            model_reference: candidate.model_reference.clone(),
        })
    }
}
