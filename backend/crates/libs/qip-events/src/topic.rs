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
    /// The kernel referenced what a connector poll or a research campaign
    /// fetched — the reference ledger's own record, so a restart rebuilds
    /// the ledger from the log (ADR 0057). A Sense-group fact about a fetch,
    /// evictable like the observations it describes; the revision a
    /// reference may reveal is the permanent record, under
    /// [`Topic::SourceRevisionDetected`].
    DataReferenceRecorded,

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
    /// The centre distributed a signed policy payload to a region — the
    /// twelve-item shipment of blueprint §41.5. Policy is a decision about
    /// what a region may do, so it sits in the Decide group and is retained
    /// permanently with the rest of that group.
    PolicyDistributed,
    RiskRejected,
    ComplianceEvaluated,
    /// The platform proposed, withdrew, enacted or refused a loosening of
    /// one risk limit's bound on counterfactual regret evidence (blueprint
    /// §12.3, first row). A decision about what a control may permit, so it
    /// sits in the Decide group and is retained permanently: the proposal
    /// is generated from evidence and can be supplied by nobody, and an
    /// enactment is an artefact two people signed for, never a mutation of
    /// the running set. See ADR 0061.
    RiskRuleRecalibration,
    /// The platform withdrew a venue on feasibility evidence (blueprint
    /// §12.3, fourth row): a cluster of feasibility refusals at one venue,
    /// and the venue omitted from the desk's order path and the cells'
    /// whitelist. A decision about what the platform may do, so Decide and
    /// retained permanently; a subtraction and never an addition, and the
    /// record is what a restarted process resumes the set from. See ADR
    /// 0062.
    VenueWithdrawn,
    /// An operator signed to reinstate a withdrawn venue — a first
    /// signature awaiting its countersignature, the reinstatement itself,
    /// or a refusal. Two people, as a promotion needs, because putting a
    /// venue back is a person widening what the platform may do. See ADR
    /// 0062.
    VenueReinstated,

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
    /// A source was found to have revised an extent this platform had
    /// already used (ADR 0057). Its own topic rather than
    /// `DataQualityFailed`, which is not retained permanently and which the
    /// ingestion suites already fill with another body: a revision is what
    /// flags a backtest, and a flag the log may have evicted is a replay
    /// that re-derives nothing.
    SourceRevisionDetected,
    /// A research campaign closed and its manifest is on the record — what
    /// a fit used, retained for as long as the log is. Its own topic rather
    /// than `LearningCompleted`, whose every frame the kernel decodes as a
    /// cycle entry.
    ResearchCampaignClosed,
    /// A closed campaign was found, after the fact, to have used an extent
    /// the source has since revised — §22.3's "the backtest that used the
    /// original is flagged", as a record naming the campaign.
    ResearchCampaignFlagged,
    /// A rule whose declined paths were mostly correctly declined, with the
    /// simulated loss it avoided (§12.3, second row). A finding about a
    /// control earning its place, and the mirror of a recalibration
    /// proposal: it changes nothing and exists so that a rule's record can
    /// say more than "fired".
    RiskRuleDefended,
    /// A rule that has not fired for a stated number of cycles while a
    /// stated number of orders were submitted (§12.3, third row). A finding,
    /// not a removal: a control that reads as protection and never binds is
    /// the defect this repository records under `MaxExpectedShortfall`, and
    /// the record is what lets a person ask whether it can still fire.
    RiskRuleDormant,

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
    pub const ALL: [Self; 75] = [
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
        Self::DataReferenceRecorded,
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
        Self::PolicyDistributed,
        Self::RiskRejected,
        Self::ComplianceEvaluated,
        Self::RiskRuleRecalibration,
        Self::VenueWithdrawn,
        Self::VenueReinstated,
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
        Self::SourceRevisionDetected,
        Self::ResearchCampaignClosed,
        Self::ResearchCampaignFlagged,
        Self::RiskRuleDefended,
        Self::RiskRuleDormant,
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
            Self::DataReferenceRecorded => "data.reference_recorded",
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
            Self::PolicyDistributed => "policy.distributed",
            Self::RiskRejected => "risk.rejected",
            Self::ComplianceEvaluated => "compliance.evaluated",
            Self::RiskRuleRecalibration => "risk.rule_recalibration",
            Self::VenueWithdrawn => "venue.withdrawn",
            Self::VenueReinstated => "venue.reinstated",
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
            Self::SourceRevisionDetected => "learning.source_revised",
            Self::ResearchCampaignClosed => "learning.campaign_closed",
            Self::ResearchCampaignFlagged => "learning.campaign_flagged",
            Self::RiskRuleDefended => "risk.rule_defended",
            Self::RiskRuleDormant => "risk.rule_dormant",
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
            | Self::DataQualityFailed
            | Self::DataReferenceRecorded => TopicGroup::Sense,

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
            | Self::PolicyDistributed
            | Self::RiskRejected
            | Self::ComplianceEvaluated
            | Self::RiskRuleRecalibration
            | Self::VenueWithdrawn
            | Self::VenueReinstated => TopicGroup::Decide,

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
            | Self::LessonRecorded
            | Self::SourceRevisionDetected
            | Self::ResearchCampaignClosed
            | Self::ResearchCampaignFlagged
            | Self::RiskRuleDefended
            | Self::RiskRuleDormant => TopicGroup::Learn,

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
