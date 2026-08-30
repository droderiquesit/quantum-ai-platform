//! The declared organisation.
//!
//! Eighteen manifests, written out rather than generated, because each one is
//! a decision about what a component of the platform is allowed to do and
//! those decisions should be readable side by side.
//!
//! The shape of the organisation is the point:
//!
//! * Seven research analysts and a causal analyst read and publish. None can
//!   reach the market.
//! * One microstructure analyst runs on the fast path with no language model
//!   at all and a fifty-millisecond budget.
//! * One adversarial reviewer challenges but cannot publish.
//! * One construction agent proposes; it cannot approve or veto.
//! * Two control agents — risk and compliance — hold the veto. Risk also holds
//!   the kill switch.
//! * One execution agent reaches venues. It cannot originate a thesis.
//! * One learning agent scores what happened.
//! * One coordinator dispatches and aggregates. It decides nothing.
//!
//! [`roster`] assembles them, and `Roster::validate` is run in the tests: the
//! organisation as declared has to pass its own governance rules.

use qip_agents::budget::Budget;
use qip_agents::capability::{Capability, CapabilitySet};
use qip_agents::governance::Roster;
use qip_agents::manifest::{AgentManifest, AgentRole};
use qip_core::time::{Duration, Timestamp};

/// Stable ids, so a decision record written today still resolves in a year.
pub mod ids {
    pub const CHIEF: &str = "chief-investment-intelligence";
    pub const MACRO: &str = "macro-analyst";
    pub const EQUITY: &str = "equity-analyst";
    pub const CREDIT: &str = "credit-analyst";
    pub const MICROSTRUCTURE: &str = "microstructure-analyst";
    pub const DERIVATIVES: &str = "derivatives-analyst";
    pub const COMMODITIES: &str = "commodities-analyst";
    pub const FX_RATES: &str = "fx-rates-analyst";
    pub const ALT_DATA: &str = "alternative-data-analyst";
    pub const CAUSAL: &str = "causal-analyst";
    pub const ADVERSARIAL: &str = "adversarial-reviewer";
    pub const SIMULATION: &str = "simulation-analyst";
    pub const CONSTRUCTION: &str = "portfolio-construction";
    pub const QUANTUM: &str = "quantum-optimization";
    pub const RISK: &str = "risk-control";
    pub const EXECUTION: &str = "execution-trader";
    pub const LEARNING: &str = "learning-attribution";
    pub const COMPLIANCE: &str = "compliance-control";
}

/// Owning teams. Used by the governance rules to check independence, so these
/// are not decorative: changing one can make the roster fail validation.
pub mod owners {
    pub const RESEARCH: &str = "investment-research";
    pub const QUANTITATIVE: &str = "quantitative-research";
    pub const REVIEW: &str = "independent-review";
    pub const PORTFOLIO: &str = "portfolio-management";
    pub const RISK: &str = "risk-management";
    pub const COMPLIANCE: &str = "compliance";
    pub const TRADING: &str = "trading";
    pub const PLATFORM: &str = "platform-engineering";
}

/// Read grants an analyst needs to look at market data and the world model.
fn analyst_reads() -> CapabilitySet {
    CapabilitySet::of([
        Capability::ReadMarketData,
        Capability::ReadReferenceData,
        Capability::ReadWorldModel,
        Capability::ReadResearchMemory,
    ])
}

fn research(
    id: &str,
    name: &str,
    purpose: &str,
    owner: &str,
    reviewed_at: Timestamp,
    extra: &[Capability],
) -> AgentManifest {
    let mut capabilities = analyst_reads().with(Capability::PublishHypothesis);
    for capability in extra {
        capabilities = capabilities.with(*capability);
    }
    AgentManifest::research(id, name, purpose, reviewed_at)
        .with_capabilities(capabilities)
        .with_owner(owner)
        .escalating_to(ids::CHIEF)
}

pub fn chief(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::CHIEF,
        "Chief Investment Intelligence",
        "dispatches questions to the specialists, aggregates what comes back, and reports coverage and dissent without adding a view of its own",
        reviewed_at,
    )
    .with_role(AgentRole::Coordination)
    .with_capabilities(
        analyst_reads()
            .with(Capability::ReadPortfolio)
            .with(Capability::ReadRiskState),
    )
    .with_competencies(vec![
        "deciding which specialists a question belongs to".into(),
        "reporting where the organisation agrees, disagrees and has no coverage".into(),
    ])
    .with_limitations(vec![
        "forms no investment view of its own; an aggregate is not an opinion".into(),
        "cannot publish a hypothesis, propose a trade, approve or veto".into(),
    ])
    .with_owner(owners::PLATFORM)
    .requiring_human()
}

pub fn macro_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::MACRO,
        "Macro Analyst",
        "reads policy rates, inflation, growth and liquidity to say what the macro environment implies for an asset",
        owners::RESEARCH,
        reviewed_at,
        &[Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "policy rate path and its transmission".into(),
        "inflation and growth surprises against expectations".into(),
        "cross-asset regime identification".into(),
    ])
    .with_limitations(vec![
        "no view on single-name idiosyncratic risk".into(),
        "macro features are published with a lag; intraday questions are outside scope".into(),
    ])
}

pub fn equity_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::EQUITY,
        "Equity Analyst",
        "assesses a listed equity from its price history, fundamentals and sector context",
        owners::RESEARCH,
        reviewed_at,
        &[Capability::ReadFundamentals, Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "trend, mean reversion and realised volatility from price history".into(),
        "valuation against sector peers".into(),
    ])
    .with_limitations(vec![
        "no view on instruments outside listed equity".into(),
        "cannot see private fundamentals or management guidance".into(),
    ])
}

pub fn credit_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::CREDIT,
        "Credit Analyst",
        "assesses credit spreads, their history and what a move in them does to a bond's price",
        owners::RESEARCH,
        reviewed_at,
        &[Capability::ReadFundamentals, Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "spread level and change against its own history".into(),
        "duration-weighted price impact of a spread move".into(),
        "rating migration risk".into(),
    ])
    .with_limitations(vec![
        "no view on equity of the same issuer beyond the capital-structure link".into(),
        "loan-level and structured-credit analysis is outside scope".into(),
    ])
}

pub fn microstructure_analyst(reviewed_at: Timestamp) -> AgentManifest {
    // The fast path. No language model, no world model, fifty milliseconds.
    // The budget is the enforcement: a fast-path agent that acquired a model
    // call would block the loop it exists to keep unblocked.
    AgentManifest::research(
        ids::MICROSTRUCTURE,
        "Microstructure Analyst",
        "reads the order book and recent tape for liquidity, imbalance and the cost of trading now",
        reviewed_at,
    )
    .with_capabilities(CapabilitySet::of([
        Capability::ReadMarketData,
        Capability::ReadReferenceData,
        Capability::PublishHypothesis,
    ]))
    .with_budget(Budget::fast_path())
    .with_competencies(vec![
        "quoted and effective spread, depth and imbalance".into(),
        "the cost of executing a given size now".into(),
    ])
    .with_limitations(vec![
        "no language model and no world model: this agent runs on the fast path".into(),
        "says nothing about whether a trade is a good idea, only what it would cost".into(),
    ])
    .with_owner(owners::QUANTITATIVE)
    .escalating_to(ids::CHIEF)
}

pub fn derivatives_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::DERIVATIVES,
        "Derivatives Analyst",
        "compares implied to realised volatility and reads the term structure for what options are pricing",
        owners::QUANTITATIVE,
        reviewed_at,
        &[Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "variance risk premium from implied against realised volatility".into(),
        "term structure and skew".into(),
    ])
    .with_limitations(vec!["no view on the underlying's direction".into()])
}

pub fn commodities_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::COMMODITIES,
        "Commodities Analyst",
        "reads futures curve shape, carry and roll yield across the commodity complex",
        owners::RESEARCH,
        reviewed_at,
        &[Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "contango and backwardation, and the roll yield they imply".into(),
        "inventory and seasonality effects where the data exists".into(),
    ])
    .with_limitations(vec![
        "physical logistics and freight are outside scope".into(),
    ])
}

pub fn fx_rates_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::FX_RATES,
        "FX and Rates Analyst",
        "reads rate differentials, curve slope and carry across currencies and the rates complex",
        owners::RESEARCH,
        reviewed_at,
        &[Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "carry from rate differentials, and its volatility adjustment".into(),
        "curve slope and its implications".into(),
    ])
    .with_limitations(vec![
        "central bank intent is inferred from prices, never asserted".into(),
    ])
}

pub fn alternative_data_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::ALT_DATA,
        "Alternative Data Analyst",
        "reads licensed alternative datasets for signal, and refuses to use data whose licence does not permit the use",
        owners::QUANTITATIVE,
        reviewed_at,
        &[
            Capability::ReadAlternativeData,
            Capability::CallLanguageModel,
        ],
    )
    .with_competencies(vec![
        "standardised alternative-data features against their own history".into(),
        "checking a dataset's licence permits the intended use".into(),
    ])
    .with_limitations(vec![
        "will not use synthetic or research-only data in a production decision".into(),
        "alternative data is noisy; findings are usually partial by construction".into(),
    ])
}

pub fn causal_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::CAUSAL,
        "Causal Analyst",
        "traces how a shock transmits through the world model's causal graph and reports the mechanism, not the correlation",
        owners::QUANTITATIVE,
        reviewed_at,
        &[Capability::CallLanguageModel],
    )
    .with_competencies(vec![
        "propagating a shock through evidenced causal edges".into(),
        "identifying the weakest link in a proposed mechanism".into(),
    ])
    .with_limitations(vec![
        "only knows the mechanisms the world model has recorded".into(),
        "an unevidenced causal edge is reported as such, never used as fact".into(),
    ])
}

pub fn adversarial_reviewer(reviewed_at: Timestamp) -> AgentManifest {
    // No PublishHypothesis: an adversary that can publish its own theses stops
    // being a check on the others and becomes one of them.
    AgentManifest::research(
        ids::ADVERSARIAL,
        "Adversarial Reviewer",
        "attacks the organisation's own conclusions, looking for the reason each one is wrong",
        reviewed_at,
    )
    .with_role(AgentRole::Adversarial)
    .with_capabilities(
        analyst_reads()
            .with(Capability::ReadPortfolio)
            .with(Capability::ChallengeHypothesis)
            .with(Capability::CallLanguageModel),
    )
    .with_competencies(vec![
        "finding the evidence a thesis rests on and asking what if it is wrong".into(),
        "checking whether the organisation has been wrong about this before".into(),
        "identifying what the market has already priced".into(),
    ])
    .with_limitations(vec![
        "publishes no thesis of its own, by design".into(),
        "cannot veto: a challenge informs the control function, it is not the control".into(),
    ])
    .with_owner(owners::REVIEW)
    .requiring_human()
}

pub fn simulation_analyst(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::SIMULATION,
        "Simulation Analyst",
        "runs the distribution of outcomes a thesis implies and reports the tail as well as the centre",
        owners::QUANTITATIVE,
        reviewed_at,
        &[Capability::RunSimulation, Capability::RunBacktest],
    )
    .with_budget(Budget::deep_research())
    .with_competencies(vec![
        "bootstrapped outcome distributions from historical returns".into(),
        "reporting the loss tail alongside the expected case".into(),
    ])
    .with_limitations(vec![
        "a simulation is only as good as the history behind it; regime change is not modelled by resampling".into(),
    ])
}

pub fn portfolio_construction(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::CONSTRUCTION,
        "Portfolio Construction",
        "turns approved theses into target weights that respect concentration and exposure limits",
        reviewed_at,
    )
    .with_role(AgentRole::Construction)
    .with_capabilities(
        analyst_reads()
            .with(Capability::ReadPortfolio)
            .with(Capability::ReadRiskState)
            .with(Capability::RunOptimization)
            .with(Capability::ProposeTrade),
    )
    .with_competencies(vec![
        "sizing positions by conviction subject to a concentration cap".into(),
        "computing the turnover a target implies".into(),
    ])
    .with_limitations(vec![
        "proposes only; it cannot approve, veto or submit".into(),
        "does not form investment views, it expresses ones already approved".into(),
    ])
    .with_owner(owners::PORTFOLIO)
    .escalating_to(ids::CHIEF)
}

pub fn quantum_optimization(reviewed_at: Timestamp) -> AgentManifest {
    research(
        ids::QUANTUM,
        "Quantum Optimization",
        "decides whether a portfolio problem is one a quantum method could help with, and always against a classical baseline",
        owners::QUANTITATIVE,
        reviewed_at,
        &[
            Capability::ReadPortfolio,
            Capability::RunOptimization,
            Capability::RunQuantumOptimization,
        ],
    )
    .with_competencies(vec![
        "judging whether a problem's size and structure suit an annealing formulation".into(),
        "requiring a classical baseline before any quantum result is used".into(),
    ])
    .with_limitations(vec![
        "claims no quantum advantage that has not been measured against the classical baseline".into(),
        "falls back to the classical solver whenever the quantum path is unavailable or worse".into(),
    ])
}

pub fn risk_control(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::RISK,
        "Risk Control",
        "evaluates the book against its limits and vetoes anything that breaches them",
        reviewed_at,
    )
    .with_role(AgentRole::Control)
    .with_capabilities(
        analyst_reads()
            .with(Capability::ReadPortfolio)
            .with(Capability::ReadRiskState)
            .with(Capability::ReadExecutionHistory)
            .with(Capability::VetoProposal)
            .with(Capability::TriggerKillSwitch),
    )
    .with_competencies(vec![
        "deterministic evaluation of the limit set against the current risk state".into(),
        "stating which limit blocks a proposal and by how much".into(),
    ])
    .with_limitations(vec![
        "cannot move a limit: overriding one is an authenticated operator action".into(),
        "does not form investment views; a veto is arithmetic, not judgement".into(),
    ])
    .with_owner(owners::RISK)
    .requiring_human()
}

pub fn compliance_control(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::COMPLIANCE,
        "Compliance Control",
        "checks proposals against restricted lists, jurisdiction rules and venue permissions",
        reviewed_at,
    )
    .with_role(AgentRole::Control)
    .with_capabilities(
        CapabilitySet::of([
            Capability::ReadReferenceData,
            Capability::ReadPortfolio,
            Capability::ReadExecutionHistory,
            Capability::ReadResearchMemory,
        ])
        .with(Capability::VetoProposal),
    )
    .with_competencies(vec![
        "deterministic restriction, jurisdiction and venue checks".into(),
    ])
    .with_limitations(vec![
        "no language model: a compliance decision that depends on how a sentence was phrased is not a compliance decision".into(),
        "escalates anything the rules do not cover rather than deciding it".into(),
    ])
    .with_owner(owners::COMPLIANCE)
    .requiring_human()
}

pub fn execution_trader(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::EXECUTION,
        "Execution Trader",
        "works approved orders, choosing schedule and venue to minimise cost against the trader's benchmark",
        reviewed_at,
    )
    .with_role(AgentRole::Execution)
    .with_capabilities(CapabilitySet::of([
        Capability::ReadMarketData,
        Capability::ReadReferenceData,
        Capability::ReadPortfolio,
        Capability::ReadExecutionHistory,
        Capability::SubmitOrder,
        Capability::CancelOrder,
    ]))
    .with_competencies(vec![
        "estimating market impact and participation for a given size".into(),
        "choosing between immediacy and impact".into(),
    ])
    .with_limitations(vec![
        "originates nothing: it works orders it is given".into(),
        "cannot publish a hypothesis, by design".into(),
    ])
    .with_owner(owners::TRADING)
    .requiring_human()
}

pub fn learning_attribution(reviewed_at: Timestamp) -> AgentManifest {
    AgentManifest::research(
        ids::LEARNING,
        "Learning and Attribution",
        "scores what the organisation predicted against what happened, and turns the difference into lessons",
        reviewed_at,
    )
    .with_role(AgentRole::Learning)
    .with_capabilities(
        analyst_reads()
            .with(Capability::ReadPortfolio)
            .with(Capability::ReadExecutionHistory)
            .with(Capability::WriteResearchMemory),
    )
    .with_competencies(vec![
        "hit rate and calibration by agent and by thesis class".into(),
        "promoting a lesson only once enough independent episodes support it".into(),
    ])
    .with_limitations(vec![
        "cannot score a thesis whose horizon has not passed".into(),
        "a lesson is recorded with its counter-examples, never without them".into(),
    ])
    .with_owner(owners::REVIEW)
    .escalating_to(ids::CHIEF)
}

/// The whole organisation.
pub fn roster(reviewed_at: Timestamp) -> Roster {
    Roster::from_manifests(vec![
        chief(reviewed_at),
        macro_analyst(reviewed_at),
        equity_analyst(reviewed_at),
        credit_analyst(reviewed_at),
        microstructure_analyst(reviewed_at),
        derivatives_analyst(reviewed_at),
        commodities_analyst(reviewed_at),
        fx_rates_analyst(reviewed_at),
        alternative_data_analyst(reviewed_at),
        causal_analyst(reviewed_at),
        adversarial_reviewer(reviewed_at),
        simulation_analyst(reviewed_at),
        portfolio_construction(reviewed_at),
        quantum_optimization(reviewed_at),
        risk_control(reviewed_at),
        execution_trader(reviewed_at),
        learning_attribution(reviewed_at),
        compliance_control(reviewed_at),
    ])
}

/// How long an authorisation lasts before it must be reviewed again.
pub const REVIEW_INTERVAL: Duration = Duration::from_days(90);
