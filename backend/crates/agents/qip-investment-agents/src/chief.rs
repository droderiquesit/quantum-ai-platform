//! The coordinator, and the organisation it coordinates.
//!
//! [`ChiefInvestmentIntelligence`] decides which specialists a question belongs
//! to, and reports what came back: where the organisation agrees, where it
//! disagrees, and — the part that is usually left out — where nobody competent
//! looked at all.
//!
//! It forms no view of its own. Its manifest grants it no `publish_hypothesis`,
//! no `propose_trade`, no approval and no veto, so the aggregate it produces
//! cannot quietly become an opinion. That matters because an aggregate reads
//! like a conclusion, and a coordinator that added its own tilt to eighteen
//! specialists' findings would be a nineteenth specialist nobody reviewed.
//!
//! [`Organisation`] is the assembly: the roster, the agents, the host and the
//! audit trail, with one entry point that dispatches a brief and collects what
//! comes back.

use crate::desk::Desk;
use crate::manifests;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction, FindingStatus};
use qip_agents::governance::{AuditTrail, GovernanceFinding, Roster};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{
    Agent, AgentBacklog, AgentContext, AgentHost, AgentRunRecord, Deferral, RunStatus,
};
use qip_ai::language::LanguageModel;
use qip_core::error::Result;
use qip_core::ids::AgentRunId;
use qip_core::lineage::Lineage;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What the organisation said about one brief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrganisationReport {
    pub as_of: Timestamp,
    pub question: String,
    /// Every run, successful or not.
    pub runs: Vec<AgentRunRecord>,
    /// The findings that carry information.
    pub findings: Vec<AgentFinding>,
    /// Agents that declined as out of scope.
    pub deferred: Vec<String>,
    /// Agents whose run failed or was refused.
    pub failed: Vec<String>,
    /// Agents that reported missing data, and what was missing.
    pub missing_inputs: Vec<String>,
    /// Agents the runtime could not run this dispatch — a manifest past
    /// review, a budget it could not fund — whose run is held on the
    /// organisation's bounded backlog (R12-AGENCY-10063). Each is also in
    /// `failed`, because for this dispatch its finding is missing; this list
    /// says the work was kept rather than lost.
    #[serde(default)]
    pub backlogged: Vec<String>,
    /// Runs the backlog refused to hold — at capacity, or past the attempt
    /// bound — with the reason. Reported, never silently dropped.
    #[serde(default)]
    pub backlog_refused: Vec<String>,
}

impl OrganisationReport {
    /// Findings that took a direction, by direction.
    pub fn directional(&self) -> (Vec<&AgentFinding>, Vec<&AgentFinding>) {
        let positive = self
            .findings
            .iter()
            .filter(|f| f.direction == Direction::Positive)
            .collect();
        let negative = self
            .findings
            .iter()
            .filter(|f| f.direction == Direction::Negative)
            .collect();
        (positive, negative)
    }

    /// Conviction-weighted balance in `[-1, 1]`.
    ///
    /// Reported as a summary statistic, deliberately not as a decision. A
    /// balance of +0.3 does not mean buy; it means the specialists lean
    /// positive, and the reasoning stage decides what to do about that.
    pub fn balance(&self) -> f64 {
        let total: f64 = self.findings.iter().map(|f| f.effective_conviction()).sum();
        if total <= 1e-12 {
            return 0.0;
        }
        self.findings
            .iter()
            .map(|f| f.direction.sign() * f.effective_conviction())
            .sum::<f64>()
            / total
    }

    /// Whether the specialists disagree in direction, not merely in degree.
    pub fn is_contested(&self) -> bool {
        let (positive, negative) = self.directional();
        !positive.is_empty() && !negative.is_empty()
    }

    /// Fraction of the agents asked that produced an informative finding.
    ///
    /// Low coverage on a confident-looking balance is the dangerous case: a
    /// strong consensus among two agents is not a strong consensus.
    pub fn coverage(&self) -> f64 {
        let asked = self.runs.len();
        if asked == 0 {
            return 0.0;
        }
        self.findings.len() as f64 / asked as f64
    }

    /// The scopes the informative findings evidence, by the fact labels each
    /// scope names (see [`crate::coverage`]).
    ///
    /// Distinct from [`Self::coverage`], which is a ratio of findings to runs
    /// and cannot say *what* went unexamined: an organisation missing its
    /// microstructure analyst has the same ratio and no queue figure.
    pub fn covered_scopes(&self) -> std::collections::BTreeSet<crate::coverage::Scope> {
        crate::coverage::covered_by(&self.findings)
    }

    /// Runs where an agent attempted something it was not granted.
    pub fn permission_violations(&self) -> Vec<&AgentRunRecord> {
        self.runs
            .iter()
            .filter(|run| !run.denied_accesses().is_empty())
            .collect()
    }
}

/// The coordinator.
#[derive(Debug)]
pub struct ChiefInvestmentIntelligence {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl ChiefInvestmentIntelligence {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for ChiefInvestmentIntelligence {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        // The coordinator reports on the state of the organisation's inputs,
        // not on the market. Its own run is what establishes that the desk was
        // reachable and what it contained at the as-of time.
        let (instruments, universe_size) = {
            let market = self.desk.market.get(ctx)?;
            (market.snapshot.len(), market.universe.len())
        };
        let statistics = {
            let world = self.desk.world.get(ctx)?;
            world.statistics()
        };

        if instruments == 0 && universe_size == 0 {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the desk holds no market state and no reference data; there is nothing to dispatch about",
            ));
        }

        let facts = statistics
            .iter()
            .map(|(key, value)| (key.clone(), *value as f64))
            .collect::<BTreeMap<String, f64>>();

        let mut builder = FindingBuilder::new(
            ctx,
            brief.as_of,
            format!(
                "the desk holds {instruments} instrument(s) with market state against a universe of {universe_size}"
            ),
        )
        // A coordinator that took a direction would be a specialist nobody
        // reviewed.
        .direction(Direction::Neutral, 0.0)
        .fact(computed(
            ctx,
            "instruments_with_state",
            instruments as f64,
            "count",
            &["market_snapshot"],
        ))
        .fact(computed(
            ctx,
            "universe_size",
            universe_size as f64,
            "count",
            &["universe"],
        ))
        .evidence(vec![format!("market-snapshot@{}", brief.as_of)])
        .caveats(vec![
            "this is a statement about coverage, not about the market".to_string(),
        ]);

        for (key, value) in facts {
            builder = builder.fact(computed(
                ctx,
                &format!("world_{key}"),
                value,
                "count",
                &["world_model"],
            ));
        }
        builder.build()
    }
}

/// What the panel said, as the second dispatch phase hands it to an
/// [`Arbiter`] (R12-AMBIENT-3024, ADR 0466).
///
/// Handed by the organisation rather than reached through a desk gate: it is
/// the brief's own answer so far, produced inside this dispatch, and no
/// facility on the desk holds it. Empty outside the second phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelView {
    /// The panel's informative findings, in dispatch order.
    pub findings: Vec<AgentFinding>,
    /// Whether the panel disagreed in direction ([`OrganisationReport::is_contested`]).
    pub contested: bool,
}

/// An agent that runs after the panel, with the panel's findings in hand.
///
/// Every other agent sees the brief alone, by design: a specialist that read
/// its colleagues would be a correlated second finding from one origin. The
/// arbiter is the one exception, and the exception is bounded: it runs once,
/// after every panel agent, never alongside them, so no panel finding was
/// formed knowing what an arbiter would say.
pub trait Arbiter: Agent {
    fn arbitrate(
        &self,
        ctx: &mut AgentContext,
        brief: &AgentBrief,
        panel: &PanelView,
    ) -> Result<AgentFinding>;
}

/// The arbiter's run as the host sees it: an [`Agent`] whose analysis is
/// the arbitration over one panel. Lets the second phase go through
/// [`AgentHost::run`] — capability checks, budget, audit — like every run.
#[derive(Debug)]
struct Arbitration<'a> {
    arbiter: &'a dyn Arbiter,
    panel: &'a PanelView,
}

impl Agent for Arbitration<'_> {
    fn manifest(&self) -> &AgentManifest {
        self.arbiter.manifest()
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        self.arbiter.accepts(brief)
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        self.arbiter.arbitrate(ctx, brief, self.panel)
    }
}

/// The assembled organisation: roster, agents, host and audit trail.
#[derive(Debug)]
pub struct Organisation {
    roster: Roster,
    agents: Vec<Arc<dyn Agent>>,
    /// Agents run after the panel with its findings (ADR 0466). Rostered
    /// like every agent; never run in the first phase.
    arbiters: Vec<Arc<dyn Arbiter>>,
    host: AgentHost,
    audit: AuditTrail,
    /// Monotonic run counter, so run ids are deterministic across a replay.
    sequence: u64,
    /// Runs the host could not perform, held for a later dispatch
    /// (R12-AGENCY-10063). Bounded, so a runtime outage degrades to a queue
    /// of fixed size and never to a process that grows for as long as the
    /// outage lasts.
    backlog: AgentBacklog,
}

impl Organisation {
    /// Assemble from a roster and the agents implementing it.
    ///
    /// The roster is validated here rather than at first use: a platform whose
    /// governance does not hold should fail at start-up, not on the first
    /// decision it makes.
    pub fn new(
        roster: Roster,
        agents: Vec<Arc<dyn Agent>>,
        host: AgentHost,
        now: Timestamp,
    ) -> Result<Self> {
        roster.validate(now)?;
        // An agent the roster does not hold runs under a manifest nobody's
        // governance review read: `Roster::validate` checks the manifests it
        // was given, and an agent outside it would escape every rule there —
        // independence, the veto, the market reach. The poisoning detector
        // was exactly that case in reverse for a while, a manifest written
        // and never rostered, and the next one should fail at assembly.
        for agent in &agents {
            let id = &agent.manifest().id;
            if roster.get(id).is_none() {
                return Err(qip_core::error::Error::invalid(format!(
                    "agent {id} is not on the roster, so its manifest was never reviewed; add \
                     it to manifests::roster or do not construct it"
                )));
            }
        }
        // One entry per agent per distinct question, with room for the
        // question to change once before the oldest entry's retries run out:
        // a roster refused on every cycle of a lapse holds one entry per
        // agent, not one per cycle (`AgentBacklog::defer` keys on both).
        let backlog = AgentBacklog::new(agents.len().max(1) * 2)?;
        Ok(Self {
            roster,
            agents,
            arbiters: Vec::new(),
            host,
            audit: AuditTrail::new(),
            sequence: 0,
            backlog,
        })
    }

    /// Add the agents that run after the panel (ADR 0466).
    ///
    /// Refused for an arbiter the roster does not hold, for the reason
    /// [`Self::new`] refuses an unrostered agent; and for one also on the
    /// panel, which would read findings it had itself just written.
    pub fn with_arbiters(mut self, arbiters: Vec<Arc<dyn Arbiter>>) -> Result<Self> {
        for arbiter in &arbiters {
            let id = &arbiter.manifest().id;
            if self.roster.get(id).is_none() {
                return Err(qip_core::error::Error::invalid(format!(
                    "arbiter {id} is not on the roster, so its manifest was never reviewed; add \
                     it to manifests::roster or do not construct it"
                )));
            }
            if self.agents.iter().any(|agent| &agent.manifest().id == id) {
                return Err(qip_core::error::Error::invalid(format!(
                    "{id} is both a panel agent and an arbiter, so it would arbitrate over its \
                     own finding; construct it in one phase only"
                )));
            }
        }
        // The backlog bound counts every agent that can be refused a run.
        let capacity = (self.agents.len() + arbiters.len()).max(1) * 2;
        if self.backlog.is_empty() && self.backlog.capacity() < capacity {
            self.backlog = AgentBacklog::new(capacity)?;
        }
        self.arbiters = arbiters;
        Ok(self)
    }

    /// Replace the backlog, for a deployment or a test that wants a bound
    /// other than two entries per agent.
    pub fn with_backlog(mut self, backlog: AgentBacklog) -> Self {
        self.backlog = backlog;
        self
    }

    /// The runs the host could not perform and the organisation is holding.
    pub fn backlog(&self) -> &AgentBacklog {
        &self.backlog
    }

    /// The standard organisation: every agent [`standard_agents`] builds,
    /// under [`manifests::roster`].
    pub fn standard(
        desk: Arc<Desk>,
        reviewed_at: Timestamp,
        now: Timestamp,
        seed: u64,
        language_model: Option<Arc<dyn LanguageModel>>,
        licensed_datasets: Vec<String>,
        quantum_backend_available: bool,
    ) -> Result<Self> {
        let mut host = AgentHost::new(seed);
        if let Some(model) = language_model {
            host = host.with_language_model(model);
        }
        let agents = standard_agents(
            desk.clone(),
            reviewed_at,
            licensed_datasets,
            quantum_backend_available,
        );
        Self::new(manifests::roster(reviewed_at), agents, host, now)?
            .with_arbiters(standard_arbiters(desk, reviewed_at))
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub fn audit(&self) -> &AuditTrail {
        &self.audit
    }

    /// Every agent the organisation runs, panel and arbiters.
    pub fn len(&self) -> usize {
        self.agents.len() + self.arbiters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.arbiters.is_empty()
    }

    /// Re-run the governance review, e.g. after time has passed and an
    /// authorisation may have lapsed.
    pub fn review_governance(&self, now: Timestamp) -> Vec<GovernanceFinding> {
        self.roster.review(now)
    }

    /// Dispatch a brief to every agent that accepts it.
    ///
    /// Failure is isolated per agent: one agent erroring does not stop the
    /// others, because an organisation where one analyst's bug silences the
    /// other seventeen is worse than one where a finding is missing.
    pub fn dispatch(
        &mut self,
        brief: &AgentBrief,
        now: Timestamp,
        lineage: &Lineage,
    ) -> OrganisationReport {
        let mut collected = Collected::default();
        for agent in &self.agents {
            run_one(
                &self.host,
                &mut self.backlog,
                &mut self.audit,
                &mut self.sequence,
                agent.as_ref(),
                brief,
                now,
                lineage,
                &mut collected,
            );
        }

        // The second phase (ADR 0466): every arbiter, after the whole panel,
        // with the panel's informative findings and whether they disagree.
        // Built from the first phase alone, so no arbiter reads another's.
        if !self.arbiters.is_empty() {
            let panel = PanelView {
                findings: collected.findings.clone(),
                contested: collected.is_contested(),
            };
            for arbiter in &self.arbiters {
                let arbitration = Arbitration {
                    arbiter: arbiter.as_ref(),
                    panel: &panel,
                };
                run_one(
                    &self.host,
                    &mut self.backlog,
                    &mut self.audit,
                    &mut self.sequence,
                    &arbitration,
                    brief,
                    now,
                    lineage,
                    &mut collected,
                );
            }
        }

        OrganisationReport {
            as_of: brief.as_of,
            question: brief.question.clone(),
            runs: collected.runs,
            findings: collected.findings,
            deferred: collected.deferred,
            failed: collected.failed,
            missing_inputs: collected.missing_inputs,
            backlogged: collected.backlogged,
            backlog_refused: collected.backlog_refused,
        }
    }
}

/// What a dispatch has gathered so far, across both phases.
#[derive(Default)]
struct Collected {
    runs: Vec<AgentRunRecord>,
    findings: Vec<AgentFinding>,
    deferred: Vec<String>,
    failed: Vec<String>,
    missing_inputs: Vec<String>,
    backlogged: Vec<String>,
    backlog_refused: Vec<String>,
}

impl Collected {
    /// The same test as [`OrganisationReport::is_contested`], over the
    /// findings gathered so far.
    fn is_contested(&self) -> bool {
        let positive = self
            .findings
            .iter()
            .any(|f| f.direction == Direction::Positive);
        let negative = self
            .findings
            .iter()
            .any(|f| f.direction == Direction::Negative);
        positive && negative
    }
}

/// Run one agent and file what came back.
///
/// Failure is isolated per agent: one agent erroring does not stop the
/// others.
#[allow(clippy::too_many_arguments)]
fn run_one(
    host: &AgentHost,
    backlog: &mut AgentBacklog,
    audit: &mut AuditTrail,
    sequence: &mut u64,
    agent: &dyn Agent,
    brief: &AgentBrief,
    now: Timestamp,
    lineage: &Lineage,
    out: &mut Collected,
) {
    if !agent.accepts(brief) {
        out.deferred.push(agent.manifest().id.clone());
        return;
    }
    *sequence += 1;
    let run_id = AgentRunId::from_string(format!("run-{}-{}", agent.manifest().id, sequence));
    let record = host.run(
        agent,
        brief,
        now,
        lineage.relabel(agent.manifest().id.clone()),
        run_id,
    );

    match (&record.status, &record.finding) {
        (RunStatus::Succeeded, Some(finding)) => {
            // The agent ran: whatever the backlog held for it is done
            // with, whether or not this finding is informative.
            backlog.retire(&finding.agent_id);
            for missing in &finding.missing_inputs {
                out.missing_inputs
                    .push(format!("{}: {missing}", finding.agent_id));
            }
            match finding.status {
                FindingStatus::Deferred => out.deferred.push(finding.agent_id.clone()),
                _ if finding.status.is_informative() => out.findings.push(finding.clone()),
                _ => {}
            }
        }
        // The runtime could not run the agent: a manifest past
        // review, or a budget it could not fund. The run degrades to
        // the backlog and the dispatch goes on to the next agent; an
        // execution that waited on this agent would be an execution
        // dependency on the agent runtime, which R12-AGENCY-10063
        // forbids. A `Failed` run is the agent's own error and is not
        // retried: a bug is not work waiting for a runtime.
        (RunStatus::Refused { reason }, _) => {
            let id = agent.manifest().id.clone();
            out.failed.push(id.clone());
            match backlog.defer(&id, brief, reason.as_str(), now) {
                Deferral::Queued | Deferral::Repeated { .. } => out.backlogged.push(id),
                Deferral::Refused { reason } => out.backlog_refused.push(reason),
            }
        }
        (RunStatus::Escalated { policy, to }, _) => {
            let id = agent.manifest().id.clone();
            out.failed.push(id.clone());
            let reason = match to {
                Some(to) => format!("budget exhausted; escalated to {to} under {policy:?}"),
                None => {
                    format!("budget exhausted; escalation policy {policy:?} names nobody")
                }
            };
            match backlog.defer(&id, brief, reason, now) {
                Deferral::Queued | Deferral::Repeated { .. } => out.backlogged.push(id),
                Deferral::Refused { reason } => out.backlog_refused.push(reason),
            }
        }
        _ => out.failed.push(agent.manifest().id.clone()),
    }
    audit.record(record.clone());
    out.runs.push(record);
}

/// The agents that run after the panel (ADR 0466): today the Meta Brain
/// alone. Split from [`Organisation::standard`] for the reason
/// [`standard_agents`] is, so a test can assemble the organisation without
/// it and show the scopes it alone covers go uncovered.
pub fn standard_arbiters(desk: Arc<Desk>, reviewed_at: Timestamp) -> Vec<Arc<dyn Arbiter>> {
    vec![Arc::new(crate::meta_brain::MetaBrain::new(
        manifests::meta_brain(reviewed_at),
        desk,
    ))]
}

/// Every agent of the standard organisation, each under its manifest, sharing
/// one desk.
///
/// Split from [`Organisation::standard`] so the organisation tests can build
/// the same list with one agent removed and show what the organisation then
/// stops covering — the integration clause every specialist row in the
/// blueprint names — without a production method that drops an agent.
pub fn standard_agents(
    desk: Arc<Desk>,
    reviewed_at: Timestamp,
    licensed_datasets: Vec<String>,
    quantum_backend_available: bool,
) -> Vec<Arc<dyn Agent>> {
    use crate::{
        adversary, analysts, construction, control, crypto_brain, execution, fast, learning,
        poisoning, portfolio_brain, reasoning, review,
    };

    vec![
        Arc::new(ChiefInvestmentIntelligence::new(
            manifests::chief(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::MacroAnalyst::new(
            manifests::macro_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::EquityAnalyst::new(
            manifests::equity_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::CreditAnalyst::new(
            manifests::credit_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(fast::MicrostructureAnalyst::new(
            manifests::microstructure_analyst(reviewed_at),
            desk.clone(),
            qip_core::Decimal::from_int(1_000),
        )),
        Arc::new(analysts::DerivativesAnalyst::new(
            manifests::derivatives_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::CommoditiesAnalyst::new(
            manifests::commodities_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::FxRatesAnalyst::new(
            manifests::fx_rates_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(analysts::AlternativeDataAnalyst::new(
            manifests::alternative_data_analyst(reviewed_at),
            desk.clone(),
            licensed_datasets,
        )),
        Arc::new(reasoning::CausalAnalyst::new(
            manifests::causal_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(review::AdversarialReviewer::new(
            manifests::adversarial_reviewer(reviewed_at),
            desk.clone(),
        )),
        Arc::new(crate::simulation::SimulationAnalyst::new(
            manifests::simulation_analyst(reviewed_at),
            desk.clone(),
        )),
        Arc::new(construction::PortfolioConstruction::new(
            manifests::portfolio_construction(reviewed_at),
            desk.clone(),
        )),
        Arc::new(construction::QuantumOptimization::new(
            manifests::quantum_optimization(reviewed_at),
            desk.clone(),
            quantum_backend_available,
        )),
        Arc::new(control::RiskControl::new(
            manifests::risk_control(reviewed_at),
            desk.clone(),
        )),
        Arc::new(execution::ExecutionTrader::new(
            manifests::execution_trader(reviewed_at),
            desk.clone(),
        )),
        Arc::new(learning::LearningAttribution::new(
            manifests::learning_attribution(reviewed_at),
            desk.clone(),
        )),
        Arc::new(control::ComplianceControl::new(
            manifests::compliance_control(reviewed_at),
            desk.clone(),
        )),
        // Appended rather than placed beside the reviewer, so the run ids
        // the agents above were already given keep their sequence.
        Arc::new(poisoning::PoisoningDetector::new(
            manifests::poisoning_detector(reviewed_at),
            desk.clone(),
        )),
        Arc::new(adversary::SelfExploitationProbe::new(
            manifests::self_exploitation_probe(reviewed_at),
            desk.clone(),
        )),
        Arc::new(adversary::RegimeBreakProbe::new(
            manifests::regime_break_probe(reviewed_at),
            desk.clone(),
        )),
        Arc::new(adversary::ManipulationProbe::new(
            manifests::manipulation_probe(reviewed_at),
            desk.clone(),
        )),
        Arc::new(adversary::ModelFailureProbe::new(
            manifests::model_failure_probe(reviewed_at),
            desk.clone(),
        )),
        Arc::new(portfolio_brain::PortfolioAssetAnalyst::new(
            manifests::portfolio_asset_brain(reviewed_at),
            desk.clone(),
        )),
        Arc::new(crypto_brain::CryptoOnChainAnalyst::new(
            manifests::crypto_onchain_brain(reviewed_at),
            desk.clone(),
        )),
        Arc::new(crate::fundamental_brain::CompanyFundamentalAnalyst::new(
            manifests::company_fundamental_brain(reviewed_at),
            desk.clone(),
        )),
        Arc::new(crate::world_model_brain::WorldModelArbitrationAnalyst::new(
            manifests::world_model_arbitration_brain(reviewed_at),
            desk,
        )),
    ]
}
