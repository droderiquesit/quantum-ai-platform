//! The event topic registry.
//!
//! Topics are a closed set, declared here. A new event type means adding a
//! variant, which forces every exhaustive match in the platform to acknowledge
//! it — the point being that a new event cannot be introduced without the
//! routing, documentation and observability for it being considered.

use crate::retention::RetentionClass;
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
    /// The centre derived a region dark: heard from at least once, and
    /// from none of its cells within `CentralConfig::region_dark_after`
    /// (ADR 0079). The record of a derivation rather than a second source
    /// of truth — replaying the cell reports re-derives it — journaled so
    /// an operator can read when the centre stopped granting into the
    /// region and froze its share. Act, because the consequence is on the
    /// order path: nothing new enters the region while it is dark.
    RegionDark,
    /// A dark region's first report reached the centre and the derivation
    /// cleared. The other half of the pair above, so the two instants
    /// bracket exactly the window in which grants were refused.
    RegionLit,

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
    /// One instrument's sizing cap changed state on the twin's fill scores
    /// (blueprint §12.3, last row, executed-order half): armed when most of
    /// a sample of its fills would have done better smaller, released when
    /// the evidence stops clearing the bar, and the proposal — never a
    /// number — when most would have done better larger. Journaled when the
    /// state changes, not on every cycle it stands. A Learn finding,
    /// retained permanently. See ADR 0063.
    SizingReviewed,
    /// The funding standing of every strategy family the foundry has
    /// registered, measured against the deflation the holdout gate itself
    /// applied (blueprint §12.3, row five). Carries the per-family standing
    /// every cycle and, where an unfunded family's evidence stands clear of
    /// every funded family's by the review's margin, a misallocation
    /// finding. A measurement and a finding only: no weight, budget, grant
    /// or bound moves from either body, and nothing in the platform reads
    /// one. A Learn finding, retained permanently. See ADR 0064.
    FamilyAllocationReviewed,

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
    pub const ALL: [Self; 79] = [
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
        Self::RegionDark,
        Self::RegionLit,
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
        Self::SizingReviewed,
        Self::FamilyAllocationReviewed,
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
            Self::RegionDark => "region.dark",
            Self::RegionLit => "region.lit",
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
            Self::SizingReviewed => "learning.sizing_reviewed",
            Self::FamilyAllocationReviewed => "learning.family_allocation_reviewed",
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
            | Self::ReconciliationCompleted
            | Self::RegionDark
            | Self::RegionLit => TopicGroup::Act,

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
            | Self::RiskRuleDormant
            | Self::SizingReviewed
            | Self::FamilyAllocationReviewed => TopicGroup::Learn,

            Self::ServiceStarted
            | Self::ServiceStopped
            | Self::KillSwitchEngaged
            | Self::KillSwitchReleased
            | Self::AutonomyLevelChanged
            | Self::BudgetExhausted
            | Self::SystemAlert => TopicGroup::System,
        }
    }

    /// The §22.1 retention class every record on this topic is filed under
    /// (ADR 0089, blueprint §56.4 rule 33).
    ///
    /// Exhaustive on purpose and with no wildcard arm: a topic added without
    /// a row here does not compile, which is the rule's "data with no class
    /// does not get written" held by the type system rather than by a
    /// reviewer. The event log's two retention seams read this and nothing
    /// else — see `qip_events::log` — and [`Self::is_lossy_tolerable`] and
    /// [`Self::requires_permanent_retention`] are derived from it, so what a
    /// record *is* is stated once.
    ///
    /// Every assignment is the row of §22.1 whose "what" column names the
    /// thing the topic carries, with three that are readings rather than
    /// quotations and are argued in ADR 0089: the Understand group's
    /// per-resolution records are *derived state* (the world model is the
    /// semantic memory the table says is kept indefinitely; the log's
    /// record of an update to it is rebuilt from the observations that
    /// produced it, and a log that kept every resolution for ever would
    /// refuse the next fill to keep one); the Simulate group and a data
    /// quality failure are *compact derived* series; and every System
    /// lifecycle fact is *irreplaceable*, because only this platform has
    /// them — which makes the kill switch's release as permanent as its
    /// engagement, where the group-derived tier kept one and dropped the
    /// other.
    pub const fn retention_class(&self) -> RetentionClass {
        match self {
            // Raw ticks, book deltas, quote updates: a bounded ring, then
            // gone. The next one arrives in milliseconds.
            Self::MarketTick | Self::MarketQuote | Self::MarketOrderBook => {
                RetentionClass::Transient
            }
            // A bar is the fallback series' business: three years behind
            // the newest, for an instrument in an active universe.
            Self::MarketBar => RetentionClass::FallbackSeries,
            // External market history, filings, registries, and the
            // platform's own manifest of what it fetched: re-readable from
            // the source, referenced by content hash, never copied.
            Self::MarketTrade
            | Self::MarketCorporateAction
            | Self::FundamentalUpdated
            | Self::MacroUpdated
            | Self::NewsReceived
            | Self::AlternativeDataReceived
            | Self::ReferenceDataUpdated
            | Self::DataReferenceRecorded => RetentionClass::Referenced,
            // A validation failure is a per-source count, kept as a series.
            Self::DataQualityFailed => RetentionClass::CompactDerived,
            // Features, moments, and the world model's change stream: the
            // structure holding the state is bounded in memory, and the
            // log's copy is rebuilt from the observations that produced it.
            Self::EntityUpdated
            | Self::EntityResolved
            | Self::RelationshipUpdated
            | Self::WorldModelUpdated
            | Self::FeatureComputed => RetentionClass::DerivedState,
            // Signals, anomalies, opportunities: compressed state with an
            // outcome, indexed for retrieval, kept indefinitely.
            Self::SignalGenerated
            | Self::AnomalyDetected
            | Self::RegimeChanged
            | Self::OpportunityDetected
            | Self::OpportunityRanked => RetentionClass::Episodic,
            // Hypotheses, evidence, challenges, verdicts on a thesis: beliefs
            // and extracted facts, kept indefinitely.
            Self::InvestigationStarted
            | Self::HypothesisCreated
            | Self::EvidenceAttached
            | Self::HypothesisChallenged
            | Self::HypothesisApproved
            | Self::HypothesisRejected
            | Self::ThesisInvalidated
            | Self::AgentRunCompleted => RetentionClass::Semantic,
            // Solver deltas and counterfactual scores: series, not
            // observations.
            Self::SimulationStarted
            | Self::SimulationCompleted
            | Self::ScenarioEvaluated
            | Self::StrategyCreated => RetentionClass::CompactDerived,
            // Verdicts, and the policy a region was given: only this
            // platform has these, permanently.
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
            | Self::VenueReinstated => RetentionClass::Irreplaceable,
            // Own orders, fills, positions, reconciliations, a region going
            // dark: permanently.
            Self::OrderProposed
            | Self::OrderApproved
            | Self::OrderSubmitted
            | Self::OrderAmended
            | Self::OrderCancelled
            | Self::OrderRejected
            | Self::OrderFilled
            | Self::PositionUpdated
            | Self::PnlUpdated
            | Self::ReconciliationCompleted
            | Self::RegionDark
            | Self::RegionLit => RetentionClass::Irreplaceable,
            // Outcomes, attributions, lessons, a source's revision, a
            // campaign's close: compressed state with its outcome, indexed
            // for retrieval, kept indefinitely.
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
            | Self::RiskRuleDormant
            | Self::SizingReviewed
            | Self::FamilyAllocationReviewed => RetentionClass::Episodic,
            // The platform's own lifecycle and control record: only this
            // platform has these, permanently.
            Self::ServiceStarted
            | Self::ServiceStopped
            | Self::KillSwitchEngaged
            | Self::KillSwitchReleased
            | Self::AutonomyLevelChanged
            | Self::BudgetExhausted
            | Self::SystemAlert => RetentionClass::Irreplaceable,
        }
    }

    /// Whether losing an event on this topic is acceptable.
    ///
    /// Market ticks are replaceable — the next one arrives in milliseconds.
    /// An order fill is not: losing one corrupts the position record.
    /// Derived from [`Self::retention_class`] rather than listed beside it,
    /// so the streaming router, the mesh and the log agree about which
    /// records are cheap to lose because they read one declaration.
    pub const fn is_lossy_tolerable(&self) -> bool {
        self.retention_class().is_replaceable()
    }

    /// Whether the topic must be retained indefinitely for audit.
    ///
    /// Derived from [`Self::retention_class`]: until ADR 0089 this was
    /// computed from [`Self::group`] with two exceptions named by hand, and
    /// the kill switch's engagement was permanent while its release was not.
    pub const fn requires_permanent_retention(&self) -> bool {
        self.retention_class().is_permanent()
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
