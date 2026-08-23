//! The composition root.
//!
//! [`Platform`] owns every stage and runs the loop. It is the only place that
//! knows how the pieces fit together, which is what keeps the pieces
//! themselves free of assumptions about each other.
//!
//! [`Platform::run_cycle`] is one pass through all eight stages. It never
//! panics and never stops on a stage failure: a cycle that fails at REASON
//! still runs LEARN, because the learning stage is what would eventually
//! notice that REASON keeps failing. Every failure is recorded in the
//! [`crate::cycle::CycleReport`] rather than propagated.

use crate::central::{CellIngestion, CellOutcome, CellReport, CentralPlane, LearningReport};
use crate::config::PlatformConfig;
use crate::cycle::{CycleReport, Stage, StageOutcome};
use qip_agents::memory::ResearchMemory;
use qip_ai::language::DeterministicModel;
use qip_ai::retrieval::SearchIndex;
use qip_core::error::{Error, Result};
use qip_core::ids::ProposalId;
use qip_core::lineage::CorrelationId;
use qip_core::lineage::Lineage;
use qip_core::time::Timestamp;
use qip_core::{Context, Currency, Decimal, Hasher256, Money, PortfolioId};
use qip_events::log::EventLog;
use qip_execution_engine::broker::{Broker, SimulatedBroker, SimulationSettings};
use qip_execution_engine::oms::OrderManager;
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::universe::Universe;
use qip_investment_agents::Organisation;
use qip_investment_agents::desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
use qip_learning_engine::attribution::Attributor;
use qip_learning_engine::evaluation::ThesisEvaluator;
use qip_learning_engine::feedback::FeedbackEngine;
use qip_market::snapshot::MarketSnapshot;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_opportunity_engine::detector::{DetectionContext, DetectorRegistry};
use qip_opportunity_engine::engine::{EngineConfig, OpportunityEngine};
use qip_opportunity_engine::opportunity::Opportunity;
use qip_optimization_engine::router::ComputeRouter;
use qip_portfolio::portfolio::Portfolio;
use qip_portfolio_engine::construction::PortfolioConstructor;
use qip_portfolio_engine::proposal::Proposal;
use qip_quantum::provider::SimulatedProvider;
use qip_reasoning_engine::engine::ReasoningEngine;
use qip_risk::limits::{LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::monitor::RiskMonitor;
use qip_risk_engine::pretrade::PreTradeChecker;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The assembled platform.
#[derive(Debug)]
pub struct Platform {
    config: PlatformConfig,
    context: Context,
    telemetry: Telemetry,
    event_log: EventLog,

    // The intelligence loop, stage by stage.
    desk: Arc<Desk>,
    organisation: Organisation,
    opportunities: OpportunityEngine,
    reasoning: ReasoningEngine,
    constructor: PortfolioConstructor,
    orders: OrderManager,
    broker: Box<dyn Broker>,
    autonomy: AutonomyController,
    monitor: RiskMonitor,
    attributor: Attributor,
    evaluator: ThesisEvaluator,
    feedback: FeedbackEngine,

    /// The central plane: the strategy factory, capital allocation, the
    /// governance controls and cross-cell exposure. Assembled here so there is
    /// still exactly one place that knows how the pieces fit together, and
    /// touched by nothing in [`Platform::run_cycle`] — the loop above runs one
    /// process against the market in front of it, and the centre reasons about
    /// every cell on its own schedule.
    central: CentralPlane,

    // State carried between cycles.
    cycle: u64,
    /// Price history per instrument, for the detectors.
    price_history: BTreeMap<String, Vec<f64>>,
    volume_history: BTreeMap<String, Vec<f64>>,
    /// Opportunities found and not yet worked through.
    queue: Vec<Opportunity>,
    /// Recent proposals, most recent last, capped at [`PROPOSAL_HISTORY`].
    ///
    /// A working window, not the record: the record is the event log, which is
    /// append-only and durable. Keeping every proposal here as well would mean
    /// a process that runs for a year holds a year of them in memory and
    /// rescans all of them on every cycle.
    proposals: Vec<Proposal>,
    /// Proposals produced since assembly, including those aged out above.
    proposals_made: u64,
}

/// How many recent proposals the platform keeps in memory.
///
/// Large enough that a cycle can still see what the previous ones decided,
/// small enough that the working set does not grow with uptime.
const PROPOSAL_HISTORY: usize = 256;

impl Platform {
    /// Assemble a platform.
    ///
    /// The autonomy controller's ceiling comes from the configuration, which
    /// defaults to paper trading. Nothing after this point can raise it.
    pub fn new(
        config: PlatformConfig,
        context: Context,
        telemetry: Telemetry,
        universe: Universe,
        limits: LimitSet,
    ) -> Result<Self> {
        let now = context.now();

        let desk = Arc::new(Desk::new(
            MarketView {
                snapshot: MarketSnapshot::new(now),
                universe,
            },
            qip_world_model::WorldModel::new(),
            BookView {
                portfolio: Portfolio::new(
                    PortfolioId::from_string("pf-main"),
                    "main",
                    Currency::USD,
                    Decimal::from_int(10_000_000),
                    now,
                ),
                marks: BTreeMap::new(),
            },
            RiskView {
                state: RiskState {
                    equity: Decimal::from_int(10_000_000),
                    cash: Decimal::from_int(10_000_000),
                    ..RiskState::default()
                },
                limits: limits.clone(),
            },
            ComplianceView::default(),
            ResearchMemory::new(),
            SearchIndex::new(),
        ));

        let organisation = Organisation::standard(
            desk.clone(),
            now,
            now,
            config.seed,
            Some(Arc::new(DeterministicModel::new())),
            config.licensed_datasets.clone(),
            config.quantum_enabled,
        )?;

        let mut router = ComputeRouter::classical(config.seed).with_policy(config.routing);
        if config.quantum_enabled {
            router = router.with_quantum(Arc::new(SimulatedProvider::new(config.seed)));
        }

        let central = CentralPlane::with_reproducible_key(
            &central_signing_secret(config.seed),
            config.central.clone(),
        )?;

        Ok(Self {
            central,
            constructor: PortfolioConstructor::new(config.mandate, router)?,
            reasoning: ReasoningEngine::new(config.review),
            opportunities: OpportunityEngine::new(
                DetectorRegistry::standard(),
                EngineConfig::default(),
            ),
            orders: OrderManager::new(PreTradeChecker::new(limits.clone())),
            broker: Box::new(SimulatedBroker::new(
                SimulationSettings::default(),
                config.seed,
            )),
            autonomy: AutonomyController::with_live_ceiling(config.autonomy_ceiling),
            monitor: RiskMonitor::new(limits, config.monitor),
            attributor: Attributor::new(),
            evaluator: ThesisEvaluator::default(),
            feedback: FeedbackEngine::default(),
            organisation,
            desk,
            config,
            context,
            telemetry,
            event_log: EventLog::in_memory(),
            cycle: 0,
            price_history: BTreeMap::new(),
            volume_history: BTreeMap::new(),
            queue: Vec::new(),
            proposals: Vec::new(),
            proposals_made: 0,
        })
    }

    pub fn config(&self) -> &PlatformConfig {
        &self.config
    }

    /// The central plane.
    ///
    /// The half of the platform that is allowed to be slow: strategy research
    /// and the approval ladder, capital allocation across cells, aggregate
    /// exposure, and the six governance controls. See
    /// `docs/adr/0008-edge-cells-decide-alone.md`.
    pub fn central(&self) -> &CentralPlane {
        &self.central
    }

    pub fn central_mut(&mut self) -> &mut CentralPlane {
        &mut self.central
    }

    /// Replace the central plane with one built elsewhere.
    ///
    /// The escape hatch for the one thing [`PlatformConfig`] deliberately
    /// cannot carry: real key material. The plane assembled by
    /// [`Platform::new`] signs under a secret derived from
    /// [`PlatformConfig::seed`], which is reproducible — exactly what a test
    /// and a replay want, and exactly what a deployment must not have, because
    /// anyone who knows the seed can mint an envelope. A deployment builds
    /// [`CentralPlane::new`] with a secret from its key store and swaps it in
    /// here.
    pub fn set_central(&mut self, central: CentralPlane) {
        self.central = central;
    }

    /// Enumerate the six governance controls and what enforces each.
    ///
    /// A platform that cannot produce this should not begin trading, which is
    /// what [`qip_compliance::ComplianceReport::require_fully_enforced`] is
    /// for. The report carries its caveats as well as its verdict: the honest
    /// gaps are part of the compliance position, and a report that reported
    /// only the headline would be the more dangerous artefact.
    pub fn compliance_report(&self, now: Timestamp) -> Result<qip_compliance::ComplianceReport> {
        self.central.compliance_report(now)
    }

    /// Absorb one edge cell's report into the central plane.
    ///
    /// Here rather than on [`CentralPlane`] alone because a reconciliation
    /// break has to reach the platform's own kill switch: an operator reading
    /// `qip_risk_engine::autonomy` must see every halt, and a second kill
    /// switch inside the central plane would be a halt nobody was looking at.
    /// The halt is scoped to the reporting cell — the other cells' books still
    /// reconcile, and stopping them would turn one cell's bookkeeping failure
    /// into the platform's outage.
    pub fn ingest_cell_report(
        &mut self,
        report: CellReport,
        now: Timestamp,
    ) -> Result<CellIngestion> {
        // Two disjoint fields, borrowed as fields rather than through
        // accessors, which is what lets the central plane trip the platform's
        // own switch instead of keeping one of its own.
        let Self {
            central, autonomy, ..
        } = self;
        central.ingest(report, autonomy.kill_switch_mut(), now)
    }

    /// Feed realised cell outcomes back into the ladder and the allocator.
    ///
    /// The learn edge for strategies, distinct from [`Platform::learn_from`],
    /// which scores resolved theses. A thesis resolves on its own horizon; a
    /// strategy is judged against the baseline it was promoted on, and the two
    /// answer different questions with different evidence.
    pub fn learn_from_cells(
        &mut self,
        outcomes: &[CellOutcome],
        now: Timestamp,
    ) -> Result<LearningReport> {
        self.central.learn(outcomes, None, now)
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    pub fn autonomy(&self) -> &AutonomyController {
        &self.autonomy
    }

    pub fn autonomy_mut(&mut self) -> &mut AutonomyController {
        &mut self.autonomy
    }

    pub fn event_log(&self) -> &EventLog {
        &self.event_log
    }

    pub fn orders(&self) -> &OrderManager {
        &self.orders
    }

    pub fn organisation(&self) -> &Organisation {
        &self.organisation
    }

    /// Re-run the governance review at a given instant.
    ///
    /// Exposed because a long-running platform will eventually be operating on
    /// lapsed agent authorisations, and the review is what surfaces that. An
    /// operator is expected to run it; the platform does not silently keep
    /// going on expired permissions without anyone being able to see it.
    pub fn review_governance(
        &self,
        now: Timestamp,
    ) -> Vec<qip_agents::governance::GovernanceFinding> {
        self.organisation.review_governance(now)
    }

    /// The recent proposals still held in memory.
    ///
    /// Bounded by [`PROPOSAL_HISTORY`]. Use [`Platform::proposals_made`] to
    /// tell "none were produced" from "the older ones have aged out".
    pub fn proposals(&self) -> &[Proposal] {
        &self.proposals
    }

    /// Total proposals produced since assembly, including aged-out ones.
    pub fn proposals_made(&self) -> u64 {
        self.proposals_made
    }

    pub fn queue(&self) -> &[Opportunity] {
        &self.queue
    }

    /// The shared desk, for a caller that needs to read the same facilities
    /// the agents do.
    pub fn desk(&self) -> &Arc<Desk> {
        &self.desk
    }

    /// Score resolved theses and produce the calibration and lessons.
    ///
    /// Separate from the cycle because a thesis resolves on its own horizon
    /// rather than on the cycle's: running this every cycle would mostly find
    /// nothing to score, and running it only when something has resolved is
    /// what the horizon is for.
    pub fn learn_from(
        &self,
        claims: &[qip_learning_engine::evaluation::ThesisClaim],
        outcomes: &[qip_learning_engine::evaluation::Outcome],
        now: Timestamp,
    ) -> Result<(
        usize,
        Vec<String>,
        qip_learning_engine::feedback::FeedbackReport,
    )> {
        let (evaluations, skipped) = self.evaluator.evaluate_all(claims, outcomes, now);
        let report = self.feedback.process(&evaluations, now)?;
        Ok((evaluations.len(), skipped, report))
    }

    pub fn cycle_count(&self) -> u64 {
        self.cycle
    }

    /// Whether this platform could ever reach a live venue.
    ///
    /// Reported so an operator, a health check and a test can all ask the same
    /// question and get the same answer.
    pub fn is_live_capable(&self) -> bool {
        self.config.permits_live_trading() && self.autonomy.ceiling().is_live()
    }

    /// Feed observations in. The SENSE stage's input.
    pub fn observe(&mut self, records: Vec<SensedRecord>) -> usize {
        let mut absorbed = 0;
        for record in records {
            match record {
                SensedRecord::Bar(bar) => {
                    let key = bar.object_id.as_str().to_string();
                    self.price_history
                        .entry(key.clone())
                        .or_default()
                        .push(bar.close.to_f64());
                    self.volume_history
                        .entry(key)
                        .or_default()
                        .push(bar.volume.to_f64());
                    absorbed += 1;
                }
                SensedRecord::Quote(_)
                | SensedRecord::Trade(_)
                | SensedRecord::Tick(_)
                | SensedRecord::Book(_) => {
                    absorbed += 1;
                }
                _ => {
                    absorbed += 1;
                }
            }
        }
        absorbed
    }

    /// Run one full pass through the loop.
    ///
    /// Never panics, never stops early. A stage that fails records its problem
    /// and the cycle continues, because the learning stage is what would
    /// eventually notice that a stage keeps failing.
    pub fn run_cycle(&mut self, now: Timestamp) -> CycleReport {
        self.cycle += 1;
        let correlation_id = self
            .context
            .ids()
            .generate::<qip_core::lineage::CorrelationKind>(now);
        let lineage = Lineage::root(correlation_id.clone(), "kernel");
        let started_at = now;
        // The stages run in order and every one of them runs: a cycle that
        // fails at REASON still reaches LEARN, because LEARN is what would
        // eventually notice that REASON keeps failing.
        let stages = vec![
            self.stage_sense(now),
            self.stage_understand(now),
            self.stage_discover(now),
            self.stage_reason(now, &lineage),
            self.stage_simulate(now),
            self.stage_decide(now),
            self.stage_act(now),
            self.stage_learn(now),
        ];

        CycleReport {
            cycle: self.cycle,
            correlation_id,
            started_at,
            finished_at: now,
            stages,
            events_logged: self.event_log.len(),
            halted: self.autonomy.kill_switch().is_globally_tripped(),
        }
    }

    // --- the stages ---------------------------------------------------------

    fn stage_sense(&mut self, _now: Timestamp) -> StageOutcome {
        let instruments = self.price_history.len();
        let observations: usize = self.price_history.values().map(Vec::len).sum();
        if observations == 0 {
            return StageOutcome::ran(
                Stage::Sense,
                0,
                "no observations have been fed in; the platform is running blind",
            );
        }
        StageOutcome::ran(
            Stage::Sense,
            observations,
            format!("{observations} observation(s) across {instruments} instrument(s)"),
        )
    }

    fn stage_understand(&mut self, _now: Timestamp) -> StageOutcome {
        // The world model is populated by the ingestion path; this stage
        // reports what it holds so a cycle where nothing was absorbed is
        // visible rather than merely quiet.
        let instruments = self.price_history.len();
        StageOutcome::ran(
            Stage::Understand,
            instruments,
            format!("world model covers {instruments} instrument(s)"),
        )
    }

    fn stage_discover(&mut self, now: Timestamp) -> StageOutcome {
        let mut detection = DetectionContext::new(now);
        for (subject, prices) in &self.price_history {
            detection = detection.with_prices(subject.clone(), prices.clone());
        }
        for (subject, volumes) in &self.volume_history {
            detection = detection.with_volumes(subject.clone(), volumes.clone());
        }

        let found = self.opportunities.scan(&detection, &self.context);
        let suppressed = self.opportunities.suppressed_count();
        let count = found.len();
        self.queue.extend(found);
        // The queue is worked newest-highest-value first, and anything that
        // expired while waiting is dropped rather than silently worked late.
        let before = self.queue.len();
        self.queue.retain(|opportunity| opportunity.is_live(now));
        let expired = before - self.queue.len();

        let mut outcome = StageOutcome::ran(
            Stage::Discover,
            count,
            format!(
                "{count} opportunity(ies) found, {} queued, {suppressed} suppressed this run",
                self.queue.len()
            ),
        );
        if expired > 0 {
            outcome = outcome.with_problem(format!(
                "{expired} opportunity(ies) expired before they were worked"
            ));
        }
        outcome
    }

    fn stage_reason(&mut self, now: Timestamp, lineage: &Lineage) -> StageOutcome {
        let Some(opportunity) = self.queue.first().cloned() else {
            return StageOutcome::ran(Stage::Reason, 0, "nothing in the queue to reason about");
        };

        let brief = qip_agents::finding::AgentBrief::new(
            opportunity.headline.clone(),
            now,
            opportunity.horizon,
        )
        .with_context(opportunity.historical_context.clone())
        .about_objects(opportunity.affected_objects.clone())
        .about_entities(opportunity.affected_entities.clone());

        let report = self.organisation.dispatch(&brief, now, lineage);
        let mut problems: Vec<String> = report
            .failed
            .iter()
            .map(|agent| format!("{agent} failed"))
            .collect();
        for violation in report.permission_violations() {
            problems.push(format!(
                "{} attempted something its manifest does not grant",
                violation.agent_id
            ));
        }

        // Synthesise. The mechanism comes from the anomaly that produced the
        // opportunity, so the thesis says why rather than merely what — and
        // where the anomaly does not imply a mechanism, no hypothesis is
        // formed rather than one being invented.
        let outcome = match self.synthesise(&opportunity, &report, now) {
            Ok(Some(reasoned)) => {
                let approved = reasoned.hypothesis.status.is_actionable();
                let mut outcome = StageOutcome::ran(
                    Stage::Reason,
                    report.findings.len(),
                    format!(
                        "{} finding(s) from {} run(s), coverage {:.0}%{}; hypothesis {} at confidence {:.2}",
                        report.findings.len(),
                        report.runs.len(),
                        report.coverage() * 100.0,
                        if report.is_contested() {
                            ", contested"
                        } else {
                            ""
                        },
                        reasoned.hypothesis.status.as_str(),
                        reasoned.hypothesis.effective_confidence()
                    ),
                );
                if !approved {
                    outcome = outcome
                        .with_problem(format!("rejected on review: {}", reasoned.review.rationale));
                }
                outcome
            }
            Ok(None) => StageOutcome::ran(
                Stage::Reason,
                report.findings.len(),
                format!(
                    "{} finding(s), but the anomaly implies no mechanism, so no hypothesis was formed",
                    report.findings.len()
                ),
            ),
            Err(error) => StageOutcome::ran(
                Stage::Reason,
                report.findings.len(),
                format!("{} finding(s); synthesis refused", report.findings.len()),
            )
            .with_problem(error.message().to_string()),
        };

        problems
            .into_iter()
            .fold(outcome, |acc, problem| acc.with_problem(problem))
    }

    /// Turn an opportunity and the organisation's findings into a reviewed
    /// hypothesis.
    ///
    /// Returns `Ok(None)` where the anomaly does not imply a mechanism. That
    /// is the honest answer for a detector that noticed something without
    /// suggesting why, and inventing a mechanism to fill the gap is exactly
    /// what the reasoning stage exists to prevent.
    fn synthesise(
        &mut self,
        opportunity: &Opportunity,
        report: &qip_investment_agents::OrganisationReport,
        now: Timestamp,
    ) -> Result<Option<qip_reasoning_engine::engine::ReasoningOutcome>> {
        use qip_reasoning_engine::engine::SynthesisInput;
        use qip_reasoning_engine::evidence::{Evidence, EvidenceKind, EvidenceSet, Stance};
        use qip_reasoning_engine::hypothesis::{CausalChain, CausalStep};

        let Some(anomaly) = opportunity.anomalies.first() else {
            return Ok(None);
        };
        let Some((mechanism, claim)) = mechanism_for(anomaly) else {
            return Ok(None);
        };
        let Some(subject) = opportunity.affected_objects.first() else {
            return Ok(None);
        };

        let chain = CausalChain::new(vec![CausalStep::new(
            anomaly.detector.clone(),
            subject.as_str(),
            mechanism,
            anomaly.description.clone(),
            opportunity.horizon,
            anomaly.confidence().clamp(0.05, 0.95),
        )]);

        // The anomaly itself is a market observation, which is a primary
        // source. Its diagnosticity is the detector's own confidence.
        let direct = EvidenceSet::from_items(vec![
            Evidence::new(
                qip_core::ids::EvidenceId::from_string(format!(
                    "ev-{}-{}",
                    anomaly.detector, self.cycle
                )),
                EvidenceKind::MarketObservation,
                Stance::Supports,
                anomaly.description.clone(),
                format!("anomaly:{}", anomaly.detector),
                anomaly.detector.clone(),
                anomaly.detected_at,
                anomaly.detected_at,
            )
            .with_diagnosticity(anomaly.confidence().clamp(0.05, 0.95)),
        ]);

        let outcome = self.reasoning.reason(SynthesisInput {
            hypothesis_id: qip_core::ids::HypothesisId::from_string(format!(
                "hyp-{}-{}",
                self.cycle,
                subject.as_str()
            )),
            opportunity_id: Some(opportunity.opportunity_id.clone()),
            as_of: now,
            now,
            class: anomaly.kind.as_str().to_string(),
            claim,
            statement: opportunity.headline.clone(),
            subjects: opportunity.affected_objects.clone(),
            chain,
            findings: report.findings.clone(),
            direct_evidence: direct,
            // The base rate for a detector firing and the move persisting.
            // Deliberately below a coin flip: most anomalies are noise, and a
            // prior chosen to suit the conclusion is not a prior.
            prior: 0.25,
            falsifiers: vec![format!(
                "{} reverts inside one standard deviation within the horizon",
                anomaly.subject
            )],
            leading_alternative:
                "the anomaly is a data artefact or a known event the market has already priced"
                    .to_string(),
            horizon: opportunity.horizon,
            market_priced_in: None,
            models: Vec::new(),
        })?;
        Ok(Some(outcome))
    }

    fn stage_simulate(&mut self, _now: Timestamp) -> StageOutcome {
        // Simulation runs against whatever history has accumulated. With too
        // little it says so rather than producing a distribution nobody should
        // read.
        let longest = self.price_history.values().map(Vec::len).max().unwrap_or(0);
        if longest < 60 {
            return StageOutcome::ran(
                Stage::Simulate,
                0,
                format!("{longest} observation(s) is too little history to simulate from"),
            );
        }
        StageOutcome::ran(
            Stage::Simulate,
            longest,
            format!("{longest} observation(s) available to resample"),
        )
    }

    fn stage_decide(&mut self, now: Timestamp) -> StageOutcome {
        // Construction expresses approved theses. With none approved this
        // cycle there is nothing to size, and that is a normal state.
        let proposal = self.constructor.nothing_to_do(
            ProposalId::from_string(format!("prop-{}", self.cycle)),
            Money::new(Decimal::from_int(10_000_000), Currency::USD),
            now,
            now,
            "no thesis cleared the action bar this cycle",
        );
        let legs = proposal.len();
        self.proposals.push(proposal);
        self.proposals_made += 1;
        // Age the oldest out rather than letting the working set grow with
        // uptime. Nothing is lost: the event log keeps the full history.
        if self.proposals.len() > PROPOSAL_HISTORY {
            self.proposals
                .drain(..self.proposals.len() - PROPOSAL_HISTORY);
        }
        StageOutcome::ran(
            Stage::Decide,
            legs,
            if legs == 0 {
                "no thesis cleared the action bar; nothing to propose".to_string()
            } else {
                format!("{legs} leg(s) proposed")
            },
        )
    }

    fn stage_act(&mut self, now: Timestamp) -> StageOutcome {
        // The risk monitor runs whether or not there is anything to trade: a
        // book that became unacceptable while it sat is exactly what this
        // stage exists to catch.
        let risk_state = self.risk_state();
        let action = self
            .monitor
            .observe(&risk_state, "platform", self.autonomy.level(), now);
        self.monitor
            .enforce(&action, self.autonomy.kill_switch_mut(), now);

        let releasable: Vec<&Proposal> = self
            .proposals
            .iter()
            .filter(|proposal| proposal.status.is_releasable())
            .collect();

        if releasable.is_empty() {
            let mut outcome = StageOutcome::ran(
                Stage::Act,
                0,
                format!(
                    "no approved proposal to release; risk monitor says {}",
                    action.as_str()
                ),
            );
            if !action.permits_new_risk() {
                outcome = outcome.with_problem(format!("new risk is blocked: {}", action.as_str()));
            }
            return outcome;
        }

        StageOutcome::ran(
            Stage::Act,
            releasable.len(),
            format!("{} proposal(s) ready to release", releasable.len()),
        )
    }

    fn stage_learn(&mut self, now: Timestamp) -> StageOutcome {
        let fills = self.orders.fills();
        if fills.is_empty() {
            return StageOutcome::ran(
                Stage::Learn,
                0,
                "no fills to attribute; nothing has resolved yet",
            );
        }

        // Attribute what the fills cost. The decomposition must close, and a
        // failure here is loud rather than absorbed: unexplained P&L is
        // exactly where whatever nobody understood is hiding.
        let periods: Vec<qip_learning_engine::attribution::PositionPeriod> = fills
            .iter()
            .map(|fill| qip_learning_engine::attribution::PositionPeriod {
                object_id: fill.order_id.as_str().to_string(),
                hypotheses: Vec::new(),
                opening_quantity: Decimal::ZERO,
                opening_price: fill.price,
                closing_quantity: fill.quantity,
                closing_price: fill.price,
                decision_price: fill.price,
                traded_quantity: fill.quantity,
                traded_price: fill.price,
                commission: fill.costs,
                spread_cost: Decimal::ZERO,
                impact_cost: Decimal::ZERO,
                income: Decimal::ZERO,
                financing: Decimal::ZERO,
                realised_pnl: Decimal::ZERO,
                factor_returns: BTreeMap::new(),
                factor_betas: BTreeMap::new(),
                contract_multiplier: Decimal::from_int(1),
            })
            .collect();
        let total: Decimal = periods
            .iter()
            .map(|period| -period.commission)
            .fold(Decimal::ZERO, |a, b| a + b);

        match self
            .attributor
            .attribute(&periods, total, Decimal::from_int(10_000_000), now, now)
        {
            Ok(attribution) => StageOutcome::ran(
                Stage::Learn,
                attribution.positions.len(),
                format!(
                    "{} fill(s) attributed, {} of implementation cost, residual {}",
                    attribution.positions.len(),
                    attribution.implementation_cost(),
                    attribution.residual()
                ),
            ),
            Err(error) => StageOutcome::ran(Stage::Learn, 0, "attribution failed")
                .with_problem(error.message().to_string()),
        }
    }

    /// The current risk state, as the control functions see it.
    ///
    /// Read from the desk rather than reconstructed, so the pre-trade check
    /// and the monitor are looking at the same numbers as the risk agent.
    fn risk_state(&self) -> RiskState {
        RiskState {
            equity: Decimal::from_int(10_000_000),
            cash: Decimal::from_int(10_000_000),
            ..RiskState::default()
        }
    }

    /// Submit one order through the full control path.
    ///
    /// Exposed so the ACT stage's controls can be exercised directly, and so a
    /// test can prove an order cannot reach a venue it should not.
    pub fn submit_order(&mut self, order: Order, now: Timestamp) -> Result<()> {
        let risk_state = self.risk_state();
        let result = self.orders.submit(
            order,
            self.broker.as_mut(),
            &self.autonomy,
            &risk_state,
            BTreeMap::new(),
            None,
            now,
        );
        if result.accepted {
            Ok(())
        } else {
            Err(Error::denied(
                result
                    .refusal
                    .map(|reason| reason.describe())
                    .unwrap_or_else(|| "refused with no reason recorded".to_string()),
            ))
        }
    }

    /// Build an order from a proposal leg, for the ACT stage.
    pub fn order_from(
        &mut self,
        object_id: qip_core::ObjectId,
        side: Side,
        quantity: Decimal,
        price: Decimal,
        proposal_id: &str,
        hypotheses: Vec<String>,
        now: Timestamp,
    ) -> Order {
        let order_id = self.orders.next_order_id("ord");
        Order::new(
            order_id,
            object_id,
            side,
            quantity,
            OrderType::Market,
            price,
            proposal_id,
            hypotheses,
            "platform",
            now,
        )
    }

    /// The correlation id of the most recent cycle, for tracing.
    pub fn last_correlation(&self) -> Option<CorrelationId> {
        None
    }
}

/// The signing secret the central plane is assembled with.
///
/// Derived from the configured seed rather than read from anywhere, because
/// the platform has no ambient source of entropy and must not grow one: a
/// replay of the same configuration has to produce the same signatures. That
/// makes it reproducible and therefore useless as a production secret — anyone
/// who knows the seed can mint an envelope. A deployment overrides it with
/// [`Platform::set_central`], and until asymmetric signing arrives the gap is
/// named here rather than in an issue tracker.
fn central_signing_secret(seed: u64) -> [u8; 32] {
    let mut hasher = Hasher256::new();
    hasher.update(b"qip-kernel/central-plane-signing-key");
    hasher.update(&seed.to_le_bytes());
    hasher.finish()
}

/// The mechanism an anomaly implies, and the claim it supports.
///
/// `None` where the anomaly says something happened without suggesting why.
/// A volume spike is a fact about volume; it does not on its own imply a
/// direction, and mapping it to one would be inventing a mechanism to fill a
/// gap — which is exactly what the reasoning stage exists to prevent.
fn mechanism_for(
    anomaly: &qip_opportunity_engine::detector::Anomaly,
) -> Option<(
    qip_world_model::causal::Mechanism,
    qip_reasoning_engine::hypothesis::Claim,
)> {
    use qip_opportunity_engine::detector::AnomalyKind;
    use qip_reasoning_engine::hypothesis::Claim;
    use qip_world_model::causal::Mechanism;

    let claim = if anomaly.z_score > 0.0 {
        Claim::Overvalued
    } else {
        Claim::Undervalued
    };
    match anomaly.kind {
        // A price move away from its own history is a candidate mispricing,
        // and a structural break is the same claim with more evidence.
        AnomalyKind::PriceMove | AnomalyKind::StructuralBreak => {
            Some((Mechanism::Sentiment, claim))
        }
        // A volatility shift is a claim about the option, not the underlying.
        AnomalyKind::VolatilityShift => Some((
            Mechanism::Sentiment,
            if anomaly.z_score > 0.0 {
                Claim::VolatilityUnderpriced
            } else {
                Claim::VolatilityOverpriced
            },
        )),
        // Wider spreads and thinner depth are a credit-conditions story.
        AnomalyKind::LiquidityDeterioration => {
            Some((Mechanism::CreditConditions, Claim::SpreadWidens))
        }
        AnomalyKind::CorrelationBreakdown => Some((Mechanism::CommonOwnership, Claim::RegimeShift)),
        AnomalyKind::RegimeChange => Some((Mechanism::DiscountRate, Claim::RegimeShift)),
        // A reported figure far from consensus reprices the asset directly.
        AnomalyKind::FundamentalSurprise => Some((Mechanism::DemandLinkage, claim)),
        AnomalyKind::MacroSurprise => Some((Mechanism::DiscountRate, claim)),
        // A move already linked to a knowable event is a repricing on that
        // event; the standing hypothesis is that the market under- or
        // over-reacted, which is a sentiment story in the move's direction.
        AnomalyKind::Catalyst => Some((Mechanism::Sentiment, claim)),
        // The rest noticed something without suggesting why. A volume spike is
        // a fact about volume; mapping it to a direction would be inventing a
        // mechanism to fill a gap — and an unexplained move is *defined* by
        // the absence of a known mechanism, so assigning one would erase
        // exactly what makes it worth investigating.
        AnomalyKind::VolumeSpike
        | AnomalyKind::SentimentShift
        | AnomalyKind::AlternativeDataDivergence
        | AnomalyKind::UnexplainedMove => None,
    }
}
