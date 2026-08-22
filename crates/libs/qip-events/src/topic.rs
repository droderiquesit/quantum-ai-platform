//! The event topic registry.
//!
//! Topics are a closed set, declared here. A new event type means adding a
//! variant, which forces every exhaustive match in the platform to acknowledge
//! it — the point being that a new event cannot be introduced without the
//! routing, documentation and observability for it being considered.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Broad grouping, used for routing between the Fast Brain and the Deep Brain
/// and for observability dashboards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicGroup {
    /// Market and reference data arriving from the outside world.
    Sense,
    /// Entity and world-model state changes.
    Understand,
    /// Signals, anomalies and opportunities.
    Discover,
    /// Hypotheses, evidence and adversarial review.
    Reason,
    /// Simulation runs and their results.
    Simulate,
    /// Portfolio construction, optimisation and risk decisions.
    Decide,
    /// Orders, executions and positions.
    Act,
    /// Attribution, evaluation and lessons.
    Learn,
    /// Platform lifecycle, health and control.
    System,
}

impl TopicGroup {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sense => "sense",
            Self::Understand => "understand",
            Self::Discover => "discover",
            Self::Reason => "reason",
            Self::Simulate => "simulate",
            Self::Decide => "decide",
            Self::Act => "act",
            Self::Learn => "learn",
            Self::System => "system",
        }
    }

    /// Whether events in this group are on the latency-critical path.
    ///
    /// Fast Brain groups must never block on a Deep Brain component; the kernel
    /// enforces this when wiring handlers.
    pub fn is_latency_critical(&self) -> bool {
        matches!(self, Self::Sense | Self::Act)
    }
}

/// Every event topic in the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    // --- SENSE ---
    MarketTick,
    MarketQuote,
    MarketTrade,
    MarketOrderBook,
    MarketBar,
    MarketCorporateAction,
    FundamentalUpdated,
    MacroUpdated,
    NewsReceived,
    AlternativeDataReceived,
    ReferenceDataUpdated,
    DataQualityFailed,

    // --- UNDERSTAND ---
    EntityUpdated,
    EntityResolved,
    RelationshipUpdated,
    WorldModelUpdated,
    FeatureComputed,

    // --- DISCOVER ---
    SignalGenerated,
    AnomalyDetected,
    RegimeChanged,
    OpportunityDetected,
    OpportunityRanked,

    // --- REASON ---
    InvestigationStarted,
    HypothesisCreated,
    EvidenceAttached,
    HypothesisChallenged,
    HypothesisApproved,
    HypothesisRejected,
    ThesisInvalidated,
    AgentRunCompleted,

    // --- SIMULATE ---
    SimulationStarted,
    SimulationCompleted,
    ScenarioEvaluated,
    StrategyCreated,

    // --- DECIDE ---
    OptimizationRequested,
    OptimizationCompleted,
    SolverBenchmarked,
    PortfolioProposed,
    RiskEvaluated,
    RiskApproved,
    RiskRejected,
    ComplianceEvaluated,

    // --- ACT ---
    OrderProposed,
    OrderApproved,
    OrderSubmitted,
    OrderAmended,
    OrderCancelled,
    OrderRejected,
    OrderFilled,
    PositionUpdated,
    PnlUpdated,
    ReconciliationCompleted,

    // --- LEARN ---
    OutcomeObserved,
    AttributionCompleted,
    HypothesisScored,
    ModelEvaluated,
    LearningCompleted,
    LessonRecorded,

    // --- SYSTEM ---
    ServiceStarted,
    ServiceStopped,
    KillSwitchEngaged,
    KillSwitchReleased,
    AutonomyLevelChanged,
    BudgetExhausted,
    SystemAlert,
}

impl Topic {
    /// Every topic, in declaration order. Used by the registry, the
    /// documentation-drift test and the observability bootstrap.
    pub const ALL: [Self; 65] = [
        Self::MarketTick,
        Self::MarketQuote,
        Self::MarketTrade,
        Self::MarketOrderBook,
        Self::MarketBar,
        Self::MarketCorporateAction,
        Self::FundamentalUpdated,
        Self::MacroUpdated,
        Self::NewsReceived,
        Self::AlternativeDataReceived,
        Self::ReferenceDataUpdated,
        Self::DataQualityFailed,
        Self::EntityUpdated,
        Self::EntityResolved,
        Self::RelationshipUpdated,
        Self::WorldModelUpdated,
        Self::FeatureComputed,
        Self::SignalGenerated,
        Self::AnomalyDetected,
        Self::RegimeChanged,
        Self::OpportunityDetected,
        Self::OpportunityRanked,
        Self::InvestigationStarted,
        Self::HypothesisCreated,
        Self::EvidenceAttached,
        Self::HypothesisChallenged,
        Self::HypothesisApproved,
        Self::HypothesisRejected,
        Self::ThesisInvalidated,
        Self::AgentRunCompleted,
        Self::SimulationStarted,
        Self::SimulationCompleted,
        Self::ScenarioEvaluated,
        Self::StrategyCreated,
        Self::OptimizationRequested,
        Self::OptimizationCompleted,
        Self::SolverBenchmarked,
        Self::PortfolioProposed,
        Self::RiskEvaluated,
        Self::RiskApproved,
        Self::RiskRejected,
        Self::ComplianceEvaluated,
        Self::OrderProposed,
        Self::OrderApproved,
        Self::OrderSubmitted,
        Self::OrderAmended,
        Self::OrderCancelled,
        Self::OrderRejected,
        Self::OrderFilled,
        Self::PositionUpdated,
        Self::PnlUpdated,
        Self::ReconciliationCompleted,
        Self::OutcomeObserved,
        Self::AttributionCompleted,
        Self::HypothesisScored,
        Self::ModelEvaluated,
        Self::LearningCompleted,
        Self::LessonRecorded,
        Self::ServiceStarted,
        Self::ServiceStopped,
        Self::KillSwitchEngaged,
        Self::KillSwitchReleased,
        Self::AutonomyLevelChanged,
        Self::BudgetExhausted,
        Self::SystemAlert,
    ];

    /// The wire name, e.g. `market.tick`. Stable across releases — changing one
    /// is a breaking contract change.
    pub fn name(&self) -> &'static str {
        match self {
            Self::MarketTick => "market.tick",
            Self::MarketQuote => "market.quote",
            Self::MarketTrade => "market.trade",
            Self::MarketOrderBook => "market.orderbook",
            Self::MarketBar => "market.bar",
            Self::MarketCorporateAction => "market.corporate_action",
            Self::FundamentalUpdated => "fundamental.updated",
            Self::MacroUpdated => "macro.updated",
            Self::NewsReceived => "news.received",
            Self::AlternativeDataReceived => "altdata.received",
            Self::ReferenceDataUpdated => "reference.updated",
            Self::DataQualityFailed => "data.quality_failed",
            Self::EntityUpdated => "entity.updated",
            Self::EntityResolved => "entity.resolved",
            Self::RelationshipUpdated => "relationship.updated",
            Self::WorldModelUpdated => "world_model.updated",
            Self::FeatureComputed => "feature.computed",
            Self::SignalGenerated => "signal.generated",
            Self::AnomalyDetected => "anomaly.detected",
            Self::RegimeChanged => "regime.changed",
            Self::OpportunityDetected => "opportunity.detected",
            Self::OpportunityRanked => "opportunity.ranked",
            Self::InvestigationStarted => "investigation.started",
            Self::HypothesisCreated => "hypothesis.created",
            Self::EvidenceAttached => "evidence.attached",
            Self::HypothesisChallenged => "hypothesis.challenged",
            Self::HypothesisApproved => "hypothesis.approved",
            Self::HypothesisRejected => "hypothesis.rejected",
            Self::ThesisInvalidated => "thesis.invalidated",
            Self::AgentRunCompleted => "agent.run_completed",
            Self::SimulationStarted => "simulation.started",
            Self::SimulationCompleted => "simulation.completed",
            Self::ScenarioEvaluated => "scenario.evaluated",
            Self::StrategyCreated => "strategy.created",
            Self::OptimizationRequested => "optimization.requested",
            Self::OptimizationCompleted => "optimization.completed",
            Self::SolverBenchmarked => "optimization.benchmarked",
            Self::PortfolioProposed => "portfolio.proposed",
            Self::RiskEvaluated => "risk.evaluated",
            Self::RiskApproved => "risk.approved",
            Self::RiskRejected => "risk.rejected",
            Self::ComplianceEvaluated => "compliance.evaluated",
            Self::OrderProposed => "order.proposed",
            Self::OrderApproved => "order.approved",
            Self::OrderSubmitted => "order.submitted",
            Self::OrderAmended => "order.amended",
            Self::OrderCancelled => "order.cancelled",
            Self::OrderRejected => "order.rejected",
            Self::OrderFilled => "order.filled",
            Self::PositionUpdated => "position.updated",
            Self::PnlUpdated => "pnl.updated",
            Self::ReconciliationCompleted => "reconciliation.completed",
            Self::OutcomeObserved => "outcome.observed",
            Self::AttributionCompleted => "attribution.completed",
            Self::HypothesisScored => "hypothesis.scored",
            Self::ModelEvaluated => "model.evaluated",
            Self::LearningCompleted => "learning.completed",
            Self::LessonRecorded => "lesson.recorded",
            Self::ServiceStarted => "system.service_started",
            Self::ServiceStopped => "system.service_stopped",
            Self::KillSwitchEngaged => "system.kill_switch_engaged",
            Self::KillSwitchReleased => "system.kill_switch_released",
            Self::AutonomyLevelChanged => "system.autonomy_changed",
            Self::BudgetExhausted => "system.budget_exhausted",
            Self::SystemAlert => "system.alert",
        }
    }

    /// Parse a wire name back to a topic.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.name() == name)
    }

    pub fn group(&self) -> TopicGroup {
        match self {
            Self::MarketTick
            | Self::MarketQuote
            | Self::MarketTrade
            | Self::MarketOrderBook
            | Self::MarketBar
            | Self::MarketCorporateAction
            | Self::FundamentalUpdated
            | Self::MacroUpdated
            | Self::NewsReceived
            | Self::AlternativeDataReceived
            | Self::ReferenceDataUpdated
            | Self::DataQualityFailed => TopicGroup::Sense,

            Self::EntityUpdated
            | Self::EntityResolved
            | Self::RelationshipUpdated
            | Self::WorldModelUpdated
            | Self::FeatureComputed => TopicGroup::Understand,

            Self::SignalGenerated
            | Self::AnomalyDetected
            | Self::RegimeChanged
            | Self::OpportunityDetected
            | Self::OpportunityRanked => TopicGroup::Discover,

            Self::InvestigationStarted
            | Self::HypothesisCreated
            | Self::EvidenceAttached
            | Self::HypothesisChallenged
            | Self::HypothesisApproved
            | Self::HypothesisRejected
            | Self::ThesisInvalidated
            | Self::AgentRunCompleted => TopicGroup::Reason,

            Self::SimulationStarted
            | Self::SimulationCompleted
            | Self::ScenarioEvaluated
            | Self::StrategyCreated => TopicGroup::Simulate,

            Self::OptimizationRequested
            | Self::OptimizationCompleted
            | Self::SolverBenchmarked
            | Self::PortfolioProposed
            | Self::RiskEvaluated
            | Self::RiskApproved
            | Self::RiskRejected
            | Self::ComplianceEvaluated => TopicGroup::Decide,

            Self::OrderProposed
            | Self::OrderApproved
            | Self::OrderSubmitted
            | Self::OrderAmended
            | Self::OrderCancelled
            | Self::OrderRejected
            | Self::OrderFilled
            | Self::PositionUpdated
            | Self::PnlUpdated
            | Self::ReconciliationCompleted => TopicGroup::Act,

            Self::OutcomeObserved
            | Self::AttributionCompleted
            | Self::HypothesisScored
            | Self::ModelEvaluated
            | Self::LearningCompleted
            | Self::LessonRecorded => TopicGroup::Learn,

            Self::ServiceStarted
            | Self::ServiceStopped
            | Self::KillSwitchEngaged
            | Self::KillSwitchReleased
            | Self::AutonomyLevelChanged
            | Self::BudgetExhausted
            | Self::SystemAlert => TopicGroup::System,
        }
    }

    /// Whether losing an event on this topic is acceptable.
    ///
    /// Market ticks are replaceable — the next one arrives in milliseconds.
    /// An order fill is not: losing one corrupts the position record.
    pub fn is_lossy_tolerable(&self) -> bool {
        matches!(
            self,
            Self::MarketTick | Self::MarketQuote | Self::MarketOrderBook | Self::FeatureComputed
        )
    }

    /// Whether the topic must be retained indefinitely for audit.
    pub fn requires_permanent_retention(&self) -> bool {
        matches!(
            self.group(),
            TopicGroup::Reason | TopicGroup::Decide | TopicGroup::Act | TopicGroup::Learn
        ) || matches!(self, Self::KillSwitchEngaged | Self::AutonomyLevelChanged)
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
