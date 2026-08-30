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
use qip_agents::runtime::{Agent, AgentContext, AgentHost, AgentRunRecord, RunStatus};
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
        Ok(Self {
            roster,
            agents,
            host,
            audit: AuditTrail::new(),
            sequence: 0,
        })
    }

    /// The standard eighteen-agent organisation.
    pub fn standard(
        desk: Arc<Desk>,
        reviewed_at: Timestamp,
        now: Timestamp,
        seed: u64,
        language_model: Option<Arc<dyn LanguageModel>>,
        licensed_datasets: Vec<String>,
        quantum_backend_available: bool,
    ) -> Result<Self> {
        use crate::{
            analysts, construction, control, execution, fast, learning, reasoning, review,
        };

        let mut host = AgentHost::new(seed);
        if let Some(model) = language_model {
            host = host.with_language_model(model);
        }

        let agents: Vec<Arc<dyn Agent>> = vec![
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
                desk,
            )),
        ];

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
                    for missing in &finding.missing_inputs {
                        missing_inputs.push(format!("{}: {missing}", finding.agent_id));
                    }
                    match finding.status {
                        FindingStatus::Deferred => deferred.push(finding.agent_id.clone()),
                        _ if finding.status.is_informative() => findings.push(finding.clone()),
                        _ => {}
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
        }
    }
}
