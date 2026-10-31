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

/// A finding [`OrganisationReport::contests_the_thesis`] does not count as
/// dissent: risk control's limit report, and an adversarial review whose
/// every objection is that the book already holds the subject.
fn is_procedural_objection(finding: &AgentFinding) -> bool {
    if finding.agent_id == crate::manifests::ids::RISK {
        return true;
    }
    if finding.agent_id != crate::manifests::ids::ADVERSARIAL {
        return false;
    }
    let fact = |label: &str| {
        finding
            .facts
            .iter()
            .find(|fact| fact.label == label)
            .map(|fact| fact.value)
    };
    match (
        fact("objection_count"),
        fact(crate::review::HOLDING_OBJECTION_COUNT),
    ) {
        (Some(all), Some(holding)) => all > 0.0 && (all - holding).abs() < f64::EPSILON,
        _ => false,
    }
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

    /// Whether the panel disputes the thesis itself (ADR 0238, owner
    /// decision 20): a finding on each side, counting only dissent about
    /// the thesis's evidence or direction.
    ///
    /// Since ADR 0238 a contested thesis is unresolved and never sized, so
    /// what counts as contest decides what the platform may trade. Two
    /// objections are deliberately not counted, because neither is a view on
    /// the thesis:
    ///
    /// - **Risk control reporting a blocking limit.** Its direction is
    ///   negative exactly when a limit blocks, and that limit still refuses
    ///   at its own gate. Counting it again here would stop every thesis on
    ///   a limited book in REASON, including the reduce-only exit and every
    ///   hedge, which is what the limit exists to let through.
    /// - **The adversarial reviewer's re-argument objection**, raised only
    ///   because the book already holds the subject. Counting it would mean
    ///   never adding to, hedging or planning the exit of a held position.
    ///   Any other objection the reviewer raises still counts: a finding
    ///   whose objections are not all [`HOLDING_OBJECTION`]s is dissent.
    ///
    /// [`Self::is_contested`] is the raw two-sided count and is kept for
    /// the records that report it.
    ///
    /// [`HOLDING_OBJECTION`]: crate::review::HOLDING_OBJECTION
    pub fn contests_the_thesis(&self) -> bool {
        let (positive, negative) = self.directional();
        positive.iter().any(|f| !is_procedural_objection(f))
            && negative.iter().any(|f| !is_procedural_objection(f))
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

/// The assembled organisation: roster, agents, host and audit trail.
#[derive(Debug)]
pub struct Organisation {
    roster: Roster,
    agents: Vec<Arc<dyn Agent>>,
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
            host,
            audit: AuditTrail::new(),
            sequence: 0,
            backlog,
        })
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
            desk,
            reviewed_at,
            licensed_datasets,
            quantum_backend_available,
        );
        Self::new(manifests::roster(reviewed_at), agents, host, now)
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub fn audit(&self) -> &AuditTrail {
        &self.audit
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
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
        let mut runs = Vec::new();
        let mut findings = Vec::new();
        let mut deferred = Vec::new();
        let mut failed = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut backlogged = Vec::new();
        let mut backlog_refused = Vec::new();

        for agent in &self.agents {
            if !agent.accepts(brief) {
                deferred.push(agent.manifest().id.clone());
                continue;
            }
            self.sequence += 1;
            let run_id =
                AgentRunId::from_string(format!("run-{}-{}", agent.manifest().id, self.sequence));
            let record = self.host.run(
                agent.as_ref(),
                brief,
                now,
                lineage.relabel(agent.manifest().id.clone()),
                run_id,
            );

            match (&record.status, &record.finding) {
                (RunStatus::Succeeded, Some(finding)) => {
                    // The agent ran: whatever the backlog held for it is done
                    // with, whether or not this finding is informative.
                    self.backlog.retire(&finding.agent_id);
                    for missing in &finding.missing_inputs {
                        missing_inputs.push(format!("{}: {missing}", finding.agent_id));
                    }
                    match finding.status {
                        FindingStatus::Deferred => deferred.push(finding.agent_id.clone()),
                        _ if finding.status.is_informative() => findings.push(finding.clone()),
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
                    failed.push(id.clone());
                    match self.backlog.defer(&id, brief, reason.as_str(), now) {
                        Deferral::Queued | Deferral::Repeated { .. } => backlogged.push(id),
                        Deferral::Refused { reason } => backlog_refused.push(reason),
                    }
                }
                (RunStatus::Escalated { policy, to }, _) => {
                    let id = agent.manifest().id.clone();
                    failed.push(id.clone());
                    let reason = match to {
                        Some(to) => format!("budget exhausted; escalated to {to} under {policy:?}"),
                        None => {
                            format!("budget exhausted; escalation policy {policy:?} names nobody")
                        }
                    };
                    match self.backlog.defer(&id, brief, reason, now) {
                        Deferral::Queued | Deferral::Repeated { .. } => backlogged.push(id),
                        Deferral::Refused { reason } => backlog_refused.push(reason),
                    }
                }
                _ => failed.push(agent.manifest().id.clone()),
            }
            self.audit.record(record.clone());
            runs.push(record);
        }

        OrganisationReport {
            as_of: brief.as_of,
            question: brief.question.clone(),
            runs,
            findings,
            deferred,
            failed,
            missing_inputs,
            backlogged,
            backlog_refused,
        }
    }
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
        adversary, analysts, construction, control, execution, fast, learning, poisoning,
        portfolio_brain, reasoning, review,
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
            desk,
        )),
    ]
}
#[cfg(test)]
mod contest_tests {
    //! ADR 0238 as owner decision 20 reads it: what counts as a panel
    //! contesting the thesis. A contested thesis is unresolved and never
    //! sized, so a finding counted here that is not dissent about the thesis
    //! stops trades the platform's own controls exist to let through.

    use super::*;
    use qip_agents::finding::{Direction, NumericFact};
    use qip_core::time::Timestamp;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn finding(agent: &str, direction: Direction, facts: &[(&str, f64)]) -> AgentFinding {
        let mut finding = AgentFinding::new(
            AgentRunId::from_string(format!("run-{agent}")),
            agent,
            at(),
            at(),
            format!("{agent} on the thesis"),
        );
        finding.direction = direction;
        finding.conviction = 0.5;
        finding.facts = facts
            .iter()
            .map(|(label, value)| NumericFact::computed(*label, *value, "count", agent, Vec::new()))
            .collect();
        finding
    }

    fn report(findings: Vec<AgentFinding>) -> OrganisationReport {
        OrganisationReport {
            as_of: at(),
            question: "is AAA undervalued".to_string(),
            runs: Vec::new(),
            findings,
            deferred: Vec::new(),
            failed: Vec::new(),
            missing_inputs: Vec::new(),
            backlogged: Vec::new(),
            backlog_refused: Vec::new(),
        }
    }

    fn supporter() -> AgentFinding {
        finding(manifests::ids::EQUITY, Direction::Positive, &[])
    }

    /// Exclusion (a). The failure prevented: a book under a blocking limit
    /// had every thesis on it — the reduce-only exit and every hedge
    /// included — stopped in REASON by risk control's limit report, a
    /// refusal the limit already makes at its own gate. Mutation: drop the
    /// `ids::RISK` arm of `is_procedural_objection` and this fails.
    #[test]
    fn a_risk_control_limit_report_alone_does_not_contest_the_thesis() {
        let panel = report(vec![
            supporter(),
            finding(
                manifests::ids::RISK,
                Direction::Negative,
                &[("blocking_breach_count", 1.0)],
            ),
        ]);
        assert!(
            panel.is_contested(),
            "premise: the panel has a finding on each side"
        );
        assert!(
            !panel.contests_the_thesis(),
            "a limit report was counted as dissent about the thesis"
        );
    }

    /// Exclusion (b), and its edge. The failure prevented: a thesis on a
    /// position the book already holds was always contested by the
    /// reviewer's re-argument objection, so the platform could never add
    /// to, hedge or plan the exit of a held position. Any other objection
    /// beside it is still dissent. Mutation: drop the reviewer arm of
    /// `is_procedural_objection` and the first assertion fails; count the
    /// reviewer out whatever it objects to and the second fails.
    #[test]
    fn a_reviewer_objecting_only_that_the_book_holds_the_name_does_not_contest_the_thesis() {
        let holding_only = report(vec![
            supporter(),
            finding(
                manifests::ids::ADVERSARIAL,
                Direction::Negative,
                &[
                    ("objection_count", 1.0),
                    (crate::review::HOLDING_OBJECTION_COUNT, 1.0),
                ],
            ),
        ]);
        assert!(
            holding_only.is_contested(),
            "premise: one finding each side"
        );
        assert!(
            !holding_only.contests_the_thesis(),
            "the re-argument objection alone was counted as dissent"
        );

        let with_another_objection = report(vec![
            supporter(),
            finding(
                manifests::ids::ADVERSARIAL,
                Direction::Negative,
                &[
                    ("objection_count", 2.0),
                    (crate::review::HOLDING_OBJECTION_COUNT, 1.0),
                ],
            ),
        ]);
        assert!(
            with_another_objection.contests_the_thesis(),
            "an objection about the thesis itself was dropped with the holding one"
        );
    }

    /// The rule the exclusions narrow, held in place: a genuine finding
    /// against the thesis contests it, whatever procedural objections sit
    /// beside it. The kernel half — that such a panel's thesis is
    /// unresolved and forms no proposal — is
    /// `qip-kernel/tests/symbolic_federation.rs`'s
    /// `a_cycle_whose_panel_is_contested_forms_no_proposal_and_journals_its_routing_as_unresolved`,
    /// whose dissent is the simulation analyst's. Mutation: make
    /// `contests_the_thesis` ignore every negative finding and this fails.
    #[test]
    fn a_genuine_opposing_finding_still_contests_the_thesis() {
        let panel = report(vec![
            supporter(),
            finding(manifests::ids::SIMULATION, Direction::Negative, &[]),
            finding(
                manifests::ids::RISK,
                Direction::Negative,
                &[("blocking_breach_count", 1.0)],
            ),
            finding(
                manifests::ids::ADVERSARIAL,
                Direction::Negative,
                &[
                    ("objection_count", 1.0),
                    (crate::review::HOLDING_OBJECTION_COUNT, 1.0),
                ],
            ),
        ]);
        assert!(
            panel.contests_the_thesis(),
            "an analyst's finding against the thesis was not counted as dissent"
        );
        let unopposed = report(vec![supporter()]);
        assert!(
            !unopposed.contests_the_thesis(),
            "premise: one side alone is no contest"
        );
    }
}
