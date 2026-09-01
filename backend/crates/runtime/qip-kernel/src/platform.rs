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
//!
//! # What a deployed process contains
//!
//! Eight further service crates are composed here, and composed means called:
//! a dependency line whose field is never read is a capability the tests
//! exercise and the deployed binary does not have.
//!
//! * [`qip_data_finder`] decides what data *should* exist, and hands every
//!   registration it makes to [`qip_mesh`]'s catalogue, so "what datasets exist"
//!   and "what should exist" are one answer rather than two.
//! * [`qip_chain`] absorbs chain observations under a stated confirmation
//!   depth. There is no accessor here that reads chain state without one.
//! * [`qip_prediction`] turns each hypothesis the REASON stage forms into a
//!   machine-evaluable proposition, scored later against what was published.
//! * [`qip_streaming`] carries the cycle journal: every cycle is sealed into a
//!   [`qip_streaming::StreamEnvelope`] and appended to a hash-chained durable
//!   log as well as to the platform's own.
//! * [`qip_twin`] records what the platform did *and what it declined*, on a
//!   hash chain, with the counterfactual engine that prices the alternatives.
//! * [`qip_capital_fabric`] forecasts where capital will be needed from what
//!   the platform has actually traded.
//! * [`qip_cost_router`] meters what each cycle consumed, which is the figure
//!   [`qip_contracts::edge::DeductionKind::ComputeCost`] has always had a slot
//!   for and nothing was filling.

use crate::central::{CellIngestion, CellOutcome, CellReport, CentralPlane, LearningReport};
use crate::config::PlatformConfig;
use crate::cycle::{CycleReport, Stage, StageOutcome};
use qip_agents::Budget;
use qip_agents::memory::ResearchMemory;
use qip_ai::language::DeterministicModel;
use qip_ai::retrieval::SearchIndex;
use qip_capital::reservation::ReservationLedger;
use qip_capital::{AllocationLimits, CapitalAllocator, DrawdownSchedule};
use qip_capital_fabric::{
    CapitalLocation, DemandForecast, DemandForecaster, DemandKind, DemandObservation, FundingCurve,
    FxRates, LocationBalance, PlanScore, PrePositioningPlan, PrePositioningPlanner,
    PrePositioningRequest, RealisedDemand, Region as CapitalRegion, SettlementCalendar,
    SettlementConvention, TransferCostModel,
};
use qip_chain::{ChainState, ChainUpdate, Confirmations, ConfirmedView};
use qip_contracts::edge::Deduction;
use qip_contracts::governance::Usage;
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionKind, EventKind, ObjectId, OrderId, ProposalId};
use qip_core::lineage::CorrelationId;
use qip_core::lineage::{Lineage, TraceId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Context, Currency, Decimal, Hasher256, Money, PortfolioId};
use qip_cost_router::{
    ComputeLedger, Conditions, CostEngine, DataCostModel, DataReads, DecisionContext, Determinism,
    Horizon, IntelligenceTier, MarketRegime, Region as CostRegion, Router, Routing, TierCharge,
    VolatilityRegime,
};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::probe::SourceProbe;
use qip_data_finder::source::SourceCandidate;
use qip_data_finder::{RegisteredSource, RegistrationDecision};
use qip_events::log::EventLog;
use qip_events::{EventBody, EventFilter, Topic};
use qip_execution_engine::broker::{Broker, SimulatedBroker, SimulationSettings};
use qip_execution_engine::oms::{OrderManager, RefusalReason, SubmissionResult};
use qip_execution_engine::order::{Order, OrderType, Side};
use qip_financial::asset_class::AssetClass;
use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
use qip_financial::universe::Universe;
use qip_investment_agents::Organisation;
use qip_investment_agents::desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
use qip_learning_engine::attribution::Attributor;
use qip_learning_engine::evaluation::ThesisEvaluator;
use qip_learning_engine::feedback::FeedbackEngine;
use qip_market::bar::Bar;
use qip_market::corporate_action::CorporateActionKind;
use qip_market::snapshot::MarketSnapshot;
use qip_market_ingestion::adapter::SensedRecord;
use qip_mesh::catalog::Catalog;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use qip_opportunity_engine::catalyst::MarketEvent;
use qip_opportunity_engine::detector::{DetectionContext, DetectorRegistry};
use qip_opportunity_engine::engine::{EngineConfig, OpportunityEngine};
use qip_opportunity_engine::opportunity::Opportunity;
use qip_optimization_engine::router::ComputeRouter;
use qip_portfolio::portfolio::Portfolio;
use qip_portfolio_engine::construction::PortfolioConstructor;
use qip_portfolio_engine::proposal::{Proposal, ProposalStatus};
use qip_prediction::resolution::{
    Comparison, Observations, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule,
    SourceKind, UndeterminedRule, Verdict,
};
use qip_quantum::provider::SimulatedProvider;
use qip_reasoning_engine::engine::{ReasoningEngine, ReasoningOutcome};
use qip_reasoning_engine::hypothesis::Claim;
use qip_risk::limits::{LimitKind, LimitSet, RiskState};
use qip_risk_engine::autonomy::AutonomyController;
use qip_risk_engine::monitor::RiskMonitor;
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_streaming::durable::DurableLogTransport;
use qip_streaming::envelope::{EventFacts, StreamEnvelope};
use qip_streaming::ports::Publisher;
use qip_streaming::provenance::{
    Region as StreamRegion, SourceId, SourceIdentity, SourceType, Subject,
};
use qip_twin::asof::TwinMarket;
use qip_twin::capture::{Action, Decision, OutcomeCapture, RealisedOutcome};
use qip_twin::counterfactual::{
    ActualTrade, AlternativeMenu, CounterfactualEngine, CounterfactualSet,
};
use qip_world_model::WorldModel;
use qip_world_model::features::{Feature, FeatureValue};
use qip_world_model::graph::{Node, NodeKind};
use qip_world_model::liquidity::{DepthObservation, LiquidityTopology};
use serde::{Deserialize, Serialize};
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
    /// Holds between a capital check and its resolution. A proposal that
    /// passes a check reserves what it was sized for, so a second proposal in
    /// the same window cannot pass against the same free balance — gap-matrix
    /// item 10, wired.
    reservations: ReservationLedger,
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
    /// The confidential gate between the plane's per-cell state and any
    /// aggregate that leaves it as insight. Held here rather than constructed
    /// per query, because its privacy budget and its release record are the
    /// whole point: a gate rebuilt each time forgets what it has already
    /// spent, and a budget that forgets is not a budget.
    insights: crate::central::insights::CellInsights,

    // --- the eight services this process now actually contains -------------
    /// Decides what data *should* exist. Opens no sockets: the caller supplies
    /// the probe, which is what keeps this composable in a test and in a
    /// deployment without two code paths.
    data_finder: DataFinder,
    /// What datasets *do* exist. Fed from the finder's own registrations, so
    /// the two answers cannot drift apart.
    catalog: Catalog,
    /// The canonical chain, once one has been observed. `None` until then,
    /// because a chain's identity comes from the blocks rather than from
    /// configuration.
    chain: Option<ChainState>,
    /// The confirmation depth this deployment requires before reading state.
    confirmations: Confirmations,
    /// Falsifiable claims the REASON stage has made, and their verdicts.
    predictions: Vec<RecordedPrediction>,
    /// The durable, hash-chained mirror of the cycle journal.
    journal: DurableLogTransport,
    /// Everything the platform decided, and what came of it — refusals
    /// included.
    outcomes: OutcomeCapture,
    /// Prices the alternatives to a decision, against a market the caller
    /// supplies.
    counterfactuals: CounterfactualEngine,
    /// Where capital will be needed, fitted from where it has been used.
    forecaster: DemandForecaster,
    /// Decides what to pre-position, against the live allocation.
    pre_positioner: PrePositioningPlanner,
    /// Observed demand per lane, in arrival order.
    demand_history: BTreeMap<(CapitalLocation, DemandKind), Vec<DemandObservation>>,
    /// Places the REASON stage's question on the intelligence ladder before
    /// the platform answers it.
    ///
    /// Held rather than constructed per cycle so the cost ceiling a decision
    /// is judged against is one value for the process, and
    /// [`qip_cost_router::RoutingPolicy::default`] rather than a configured
    /// one because that type's own documentation says a deployment raising it
    /// has to say so in a diff. A ceiling that could be widened at runtime is
    /// a ceiling that gets widened on the cycle it binds.
    cost_router: Router,
    /// What the most recent cycle's REASON stage was routed to, and whether
    /// the panel was convened as a result.
    ///
    /// The single record of that decision. [`Platform::charge_cycle`] bills
    /// from it rather than from what the stage happened to produce, so there
    /// is one answer to "what did this cycle cost" instead of a ledger and a
    /// separate assertion that can disagree with it.
    reason_routing: Option<ReasonRouting>,
    /// The asset class of every instrument this platform was assembled to
    /// trade, taken from the universe at assembly.
    ///
    /// A projection of reference data rather than a second copy of a facility.
    /// The universe itself lives behind the desk's `read_market_data`
    /// capability gate and the composition root holds no agent context to
    /// unlock it — see [`Platform::world`] for the same trade-off made the
    /// other way. Reference data does not change under the platform, so a
    /// projection of it cannot drift from the desk's copy the way absorbed
    /// state would.
    asset_classes: BTreeMap<String, AssetClass>,
    /// Turns what a decision spent into what its edge has to survive.
    cost_engine: CostEngine,
    /// The rungs the most recent cycle actually used.
    cycle_ledger: Option<ComputeLedger>,
    /// Compute charged since assembly. Monotone.
    compute_spend: Decimal,
    /// Data licences the platform has read from. Empty until sources register.
    data_reads: DataReads,
    /// Capture and absorption failures, surfaced by LEARN rather than
    /// swallowed. A record that can lose an entry silently is not a record,
    /// and neither is one that can fail to write silently.
    capture_problems: Vec<String>,

    /// The platform's own world model — the one [`Platform::observe`] feeds.
    ///
    /// Distinct from the copy handed to the agents' desk at assembly: the
    /// desk's sits behind a read-only capability gate inside an `Arc` every
    /// agent shares, so there is deliberately no mutable path to it. The
    /// absorbed state therefore lives here, where the UNDERSTAND stage reads
    /// its coverage and the DISCOVER stage reads its series. Sharing one
    /// instance with the desk would require interior mutability in the agent
    /// runtime, which is a change with its own review; until then the desk's
    /// copy is honestly a cold start, exactly as `Desk::empty` documents.
    world: WorldModel,
    /// Where liquidity lives per instrument across venues, fed from books and
    /// quotes as they arrive, each at its own observed instant.
    liquidity: LiquidityTopology,
    /// Knowable market events for the catalyst path, in arrival order.
    ///
    /// Bounded twice: [`MARKET_EVENT_HISTORY`] caps the count and the
    /// DISCOVER stage ages out anything older than [`MARKET_EVENT_RETENTION`]
    /// — the catalyst detector links moves to events days old, not months.
    market_events: Vec<MarketEvent>,

    // State carried between cycles.
    cycle: u64,
    /// The correlation id of the most recent cycle, for tracing.
    last_correlation: Option<CorrelationId>,
    /// Price history per instrument, for the detectors.
    price_history: BTreeMap<String, Vec<f64>>,
    volume_history: BTreeMap<String, Vec<f64>>,
    /// Quoted spread history per instrument, in basis points — the series the
    /// liquidity-deterioration detector reads. A statistic, so `f64`.
    spread_history: BTreeMap<String, Vec<f64>>,
    /// Named surprise series — fundamentals against consensus, keyed
    /// `entity:metric` — the series the observation detector reads.
    observation_history: BTreeMap<String, Vec<f64>>,
    /// The book as realised fills have moved it. What the risk monitor and
    /// the decide stage read instead of the constant they used to watch.
    capital: TrackedCapital,
    /// Opportunities found and not yet worked through.
    queue: Vec<Opportunity>,
    /// Recent proposals, most recent last, capped at [`PROPOSAL_HISTORY`].
    ///
    /// A working window, not the record: the record is the event log, which is
    /// append-only and durable. Keeping every proposal here as well would mean
    /// a process that runs for a year holds a year of them in memory and
    /// rescans all of them on every cycle.
    proposals: Vec<Proposal>,
    /// The book's equity, one sample per cycle, oldest first.
    ///
    /// The series the value-at-risk and expected-shortfall limits are computed
    /// from. Realised-only, like everything else the capital tracker holds —
    /// see `risk_state` for what that excludes and why it is still worth
    /// having.
    equity_history: Vec<f64>,
    /// Theses the reason stage approved and the decide stage has not yet
    /// expressed. The audit's first-ranked finding was that this queue did not
    /// exist: `stage_decide` unconditionally constructed the empty proposal,
    /// so an approved thesis — however good — could never become a trade. A
    /// thesis is queued once and drained once; re-expressing it every cycle
    /// would pyramid the same idea until the mandate cap alone stopped it.
    pending_theses: Vec<qip_portfolio_engine::construction::ApprovedThesis>,
    /// Proposals produced since assembly, including those aged out above.
    proposals_made: u64,
}

/// How many recent proposals the platform keeps in memory.
///
/// Large enough that a cycle can still see what the previous ones decided,
/// small enough that the working set does not grow with uptime.
const PROPOSAL_HISTORY: usize = 256;

/// How many cycles of the book's own equity the platform keeps, for the tail
/// statistics the risk limits read.
///
/// Two hundred and fifty-six closes is roughly a trading year at one sample a
/// cycle, which is enough for a 99% quantile to be estimated from more than a
/// handful of observations rather than from the single worst one. Bounded like
/// every other working set here: the event log is the record, and a series
/// that grew with uptime would make the oldest deployment the slowest.
const EQUITY_HISTORY: usize = 256;

/// How many observations each per-instrument history series keeps — the
/// price, volume, quoted-spread and named-observation series the detectors
/// scan.
///
/// The failure this prevents has happened: the deployed fastbrain held every
/// observation since assembly, and because the DISCOVER stage clones and
/// rescans every series on every cycle, cycle time grew with uptime — from
/// 2.4ms at cycle 255 to 310ms at cycle 16,728, six times the 50ms fast-path
/// ceiling, and the readiness probe correctly took the node out of rotation.
/// The bound sits here, at the point of retention, so no per-cycle consumer
/// has to defend itself against an unbounded series.
///
/// Five hundred and twelve is roughly two trading years of daily bars and at
/// least double the longest lookback any consumer states: the regime
/// detector's 250-return fit window is the largest, the structural-break and
/// return-anomaly detectors need 60, the correlation baseline 90, the
/// simulate stage 60, and the covariance estimate 20 shared returns. Nothing
/// that reads these series can tell the difference between this bound and
/// unbounded history except by being fast.
pub const SERIES_HISTORY: usize = 512;

/// How many falsifiable claims the platform keeps in memory, open or scored.
///
/// A working window like [`PROPOSAL_HISTORY`], not the record — the cycle
/// journal keeps every hypothesis with its confidence the cycle it was made.
/// The failure this prevents has happened: every unsettled claim rolls
/// forward by design ([`UndeterminedRule::RollForward`]), so the deployed
/// fastbrain accumulated 16,674 open predictions in seven hours, and both the
/// per-cycle open-count and every scoring pass walked all of them. A claim
/// still unsettled after a thousand newer claims is a question the source
/// stopped answering, not one worth carrying in memory forever.
const PREDICTION_HISTORY: usize = 1024;

/// How many price levels per side a book observation sums into the liquidity
/// topology.
///
/// Three: the touch and the two levels behind it. Deep-book size is real but
/// it is not the size a marketable order meets first, and a topology summed
/// over forty levels would call a venue deep whose touch is empty.
const BOOK_DEPTH_LEVELS: usize = 3;

/// How many knowable market events the platform holds for the catalyst path.
///
/// A cap on count so the working set cannot grow with uptime; the DISCOVER
/// stage additionally ages events out by [`MARKET_EVENT_RETENTION`]. Nothing
/// is lost that mattered: the catalyst detector links moves to events at most
/// days old, and every event's source record went through the event log.
const MARKET_EVENT_HISTORY: usize = 1024;

/// How old an event may be and still be offered to the detectors.
///
/// Thirty days: an order of magnitude past the catalyst detector's own
/// three-day explanation window, so retention never decides what the detector
/// sees — only that memory stays bounded.
const MARKET_EVENT_RETENTION: Duration = Duration::from_days(30);

/// How many blocks of undo history the chain state keeps.
///
/// The deepest reorg the platform can survive, and therefore also the depth at
/// which a confirmed view becomes readable. Generous relative to any
/// confirmation depth a deployment would sensibly state.
const CHAIN_RETENTION: u32 = 256;

/// How far ahead the capital demand forecast reaches.
///
/// A day. Far enough that a transfer instructed now can settle inside it on a
/// T+1 calendar, and near enough that the extrapolation is still about the
/// book the platform is holding rather than about a different one.
const CAPITAL_HORIZON: Duration = Duration::from_days(1);

/// The venue region the platform books its own fills against.
///
/// A single region because one process trades one book. A deployment spanning
/// regions runs a cell per region, which is what `docs/adr/0008` describes.
const HOME_REGION: &str = "home";

/// How many of a subject's most recent returns stand for "now" when the
/// platform describes the conditions a routing decision was made under.
///
/// Twenty sessions: long enough that one print does not define the regime,
/// short enough that a month-old calm still reads as calm. It is deliberately
/// far shorter than the history it is compared against — a window as long as
/// the series would measure the series against itself and every tape would
/// come out normal.
const REGIME_WINDOW: usize = 20;

/// Ratios of recent realised volatility to the subject's own long-run realised
/// volatility, and the band each one puts the tape in.
///
/// Measured against the instrument's own history rather than a cross-sectional
/// figure, because a name that is always volatile is not in an extreme regime
/// merely for being itself, and calling it one would score every model that
/// ever traded it under a condition it was never in.
const VOLATILITY_BANDS: [(f64, VolatilityRegime); 3] = [
    (0.5, VolatilityRegime::Low),
    (1.5, VolatilityRegime::Normal),
    (3.0, VolatilityRegime::High),
];

/// Realised drawdown past which the platform calls the conditions a crisis.
///
/// Its own book is the only aggregate stress this process can observe: it
/// holds no cross-sectional correlation estimate and no funding series, and
/// `qip_cost_router::MarketRegime::Crisis` is about correlations going to one.
/// Ten percent of realised equity is far outside the ordinary variation of a
/// book that is working and well inside the drawdown schedule the capital
/// allocator would already be cutting into, so it names a state an operator
/// would recognise rather than a threshold chosen to fire.
const CRISIS_DRAWDOWN: f64 = 0.10;

/// How far above its own median a subject's recent quoted spread has to sit
/// before the platform calls the book thin.
///
/// Three times: a spread that has tripled is not a wide market, it is a market
/// whose price has become an opinion, which is what
/// `qip_cost_router::MarketRegime::Illiquid` says.
const ILLIQUID_SPREAD_MULTIPLE: f64 = 3.0;

// --- what the platform records, in the shapes the services take -------------

/// A falsifiable claim the REASON stage made, and what became of it.
///
/// The hypothesis says the platform believes something; this says what would
/// have to be published for that belief to be wrong, and is the thing that can
/// later be scored. A confidence with no resolution criteria is an opinion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedPrediction {
    /// The hypothesis this prediction stands behind.
    pub hypothesis: String,
    /// The cycle that made it.
    pub cycle: u64,
    /// The machine-evaluable claim, its source and its settlement rule.
    pub proposition: Proposition,
    /// When it was recorded.
    pub recorded_at: Timestamp,
    /// What the published observations said, once they arrived.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub verdict: Option<Verdict>,
    /// When it was scored.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scored_at: Option<Timestamp>,
}

impl RecordedPrediction {
    /// Whether this prediction is still waiting for its horizon.
    pub fn is_open(&self) -> bool {
        self.verdict.is_none()
    }

    /// Whether the claim was resolved and held.
    pub fn held(&self) -> bool {
        self.verdict.as_ref().is_some_and(Verdict::holds)
    }
}

/// What absorbing a batch of chain observations did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainAbsorption {
    /// Updates that extended the canonical chain.
    pub extended: usize,
    /// Updates that landed on a branch that is not canonical.
    pub side_branch: usize,
    /// Blocks the node had already served.
    pub duplicates: usize,
    /// Updates that were not blocks: pending transactions and drops.
    pub non_block: usize,
    /// Reorganisations, and the deepest one.
    pub reorgs: usize,
    pub deepest_reorg: u32,
    /// Swaps that stopped having happened.
    pub invalidated_trades: u64,
    /// Trades derived from state buried at least [`PlatformConfig::chain_confirmations`]
    /// deep. `None` when the chain is not yet that deep, which is a real
    /// answer and not a zero.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confirmed_trades: Option<u64>,
    /// Why no confirmed view was available, when there was not one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub unconfirmable: Option<String>,
    /// Blocks that were refused, with the reason.
    pub problems: Vec<String>,
}

impl ChainAbsorption {
    /// A sentence for a stage detail.
    pub fn describe(&self) -> String {
        let confirmed = match (self.confirmed_trades, &self.unconfirmable) {
            (Some(trades), _) => format!("{trades} confirmed trade(s)"),
            (None, Some(reason)) => reason.clone(),
            (None, None) => "no confirmed view".to_string(),
        };
        format!(
            "{} block(s) applied, {} on a side branch, {} reorg(s) (deepest {}); {confirmed}",
            self.extended, self.side_branch, self.reorgs, self.deepest_reorg
        )
    }
}

/// What assessing a batch of candidate sources produced.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceAssessment {
    /// One decision per candidate, in identifier order.
    pub decisions: Vec<RegistrationDecision>,
    /// Datasets that reached the mesh catalogue as a result.
    pub catalogued: Vec<String>,
    /// Registrations the catalogue refused, with its reason. Recorded rather
    /// than raised: one unusable entry must not lose the others.
    pub catalogue_problems: Vec<String>,
}

impl SourceAssessment {
    /// How many candidates were registered.
    pub fn registered(&self) -> usize {
        self.decisions
            .iter()
            .filter(|decision| decision.is_registered())
            .count()
    }
}

/// Where the REASON stage's question was placed on the intelligence ladder,
/// and what the platform did about it.
///
/// The reason this is a recorded value and not a log line. `qip-cost-router`
/// exists to refuse a rung that costs more than the decision is worth, and a
/// refusal nobody can read afterwards is indistinguishable from a stage that
/// quietly did nothing. So the routing keeps the router's own `rationale` — the
/// sentence naming the rung, the money and the confidence it was weighed
/// against — and keeps it whether the panel convened or not.
///
/// It is also the only thing [`Platform::charge_cycle`] bills the REASON stage
/// from. Before it existed, the ledger asserted a
/// [`IntelligenceTier::MultiAgentReasoning`] rung from the fact that the stage
/// had produced findings, which is a second, independent claim about what a
/// cycle cost — and the moment the router declines, the two disagree and the
/// louder one is wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReasonRouting {
    /// The cycle whose REASON stage this was.
    pub cycle: u64,
    /// The opportunity the decision was about.
    pub opportunity_id: String,
    /// What was being decided, as the router was asked it.
    pub subject: String,
    /// Where it landed, or `None` when the router refused to place it at all.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub routing: Option<Routing>,
    /// Why, in the router's own words — its rationale when a rung was found,
    /// its refusal message when none was.
    pub rationale: String,
    /// Whether the agent organisation was actually dispatched.
    pub dispatched: bool,
    /// The rung the platform actually ran on, which is not always the rung the
    /// router placed the decision on.
    ///
    /// The organisation is the only reasoner this platform has, and it sits at
    /// [`IntelligenceTier::MultiAgentReasoning`]. When the router places a
    /// decision lower — a tiny model would have sufficed — there is nothing at
    /// that rung to run it, so the platform either convenes the panel or
    /// answers nothing. It convenes, and records here that it did, because the
    /// ledger has to bill what ran rather than what was chosen. The gap between
    /// this and [`Self::tier`] is the measured case for building the cheaper
    /// rung.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub convened: Option<IntelligenceTier>,
}

impl ReasonRouting {
    /// The rung the decision was placed on, or `None` when it was refused.
    pub fn tier(&self) -> Option<IntelligenceTier> {
        self.routing.as_ref().map(Routing::tier)
    }

    /// Every rung the cycle is charged for on account of this decision.
    ///
    /// Empty unless the panel actually convened, and that is the honest
    /// answer rather than a convenience: a rung the platform declined to climb
    /// is a rung nothing ran on. Billing for it would make declining cost the
    /// same as dispatching, which would remove the only pressure the router
    /// applies.
    pub fn charges(&self) -> Vec<TierCharge> {
        self.convened.map(TierCharge::of).into_iter().collect()
    }

    /// What this decision cost, exact. Zero when nothing was convened.
    pub fn cost(&self) -> Decimal {
        self.charges()
            .iter()
            .fold(Decimal::ZERO, |sum, charge| sum + charge.cost)
    }

    fn record(
        cycle: u64,
        opportunity: &Opportunity,
        subject: String,
        routing: Option<Routing>,
        rationale: String,
        convened: Option<IntelligenceTier>,
    ) -> Self {
        Self {
            cycle,
            opportunity_id: opportunity.opportunity_id.as_str().to_string(),
            subject,
            routing,
            rationale,
            dispatched: convened.is_some(),
            convened,
        }
    }
}

/// One pass through the loop, as the journal records it.
///
/// The payload of the [`qip_streaming::StreamEnvelope`] the platform seals per
/// cycle. It carries the operator-readable summary rather than only the counts
/// so that a replay of the durable log reconstructs what an operator would
/// have read at the time, not a reconstruction of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CycleJournalEntry {
    pub cycle: u64,
    pub correlation_id: String,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    /// How many of the eight stages ran.
    pub stages_ran: usize,
    /// Everything every stage produced, summed.
    pub produced: usize,
    pub problems: Vec<String>,
    pub halted: bool,
    /// What the cycle cost in compute. Exact, because it ends up inside a
    /// [`qip_contracts::edge::NetEdge`].
    pub compute_cost: Decimal,
    /// The lines an operator would have read at three in the morning.
    pub summary: String,
}

impl EventBody for CycleJournalEntry {
    // The cycle's record belongs to the stage that closes it. LEARN is what
    // eventually notices that a stage keeps failing, and this is the artefact
    // it would notice it from.
    const TOPIC: Topic = Topic::LearningCompleted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // One entry per cycle. A journal that could record the same cycle
        // twice would make "how many cycles has this process run" unanswerable
        // from its own log.
        Some(format!("cycle:{}", self.correlation_id))
    }
}

/// One open position at its average entry cost. Quantity is signed: negative
/// is short.
#[derive(Clone, Copy, Debug)]
struct PositionLot {
    quantity: Decimal,
    average_price: Decimal,
}

/// The book's capital state, tracked from realised fills and nothing else.
///
/// Positions are carried at average entry cost, so equity here is the initial
/// equity plus realised P&L minus costs paid. Unrealised P&L is deliberately
/// excluded: the platform holds no marks, and a mark invented so the monitor
/// has something to watch is exactly the fabricated number the risk stack
/// exists to refuse. The trade-off is stated where it is read
/// ([`Platform::risk_state`]): drawdown and daily loss driven by adverse
/// marks are invisible until realised. Deterministic by construction — the
/// same fills in the same order produce the same state, with no clock read
/// anywhere.
#[derive(Debug)]
struct TrackedCapital {
    /// Cash after every fill's notional and costs. Starts at the configured
    /// initial equity — an unfilled book is all cash.
    cash: Decimal,
    /// P&L realised by position-reducing fills, cumulative.
    realised_pnl: Decimal,
    /// Commissions and fees paid, cumulative.
    costs_paid: Decimal,
    /// The highest equity seen, for the drawdown the monitor watches.
    peak_equity: Decimal,
    /// Open positions keyed by instrument.
    positions: BTreeMap<String, PositionLot>,
}

impl TrackedCapital {
    fn new(initial_equity: Decimal) -> Self {
        Self {
            cash: initial_equity,
            realised_pnl: Decimal::ZERO,
            costs_paid: Decimal::ZERO,
            peak_equity: initial_equity,
            positions: BTreeMap::new(),
        }
    }

    /// Book one fill: move cash, update the lot, realise P&L on the reducing
    /// portion.
    ///
    /// Average-cost accounting, written out once: a same-direction fill
    /// re-averages the entry; an opposite-direction fill realises
    /// `(price − average) × closed × direction` on the quantity it closes and
    /// opens any remainder at the fill price. Closing to exactly zero removes
    /// the lot, so an empty book is an empty map rather than a map of zeros.
    fn apply_fill(
        &mut self,
        object_id: &str,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        costs: Decimal,
    ) {
        self.costs_paid += costs;
        let signed = match side {
            Side::Buy => quantity,
            Side::Sell => -quantity,
        };
        // A buy spends notional and costs; a sell earns notional less costs.
        self.cash = self.cash - signed * price - costs;

        if quantity.is_positive() {
            let lot = self
                .positions
                .entry(object_id.to_string())
                .or_insert(PositionLot {
                    quantity: Decimal::ZERO,
                    average_price: Decimal::ZERO,
                });
            if lot.quantity.signum() == 0 || lot.quantity.signum() == signed.signum() {
                // Extending: re-average the entry over the combined size.
                let combined = lot.quantity + signed;
                let basis = lot.quantity.abs() * lot.average_price + quantity * price;
                lot.average_price = basis.checked_div(combined.abs()).unwrap_or(price);
                lot.quantity = combined;
            } else {
                // Reducing, possibly through zero.
                let closing = quantity.min(lot.quantity.abs());
                let direction = Decimal::from_int(i64::from(lot.quantity.signum()));
                self.realised_pnl += (price - lot.average_price) * closing * direction;
                let opened = quantity - closing;
                if opened.is_positive() {
                    // The fill flipped the position; the remainder is a new
                    // lot entered at this fill's price.
                    lot.quantity = if signed.is_positive() {
                        opened
                    } else {
                        -opened
                    };
                    lot.average_price = price;
                } else {
                    lot.quantity += signed;
                }
            }
            if lot.quantity.signum() == 0 {
                self.positions.remove(object_id);
            }
        }

        let equity = self.equity();
        self.peak_equity = self.peak_equity.max(equity);
    }

    /// Cash plus open positions at cost: initial equity plus realised P&L
    /// minus costs paid, exactly.
    fn equity(&self) -> Decimal {
        self.positions
            .values()
            .fold(self.cash, |sum, lot| sum + lot.quantity * lot.average_price)
    }

    /// Open positions as signed weights of the given equity, at cost.
    ///
    /// At cost rather than at market, consistently with everything here:
    /// weights against marks the platform does not track would disagree with
    /// the equity they are divided by.
    fn position_weights(&self, equity: Decimal) -> Vec<(String, f64)> {
        if !equity.is_positive() {
            return Vec::new();
        }
        self.positions
            .iter()
            .map(|(object, lot)| {
                let notional = lot.quantity * lot.average_price;
                (object.clone(), notional.to_f64() / equity.to_f64())
            })
            .collect()
    }

    /// Statistic: drawdown of realised equity from its running peak.
    ///
    /// Realised-only, like everything here — adverse marks on open positions
    /// do not appear in it until they are realised.
    fn drawdown(&self) -> f64 {
        if !self.peak_equity.is_positive() {
            return 0.0;
        }
        let equity = self.equity();
        if equity >= self.peak_equity {
            return 0.0;
        }
        (self.peak_equity - equity).to_f64() / self.peak_equity.to_f64()
    }
}

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

        // Opened here, before anything else is built, for the same reason the
        // storage preflight runs before a node serves: a log destination that
        // cannot be opened — an unwritable directory, a corrupt line in a file
        // this process is meant to continue — is a deployment fault, and
        // discovering it at the first append means the platform is already
        // running and already believed. A file destination also *reads back*
        // here, so the chain continues from the last record on disk instead of
        // beginning again at sequence one.
        let event_log = config.event_log.open()?;
        // The configured book size, used everywhere a "how big is the book"
        // number is needed at assembly — one source instead of the same
        // literal in six places.
        let initial_equity = config.initial_equity;

        // Taken before the universe moves into the desk, where it is only
        // reachable with a capability grant this process has no context to
        // present. The routing record has to be able to say what asset class a
        // decision was made in, and an opportunity about an instrument the
        // platform holds no reference data for is one it could never size into
        // the book anyway.
        let asset_classes: BTreeMap<String, AssetClass> = universe
            .iter()
            .map(|object| (object.object_id.as_str().to_string(), object.asset_class))
            .collect();

        let desk = Arc::new(Desk::new(
            MarketView {
                snapshot: MarketSnapshot::new(now),
                universe,
            },
            WorldModel::new(),
            BookView {
                portfolio: Portfolio::new(
                    PortfolioId::from_string("pf-main"),
                    "main",
                    Currency::USD,
                    initial_equity,
                    now,
                ),
                marks: BTreeMap::new(),
            },
            RiskView {
                state: RiskState {
                    equity: initial_equity,
                    cash: initial_equity,
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

        // The finder is configured for the usage the platform actually intends.
        // `Usage::Trade` is the strictest of the four, so a source registered
        // here is one a live order may be based on; a licence that permits only
        // research is rejected rather than quietly downgraded.
        let data_finder = DataFinder::new(FinderConfig::new(
            config.data_user_agent.clone(),
            Usage::Trade,
            config.owner.clone(),
            config.seed,
        )?);

        // The delayed-entry alternative waits one cycle, because that is the
        // decision the platform could actually have taken: it does not have the
        // option of acting between cycles. The horizon has to outlast the delay
        // or the delayed alternative enters after the window closed.
        let delay = if config.cycle_interval > Duration::ZERO {
            config.cycle_interval
        } else {
            Duration::from_mins(5)
        };
        let counterfactuals = CounterfactualEngine::new(
            config.seed,
            AlternativeMenu::standard(delay),
            delay + CAPITAL_HORIZON,
        )?;

        let quarter_book = initial_equity
            .checked_div(Decimal::from_int(4))
            .unwrap_or(initial_equity);
        let half_book = initial_equity
            .checked_div(Decimal::from_int(2))
            .unwrap_or(initial_equity);
        let pre_positioner = PrePositioningPlanner::new(
            CapitalAllocator::new(
                AllocationLimits::new(initial_equity, quarter_book, half_book, half_book)?,
                DrawdownSchedule::default(),
            ),
            TransferCostModel::new(
                TransactionCostModel::listed(1.0),
                LiquidityProfile::listed(Decimal::from_int(5_000_000_000_000), 1.0),
                FundingCurve::flat(400.0)?,
                Decimal::from_int(25),
                300.0,
            )?,
            SettlementCalendar::weekday(SettlementConvention::T1)?,
        );

        let platform = Self {
            central,
            insights: crate::central::insights::CellInsights::new(config.seed),
            data_finder,
            catalog: Catalog::new(),
            chain: None,
            confirmations: Confirmations::exactly(config.chain_confirmations),
            predictions: Vec::new(),
            journal: DurableLogTransport::in_memory("kernel-journal"),
            outcomes: OutcomeCapture::new(),
            counterfactuals,
            forecaster: DemandForecaster::new(),
            pre_positioner,
            demand_history: BTreeMap::new(),
            cost_engine: CostEngine::new(DataCostModel::new()),
            cost_router: Router::default(),
            reason_routing: None,
            asset_classes,
            cycle_ledger: None,
            compute_spend: Decimal::ZERO,
            data_reads: DataReads::new(),
            capture_problems: Vec::new(),
            last_correlation: None,
            constructor: PortfolioConstructor::new(config.mandate, router)?,
            pending_theses: Vec::new(),
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
            // Opened empty and re-anchored to tracked equity at every sizing
            // pass; zero here is one honest cycle of refusals at worst.
            reservations: ReservationLedger::new(Decimal::ZERO)
                .unwrap_or_else(|_| unreachable!("zero is not negative")),
            attributor: Attributor::new(),
            evaluator: ThesisEvaluator::default(),
            feedback: FeedbackEngine::default(),
            organisation,
            desk,
            config,
            context,
            telemetry,
            event_log,
            cycle: 0,
            price_history: BTreeMap::new(),
            volume_history: BTreeMap::new(),
            spread_history: BTreeMap::new(),
            observation_history: BTreeMap::new(),
            world: WorldModel::new(),
            liquidity: LiquidityTopology::default(),
            market_events: Vec::new(),
            capital: TrackedCapital::new(initial_equity),
            queue: Vec::new(),
            proposals: Vec::new(),
            equity_history: Vec::new(),
            proposals_made: 0,
        };
        platform.describe_metrics();
        Ok(platform)
    }

    /// Say what each metric the loop publishes means, once, here.
    ///
    /// Here because this is the only function that runs exactly once per
    /// platform and knows the whole loop. A `# HELP` line is what an operator
    /// reads when a series they have never seen appears at three in the
    /// morning, and a name alone does not tell them whether a rising number is
    /// the platform working or the platform failing — `qip_orders_refused_total`
    /// climbing is a control doing its job, `qip_journal_write_failures_total`
    /// climbing is trading with no record.
    ///
    /// Describing creates no series. Every name below stays absent from
    /// `/metrics` until something records it, which is the property that makes
    /// the export answer "what has this process actually done" rather than
    /// "what could it have done".
    fn describe_metrics(&self) {
        let metrics = &self.telemetry.metrics;
        metrics.describe(names::CYCLES_RUN, "cycles of the eight-stage loop begun");
        metrics.describe(
            names::CYCLE_DURATION_MS,
            "wall time for one full cycle, on the injected clock",
        );
        metrics.describe(
            names::STAGE_RUNS,
            "stage outcomes, labelled by stage and by whether the stage ran at all",
        );
        metrics.describe(
            names::STAGE_DURATION_MS,
            "wall time for one stage, on the injected clock",
        );
        metrics.describe(
            names::STAGE_PROBLEMS,
            "problems a stage reported without stopping the cycle",
        );
        metrics.describe(
            names::EVENT_LOG_ENTRIES,
            "entries in the hash-chained event log",
        );
        metrics.describe(
            names::JOURNAL_FAILURES,
            "cycles that ran but could not be journalled; the platform traded with no record",
        );
        metrics.describe(
            names::EVENTS_PUBLISHED,
            "cycle records sealed onto the durable transport",
        );
        metrics.describe(
            names::PROPOSALS_SIGNED,
            "proposals that took both the risk and the compliance signature",
        );
        metrics.describe(
            names::PROPOSALS_UNSIGNED,
            "cycles in which no proposal could be signed, labelled by the control that withheld",
        );
        metrics.describe(
            names::ORDERS_SUBMITTED,
            "orders the control path accepted and sent to a venue",
        );
        metrics.describe(
            names::ORDERS_REFUSED,
            "orders a control refused, labelled by the control that refused them",
        );
        metrics.describe(names::ORDERS_FILLED, "fills received, by venue");
        metrics.describe(
            names::RISK_EVALUATIONS,
            "passes of the risk monitor over the book",
        );
        metrics.describe(
            names::REASON_ROUTINGS,
            "cost-router placements of the REASON decision, and whether the panel convened",
        );
        metrics.describe(
            names::CYCLE_COMPUTE_COST,
            "compute units the last cycle's ledger charged",
        );
        metrics.describe(
            names::COMPUTE_SPEND,
            "compute units charged since the process started",
        );
        metrics.describe(
            names::KILL_SWITCH_TRIPPED,
            "1 while the kill switch is globally tripped and nothing may trade",
        );
        metrics.describe(
            names::LIVE_FILLS,
            "fills that did not come from a simulated venue; must stay at zero",
        );
        metrics.describe(
            names::LIMIT_BREACHES,
            "risk limits blocking as of the last pass of the monitor",
        );
        metrics.describe(
            names::PERMISSION_DENIALS,
            "agent attempts at something the agent's manifest does not grant",
        );
        metrics.describe(
            names::RESERVATION_SHORTFALL,
            "resyncs that found capital holds exceeding equity, by reason",
        );
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
    /// The configured risk limits, for the policy payload's envelope slot.
    ///
    /// The monitor owns them; this is a read, added so the centre can ship
    /// what it actually enforces rather than a copy that could drift.
    pub fn risk_limits(&self) -> &qip_risk::limits::LimitSet {
        self.monitor.limits()
    }

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
    /// The confidential gate over the plane's cross-cell state.
    ///
    /// `&mut` because a release spends privacy budget — reading an insight is
    /// a consuming act, and the signature says so.
    pub fn insights_mut(&mut self) -> (&mut crate::central::insights::CellInsights, &CentralPlane) {
        (&mut self.insights, &self.central)
    }

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
    /// The capital holds between a check and its resolution — read-only, for
    /// the consistency the wiring owes: free plus active holds must equal the
    /// tracked equity it was last anchored to.
    pub fn reservations(&self) -> &ReservationLedger {
        &self.reservations
    }

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

    /// The platform's world model — the one [`Platform::observe`] feeds.
    ///
    /// Read-only: `observe` is the writer, and a second writer would be a
    /// second story about what the platform believes.
    pub fn world(&self) -> &WorldModel {
        &self.world
    }

    /// Where liquidity lives, per instrument across venues, as fed from the
    /// books and quotes the platform has observed.
    pub fn liquidity(&self) -> &LiquidityTopology {
        &self.liquidity
    }

    /// The knowable market events currently held for the catalyst path, in
    /// arrival order.
    pub fn market_events(&self) -> &[MarketEvent] {
        &self.market_events
    }

    /// Equity as tracked from realised fills: the configured initial book
    /// plus realised P&L minus costs paid, positions carried at cost.
    ///
    /// Excludes unrealised P&L, and says so: the platform holds no marks, and
    /// realised-only is the honest number. This is what the risk monitor and
    /// the decide stage read.
    pub fn equity(&self) -> Decimal {
        self.capital.equity()
    }

    /// P&L realised by position-reducing fills, cumulative.
    pub fn realised_pnl(&self) -> Decimal {
        self.capital.realised_pnl
    }

    /// Commissions and fees paid across every fill, cumulative.
    pub fn trading_costs(&self) -> Decimal {
        self.capital.costs_paid
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
    ///
    /// Every record kind lands somewhere it is genuinely consumed — the `_ =>`
    /// arm that used to discard everything but bars is gone, and the exhaustive
    /// match is what keeps a new record kind from quietly rejoining it:
    ///
    /// * **Bars** keep feeding the price/volume `f64` fast path the detectors
    ///   scan, and additionally land in the world model's point-in-time
    ///   feature store with both of their instants.
    /// * **Trades and ticks** record the instrument's last traded price —
    ///   the feature store's own definition of `close` — at the record's
    ///   venue instant.
    /// * **Quotes and books** feed the liquidity topology (per-venue depth at
    ///   the observed instant) and the spread series the liquidity detector
    ///   reads.
    /// * **News, fundamentals and macro** go to the world model's absorbers
    ///   — entity resolution, the evidence index, sentiment and surprise
    ///   features — and become knowable [`MarketEvent`]s for the catalyst
    ///   detector; fundamental surprises also join the observation series the
    ///   surprise detector scans.
    /// * **Corporate actions** become knowable events at their announcement —
    ///   the announcement is the knowable happening; the ex-date is a schedule.
    /// * **Reference data** asserts the instrument's identity in the graph,
    ///   and a numeric value becomes a feature true from its effective date
    ///   and knowable from its ingestion — a change effective next week is
    ///   not readable this week.
    /// * **Alternative data** lands as a per-dataset point-in-time feature
    ///   with its observed and ingestion instants.
    ///
    /// Point in time, structurally: only the records' own instants travel.
    /// This method reads no clock, so a replay absorbs exactly what the live
    /// run absorbed. A market record carries no separate arrival stamp, so
    /// its venue instant is its knowability — the ingestion adapters have
    /// already withheld anything not yet knowable, and inventing a later
    /// arrival here would be a number nobody measured.
    ///
    /// Returns the number of records taken in. A malformed depth observation
    /// (negative depth, crossed book) is refused by the topology; the refusal
    /// is surfaced as a LEARN-stage problem rather than swallowed.
    pub fn observe(&mut self, records: Vec<SensedRecord>) -> usize {
        let mut absorbed = 0;
        // Bars are batched: history typically arrives newest-first, and one
        // merge per series is linear where per-bar sorted inserts are
        // quadratic in the series they build.
        let mut bars: Vec<Box<Bar>> = Vec::new();
        for record in records {
            match record {
                SensedRecord::Bar(bar) => {
                    let key = bar.object_id.as_str().to_string();
                    self.ensure_world_object(key.as_str(), bar.close_time());
                    push_bounded(
                        self.price_history.entry(key.clone()).or_default(),
                        bar.close.to_f64(),
                    );
                    push_bounded(
                        self.volume_history.entry(key).or_default(),
                        bar.volume.to_f64(),
                    );
                    bars.push(bar);
                    absorbed += 1;
                }
                SensedRecord::Trade(trade) => {
                    self.ensure_world_object(trade.object_id.as_str(), trade.at);
                    // "Last traded price" is the feature store's own
                    // definition of `close`, and a trade is exactly that.
                    self.world.features_mut().record(
                        "close",
                        trade.object_id.as_str(),
                        FeatureValue::new(trade.price.to_f64(), trade.at, trade.at),
                    );
                    absorbed += 1;
                }
                SensedRecord::Tick(tick) => {
                    self.ensure_world_object(tick.object_id.as_str(), tick.at);
                    self.world.features_mut().record(
                        "close",
                        tick.object_id.as_str(),
                        FeatureValue::new(tick.price.to_f64(), tick.at, tick.at),
                    );
                    absorbed += 1;
                }
                SensedRecord::Quote(quote) => {
                    self.ensure_world_object(quote.object_id.as_str(), quote.at);
                    if let Some(bps) = spread_bps(quote.bid, quote.ask) {
                        push_bounded(
                            self.spread_history
                                .entry(quote.object_id.as_str().to_string())
                                .or_default(),
                            bps,
                        );
                    }
                    // A quote is one level of depth. A venue publishing a
                    // live quote is quoting continuously as far as this
                    // process can observe, which is what `Open` states.
                    let observation = DepthObservation::new(
                        quote.object_id.clone(),
                        VenueId::new(quote.venue.clone()),
                        VenueStatus::Open,
                        quote.bid_size,
                        quote.ask_size,
                        quote.at,
                    )
                    .with_spread(quote.ask - quote.bid);
                    self.absorb_depth(observation, quote.at);
                    absorbed += 1;
                }
                SensedRecord::Book(book) => {
                    self.ensure_world_object(book.object_id.as_str(), book.at);
                    if let (Some(spread), Some(mid)) = (book.spread(), book.mid())
                        && mid.is_positive()
                    {
                        push_bounded(
                            self.spread_history
                                .entry(book.object_id.as_str().to_string())
                                .or_default(),
                            spread.to_f64() / mid.to_f64() * 10_000.0,
                        );
                    }
                    let observation =
                        DepthObservation::from_book(&book, BOOK_DEPTH_LEVELS, VenueStatus::Open);
                    self.absorb_depth(observation, book.at);
                    absorbed += 1;
                }
                SensedRecord::News(item) => {
                    // Resolves entities, indexes the document as evidence and
                    // records sentiment at the item's published instant; the
                    // context supplies only entity-resolution bookkeeping,
                    // never a knowability stamp.
                    self.world.absorb_news(&item, &self.context);
                    for event in MarketEvent::from_news(&item) {
                        self.push_market_event(event);
                    }
                    absorbed += 1;
                }
                SensedRecord::Fundamental(update) => {
                    self.define_fundamental_features(&update.metric, &update.provenance.source);
                    self.world.absorb_fundamental(&update);
                    if let Some(surprise) = update.surprise() {
                        // The surprise series the observation detector scans,
                        // keyed the way `SensedRecord::subject` names it.
                        push_bounded(
                            self.observation_history
                                .entry(format!("{}:{}", update.entity_id, update.metric))
                                .or_default(),
                            surprise,
                        );
                    }
                    self.push_market_event(MarketEvent::from_fundamental(&update));
                    absorbed += 1;
                }
                SensedRecord::Macro(observation) => {
                    self.world.absorb_macro(&observation);
                    self.push_market_event(MarketEvent::from_macro(&observation));
                    absorbed += 1;
                }
                SensedRecord::CorporateAction(action) => {
                    self.ensure_world_object(action.object_id.as_str(), action.announced_at);
                    // The announcement is the knowable event; the ex-date is
                    // a schedule. Modelling the ex-date as the happening
                    // would make a dividend "knowable" weeks after everyone
                    // traded on it.
                    let class = corporate_action_class(&action.kind);
                    let event = MarketEvent::new(
                        format!("corp:{}:{}", action.object_id, action.ex_date.as_nanos()),
                        action.object_id.as_str(),
                        class,
                        action.announced_at,
                        action.announced_at,
                    )
                    .with_description(format!(
                        "{class} on {} ex {}",
                        action.object_id,
                        action.ex_date.to_rfc3339()
                    ));
                    self.push_market_event(event);
                    absorbed += 1;
                }
                SensedRecord::AlternativeData(point) => {
                    let feature = format!("alt/{}/{}", point.dataset, point.metric);
                    if self.world.features().definition(&feature).is_none() {
                        self.world.features_mut().define(
                            Feature::new(
                                &feature,
                                "alternative data series",
                                point.provenance.source.clone(),
                            )
                            .with_staleness(Duration::from_days(30)),
                        );
                    }
                    self.world.features_mut().record(
                        &feature,
                        &point.subject_id,
                        FeatureValue {
                            value: point.value,
                            valid_at: point.observed_at,
                            available_at: point.provenance.ingestion_time,
                            confidence: point.quality.score(),
                            imputed: false,
                        },
                    );
                    absorbed += 1;
                }
                SensedRecord::ReferenceData(update) => {
                    self.ensure_world_object(&update.object_id, update.provenance.ingestion_time);
                    // A numeric value becomes a bitemporal feature: true from
                    // its effective date, knowable from its ingestion. The
                    // gap between the two is the whole point — a lot-size
                    // change effective next week must not read as current
                    // this week. A non-numeric value (a rename, a venue
                    // change) has no bitemporal home in the feature store;
                    // the identity node above still records the instrument,
                    // and applying the change to identity ahead of its
                    // effective date would be the look-ahead this route
                    // exists to refuse.
                    if let Ok(value) = update.new_value.trim().parse::<f64>()
                        && value.is_finite()
                    {
                        let feature = format!("reference/{}", update.field);
                        if self.world.features().definition(&feature).is_none() {
                            self.world.features_mut().define(
                                Feature::new(
                                    &feature,
                                    "reference data field",
                                    update.provenance.source.clone(),
                                )
                                // Reference values persist until restated;
                                // ten years is "no staleness bound" said
                                // with a number.
                                .with_staleness(Duration::from_days(3_650)),
                            );
                        }
                        self.world.features_mut().record(
                            &feature,
                            &update.object_id,
                            FeatureValue::new(
                                value,
                                update.effective_from,
                                update.provenance.ingestion_time,
                            ),
                        );
                    }
                    absorbed += 1;
                }
            }
        }
        if !bars.is_empty() {
            self.world
                .absorb_bars(bars.iter().map(|bar| (bar.as_ref(), bar.close_time())));
        }
        absorbed
    }

    /// The instrument exists: make sure the graph says so.
    ///
    /// Ensure-only, because a node's `recorded_at` answers "when did the
    /// platform first hear of this instrument" and re-observing it must not
    /// rewrite that. `recorded_at` is the record's own knowable instant, never
    /// the wall clock.
    fn ensure_world_object(&mut self, object_id: &str, recorded_at: Timestamp) {
        if self.world.graph().node(object_id).is_none() {
            self.world.graph_mut().add_node(Node::new(
                object_id,
                NodeKind::FinancialObject,
                object_id,
                recorded_at,
            ));
        }
    }

    /// Hand a depth observation to the topology, surfacing a refusal.
    ///
    /// A refused observation (negative depth, crossed book) is a problem the
    /// LEARN stage reports, not a reason to drop the batch and not a thing to
    /// swallow: a map quietly missing a venue looks exactly like a venue with
    /// no liquidity.
    fn absorb_depth(&mut self, observation: DepthObservation, known_at: Timestamp) {
        if let Err(error) = self.liquidity.absorb(observation, known_at) {
            self.capture_problems.push(format!(
                "a depth observation was refused: {}",
                error.message()
            ));
        }
    }

    /// Define a fundamental metric and its surprise on first sight, with the
    /// same lag and staleness the standard `revenue` definitions carry, so
    /// point-in-time reads and the coverage line can see metrics the standard
    /// set never named.
    fn define_fundamental_features(&mut self, metric: &str, source: &str) {
        let surprise = format!("{metric}_surprise");
        for (name, description) in [
            (metric, "reported fundamental"),
            (surprise.as_str(), "reported fundamental against consensus"),
        ] {
            if self.world.features().definition(name).is_none() {
                self.world.features_mut().define(
                    Feature::new(name, description, source)
                        .with_lag(Duration::from_days(30))
                        .with_staleness(Duration::from_days(200)),
                );
            }
        }
    }

    /// Hold a knowable event for the catalyst path, bounded by count.
    fn push_market_event(&mut self, event: MarketEvent) {
        self.market_events.push(event);
        if self.market_events.len() > MARKET_EVENT_HISTORY {
            let excess = self.market_events.len() - MARKET_EVENT_HISTORY;
            self.market_events.drain(..excess);
        }
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
        self.last_correlation = Some(correlation_id.clone());
        let lineage = Lineage::root(correlation_id.clone(), "kernel");
        let started_at = now;
        self.telemetry.metrics.count(names::CYCLES_RUN, labels([]));

        // The stages run in order and every one of them runs: a cycle that
        // fails at REASON still reaches LEARN, because LEARN is what would
        // eventually notice that REASON keeps failing.
        //
        // Each is timed as it returns rather than all of them being handed the
        // one `now` the caller passed. `StageOutcome::elapsed` and
        // `StageOutcome::with_elapsed` have existed since the loop was written
        // and nothing ever called the setter, so every stage in every report
        // and every journal entry claimed to have taken zero time. An operator
        // asking which stage is slow got eight zeros — a field that looks like
        // a measurement and is a constant is worse than no field, because it
        // answers the question wrongly instead of admitting it cannot.
        let mut stages = Vec::with_capacity(Stage::all().len());
        let mut mark = self.context.now();
        let sensed = self.stage_sense(now);
        self.finish_stage(&mut stages, sensed, &mut mark);
        let understood = self.stage_understand(now);
        self.finish_stage(&mut stages, understood, &mut mark);
        let discovered = self.stage_discover(now);
        self.finish_stage(&mut stages, discovered, &mut mark);
        let reasoned = self.stage_reason(now, &lineage);
        self.finish_stage(&mut stages, reasoned, &mut mark);
        let simulated = self.stage_simulate(now);
        self.finish_stage(&mut stages, simulated, &mut mark);
        let decided = self.stage_decide(now);
        self.finish_stage(&mut stages, decided, &mut mark);
        let acted = self.stage_act(now, &correlation_id);
        self.finish_stage(&mut stages, acted, &mut mark);
        let learned = self.stage_learn(now);
        self.finish_stage(&mut stages, learned, &mut mark);

        // Charge what the cycle consumed. A ledger per cycle rather than one
        // per process: the agent budget inside it is what refuses the next
        // rung, and a ledger that never resets would refuse every rung after
        // the first few hours of uptime. The running total is kept separately
        // and is monotone.
        self.record_equity();
        let charged = self.charge_cycle(&stages);
        for problem in charged {
            if let Some(learn) = stages.last_mut() {
                learn.problems.push(problem);
            }
        }

        let mut report = CycleReport {
            cycle: self.cycle,
            correlation_id,
            started_at,
            finished_at: now,
            stages,
            events_logged: self.event_log.len(),
            halted: self.autonomy.kill_switch().is_globally_tripped(),
        };

        for outcome in &report.stages {
            let stage = labels([("stage", outcome.stage.as_str())]);
            let mut ran = stage.clone();
            ran.insert("ran".to_string(), outcome.ran.to_string());
            self.telemetry.metrics.count(names::STAGE_RUNS, ran);
            // Statistics are `f64` and money is `Decimal`; a duration is a
            // statistic, and this is where nanoseconds on the injected clock
            // become the milliseconds the latency buckets are cut in.
            self.telemetry.metrics.observe_latency_ms(
                names::STAGE_DURATION_MS,
                stage.clone(),
                outcome.elapsed.as_nanos() as f64 / 1_000_000.0,
            );
            if !outcome.problems.is_empty() {
                self.telemetry.metrics.increment(
                    names::STAGE_PROBLEMS,
                    stage,
                    outcome.problems.len() as u64,
                );
            }
        }
        self.telemetry.metrics.observe_latency_ms(
            names::CYCLE_DURATION_MS,
            labels([]),
            self.context.now().since(started_at).as_nanos() as f64 / 1_000_000.0,
        );
        // The gauge the `qip_kill_switch_tripped` alert policy queries. Set on
        // every cycle rather than only when it changes: a gauge written once at
        // the moment of a trip goes stale the instant the scrape interval
        // misses it, and an alert reading `max() > 0` over a series that
        // stopped reporting sees nothing rather than sees a halt.
        self.telemetry.metrics.gauge(
            names::KILL_SWITCH_TRIPPED,
            labels([]),
            if report.halted { 1.0 } else { 0.0 },
        );

        // The journal is written last, so it records the cycle that happened
        // rather than the one that was about to. A journal failure is a problem
        // on the cycle and not the end of it: a process that stopped trading
        // because it could not write its own diary would be a worse outcome
        // than one that traded and said it could not write it down.
        //
        // Which is exactly why it is counted. "Traded and could not write it
        // down" is the state nobody notices from a report that still says the
        // cycle ran, and `qip_journal_write_failures_total` is the only place
        // it is a number rather than a sentence in a problem list.
        match self.journal_cycle(&report, now) {
            Ok(()) => self
                .telemetry
                .metrics
                .count(names::EVENTS_PUBLISHED, labels([("topic", "cycle")])),
            Err(error) => {
                self.telemetry
                    .metrics
                    .count(names::JOURNAL_FAILURES, labels([]));
                if let Some(learn) = report.stages.last_mut() {
                    learn.problems.push(format!(
                        "the cycle journal was not written: {}",
                        error.message()
                    ));
                }
            }
        }
        report.events_logged = self.event_log.len();
        self.telemetry.metrics.gauge(
            names::EVENT_LOG_ENTRIES,
            labels([]),
            report.events_logged as f64,
        );
        report
    }

    /// Close one stage off: time it on the injected clock and keep it.
    ///
    /// The clock is the platform's own rather than the wall, so a replay
    /// against a [`qip_core::ManualClock`] reports the durations that clock
    /// says elapsed. That makes a replayed cycle's stage timings reproducible
    /// instead of a fresh measurement of the replay machine, which is the same
    /// reason every other time in this loop comes from the injected clock.
    fn finish_stage(
        &self,
        stages: &mut Vec<StageOutcome>,
        outcome: StageOutcome,
        mark: &mut Timestamp,
    ) {
        let at = self.context.now();
        let elapsed = at.since(*mark);
        *mark = at;
        stages.push(outcome.with_elapsed(elapsed));
    }

    /// Record the book's equity for this cycle, bounded.
    ///
    /// Sampled once per cycle rather than per fill, so the series is evenly
    /// spaced in cycles and a quantile over it means "a bad cycle" rather than
    /// "a bad moment during a busy one". A per-fill series would weight the
    /// cycles that traded most, which is the opposite of what a tail statistic
    /// wants.
    fn record_equity(&mut self) {
        self.equity_history.push(self.capital.equity().to_f64());
        if self.equity_history.len() > EQUITY_HISTORY {
            self.equity_history
                .drain(..self.equity_history.len() - EQUITY_HISTORY);
        }
    }

    /// The book's period returns, from the equity series.
    ///
    /// Simple returns between consecutive samples. A step from a non-positive
    /// equity is skipped rather than divided by: a book that reached zero has
    /// no meaningful return, and dividing by it would produce an infinity that
    /// poisons every statistic downstream of it.
    fn equity_returns(&self) -> Vec<f64> {
        self.equity_history
            .windows(2)
            .filter(|w| w[0] > 0.0)
            .map(|w| (w[1] - w[0]) / w[0])
            .collect()
    }

    /// Charge the rungs this cycle actually used.
    ///
    /// Derived from what the stages report rather than asserted, so the bill
    /// tracks the work: a quiet cycle is charged for eight deterministic passes
    /// and nothing else, and a cycle that dispatched the organisation is
    /// charged for the panel it convened. Returns whatever the budget refused,
    /// for the report to carry.
    fn charge_cycle(&mut self, stages: &[StageOutcome]) -> Vec<String> {
        let mut tiers = Vec::new();
        for outcome in stages {
            if !outcome.ran {
                continue;
            }
            // Every stage that ran is at least one capability invocation. A
            // rule that runs a billion times is not free, and a ledger that
            // only counted model calls could not see that.
            tiers.push(IntelligenceTier::DeterministicCode);
            match outcome.stage {
                // The detectors are fitted estimators evaluated in process.
                Stage::Discover => tiers.push(IntelligenceTier::StatisticalModel),
                // The panel is billed from the router's own record of where
                // the decision was placed, never from the fact that findings
                // came back. Those were two independent claims about what a
                // cycle cost, and they disagree the moment the router declines
                // — which it now can.
                Stage::Reason => {
                    if let Some(routing) = &self.reason_routing {
                        tiers.extend(routing.charges().into_iter().map(|charge| charge.tier));
                    }
                }
                // Resampling a path is a counterfactual run, not a rule.
                Stage::Simulate if outcome.produced > 0 => {
                    tiers.push(IntelligenceTier::Simulation);
                }
                _ => {}
            }
        }

        let mut ledger =
            match ComputeLedger::new(format!("cycle {}", self.cycle), Budget::research_default()) {
                Ok(ledger) => ledger,
                Err(error) => return vec![format!("the compute ledger refused to open: {error}")],
            };
        let mut problems = Vec::new();
        for tier in tiers {
            if let Err(error) = ledger.charge(tier) {
                problems.push(format!(
                    "the cycle exhausted its compute budget at {}: {}",
                    tier.as_str(),
                    error.message()
                ));
                break;
            }
        }
        self.compute_spend += ledger.total_cost();

        // The bill, read off the ledger that was just charged rather than
        // recomputed from the stage list. Recomputing would be a second claim
        // about what the cycle cost, and the two would disagree the first time
        // the budget refused a rung part-way through — the ledger would hold
        // the truncated bill and the recomputation the full one, with the
        // larger and wronger number being the one on the dashboard.
        //
        // A compute charge is a `Decimal` and a metric is an `f64`; this is the
        // crossing point, and it is here rather than at the export because the
        // ledger's own arithmetic must stay exact right up to the moment the
        // number leaves the platform.
        self.telemetry.metrics.gauge(
            names::CYCLE_COMPUTE_COST,
            labels([]),
            ledger.total_cost().to_f64(),
        );
        self.telemetry.metrics.gauge(
            names::COMPUTE_SPEND,
            labels([]),
            self.compute_spend.to_f64(),
        );

        self.cycle_ledger = Some(ledger);
        problems
    }

    /// Seal one cycle into the journal.
    ///
    /// The same frame reaches two logs: the platform's own [`EventLog`], which
    /// is what `events_logged` counts and what the correlation index walks, and
    /// the durable transport, which is what a downstream consumer replays. Both
    /// are hash-chained, so a truncated or edited history is detectable in
    /// either.
    ///
    /// The local transport is not an option here and the type says so: a cycle
    /// record is not lossy-tolerable, and
    /// [`qip_streaming::RoutingClass::check`] refuses to route one down a queue
    /// whose overload policy is to drop its oldest entry.
    fn journal_cycle(&mut self, report: &CycleReport, now: Timestamp) -> Result<()> {
        let entry = CycleJournalEntry {
            cycle: report.cycle,
            correlation_id: report.correlation_id.as_str().to_string(),
            started_at: report.started_at,
            finished_at: report.finished_at,
            stages_ran: report.stages.iter().filter(|stage| stage.ran).count(),
            produced: report.stages.iter().map(|stage| stage.produced).sum(),
            problems: report
                .problems()
                .into_iter()
                .map(|(stage, problem)| format!("{}: {problem}", stage.as_str()))
                .collect(),
            halted: report.halted,
            compute_cost: self.last_cycle_cost(),
            summary: report.summarise(),
        };

        let facts = EventFacts::derived(
            SourceIdentity::new(
                SourceId::new("qip-kernel"),
                SourceType::Internal,
                StreamRegion::new(HOME_REGION),
            ),
            Subject::unattributed(),
            CycleJournalEntry::TOPIC,
        );
        let envelope = StreamEnvelope::seal(
            self.context.ids().generate::<EventKind>(now),
            Lineage::root(report.correlation_id.clone(), "kernel/journal"),
            entry,
            now,
            now,
            facts,
        )?;

        self.event_log.append(&envelope.to_frame()?)?;
        self.journal.publish(envelope, now)?;
        Ok(())
    }

    // --- the stages ---------------------------------------------------------

    fn stage_sense(&mut self, _now: Timestamp) -> StageOutcome {
        let instruments = self.price_history.len();
        let observations: usize = self.price_history.values().map(Vec::len).sum();
        // What the platform has decided it should be collecting, next to what
        // it is actually receiving. A registry that is filling while the
        // observation count stays at zero is the interesting failure, and a
        // detail that mentioned only one of the two would hide it.
        let sources = self.data_finder.registry().len();
        let sourced = if sources == 0 {
            String::new()
        } else {
            format!("; {sources} registered source(s)")
        };
        if observations == 0 {
            return StageOutcome::ran(
                Stage::Sense,
                0,
                format!("no observations have been fed in; the platform is running blind{sourced}"),
            );
        }
        StageOutcome::ran(
            Stage::Sense,
            observations,
            format!("{observations} observation(s) across {instruments} instrument(s){sourced}"),
        )
    }

    fn stage_understand(&mut self, now: Timestamp) -> StageOutcome {
        // Read back from the world model at this instant in both time
        // dimensions — not the price-history count this line used to quote
        // while the model sat empty. A coverage line that cannot go down when
        // absorption stops is not a coverage line.
        let state = self.world.state_at(now, now);
        let documents = self.world.index().len();
        let liquidity = if self.liquidity.observation_count() == 0 {
            String::new()
        } else {
            format!(
                "; liquidity mapped for {} instrument(s) from {} depth observation(s)",
                self.liquidity.instruments().len(),
                self.liquidity.observation_count()
            )
        };
        let events = if self.market_events.is_empty() {
            String::new()
        } else {
            format!(
                "; {} knowable event(s) held for the catalyst path",
                self.market_events.len()
            )
        };
        // The chain, when one has been observed, is part of what the platform
        // understands — and it is reported at the confirmation depth this
        // deployment stated, because head state is revisable and a detail that
        // quoted it would be quoting a number that can stop having been true.
        let chain = match (&self.chain, self.confirmed_chain()) {
            (Some(state), Ok(view)) => format!(
                "; chain at height {} with {} trade(s) confirmed {}",
                state
                    .head_number()
                    .map_or_else(|| "none".to_string(), |number| number.to_string()),
                view.state().trades(),
                self.confirmations
            ),
            (Some(state), Err(_)) => format!(
                "; chain at height {} is not yet {} deep",
                state
                    .head_number()
                    .map_or_else(|| "none".to_string(), |number| number.to_string()),
                self.confirmations
            ),
            (None, _) => String::new(),
        };
        StageOutcome::ran(
            Stage::Understand,
            state.object_count + state.entity_count,
            format!(
                "world model holds {} instrument(s), {} entity(ies), {} relationship(s), \
                 {} causal claim(s), {} readable feature value(s), {} document(s)\
                 {liquidity}{events}{chain}",
                state.object_count,
                state.entity_count,
                state.relationship_count,
                state.causal_claim_count,
                state.features.len(),
                documents
            ),
        )
    }

    fn stage_discover(&mut self, now: Timestamp) -> StageOutcome {
        // Events past retention age out first, so the working set stays
        // bounded on a long-running process. Retention is far outside the
        // catalyst detector's own explanation window, so it never decides
        // what the detector sees.
        self.market_events
            .retain(|event| now.since(event.known_at()) <= MARKET_EVENT_RETENTION);

        let mut detection = DetectionContext::new(now);
        for (subject, prices) in &self.price_history {
            detection = detection.with_prices(subject.clone(), prices.clone());
        }
        for (subject, volumes) in &self.volume_history {
            detection = detection.with_volumes(subject.clone(), volumes.clone());
        }
        for (subject, spreads) in &self.spread_history {
            detection = detection.with_spreads(subject.clone(), spreads.clone());
        }
        for (series, values) in &self.observation_history {
            detection = detection.with_observations(series.clone(), values.clone());
        }
        // Attaching events claims the stream was watched — the precondition
        // for the detector ever calling a move *unexplained*. Claimed only
        // once an intelligence record has actually arrived: an empty set with
        // coverage would let "no events supplied" masquerade as "no catalyst
        // existed".
        if !self.market_events.is_empty() {
            detection = detection.with_events(self.market_events.clone());
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

    /// Put a price, a deadline and a confidence bar on the REASON stage's
    /// question, so the router has something to weigh.
    ///
    /// **`value_at_stake` is not the notional.** `DecisionContext` documents
    /// the distinction and the affordability rule depends on it: quoting the
    /// notional makes every rung look affordable, which is exactly the check
    /// being defeated. What is at stake here is the difference between acting
    /// on this opportunity and not — so it is the book's equity scaled by how
    /// much of it this opportunity could plausibly move, which is the
    /// detector's own `importance` discounted by its `confidence` that the
    /// observation is real rather than noise. An opportunity the detectors are
    /// half sure about is worth half as much to get right.
    ///
    /// The context is refused rather than clamped where that product rounds to
    /// nothing. `DecisionContext::validate` rejects a non-positive value, and
    /// substituting a floor would route an opportunity worth nothing as though
    /// it were a real decision — which is the failure this whole path exists
    /// to prevent.
    fn reason_decision_context(
        &self,
        opportunity: &Opportunity,
        subject: String,
    ) -> Result<DecisionContext> {
        let share = opportunity.rank.importance * opportunity.rank.confidence;
        let share = Decimal::from_f64(share)
            .ok_or_else(|| Error::numeric("the opportunity's rank is not a representable share"))?;
        let value_at_stake = self
            .capital
            .equity()
            .checked_mul(share)
            .ok_or_else(|| Error::numeric("the value at stake in this decision overflowed"))?;

        // The deadline, not the horizon. The horizon is how long the
        // implication takes to play out; `expires_at` is when the opportunity
        // stops being worth acting on, and an answer arriving after it is
        // worthless however good it is.
        let latency_budget = opportunity.expires_at.since(self.context.now());

        Ok(DecisionContext::new(
            subject,
            value_at_stake,
            latency_budget,
            // The bar the detectors already had to clear for the opportunity to
            // be queued at all, reused rather than reinvented: a separate
            // constant here would let the platform investigate something it had
            // already decided was not credible enough to look at.
            opportunity.rank.confidence,
            // A thesis is an estimate. It is not a pre-trade risk check, and
            // the router's `Required` arm returns a type that cannot name a
            // model rung — see `qip_cost_router::router`, where that is the
            // structural guarantee keeping risk checks off the ladder.
            Determinism::NotRequired,
            self.routing_conditions(opportunity),
        ))
    }

    /// Describe the market this decision is being made in.
    ///
    /// Every field is read off something the platform already tracks. Nothing
    /// here is asked of a vendor and nothing is guessed: a routing record whose
    /// conditions were invented would make the model reputation book — which is
    /// keyed on exactly this label — an index over fiction.
    fn routing_conditions(&self, opportunity: &Opportunity) -> Conditions {
        // The asset class of the first instrument the opportunity concerns.
        // An opportunity spanning classes is routed under the one it names
        // first rather than under a blend, because the reputation key has to be
        // a value a later lookup can reproduce.
        let asset_class = opportunity
            .affected_objects
            .first()
            .and_then(|object| self.asset_classes.get(object.as_str()))
            .copied()
            .unwrap_or(AssetClass::Equity);

        let subject = opportunity
            .affected_objects
            .first()
            .map(|object| object.as_str().to_string())
            .unwrap_or_default();

        Conditions::new(
            asset_class,
            CostRegion::new(HOME_REGION),
            self.market_regime(&subject),
            self.volatility_regime(&subject),
            Self::routing_horizon(opportunity.horizon),
        )
    }

    /// Which of the five regimes the tape is in, on the evidence this process
    /// holds.
    ///
    /// The order is a precedence, not a search: a book in drawdown is in a
    /// crisis whatever its spreads say, and a market whose price has become an
    /// opinion is illiquid whatever its returns say. Trending versus
    /// mean-reverting is decided last and only for a subject with enough
    /// history to tell them apart — below that the honest answer is `Quiet`,
    /// which is a regime and not a missing value.
    fn market_regime(&self, subject: &str) -> MarketRegime {
        if self.capital.drawdown() >= CRISIS_DRAWDOWN {
            return MarketRegime::Crisis;
        }
        if self.spread_has_widened(subject) {
            return MarketRegime::Illiquid;
        }
        let Some(returns) = self.recent_returns(subject) else {
            return MarketRegime::Quiet;
        };
        if returns.len() < 3 {
            return MarketRegime::Quiet;
        }
        // Sign persistence: how often a move is followed by a move the same
        // way. Above half the tape continues, below half it comes back. The
        // measure is crude and deliberately so — it needs no fitted parameter,
        // so it cannot be the thing that is stale when the regime turns.
        let pairs = returns.windows(2).filter(|w| w[0] != 0.0 && w[1] != 0.0);
        let (same, total) = pairs.fold((0usize, 0usize), |(same, total), w| {
            let continued = usize::from(w[0].is_sign_positive() == w[1].is_sign_positive());
            (same + continued, total + 1)
        });
        if total == 0 {
            return MarketRegime::Quiet;
        }
        if same * 2 > total {
            MarketRegime::Trending
        } else {
            MarketRegime::MeanReverting
        }
    }

    /// How violent the tape is, relative to the subject's own history.
    ///
    /// Relative rather than absolute because a fixed basis-point threshold
    /// would call every digital asset extreme and every government bond low,
    /// which says something about the asset class and nothing about the day.
    fn volatility_regime(&self, subject: &str) -> VolatilityRegime {
        let Some(values) = self.observation_history.get(subject) else {
            return VolatilityRegime::Normal;
        };
        if values.len() <= REGIME_WINDOW + 1 {
            // Not enough history to compare a window against. `Normal` is the
            // answer that claims least: it neither excuses spending on a quiet
            // tape nor refuses it on a violent one.
            return VolatilityRegime::Normal;
        }
        let returns = Self::returns_of(values);
        let long_run = Self::deviation(&returns);
        let recent = Self::deviation(&returns[returns.len().saturating_sub(REGIME_WINDOW)..]);
        if long_run <= 0.0 {
            return VolatilityRegime::Normal;
        }
        let ratio = recent / long_run;
        VOLATILITY_BANDS
            .iter()
            .find(|(ceiling, _)| ratio < *ceiling)
            .map_or(VolatilityRegime::Extreme, |(_, band)| *band)
    }

    /// Whether the subject's recent quoted spread sits far enough above its own
    /// median that the price should be treated as an opinion.
    fn spread_has_widened(&self, subject: &str) -> bool {
        let Some(series) = self.spread_history.get(subject) else {
            return false;
        };
        if series.len() < 3 {
            return false;
        }
        let mut sorted = series.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        if median <= 0.0 {
            return false;
        }
        series
            .last()
            .is_some_and(|latest| latest / median >= ILLIQUID_SPREAD_MULTIPLE)
    }

    /// The subject's most recent returns, or `None` where nothing has been
    /// observed for it.
    fn recent_returns(&self, subject: &str) -> Option<Vec<f64>> {
        let values = self.observation_history.get(subject)?;
        if values.len() < 2 {
            return None;
        }
        let returns = Self::returns_of(values);
        let from = returns.len().saturating_sub(REGIME_WINDOW);
        Some(returns[from..].to_vec())
    }

    /// Simple returns of a price series, skipping any step from a
    /// non-positive price rather than dividing by it.
    fn returns_of(values: &[f64]) -> Vec<f64> {
        values
            .windows(2)
            .filter(|w| w[0] > 0.0)
            .map(|w| (w[1] - w[0]) / w[0])
            .collect()
    }

    /// Population standard deviation. Zero for fewer than two observations,
    /// which every caller above treats as "cannot tell" rather than as "calm".
    fn deviation(values: &[f64]) -> f64 {
        if values.len() < 2 {
            return 0.0;
        }
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt()
    }

    /// Which horizon band an opportunity's own horizon falls in.
    fn routing_horizon(horizon: Duration) -> Horizon {
        let seconds = horizon.as_nanos() / 1_000_000_000;
        match seconds {
            ..=0 => Horizon::Microsecond,
            1..=86_400 => Horizon::Intraday,
            86_401..=604_800 => Horizon::Daily,
            604_801..=2_592_000 => Horizon::Weekly,
            _ => Horizon::Strategic,
        }
    }

    /// Keep the REASON routing record, and count it.
    ///
    /// The only way `reason_routing` is written, and that is the point rather
    /// than tidiness. There are four paths out of REASON — the context could
    /// not be built, the router refused to place the decision, the panel's rung
    /// cost more than the decision was worth, and the panel convened — and a
    /// counter incremented at three of them would report a decline rate that
    /// silently excluded whichever one somebody forgot. Routing the assignment
    /// through here makes the record and the metric the same act: a fifth path
    /// added later cannot store a routing without also counting it, because
    /// there is no other way to store one.
    ///
    /// Labelled by the rung the router chose and by whether the panel actually
    /// convened, which is the pair that matters: `qip-cost-router` exists to
    /// place decisions below the panel, and a placement that is never followed
    /// by a convening is a saving while a placement that always is means the
    /// router is a receipt rather than a control. Both label values come from
    /// closed enums, so the series count is bounded by the ladder.
    fn place_reason_routing(&mut self, routing: ReasonRouting) {
        self.telemetry.metrics.count(
            names::REASON_ROUTINGS,
            labels([
                ("tier", routing.tier().map_or("none", |tier| tier.as_str())),
                (
                    "outcome",
                    if routing.dispatched {
                        "convened"
                    } else {
                        "declined"
                    },
                ),
            ]),
        );
        self.reason_routing = Some(routing);
    }

    fn stage_reason(&mut self, now: Timestamp, lineage: &Lineage) -> StageOutcome {
        let Some(opportunity) = self.queue.first().cloned() else {
            return StageOutcome::ran(Stage::Reason, 0, "nothing in the queue to reason about");
        };

        // Where this decision belongs on the intelligence ladder, asked before
        // anything is spent reaching it. Convening the organisation is the most
        // expensive thing a cycle does, and until this call existed it was done
        // unconditionally for whatever sat at the head of the queue — then
        // billed for afterwards, from the fact that findings had come back.
        // That is not a cost control; it is a receipt.
        let subject = format!("whether to act on '{}'", opportunity.headline);
        let context = match self.reason_decision_context(&opportunity, subject.clone()) {
            Ok(context) => context,
            Err(error) => {
                let rationale = error.message().to_string();
                self.place_reason_routing(ReasonRouting::record(
                    self.cycle,
                    &opportunity,
                    subject,
                    None,
                    rationale.clone(),
                    None,
                ));
                return StageOutcome::ran(
                    Stage::Reason,
                    0,
                    format!("the panel was not convened: {rationale}"),
                );
            }
        };

        let placed = match self.cost_router.select(&context) {
            Ok(placed) => placed,
            // The router refused to place the decision at all: no rung reaches
            // the confidence this opportunity needs at a price it is worth, or
            // the opportunity could not be priced. Either way the panel does
            // not convene, and the stage says so in the router's own words
            // rather than reporting an empty queue.
            Err(error) => {
                let rationale = error.message().to_string();
                self.place_reason_routing(ReasonRouting::record(
                    self.cycle,
                    &opportunity,
                    subject,
                    None,
                    rationale.clone(),
                    None,
                ));
                return StageOutcome::ran(
                    Stage::Reason,
                    0,
                    format!("the panel was not convened: {rationale}"),
                );
            }
        };

        // The organisation is the only reasoner this platform has, and it sits
        // at the MultiAgentReasoning rung. So the question is not "did the
        // router pick this rung" — it usually picks a cheaper one, because a
        // cheaper one would genuinely suffice and none is implemented. The
        // question the cost router actually exists to answer is whether this
        // rung costs more than the decision is worth, and that is what is asked
        // here.
        //
        // Getting this wrong in the other direction is worse than overspending:
        // a gate that refused every decision the router placed below the panel
        // would decline nearly all of them, and the REASON stage would go
        // silently dead while every rationale read as a deliberate saving.
        const PANEL: IntelligenceTier = IntelligenceTier::MultiAgentReasoning;
        // `assess` fails only where the context or policy is invalid, and both
        // were validated by the `select` above. It is still handled rather than
        // unwrapped: a refusal the platform cannot explain must not become a
        // panel it convenes anyway.
        let refusal = if placed.tier() >= PANEL {
            None
        } else {
            match self.cost_router.assess(PANEL, &context) {
                Ok(verdict) if verdict.is_usable() => None,
                Ok(verdict) => Some(verdict.reason(PANEL)),
                Err(error) => Some(error.message().to_string()),
            }
        };

        let placed_rationale = placed.rationale().to_string();
        if let Some(rationale) = refusal {
            self.place_reason_routing(ReasonRouting::record(
                self.cycle,
                &opportunity,
                subject,
                Some(placed),
                rationale.clone(),
                None,
            ));
            return StageOutcome::ran(
                Stage::Reason,
                0,
                format!("the panel was not convened: {rationale}"),
            );
        }

        // Convening. The rationale keeps the router's own sentence — including
        // the case where it named a cheaper rung — so the record shows both
        // where the decision belonged and what the platform had available.
        let rationale = if placed.tier() < PANEL {
            format!(
                "{placed_rationale}; convened at {} regardless, the only rung this platform implements",
                PANEL.as_str()
            )
        } else {
            placed_rationale
        };
        self.place_reason_routing(ReasonRouting::record(
            self.cycle,
            &opportunity,
            subject,
            Some(placed),
            rationale,
            Some(PANEL),
        ));

        let brief = qip_agents::finding::AgentBrief::new(
            opportunity.headline.clone(),
            now,
            opportunity.horizon,
        )
        .with_context(opportunity.historical_context.clone())
        .about_objects(opportunity.affected_objects.clone())
        .about_entities(opportunity.affected_entities.clone());

        let report = self.organisation.dispatch(&brief, now, lineage);
        self.telemetry
            .metrics
            .increment(names::AGENT_RUNS, labels([]), report.runs.len() as u64);
        let mut problems: Vec<String> = report
            .failed
            .iter()
            .map(|agent| format!("{agent} failed"))
            .collect();
        if !report.failed.is_empty() {
            self.telemetry.metrics.increment(
                names::AGENT_FAILURES,
                labels([]),
                report.failed.len() as u64,
            );
        }
        // The counter the `qip_permission_denials_total` alert policy queries.
        // Counted per offending agent rather than per cycle, because an agent
        // reaching for the same thing it was denied on every cycle and an
        // agent that did it once look identical in a per-cycle count, and only
        // the first is a manifest that needs changing or an agent that needs
        // stopping. The agent id is a roster name from a committed manifest —
        // never a credential, an account or anything a caller supplied — so it
        // is safe as a label and its cardinality is the roster's size.
        for violation in report.permission_violations() {
            self.telemetry.metrics.count(
                names::PERMISSION_DENIALS,
                labels([("agent", violation.agent_id.as_str())]),
            );
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
                // A confidence with no resolution criteria is an opinion. The
                // prediction is what makes the hypothesis scoreable later
                // against something a source published, rather than against
                // whether it felt right.
                let predicted = self.record_prediction(&opportunity, &reasoned, now);
                let mut outcome = StageOutcome::ran(
                    Stage::Reason,
                    report.findings.len(),
                    format!(
                        "{} finding(s) from {} run(s), coverage {:.0}%{}; hypothesis {} at confidence {:.2}{}",
                        report.findings.len(),
                        report.runs.len(),
                        report.coverage() * 100.0,
                        if report.is_contested() {
                            ", contested"
                        } else {
                            ""
                        },
                        reasoned.hypothesis.status.as_str(),
                        reasoned.hypothesis.effective_confidence(),
                        match &predicted {
                            Ok(true) => format!(
                                ", falsifiable ({} open prediction(s))",
                                self.predictions.iter().filter(|p| p.is_open()).count()
                            ),
                            Ok(false) => String::new(),
                            Err(_) => String::new(),
                        }
                    ),
                );
                if let Err(error) = predicted {
                    outcome = outcome.with_problem(format!(
                        "the hypothesis could not be written as a resolvable claim: {}",
                        error.message()
                    ));
                }
                if !approved {
                    outcome = outcome
                        .with_problem(format!("rejected on review: {}", reasoned.review.rationale));
                } else {
                    match self.thesis_from(&opportunity, &reasoned) {
                        Ok(thesis) => {
                            self.pending_theses.push(thesis);
                            // Bounded like the proposal working set: an idea
                            // nobody sized for this many cycles is stale, and
                            // the event log keeps the record.
                            if self.pending_theses.len() > PROPOSAL_HISTORY {
                                self.pending_theses
                                    .drain(..self.pending_theses.len() - PROPOSAL_HISTORY);
                            }
                        }
                        // An approved thesis that cannot be sized is reported,
                        // not dropped silently — the difference between "we
                        // chose not to" and "we could not" is the difference
                        // an operator acts on.
                        Err(reason) => {
                            outcome = outcome.with_problem(format!(
                                "approved but not sizeable: {}",
                                reason.message()
                            ));
                        }
                    }
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

    /// Turn an approved hypothesis into the thesis construction can size.
    ///
    /// Every number here is traceable to something observed: the direction is
    /// the claim's own implied sign, the conviction is the review's effective
    /// confidence under that sign, the expected return is the reversion of the
    /// anomaly's measured displacement, and the price is the last close this
    /// platform saw. Where any of those is missing the thesis is refused with
    /// the missing thing named — a `RegimeShift` has no inherent direction, an
    /// instrument with no price history cannot be sized — because inventing a
    /// number here would put fabricated conviction one governed approval away
    /// from an order.
    /// Size approved theses against the platform's own history and book.
    ///
    /// The covariance is estimated from the closes this platform observed —
    /// the same series the simulate stage resamples — over the longest window
    /// every named instrument shares. Too little shared history is a refusal
    /// naming the count, not a guess: a covariance from a handful of points
    /// is a number wearing the costume of an estimate, and the mandate's risk
    /// bound would be enforced against the costume.
    fn construct_from(
        &mut self,
        theses: &[qip_portfolio_engine::construction::ApprovedThesis],
        now: Timestamp,
    ) -> Result<qip_portfolio_engine::construction::ConstructionOutcome> {
        const MIN_SHARED_RETURNS: usize = 20;

        let mut returns: Vec<Vec<f64>> = Vec::with_capacity(theses.len());
        for thesis in theses {
            let series = self
                .price_history
                .get(thesis.object_id.as_str())
                .ok_or_else(|| {
                    Error::not_found(format!("{} has no price history", thesis.object_id))
                })?;
            let mut asset_returns = Vec::with_capacity(series.len().saturating_sub(1));
            for pair in series.windows(2) {
                if pair[0] != 0.0 {
                    asset_returns.push(pair[1] / pair[0] - 1.0);
                }
            }
            returns.push(asset_returns);
        }
        let shared = returns.iter().map(Vec::len).min().unwrap_or(0);
        if shared < MIN_SHARED_RETURNS {
            return Err(Error::invalid(format!(
                "{shared} shared return observation(s) is too little history to estimate a \
                 covariance for {} instrument(s); {MIN_SHARED_RETURNS} are needed",
                theses.len()
            )));
        }
        for series in &mut returns {
            let start = series.len() - shared;
            series.drain(..start);
        }

        let n = theses.len();
        let mut covariance = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in i..n {
                let value = qip_numerics::stats::covariance(&returns[i], &returns[j]);
                covariance[i][j] = value;
                covariance[j][i] = value;
            }
        }

        // The book as weights of tracked equity, at cost — the same
        // realised-only accounting the monitor watches, stated there.
        let equity = self.capital.equity();
        let current: std::collections::BTreeMap<String, f64> =
            self.capital.position_weights(equity).into_iter().collect();

        // The budget is what is actually free, not the raw equity: equity
        // minus every active hold, as `stage_decide` anchored it this pass.
        // This is the line that makes a second proposal unable to pass
        // against capital a first one already claimed.
        let free = self.reservations.free(now);

        let outcome = self.constructor.construct(
            theses,
            &covariance,
            &current,
            Money::new(free, Currency::USD),
            now,
            now,
            ProposalId::from_string(format!("prop-{}", self.cycle)),
        )?;

        // Hold what the proposal was sized for, keyed by its id, until the
        // act stage commits or releases it. A proposal whose capital cannot
        // be held must not enter the pipeline: refusing here is the control
        // working, not an error to route around. Zero-notional proposals —
        // nothing-to-do cycles — hold nothing.
        let notional = outcome.proposal.traded_notional();
        if notional.is_positive() {
            self.reservations.reserve(
                outcome.proposal.proposal_id.as_str().to_string(),
                notional,
                now,
                // One day bounds an abandoned proposal's hold; the act stage
                // resolves a live one within the same cycle, so the expiry
                // only matters when a proposal is constructed and never
                // offered — the exact leak `expire_due` exists to drain.
                Duration::from_hours(24),
            )?;
        }
        Ok(outcome)
    }

    fn thesis_from(
        &self,
        opportunity: &Opportunity,
        reasoned: &qip_reasoning_engine::engine::ReasoningOutcome,
    ) -> Result<qip_portfolio_engine::construction::ApprovedThesis> {
        let object_id = opportunity
            .affected_objects
            .first()
            .ok_or_else(|| Error::invalid("the opportunity names no instrument to size"))?;
        let sign = reasoned.hypothesis.claim.implied_sign().ok_or_else(|| {
            Error::invalid(format!(
                "a {} claim has no inherent direction; what to do about it depends on the book",
                reasoned.hypothesis.claim
            ))
        })?;
        let price = self
            .price_history
            .get(object_id.as_str())
            .and_then(|series| series.last())
            .copied()
            .ok_or_else(|| {
                Error::not_found(format!(
                    "{object_id} has no price history; a thesis cannot be sized without a \
                     reference price"
                ))
            })?;
        let anomaly = opportunity
            .anomalies
            .first()
            .ok_or_else(|| Error::invalid("the opportunity carries no anomaly to measure"))?;
        Ok(qip_portfolio_engine::construction::ApprovedThesis {
            hypothesis_id: reasoned.hypothesis.hypothesis_id.to_string(),
            object_id: object_id.clone(),
            conviction: sign * reasoned.hypothesis.effective_confidence(),
            // The reversion of what was measured: the anomaly observed a
            // displacement from expectation, and the thesis is that it closes.
            // Bounded to the mandate-scale range construction validates, so an
            // extreme print proposes a large-but-finite return rather than an
            // absurd one.
            expected_return: (anomaly.expected - anomaly.observed).clamp(-0.5, 0.5),
            price: Decimal::from_f64(price).ok_or_else(|| {
                Error::numeric(format!(
                    "{object_id}'s last close {price} is not representable as a Decimal"
                ))
            })?,
        })
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

    /// Write a hypothesis down as something a source can later contradict.
    ///
    /// The criterion is written against the same series the detector measured,
    /// at the level it actually observed, in the direction the claim implies.
    /// That pairing is not a convenience: a claim about volatility scored
    /// against a closing price would produce a number, and the number would be
    /// about a different question than the one the hypothesis asked.
    ///
    /// `Ok(false)` where the claim has no direction. What to do about a regime
    /// shift or an event depends on the book, so there is no level a later
    /// observation could contradict, and inventing one would make the platform
    /// scoreable on a question it never asked.
    fn record_prediction(
        &mut self,
        opportunity: &Opportunity,
        reasoned: &ReasoningOutcome,
        now: Timestamp,
    ) -> Result<bool> {
        let (observable, comparison) = match reasoned.hypothesis.claim {
            Claim::Undervalued => ("close", Comparison::GreaterThan),
            Claim::Overvalued => ("close", Comparison::LessThan),
            Claim::VolatilityUnderpriced => ("volatility", Comparison::GreaterThan),
            Claim::VolatilityOverpriced => ("volatility", Comparison::LessThan),
            Claim::SpreadWidens => ("spread", Comparison::GreaterThan),
            Claim::SpreadNarrows => ("spread", Comparison::LessThan),
            Claim::RegimeShift | Claim::EventOccurs => return Ok(false),
        };
        let Some(anomaly) = opportunity.anomalies.first() else {
            return Ok(false);
        };
        let Some(reference) = Decimal::from_f64(anomaly.observed) else {
            return Ok(false);
        };

        // The metric names the observable and the series, so a proposition
        // about one instrument cannot be settled by an observation about
        // another, and one about volatility cannot be settled by a price. The
        // source is required to publish it, which the constructor checks.
        let metric = format!("{observable}:{}", anomaly.subject);
        let proposition = Proposition::new(
            format!(
                "{} — {metric} is {} {reference} by the horizon",
                reasoned.hypothesis.statement,
                comparison.as_str()
            ),
            ResolutionCriteria::Threshold {
                metric: metric.clone(),
                comparison,
                value: reference,
            },
            ResolutionSource::new("platform-market-data", SourceKind::Official, vec![metric]),
            now.saturating_add(reasoned.hypothesis.horizon),
            // A claim the source has not published enough to settle rolls
            // forward. Resolving it as failure is how a system marks itself
            // right by scoring the questions nobody answered.
            SettlementRule::unit(UndeterminedRule::RollForward),
            Duration::from_days(1),
        )?;
        self.keep_prediction(RecordedPrediction {
            hypothesis: reasoned.hypothesis.hypothesis_id.as_str().to_string(),
            cycle: self.cycle,
            proposition,
            recorded_at: now,
            verdict: None,
            scored_at: None,
        });
        Ok(true)
    }

    /// Keep a falsifiable claim, bounded by [`PREDICTION_HISTORY`].
    ///
    /// The only way a prediction enters the working set, and eviction happens
    /// here at the insert so no cycle ever walks more than the cap — the
    /// REASON stage counts the open ones every time it records a claim, and a
    /// scoring pass walks all of them. Oldest first: with roughly one claim a
    /// cycle, the evicted claim's horizon passed over a thousand cycles ago
    /// and its source never published enough to settle it.
    fn keep_prediction(&mut self, prediction: RecordedPrediction) {
        self.predictions.push(prediction);
        if self.predictions.len() > PREDICTION_HISTORY {
            let excess = self.predictions.len() - PREDICTION_HISTORY;
            self.predictions.drain(..excess);
        }
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
        // Anchor the reservation ledger to the book before anything is sized,
        // on every pass including quiet ones: free = equity minus active
        // holds, one claim about one balance. The failure mode — holds
        // exceeding equity after a drawdown — floors free at zero, which
        // refuses new reservations, and is counted where the alerts look.
        if self
            .reservations
            .resync_free(self.capital.equity(), now)
            .is_err()
        {
            self.telemetry.metrics.count(
                names::RESERVATION_SHORTFALL,
                labels([("reason", "holds_exceed_equity")]),
            );
        }
        // Construction expresses approved theses. With none pending there is
        // nothing to size, and that is a normal state. The equity is the
        // tracked number — the same one the risk monitor watches — so a
        // proposal sized after a losing run is sized against the book that
        // lost, not against the book at assembly.
        //
        // Theses are drained, not read: a thesis expresses once. Re-expressing
        // the queue every cycle would pyramid the same idea until the mandate
        // cap alone stopped it.
        let theses = std::mem::take(&mut self.pending_theses);
        let proposal = if theses.is_empty() {
            self.constructor.nothing_to_do(
                ProposalId::from_string(format!("prop-{}", self.cycle)),
                Money::new(self.capital.equity(), Currency::USD),
                now,
                now,
                "no thesis cleared the action bar this cycle",
            )
        } else {
            match self.construct_from(&theses, now) {
                Ok(outcome) => outcome.proposal,
                // A refusal is a normal state, and it is *this* cycle's
                // record: the proposal says why nothing was sized, and the
                // theses are not requeued — an idea that could not be sized
                // against this history will not size better against the same
                // history next cycle, and the event log keeps the attempt.
                Err(error) => self.constructor.nothing_to_do(
                    ProposalId::from_string(format!("prop-{}", self.cycle)),
                    Money::new(self.capital.equity(), Currency::USD),
                    now,
                    now,
                    format!(
                        "{} thesis(es) approved and none sized: {}",
                        theses.len(),
                        error.message()
                    ),
                ),
            }
        };
        let legs = proposal.len();
        self.proposals.push(proposal);
        self.proposals_made += 1;
        // Age the oldest out rather than letting the working set grow with
        // uptime. Nothing is lost: the event log keeps the full history.
        if self.proposals.len() > PROPOSAL_HISTORY {
            self.proposals
                .drain(..self.proposals.len() - PROPOSAL_HISTORY);
        }

        // Where capital will have to be, and when. Reported next to what is
        // being proposed because the two decisions are the same decision seen
        // from opposite ends: a leg that cannot be funded where it has to trade
        // is a leg that will not trade.
        let forecasts = self.forecast_capital_demand(now, CAPITAL_HORIZON);
        let funding = if forecasts.is_empty() {
            String::new()
        } else {
            let confident: Decimal = forecasts
                .iter()
                .map(|forecast| forecast.interval().lower())
                .fold(Decimal::ZERO, |sum, lower| sum + lower);
            format!(
                "; {} funding lane(s) forecast, at least {confident} needed within {:.0} day(s)",
                forecasts.len(),
                CAPITAL_HORIZON.as_days_f64()
            )
        };

        StageOutcome::ran(
            Stage::Decide,
            legs,
            if legs == 0 {
                format!("no thesis cleared the action bar; nothing to propose{funding}")
            } else {
                format!("{legs} leg(s) proposed{funding}")
            },
        )
    }

    fn stage_act(&mut self, now: Timestamp, correlation: &CorrelationId) -> StageOutcome {
        // The risk monitor runs whether or not there is anything to trade: a
        // book that became unacceptable while it sat is exactly what this
        // stage exists to catch.
        let risk_state = self.risk_state();
        let action = self
            .monitor
            .observe(&risk_state, "platform", self.autonomy.level(), now);
        self.monitor
            .enforce(&action, self.autonomy.kill_switch_mut(), now);

        self.telemetry
            .metrics
            .count(names::RISK_EVALUATIONS, labels([]));
        // The gauge the `qip_limit_breaches` alert policy queries, read off the
        // observation the monitor just recorded rather than recounted from the
        // action it returned. `MonitorAction` carries breaches as sentences and
        // only on two of its five arms, so counting them from there would
        // report zero breaches on a global halt — the one moment the number
        // matters most. The observation holds the whole `LimitCheck`.
        //
        // Set unconditionally, including to zero. A gauge only written when
        // something is wrong never falls back, and an alert on `max() > 0`
        // would stay lit after the breach cleared.
        let blocking = self
            .monitor
            .observations()
            .last()
            .map_or(0, |observation| observation.check.blocking().len());
        self.telemetry
            .metrics
            .gauge(names::LIMIT_BREACHES, labels([]), blocking as f64);

        let mut sign_off_problems: Vec<String> = Vec::new();
        // Sign off the drafts, or do not. `Proposal::approve` requires two
        // controls because a single approver is a single point of failure, and
        // until this call existed nothing in the platform called it at all:
        // every proposal stayed a draft, `is_releasable` was permanently
        // false, and the release loop below was unreachable code. The platform
        // sized positions it could never act on.
        //
        // Both signatures are deterministic and neither is a model. The risk
        // monitor has already ruled above, on this cycle's own book; the
        // compliance report must cover all six governance controls and find
        // every one of them enforced. A model may propose, but it does not
        // sign — which is the property `.claude/rules/domains/risk-and-execution.md`
        // exists to protect.
        let compliance = self.compliance_report(now).and_then(|report| {
            report.require_fully_enforced()?;
            Ok(())
        });
        let signable = action.permits_new_risk() && compliance.is_ok();
        if !signable {
            let reason = if action.permits_new_risk() {
                compliance
                    .err()
                    .map(|error| error.message().to_string())
                    .unwrap_or_else(|| "compliance did not sign".to_string())
            } else {
                format!("the risk monitor is at {}", action.as_str())
            };
            // Said once per cycle, not once per proposal: a stage that repeats
            // the same refusal for every draft buries the reason in its own
            // noise.
            sign_off_problems.push(format!("no proposal was signed off: {reason}"));
            // Labelled by which of the two signatures was withheld, never by
            // the reason text. The reason is a message that names a particular
            // control and a particular number, so as a label it would mint a
            // new series per distinct failure and bury the shape of the problem
            // in its own detail. Which control refused is the question a
            // dashboard answers; why is the question the stage report and the
            // event log answer.
            self.telemetry.metrics.count(
                names::PROPOSALS_UNSIGNED,
                labels([(
                    "control",
                    if action.permits_new_risk() {
                        "compliance"
                    } else {
                        "risk-monitor"
                    },
                )]),
            );
        }
        if signable {
            let at = now;
            let mut signed = 0u64;
            for proposal in &mut self.proposals {
                if !matches!(proposal.status, ProposalStatus::Draft) {
                    continue;
                }
                // A proposal with no legs proposes nothing. `stage_decide`
                // records one on every quiet cycle so the log says the cycle
                // ran and chose not to trade, and signing those off would put
                // a risk and a compliance signature against a decision neither
                // control examined — filling the audit trail with approvals
                // that approved nothing, and making a real approval harder to
                // find rather than easier.
                if proposal.is_empty() {
                    continue;
                }
                match proposal.approve(
                    at,
                    vec!["risk-monitor".to_string(), "compliance".to_string()],
                ) {
                    // Counted from `approve` returning, not from the draft
                    // being offered to it. A proposal that reached this call
                    // and was rejected by it is not a signed proposal, and a
                    // counter incremented before the call would say the
                    // signatures were taken whenever they were merely asked
                    // for.
                    Ok(()) => signed += 1,
                    Err(error) => sign_off_problems.push(format!(
                        "{} could not be approved: {}",
                        proposal.proposal_id.as_str(),
                        error.message()
                    )),
                }
            }
            if signed > 0 {
                self.telemetry
                    .metrics
                    .increment(names::PROPOSALS_SIGNED, labels([]), signed);
            }
        }

        let releasable = self
            .proposals
            .iter()
            .filter(|proposal| proposal.status.is_releasable())
            .count();

        // A control that ruled and left no record is a refusal nobody can
        // count. The ruling goes on the same hash chain as the fills, because
        // "what did we decline" is answerable only if the two live together.
        if !action.permits_new_risk() {
            self.capture(
                now,
                correlation,
                ObjectId::from_string("book"),
                Action::RiskDecision {
                    control: "risk-monitor".to_string(),
                    allowed: false,
                    reason: action.as_str().to_string(),
                },
                RealisedOutcome::nothing_happened(now),
                format!(
                    "the risk monitor moved to {} while {releasable} proposal(s) were releasable",
                    action.as_str()
                ),
            );
        }

        if releasable == 0 {
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
            // Why nothing was signable reaches the report on this path too.
            // Without it, a cycle blocked by an unenforced compliance control
            // is indistinguishable from a quiet cycle with nothing to trade,
            // and those need different actions from an operator.
            for problem in sign_off_problems {
                outcome = outcome.with_problem(problem);
            }
            return outcome;
        }

        // Release them. Until this loop existed the stage counted approved
        // proposals and returned, so the platform sensed, reasoned, sized and
        // approved — and then never acted, not even against the simulator.
        // Nine rows of the canonical architecture starved on this one seam,
        // and LEARN had nothing to attribute because no fill the cycle
        // produced ever existed.
        //
        // Every order goes through `submit_order`, which is the only path to a
        // venue: it re-reads risk state, runs the deterministic pre-trade
        // controls, consults the autonomy ceiling and the kill switch, and
        // records the result — accepted or refused — on the same hash chain.
        // There is deliberately no second path. A release loop that built
        // orders and handed them to the broker directly would be a way around
        // every control in the paragraph above.
        let mut released = 0usize;
        let mut refused = 0usize;
        let mut problems: Vec<String> = sign_off_problems.clone();

        let approved: Vec<Proposal> = self
            .proposals
            .iter()
            .filter(|proposal| proposal.status.is_releasable())
            .cloned()
            .collect();

        // Which proposals actually put an order on the book, so the capital
        // held for each can be committed or returned below. BTreeMap because
        // the resolution order reaches the journal.
        let mut placed_by_proposal: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for proposal in &approved {
            for (index, leg) in proposal.legs.iter().enumerate() {
                // A leg that would trade nothing is not an order. Submitting a
                // zero-quantity order would consume a control decision and a
                // ledger row to accomplish nothing, and would make the
                // released count a lie about how much the platform did.
                if !leg.quantity.is_positive() {
                    continue;
                }

                // Derived from the proposal and the leg's position within it,
                // never from a clock or a counter. Two runs of the same cycle
                // over the same inputs produce the same order ids, which is
                // what lets a replay be compared to the original rather than
                // merely resembling it. It is also the idempotency key: a
                // retry cannot manufacture a second order for the same leg.
                let order_id =
                    OrderId::from_string(format!("ord-{}-{index}", proposal.proposal_id.as_str()));

                let order = Order::new(
                    order_id,
                    leg.object_id.clone(),
                    Self::release_side(leg.side),
                    leg.quantity,
                    // Market, because the simulated broker fills against the
                    // book it was given and a limit price invented here would
                    // be a number with no source. A working-algo order type is
                    // a scheduling decision the execution engine owns, and it
                    // belongs there rather than in the stage that releases.
                    OrderType::Market,
                    leg.reference_price,
                    proposal.proposal_id.as_str().to_string(),
                    leg.hypotheses.clone(),
                    "platform",
                    now,
                );

                match self.submit_order(order, now) {
                    Ok(()) => {
                        released += 1;
                        *placed_by_proposal
                            .entry(proposal.proposal_id.as_str().to_string())
                            .or_insert(0) += 1;
                    }
                    // A refusal is an outcome, not an error to abort on. The
                    // control said no and `submit_order` has already put that
                    // on the record; stopping the loop here would let one
                    // refused leg silently prevent every later proposal from
                    // being offered to the controls at all.
                    Err(error) => {
                        refused += 1;
                        problems.push(format!(
                            "{} leg {index} was refused: {}",
                            proposal.proposal_id.as_str(),
                            error.message()
                        ));
                    }
                }
            }
        }

        // Move every proposal offered to the controls out of `Approved`, so a
        // later cycle cannot offer it again. Without this the same approved
        // proposal is re-released on every subsequent cycle, and the platform
        // pyramids one decision into a position nobody sized — the exact
        // duplicate-order failure the idempotent order id is a second line of
        // defence against, not a substitute for.
        //
        // A refused leg still marks its proposal released. It was offered to
        // the controls and they ruled; re-offering the same proposal next
        // cycle would ask the same question of the same book and record the
        // same refusal indefinitely.
        let offered: Vec<String> = approved
            .iter()
            .map(|proposal| proposal.proposal_id.as_str().to_string())
            .collect();

        // Resolve each offered proposal's capital hold: committed where at
        // least one leg became an order — that capital is now in the book and
        // does not return to free — and released where every leg was refused
        // or empty, because a control saying no must hand the capital back.
        // A hold that is simply missing is recorded rather than invented:
        // proposals sized before this wiring existed have none, and a
        // problem line beats a phantom balance. Lapsed holds are swept on the
        // same clock so an abandoned proposal's capital returns at expiry.
        for (proposal_id, amount) in self.reservations.expire_due(now) {
            problems.push(format!(
                "the hold for {proposal_id} ({amount}) lapsed unresolved and was swept"
            ));
        }
        for proposal in &approved {
            // A proposal that held nothing has nothing to resolve — the
            // nothing-to-do proposal a quiet cycle records is the common case,
            // and a problem line for it every cycle would bury the real ones.
            if !proposal.traded_notional().is_positive() {
                continue;
            }
            let proposal_id = proposal.proposal_id.as_str();
            let resolved = if placed_by_proposal.contains_key(proposal_id) {
                self.reservations.commit(proposal_id, now).map(|_| ())
            } else {
                self.reservations.release(proposal_id, now).map(|_| ())
            };
            if let Err(error) = resolved {
                problems.push(format!(
                    "the hold for {proposal_id} could not be resolved: {}",
                    error.message()
                ));
            }
        }
        for proposal in &mut self.proposals {
            if offered.contains(&proposal.proposal_id.as_str().to_string()) {
                if let Err(error) = proposal.release(now) {
                    problems.push(format!(
                        "{} was released but its status could not be advanced: {}",
                        proposal.proposal_id.as_str(),
                        error.message()
                    ));
                }
            }
        }

        let mut outcome = StageOutcome::ran(
            Stage::Act,
            released,
            format!(
                "{released} order(s) released from {} approved proposal(s), {refused} refused; \
                 risk monitor says {}",
                approved.len(),
                action.as_str()
            ),
        );
        for problem in problems {
            outcome = outcome.with_problem(problem);
        }
        outcome
    }

    /// Carry a proposal leg's direction across to the execution engine's own.
    ///
    /// Two crates declare a `Side` and neither may depend on the other — a service
    /// owning its domain means owning its vocabulary. The kernel is the only place
    /// that composes both, so the translation lives here, which is the same reason
    /// the rest of the wiring does.
    ///
    /// Matched exhaustively and deliberately: a third direction added to either
    /// enum becomes a compile error here rather than falling through a wildcard to
    /// a default. A `_ => Buy` arm in this function would turn a new sell-like
    /// variant into a purchase, silently, in the one function standing between a
    /// sizing decision and an order.
    const fn release_side(side: qip_portfolio_engine::proposal::Side) -> Side {
        match side {
            qip_portfolio_engine::proposal::Side::Buy => Side::Buy,
            qip_portfolio_engine::proposal::Side::Sell => Side::Sell,
        }
    }

    fn stage_learn(&mut self, now: Timestamp) -> StageOutcome {
        let outcome = self.attribute(now);
        // What the platform did and what it declined, side by side. The tally
        // is the answer to the question a report of trades alone cannot
        // answer: whether the gates are calibrated or merely shut.
        let captured = self.outcomes.len();
        let refused = self.outcomes.refusals().len();
        let taken = self.outcomes.taken().len();
        let mut outcome = if captured == 0 {
            outcome
        } else {
            let detail = format!(
                "{}; {captured} outcome(s) captured ({taken} taken, {refused} declined)",
                outcome.detail
            );
            StageOutcome { detail, ..outcome }
        };
        for problem in std::mem::take(&mut self.capture_problems) {
            outcome = outcome.with_problem(problem);
        }
        outcome
    }

    /// Attribute what the fills cost. The body of LEARN, without the capture
    /// reporting wrapped around it.
    fn attribute(&mut self, now: Timestamp) -> StageOutcome {
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
                // The order behind the fill already carries its hypotheses —
                // `Order::hypotheses` is set from the proposal leg at release
                // and documents itself as required, "an untraceable order is
                // one nobody can explain after the fact". This site used to
                // pass an empty vector, so `by_hypothesis` skipped every
                // position and returned nothing for everything the platform
                // traded: the join that makes learning possible was empty on a
                // platform whose whole purpose is saying why it did what it
                // did. An order that cannot be found contributes no hypothesis
                // rather than a guessed one.
                hypotheses: self
                    .orders
                    .order(&fill.order_id)
                    .map(|order| order.hypotheses.clone())
                    .unwrap_or_default(),
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
            .attribute(&periods, total, self.capital.equity(), now, now)
        {
            Ok(attribution) => {
                // The hypothesis count is reported because it is the join that
                // makes learning possible, and because it was silently zero:
                // this stage said "N fill(s) attributed" every cycle while
                // `by_hypothesis` returned an empty map, so the number that
                // was wrong was the one nobody printed. An operator reading
                // fills attributed across no hypotheses now sees the gap.
                let hypotheses = attribution.by_hypothesis().len();
                StageOutcome::ran(
                    Stage::Learn,
                    attribution.positions.len(),
                    format!(
                        "{} fill(s) attributed across {} hypothesis(es), {} of implementation \
                         cost, residual {}",
                        attribution.positions.len(),
                        hypotheses,
                        attribution.implementation_cost(),
                        attribution.residual()
                    ),
                )
            }
            Err(error) => StageOutcome::ran(Stage::Learn, 0, "attribution failed")
                .with_problem(error.message().to_string()),
        }
    }

    /// The current risk state, as the control functions see it.
    ///
    /// Tracked from realised fills, deterministically: equity is the
    /// configured initial book plus realised P&L minus costs paid, positions
    /// are carried at average entry cost, and drawdown is realised equity
    /// against its own peak. Until this existed the monitor ran every cycle
    /// against a hardcoded ten million — real-time in cadence, constant in
    /// content.
    ///
    /// Stated exclusions, because an honest smaller claim beats a fabricated
    /// larger one: no unrealised P&L and no mark-to-market exposure — the
    /// platform holds no marks, so an adverse move on an open position is
    /// invisible here until a fill realises it; and no daily-loss figure —
    /// the loop owns no day-boundary convention, and a "daily" number cut at
    /// an arbitrary anchor would be a statement about the anchor.
    fn risk_state(&self) -> RiskState {
        let mut position_notionals = BTreeMap::new();
        let mut gross = Decimal::ZERO;
        let mut net = Decimal::ZERO;
        for (object, lot) in &self.capital.positions {
            let notional = lot.quantity * lot.average_price;
            gross += notional.abs();
            net += notional;
            position_notionals.insert(object.clone(), notional.abs());
        }
        // The tail statistics the limits read. Until these were populated,
        // `LimitKind::MaxValueAtRisk` and `LimitKind::MaxExpectedShortfall`
        // both looked their figure up in a map that was always empty, took the
        // `None` arm, and recorded nothing — so two limits that
        // `LimitSet::conservative_default` ships by default, and that every
        // deployment therefore believed it had, could never fire. A control
        // that cannot fire reads as protection and is not.
        //
        // The keys are derived from each configured limit's own confidence,
        // formatted exactly as the limit will format it. Computing a fixed set
        // of confidences here instead would put the key on one side of a
        // rounding boundary and the lookup on the other — `{:.2}` of 0.975 is
        // one such value, and the default expected-shortfall limit uses it —
        // and the limit would go on silently never evaluating with no visible
        // difference from today.
        let returns = self.equity_returns();
        let mut value_at_risk = BTreeMap::new();
        let mut expected_shortfall = BTreeMap::new();
        if returns.len() >= 2 {
            for limit in &self.monitor.limits().limits {
                match limit.kind {
                    LimitKind::MaxValueAtRisk { confidence, .. } => {
                        value_at_risk.insert(
                            format!("{confidence:.2}"),
                            qip_risk::metrics::historical_var(&returns, confidence),
                        );
                    }
                    LimitKind::MaxExpectedShortfall { confidence, .. } => {
                        expected_shortfall.insert(
                            format!("{confidence:.2}"),
                            qip_risk::metrics::expected_shortfall(&returns, confidence),
                        );
                    }
                    _ => {}
                }
            }
        }

        RiskState {
            equity: self.capital.equity(),
            cash: self.capital.cash,
            gross_exposure: gross,
            net_exposure: net,
            position_notionals,
            drawdown: self.capital.drawdown(),
            value_at_risk,
            expected_shortfall,
            ..RiskState::default()
        }
    }

    /// Submit one order through the full control path.
    ///
    /// Exposed so the ACT stage's controls can be exercised directly, and so a
    /// test can prove an order cannot reach a venue it should not.
    ///
    /// Both outcomes are recorded on the outcome capture's hash chain: a fill
    /// and a refusal are the same kind of fact about the platform's behaviour,
    /// and a record that kept only the first could say what the platform earned
    /// and not what it declined.
    pub fn submit_order(&mut self, order: Order, now: Timestamp) -> Result<()> {
        let risk_state = self.risk_state();
        let object_id = order.object_id.clone();
        let side = order.side;
        let quantity = order.quantity;
        let arrival = order.arrival_price;
        let result = self.orders.submit(
            order,
            self.broker.as_mut(),
            &self.autonomy,
            &risk_state,
            BTreeMap::new(),
            None,
            now,
        );

        self.capture_submission(&result, &object_id, side, quantity, arrival, now);

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

    /// Put one submission on the record, whichever way it went.
    ///
    /// A refusal produces a [`RealisedOutcome::nothing_happened`] rather than
    /// no row. A refusal with no outcome row is indistinguishable from a
    /// decision nobody made, and telling those apart is the entire reason
    /// refusals are captured.
    fn capture_submission(
        &mut self,
        result: &SubmissionResult,
        object_id: &ObjectId,
        side: Side,
        quantity: Decimal,
        arrival: Decimal,
        now: Timestamp,
    ) {
        let correlation = self.correlation_for(now);

        let Some(venue) = result.venue.clone() else {
            let (gate, reason) = result.refusal.as_ref().map_or_else(
                || {
                    (
                        "unnamed".to_string(),
                        "refused with no reason recorded".to_string(),
                    )
                },
                |refusal| (gate_of(refusal), refusal.describe()),
            );
            // Labelled with the same gate name the twin's `Rejected` record
            // carries, from the same `gate_of` call, so the count on a
            // dashboard and the rows in the event log cannot name different
            // controls for the same refusal. `gate_of` matches `RefusalReason`
            // exhaustively, so the label's cardinality is that enum's and a new
            // refusal reason is a compile error there rather than a new series
            // appearing here unannounced.
            //
            // Recorded on the refusal path specifically, not inferred later
            // from the absence of a fill: an order that was accepted and simply
            // did not fill is not an order a control refused, and a metric that
            // could not tell those apart would make the controls look like they
            // fire constantly.
            self.telemetry
                .metrics
                .count(names::ORDERS_REFUSED, labels([("control", gate.as_str())]));
            self.capture(
                now,
                &correlation,
                object_id.clone(),
                Action::Rejected {
                    order_id: result.order_id.clone(),
                    gate,
                    reason: reason.clone(),
                },
                RealisedOutcome::nothing_happened(now),
                reason,
            );
            return;
        };

        let venue = VenueId::new(venue);
        // Every control said yes and the order reached a venue. Counted here,
        // at the one place in the platform that learns an order was accepted,
        // rather than in the release loop that asked: the release loop knows
        // what it offered, and what was offered and what was accepted are the
        // two numbers that must never be sourced from the same count.
        self.telemetry
            .metrics
            .count(names::ORDERS_SUBMITTED, labels([("venue", venue.as_str())]));

        for fill in &result.fills {
            self.telemetry
                .metrics
                .count(names::ORDERS_FILLED, labels([("venue", venue.as_str())]));
            // The counter the `qip_live_fills_total` alert policy queries, and
            // in a paper deployment it must stay at zero forever. Read from
            // `fill.simulated` — the fill's own account of where it came from —
            // rather than from the autonomy ceiling or any other configured
            // value, for the reason `OrderManager::has_live_fills` gives: a
            // configuration is exactly the thing that gets confused between a
            // test and a deployment, and a paper-trading assertion that trusts
            // configuration asserts nothing about what actually happened.
            //
            // This does not gate anything; three other layers do that. It is
            // the alarm for the case where all three somehow did not.
            if !fill.simulated {
                self.telemetry
                    .metrics
                    .count(names::LIVE_FILLS, labels([("venue", venue.as_str())]));
            }
        }

        let placed = self.capture(
            now,
            &correlation,
            object_id.clone(),
            Action::OrderPlaced {
                order_id: result.order_id.clone(),
                venue: venue.clone(),
                side: book_side(side),
                quantity,
                method: "market".to_string(),
            },
            RealisedOutcome::nothing_happened(now),
            "the control path accepted the order and it reached a venue",
        );

        for fill in &result.fills {
            // P&L is not realised by opening a position, so the only money that
            // has moved is what the fill cost. Reporting an opening trade's
            // gross as profit is the direction that flatters.
            let slippage = slippage_bps(arrival, fill.price, side);
            let outcome = RealisedOutcome::realised(now, Decimal::ZERO, fill.costs, fill.quantity)
                .with_slippage_bps(slippage);
            self.capture_after(
                placed.as_ref(),
                now,
                &correlation,
                object_id.clone(),
                Action::Filled {
                    order_id: result.order_id.clone(),
                    venue: venue.clone(),
                    quantity: fill.quantity,
                    price: fill.price,
                },
                outcome,
                format!("filled at {} against an arrival of {arrival}", fill.price),
            );

            // A fill is capital that had to be at a venue on a date. That is
            // exactly the observation the demand forecaster is fitted on, and
            // it is the only source of it the loop actually sees.
            self.record_capital_demand(
                CapitalLocation::new(
                    CapitalRegion::new(HOME_REGION),
                    Currency::USD,
                    venue.clone(),
                ),
                DemandKind::Cash,
                fill.at,
                fill.notional().abs(),
            );

            // And it is capital that moved. This is the edge that makes the
            // risk state real: the same fills the outcome capture records are
            // the fills the monitor's equity is built from, so the two can
            // never tell different stories.
            self.capital.apply_fill(
                object_id.as_str(),
                side,
                fill.price,
                fill.quantity,
                fill.costs,
            );
        }
    }

    /// The correlation the current work belongs to.
    ///
    /// The running cycle's, where there is one. Outside a cycle a fresh key is
    /// minted rather than reusing a stale one, so a manual submission is not
    /// filed under the last cycle that happened to run.
    fn correlation_for(&self, now: Timestamp) -> CorrelationId {
        self.last_correlation.clone().unwrap_or_else(|| {
            self.context
                .ids()
                .generate::<qip_core::lineage::CorrelationKind>(now)
        })
    }

    /// Record one decision and its outcome, returning it for chaining.
    ///
    /// Never panics and never propagates: a capture that failed is a problem
    /// the LEARN stage reports, not a reason to abandon the order it was
    /// describing.
    fn capture(
        &mut self,
        now: Timestamp,
        correlation: &CorrelationId,
        object_id: ObjectId,
        action: Action,
        outcome: RealisedOutcome,
        rationale: impl Into<String>,
    ) -> Option<Decision> {
        self.capture_after(
            None,
            now,
            correlation,
            object_id,
            action,
            outcome,
            rationale,
        )
    }

    /// Record a decision that followed from another.
    fn capture_after(
        &mut self,
        parent: Option<&Decision>,
        now: Timestamp,
        correlation: &CorrelationId,
        object_id: ObjectId,
        action: Action,
        outcome: RealisedOutcome,
        rationale: impl Into<String>,
    ) -> Option<Decision> {
        let mut decision = Decision::new(
            self.context.ids().generate::<DecisionKind>(now),
            TraceId::new(format!("trace-{}", correlation.as_str())),
            correlation.clone(),
            now,
            object_id,
            action,
        )
        .because(rationale);
        if let Some(parent) = parent {
            decision = decision.after(parent);
        }
        let recorded = decision.clone();
        match self.outcomes.record(decision, outcome) {
            Ok(_) => Some(recorded),
            Err(error) => {
                self.capture_problems
                    .push(format!("an outcome was not captured: {}", error.message()));
                None
            }
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
    ///
    /// `None` before the first cycle. Every record the cycle produced — the
    /// journal envelope, every captured outcome — carries it, so one key
    /// reconstructs the whole pass from either log.
    pub fn last_correlation(&self) -> Option<CorrelationId> {
        self.last_correlation.clone()
    }

    // --- the data finder, and the catalogue it feeds ------------------------

    /// The source registry: what this deployment has decided to collect.
    pub fn data_finder(&self) -> &DataFinder {
        &self.data_finder
    }

    /// Sources currently registered, by identifier.
    pub fn registered_sources(&self) -> &BTreeMap<String, RegisteredSource> {
        self.data_finder.registry()
    }

    /// What datasets the mesh knows about.
    ///
    /// Not a second catalogue. Every entry in here came from a registration
    /// decision's own [`qip_data_finder::RegistrationDecision::catalogue_entry`],
    /// so there is one answer to "what may this dataset be used for" rather
    /// than two that can disagree.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Run the source lifecycle over a candidate set and catalogue what
    /// registers.
    ///
    /// The probe is the caller's: this opens no sockets, and a deployment
    /// supplies the transport exactly as a test supplies a script. Anything
    /// that registers produces a mesh catalogue entry in the same call, because
    /// a source the platform is collecting and a dataset the mesh has never
    /// heard of is precisely the gap this composition closes.
    ///
    /// A catalogue refusal is recorded rather than raised: one unusable entry
    /// must not lose the decisions for every other candidate.
    pub fn assess_sources(
        &mut self,
        candidates: Vec<SourceCandidate>,
        probe: &mut dyn SourceProbe,
        now: Timestamp,
    ) -> Result<SourceAssessment> {
        let decisions = self.data_finder.assess(candidates, probe, now)?;
        let mut catalogued = Vec::new();
        let mut catalogue_problems = Vec::new();
        for decision in &decisions {
            if !decision.is_registered() {
                continue;
            }
            match decision.catalogue_entry(&self.config.owner) {
                Ok(registration) => {
                    let dataset = registration.dataset.clone();
                    match self.catalog.register(registration) {
                        Ok(()) => catalogued.push(dataset),
                        Err(error) => {
                            catalogue_problems.push(format!("{dataset}: {}", error.message()))
                        }
                    }
                }
                Err(error) => catalogue_problems.push(error.message().to_string()),
            }
        }
        Ok(SourceAssessment {
            decisions,
            catalogued,
            catalogue_problems,
        })
    }

    // --- the chain ----------------------------------------------------------

    /// The canonical chain, once one has been observed.
    pub fn chain(&self) -> Option<&ChainState> {
        self.chain.as_ref()
    }

    /// The confirmation depth this deployment requires.
    pub fn confirmations(&self) -> Confirmations {
        self.confirmations
    }

    /// Chain state buried at least [`Platform::confirmations`] deep.
    ///
    /// There is no accessor here that returns chain state without a depth. A
    /// caller that genuinely wants the head asks `qip-chain` for
    /// [`Confirmations::AT_RISK`] and says so in its own code; this one cannot
    /// be used to forget that the past is revisable.
    pub fn confirmed_chain(&self) -> Result<ConfirmedView> {
        let chain = self.chain.as_ref().ok_or_else(|| {
            Error::not_found("no chain has been observed, so there is no state to confirm")
        })?;
        chain.view(self.confirmations)
    }

    /// Absorb chain observations.
    ///
    /// The updates are `qip-chain`'s own: this invents no transport and decides
    /// nothing about canonicality, which is [`ChainState`]'s job — a block that
    /// extends, forks or displaces is worked out there and reported here. A
    /// refused block is recorded and the rest are still applied, because one
    /// malformed block from a node must not cost the platform the batch.
    pub fn observe_chain(&mut self, updates: Vec<ChainUpdate>) -> ChainAbsorption {
        let mut absorption = ChainAbsorption {
            extended: 0,
            side_branch: 0,
            duplicates: 0,
            non_block: 0,
            reorgs: 0,
            deepest_reorg: 0,
            invalidated_trades: 0,
            confirmed_trades: None,
            unconfirmable: None,
            problems: Vec::new(),
        };

        for update in updates {
            let ChainUpdate::Block(block) = update else {
                absorption.non_block += 1;
                continue;
            };
            let chain = self
                .chain
                .get_or_insert_with(|| ChainState::new(block.chain.clone(), CHAIN_RETENTION));
            match chain.apply(*block) {
                Ok(qip_chain::Applied::Extended { .. }) => absorption.extended += 1,
                Ok(qip_chain::Applied::SideBranch { .. }) => absorption.side_branch += 1,
                Ok(qip_chain::Applied::Duplicate) => absorption.duplicates += 1,
                Ok(qip_chain::Applied::Reorganised(reorg)) => {
                    absorption.extended += 1;
                    absorption.reorgs += 1;
                    absorption.deepest_reorg = absorption.deepest_reorg.max(reorg.depth());
                    absorption.invalidated_trades += reorg.invalidated_trades;
                }
                Err(error) => absorption.problems.push(error.message().to_string()),
            }
        }

        match self.confirmed_chain() {
            Ok(view) => absorption.confirmed_trades = Some(view.state().trades()),
            Err(error) => absorption.unconfirmable = Some(error.message().to_string()),
        }
        absorption
    }

    // --- predictions --------------------------------------------------------

    /// Every falsifiable claim the platform has made, open and resolved.
    pub fn predictions(&self) -> &[RecordedPrediction] {
        &self.predictions
    }

    /// Score the predictions whose horizon has passed against what was
    /// published.
    ///
    /// Returns the hypothesis and the verdict for each one scored. A claim the
    /// source has not published enough to settle comes back
    /// [`Verdict::Undetermined`] and stays open — resolving it as failure is
    /// how a system marks itself right by scoring the questions nobody
    /// answered.
    pub fn score_predictions(
        &mut self,
        observations: &Observations,
        now: Timestamp,
    ) -> Vec<(String, Verdict)> {
        let mut scored = Vec::new();
        for prediction in &mut self.predictions {
            if !prediction.is_open() || prediction.proposition.resolves_at > now {
                continue;
            }
            let verdict = prediction.proposition.criteria.evaluate(observations);
            if !verdict.is_determined() {
                continue;
            }
            prediction.verdict = Some(verdict.clone());
            prediction.scored_at = Some(now);
            scored.push((prediction.hypothesis.clone(), verdict));
        }
        scored
    }

    // --- the journal --------------------------------------------------------

    /// The durable, hash-chained mirror of the cycle journal.
    ///
    /// Append-only and replayable. The platform's own [`EventLog`] holds the
    /// same frames; this one holds them wearing the ingestion envelope, which
    /// is what a downstream consumer subscribes to.
    pub fn journal(&self) -> &DurableLogTransport {
        &self.journal
    }

    /// Everything the journal holds that matches a filter, as envelopes.
    ///
    /// The replay entry point. `EventFilter::as_of` gives the point-in-time
    /// view, so a replay reads exactly what was knowable at an instant.
    pub fn replay_journal(&self, filter: &EventFilter) -> Result<Vec<StreamEnvelope>> {
        self.journal.replay(filter)
    }

    /// Decode the journal back into the entries that were written.
    pub fn journal_entries(&self) -> Result<Vec<CycleJournalEntry>> {
        self.replay_journal(&EventFilter::new())?
            .iter()
            .map(|envelope| Ok(envelope.decode::<CycleJournalEntry>()?.body))
            .collect()
    }

    // --- the twin -----------------------------------------------------------

    /// Everything the platform decided, and what came of it.
    ///
    /// Refusals are in here alongside fills, which is the whole point: a
    /// platform that records only its trades can say what it earned and not
    /// what it declined, and the second number is the one that says whether
    /// the gates are calibrated or merely shut.
    pub fn outcomes(&self) -> &OutcomeCapture {
        &self.outcomes
    }

    /// The engine that prices the alternatives to a decision.
    pub fn counterfactuals(&self) -> &CounterfactualEngine {
        &self.counterfactuals
    }

    /// Price every alternative to one order the platform actually sent.
    ///
    /// The market is the caller's, because the twin evaluates against history
    /// and the platform holds no bar store of its own. Everything the set
    /// reports is [`qip_twin::Simulated`] and stays that way: there is no
    /// conversion out of it, so no figure in here can reach
    /// [`qip_twin::capture::OutcomeCapture::realised_pnl`].
    pub fn evaluate_alternatives(
        &self,
        order_id: &OrderId,
        market: &mut TwinMarket,
    ) -> Result<CounterfactualSet> {
        let placed = self
            .outcomes
            .entries()
            .iter()
            .find(|entry| match &entry.decision.action {
                Action::OrderPlaced { order_id: id, .. } => id == order_id,
                _ => false,
            })
            .ok_or_else(|| {
                Error::not_found(format!(
                    "no order {order_id} was captured, so there is nothing to counterfact"
                ))
            })?;
        let Action::OrderPlaced {
            venue,
            side,
            quantity,
            ..
        } = &placed.decision.action
        else {
            return Err(Error::invalid("the captured action is not an order"));
        };

        // What was realised is the fill's, not the placement's: placing an
        // order costs nothing on its own, and pricing an alternative against a
        // zero would make every alternative look like a regret.
        let realised = self
            .outcomes
            .entries()
            .iter()
            .find(|entry| match &entry.decision.action {
                Action::Filled { order_id: id, .. } => id == order_id,
                _ => false,
            })
            .map_or_else(
                || RealisedOutcome::nothing_happened(placed.decision.at),
                |entry| entry.outcome,
            );

        let actual = ActualTrade::new(
            placed.decision.object_id.clone(),
            *side,
            *quantity,
            venue.clone(),
            HOME_REGION,
            placed.decision.at,
        )?;
        self.counterfactuals
            .evaluate(market, &placed.decision, &actual, &realised)
    }

    // --- the capital fabric -------------------------------------------------

    /// Record demand for capital at a lane.
    ///
    /// Fed from fills as they happen: a fill at a venue is capital that had to
    /// be there. Exposed so a caller can also feed it from allocations and
    /// margin calls, which the loop above does not see.
    pub fn record_capital_demand(
        &mut self,
        location: CapitalLocation,
        kind: DemandKind,
        at: Timestamp,
        amount: Decimal,
    ) {
        self.demand_history
            .entry((location, kind))
            .or_default()
            .push(DemandObservation::new(at, amount));
    }

    /// Every lane the platform has observed demand at.
    pub fn demand_lanes(&self) -> Vec<(&CapitalLocation, DemandKind, usize)> {
        self.demand_history
            .iter()
            .map(|((location, kind), history)| (location, *kind, history.len()))
            .collect()
    }

    /// Fit a forecast per lane from what the platform has actually used.
    ///
    /// A lane the fabric has never observed gets no forecast rather than a
    /// default one, so a lane missing from this list is a lane nothing is
    /// known about — which is different from one forecast at zero.
    pub fn forecast_capital_demand(
        &self,
        now: Timestamp,
        horizon: Duration,
    ) -> Vec<DemandForecast> {
        self.demand_history
            .iter()
            .filter_map(|((location, kind), history)| {
                self.forecaster
                    .forecast(location.clone(), *kind, history, now, horizon)
                    .ok()
            })
            .collect()
    }

    /// Build the pre-positioning plan implied by the current forecasts.
    ///
    /// Benefit at each interval's lower bound against cost at its upper bound,
    /// checked against the allocator's live limits. Draws no random numbers and
    /// reads no clock: `now` is the instant being reasoned about.
    pub fn pre_position(&self, now: Timestamp, horizon: Duration) -> Result<PrePositioningPlan> {
        let treasury = CapitalLocation::new(
            CapitalRegion::new(HOME_REGION),
            Currency::USD,
            VenueId::new("TREASURY"),
        );
        let mut request = PrePositioningRequest::new(
            treasury,
            self.config
                .initial_equity
                .checked_div(Decimal::from_int(2))
                .unwrap_or(self.config.initial_equity),
            FxRates::new(Currency::USD),
        )?;
        for forecast in self.forecast_capital_demand(now, horizon) {
            // Nothing is assumed to be sitting at a venue already. Claiming a
            // balance the platform has not been told about is how a plan
            // declines the transfer that turns out to have been needed.
            request = request
                .with_balance(LocationBalance::new(
                    forecast.location.clone(),
                    forecast.kind,
                    Decimal::ZERO,
                )?)
                .with_forecast(forecast);
        }
        let live = self.pre_positioner.allocator().allocate(&[], 0.0, now)?;
        self.pre_positioner.plan(&request, &live, now)
    }

    /// Score the pre-positioning plan against what the world turned out to
    /// need.
    ///
    /// The number that makes the forecaster improvable rather than merely
    /// confident. A plan that moved nothing scores exactly zero, which is what
    /// distinguishes a forecaster that declined correctly from one that never
    /// looked.
    pub fn evaluate_pre_positioning(
        &self,
        realised: &RealisedDemand,
        now: Timestamp,
        horizon: Duration,
    ) -> Result<PlanScore> {
        let plan = self.pre_position(now, horizon)?;
        qip_capital_fabric::evaluate(&plan, realised)
    }

    // --- what the platform charges itself -----------------------------------

    /// Compute charged since assembly. Monotone, and exact.
    ///
    /// An opportunity that earns less than it cost to find is not an
    /// opportunity. This is the figure that makes that statement checkable.
    pub fn compute_spend(&self) -> Decimal {
        self.compute_spend
    }

    /// What the most recent cycle consumed, rung by rung.
    pub fn cycle_ledger(&self) -> Option<&ComputeLedger> {
        self.cycle_ledger.as_ref()
    }

    /// What the most recent cycle cost. Zero before the first cycle.
    pub fn last_cycle_cost(&self) -> Decimal {
        self.cycle_ledger
            .as_ref()
            .map_or(Decimal::ZERO, ComputeLedger::total_cost)
    }

    /// The compute and data deductions the last cycle earned, in that order.
    ///
    /// Handed back rather than applied, because the edge belongs to whoever is
    /// computing it. These are two of [`qip_contracts::edge::NetEdge`]'s nine,
    /// and the two the platform charges itself; the other seven are the
    /// market's.
    pub fn cost_deductions(&self) -> Result<(Deduction, Deduction)> {
        let ledger = self.cycle_ledger.as_ref().ok_or_else(|| {
            Error::not_found("no cycle has run, so nothing has been charged to one")
        })?;
        self.cost_engine.cost_deductions(ledger, &self.data_reads)
    }

    /// The cost engine, for a caller pricing an edge of its own.
    pub fn cost_engine(&self) -> &CostEngine {
        &self.cost_engine
    }
}

/// Which control refused an order, as a name refusals can be counted by.
///
/// Short and stable on purpose: the description says what happened to a human,
/// and this says which gate to tally it against. A count keyed on the
/// description would fragment the moment a message was reworded.
fn gate_of(refusal: &RefusalReason) -> String {
    match refusal {
        RefusalReason::Malformed { .. } => "order-validation",
        RefusalReason::Halted { .. } => "kill-switch",
        RefusalReason::AutonomyTooLow { .. } => "autonomy",
        RefusalReason::LiveVenueBelowLiveAutonomy { .. } => "autonomy-live-venue",
        RefusalReason::VenueUnavailable { .. } => "venue-availability",
        RefusalReason::RiskRejected { .. } => "pre-trade-risk",
        RefusalReason::VenueRejected { .. } => "venue",
    }
    .to_string()
}

/// The execution engine's side, in the vocabulary the twin's record uses.
///
/// A buy lifts the offer and a sell hits the bid, so a buy is recorded against
/// the ask. Written out rather than assumed, because getting it backwards would
/// make every counterfactual price the opposite trade.
const fn book_side(side: Side) -> BookSide {
    match side {
        Side::Buy => BookSide::Ask,
        Side::Sell => BookSide::Bid,
    }
}

/// Slippage in basis points, signed so that positive is worse for the platform.
///
/// A statistic and therefore `f64`. Zero where there is no arrival price to
/// measure against, which is honest: an unmeasurable slippage is not a
/// slippage of zero, and the field carrying it says only what could be
/// computed.
fn slippage_bps(arrival: Decimal, achieved: Decimal, side: Side) -> f64 {
    let arrival = arrival.to_f64();
    if arrival.abs() < f64::EPSILON {
        return 0.0;
    }
    let direction = match side {
        Side::Buy => 1.0,
        Side::Sell => -1.0,
    };
    direction * (achieved.to_f64() - arrival) / arrival * 10_000.0
}

/// Append to a history series, evicting the oldest observation once the
/// series holds [`SERIES_HISTORY`].
///
/// Eviction happens here, at the insert, rather than by a scan somewhere in
/// the cycle: a cap enforced by a periodic prune is a cap that is over budget
/// between prunes, and the whole point of the bound is that no cycle ever
/// meets a series longer than it. Oldest-first, because every consumer of
/// these series reads recency — a detector fed the newest 512 sees the same
/// tape it saw unbounded; one fed a hole in the middle would not.
fn push_bounded(series: &mut Vec<f64>, value: f64) {
    series.push(value);
    if series.len() > SERIES_HISTORY {
        // One drain rather than a remove per excess element. On the hot path
        // the overshoot is always the single value just pushed, but a series
        // that arrived longer by any other route would otherwise converge one
        // element per observation, paying the over-budget cost on every cycle
        // in between — which is the failure the bound exists to prevent.
        series.drain(..series.len() - SERIES_HISTORY);
    }
}

/// Quoted spread in basis points — a statistic, and therefore `f64` like
/// every other series the detectors read.
///
/// `None` for a crossed or one-sided quote: a spread computed from either
/// would be a number about a book that should not reach a decision, and the
/// liquidity detector reading it would learn something false.
fn spread_bps(bid: Decimal, ask: Decimal) -> Option<f64> {
    let bid = bid.to_f64();
    let ask = ask.to_f64();
    let mid = (bid + ask) / 2.0;
    if mid <= 0.0 || ask < bid {
        return None;
    }
    Some((ask - bid) / mid * 10_000.0)
}

/// The impact-history class a corporate action lands under.
///
/// Named per kind rather than one blanket class, because "what does an event
/// of this class historically do" is only answerable if a split and a
/// delisting are not the same class.
fn corporate_action_class(kind: &CorporateActionKind) -> &'static str {
    match kind {
        CorporateActionKind::Split { .. } => "corporate_action/split",
        CorporateActionKind::CashDividend { .. } => "corporate_action/cash_dividend",
        CorporateActionKind::StockDividend { .. } => "corporate_action/stock_dividend",
        CorporateActionKind::RightsIssue { .. } => "corporate_action/rights_issue",
        CorporateActionKind::Merger { .. } => "corporate_action/merger",
        CorporateActionKind::Spinoff { .. } => "corporate_action/spinoff",
        CorporateActionKind::Delisting { .. } => "corporate_action/delisting",
        CorporateActionKind::Renamed { .. } => "corporate_action/renamed",
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

#[cfg(test)]
mod decide_tests {
    //! The decide stage stops being a stub: unit tests, because the seam
    //! between an approved thesis and a sized proposal is private on purpose —
    //! the only public road to the queue is the reason stage's review.

    use super::*;
    use qip_financial::universe::Universe;
    use qip_observability::Telemetry;
    use qip_portfolio_engine::construction::ApprovedThesis;
    use qip_risk::limits::LimitSet;

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) =
            qip_core::Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    /// A platform whose book is small enough that one sized leg fits inside
    /// the conservative single-order notional limit.
    ///
    /// The limit is untouched. Only the equity the weights are fractions of is
    /// smaller, which is the honest way to test a release: a control that has
    /// to be relaxed for the happy path to pass is a control the happy path
    /// was never inside.
    fn small_book_platform() -> Platform {
        let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(200_000));
        let (context, _clock) =
            qip_core::Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    fn thesis(object: &str, conviction: f64) -> ApprovedThesis {
        ApprovedThesis {
            hypothesis_id: format!("HYP-{object}"),
            object_id: qip_core::ObjectId::from_string(object),
            conviction,
            expected_return: 0.04 * conviction.signum(),
            price: Decimal::from_int(100),
        }
    }

    /// Alternating closes, so returns have real variance and a covariance
    /// exists to estimate. A flat series has zero variance and a mandate risk
    /// bound divided by it.
    fn feed_history(platform: &mut Platform, object: &str, closes: usize) {
        let series = platform
            .price_history
            .entry(object.to_string())
            .or_default();
        for index in 0..closes {
            let wiggle = if index % 2 == 0 { 0.7 } else { -0.5 };
            series.push(100.0 + index as f64 * 0.1 + wiggle);
        }
    }

    #[test]
    fn with_nothing_pending_the_decide_stage_still_reports_the_quiet_cycle() {
        let mut platform = platform();
        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);
        let proposal = platform.proposals.last().expect("a proposal is recorded");
        assert_eq!(
            proposal.len(),
            0,
            "an empty cycle proposed legs from nothing"
        );
    }

    #[test]
    fn the_expected_shortfall_limit_can_actually_fire() {
        // `.claude/rules/domains/risk-and-execution.md` names this limit as
        // the template for what not to add: `RiskState::expected_shortfall`
        // was always empty, so `LimitKind::MaxExpectedShortfall` looked its
        // figure up, took the `None` arm and recorded nothing. It could not
        // fire under any book. `LimitSet::conservative_default` ships it, so
        // every deployment believed it held a control it did not have — and
        // the same was true of `MaxValueAtRisk`.
        //
        // A control that cannot fire reads as protection and is not. This test
        // exists to prove this one now can.
        let mut platform = platform();

        // An equity path with a real left tail. Mostly small gains, then a run
        // of losses far outside them — which is precisely the shape expected
        // shortfall exists to price and that a volatility number would report
        // as merely elevated.
        for equity in [
            100_000.0, 100_400.0, 100_900.0, 101_200.0, 101_500.0, 101_100.0, 101_600.0, 102_000.0,
            101_800.0, 102_300.0, 96_000.0, 90_500.0, 84_000.0,
        ] {
            platform.equity_history.push(equity);
        }

        let state = platform.risk_state();

        // The premise, before the conclusion. If the map is empty the
        // assertion below would be measuring nothing — which is exactly the
        // state this test was written to end, so it must be checked and not
        // assumed.
        let shortfall = state
            .expected_shortfall
            .get("0.97")
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "expected shortfall was not computed for the confidence the \
                     default limit names; the map holds {:?}",
                    state.expected_shortfall.keys().collect::<Vec<_>>()
                )
            });
        assert!(
            shortfall > 0.0,
            "a book that lost sixteen percent over three cycles has an expected \
             shortfall of {shortfall}"
        );

        // And it is expected shortfall, not value at risk wearing its name.
        // Both are positive on a book with a tail and both breach the same
        // bound, so an assertion that the number is large cannot tell them
        // apart — a mutation swapping one for the other passed until this
        // check existed.
        //
        // The defining relationship separates them: expected shortfall is the
        // mean loss *beyond* the value-at-risk point, so it is never smaller,
        // and on a series whose tail is genuinely worse than its threshold it
        // is strictly larger. That inequality is the whole reason the platform
        // sizes on this number rather than on VaR.
        let var_at_same_confidence =
            qip_risk::metrics::historical_var(&platform.equity_returns(), 0.975);
        assert!(
            shortfall > var_at_same_confidence,
            "expected shortfall ({shortfall}) is not greater than value at risk \
             ({var_at_same_confidence}) at the same confidence, so the figure \
             being recorded is not the mean of the tail beyond the threshold"
        );

        // And the limit reads it and breaches. The default bound is 0.08; the
        // tail above is far past it.
        let breached = LimitSet::conservative_default()
            .check(&state)
            .breaches
            .into_iter()
            .any(|breach| breach.limit_name == "expected-shortfall");
        assert!(
            breached,
            "expected shortfall is {shortfall} against a limit of 0.08 and the \
             limit did not breach; it is still incapable of firing"
        );
    }

    #[test]
    fn running_a_cycle_records_the_book_equity_the_tail_limits_read() {
        // The seam between the two halves, and the one a mutation caught as
        // untested. The other tail-risk tests push equity into the series
        // directly, so deleting the per-cycle sample broke nothing they could
        // see — and in a real deployment the series would stay empty, the maps
        // would stay empty, and both limits would go back to never firing with
        // no test anywhere objecting.
        //
        // A statistic computed from a series nothing appends to is the same
        // defect as a map nothing populates, one layer down.
        let mut platform = platform();
        assert!(
            platform.equity_history.is_empty(),
            "the premise failed: the series was not empty before the first cycle"
        );

        let start = Timestamp::from_secs(1_760_000_100);
        for step in 0..3 {
            platform.run_cycle(start.saturating_add(Duration::from_secs(step * 60)));
        }

        assert_eq!(
            platform.equity_history.len(),
            3,
            "three cycles ran and the equity series holds {} sample(s)",
            platform.equity_history.len()
        );
        assert!(
            platform.equity_history.iter().all(|equity| *equity > 0.0),
            "a sample is non-positive, so the return series would divide by it"
        );
    }

    #[test]
    fn a_quiet_book_does_not_breach_the_tail_limits() {
        // The other half, and the half that makes the first one mean
        // something. A limit that fires on every book is not a control either
        // — it is an outage — and a test that only ever asserts a breach
        // cannot tell the two apart.
        let mut platform = platform();
        for step in 0..13 {
            platform
                .equity_history
                .push(100_000.0 + f64::from(step) * 120.0);
        }

        let state = platform.risk_state();
        assert!(
            !state.expected_shortfall.is_empty(),
            "the premise failed: nothing was computed, so no conclusion about \
             not breaching is available"
        );
        let breaches = LimitSet::conservative_default().check(&state).breaches;
        assert!(
            !breaches
                .iter()
                .any(|breach| breach.limit_name == "expected-shortfall"
                    || breach.limit_name == "value-at-risk"),
            "a book that only gained breached a tail limit: {:?}",
            breaches.iter().map(|b| &b.limit_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_sized_proposal_is_signed_by_two_controls_and_released_as_orders() {
        // The trading spine's last seam. Until this passed, `stage_act`
        // counted approved proposals and returned without submitting anything,
        // and nothing anywhere called `Proposal::approve` — so every proposal
        // stayed a draft, `is_releasable` was permanently false, and the
        // release path was unreachable code. The platform sized positions it
        // could never act on, and LEARN had no fill of its own to attribute.
        // A book small enough that a sized leg clears
        // `LimitSet::conservative_default`'s single-order notional cap on its
        // merits. The default test book is large enough that one leg is 800k
        // against a 250k limit — a legitimate refusal, asserted separately in
        // `an_order_over_the_notional_limit_is_refused_and_recorded`. Raising
        // the limit to fit the order would have turned a working control into
        // a passing test, which is the trade this repository does not make.
        let mut platform = small_book_platform();
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);

        // The premise, asserted before the conclusion: there is something to
        // release. A release assertion over an empty proposal would pass by
        // measuring nothing.
        let sized = platform.proposals.last().expect("a proposal is recorded");
        assert!(!sized.is_empty(), "the premise failed: no legs were sized");
        assert!(
            !sized.status.is_releasable(),
            "construction is not permission; a fresh proposal must be a draft"
        );
        let legs = sized.legs.len();

        let correlation = CorrelationId::from_string("corr-release");
        let outcome = platform.stage_act(now, &correlation);

        // Two controls signed, and both are named. A single approver is a
        // single point of failure, which is why `approve` refuses one name.
        let released = platform
            .proposals
            .iter()
            .find(|proposal| matches!(proposal.status, ProposalStatus::Released { .. }))
            .expect("the sized proposal was released");
        assert_eq!(
            released.checks_passed,
            vec!["risk-monitor".to_string(), "compliance".to_string()],
            "the proposal was released without both control signatures"
        );

        // And it became orders — one per leg, each naming the proposal that
        // caused it.
        assert_eq!(
            outcome.produced, legs,
            "{legs} leg(s) were sized and {} order(s) were released: {} :: problems={:?}",
            outcome.produced, outcome.detail, outcome.problems
        );
        let orders: Vec<_> = platform.orders.orders().collect();
        assert_eq!(orders.len(), legs, "an order object is missing");
        assert!(
            orders
                .iter()
                .all(|order| order.proposal_id == released.proposal_id.as_str()),
            "an order was released that cannot name the proposal that caused it"
        );

        // Nothing reached a real venue.
        assert!(!platform.orders.has_live_fills());
        assert!(!platform.is_live_capable());
    }

    #[test]
    fn an_order_over_the_notional_limit_is_refused_and_recorded() {
        // The other half of the release path, and the more important half. The
        // controls are not decoration: a leg sized against the default test
        // book is 800k against a 250k single-order notional cap, and the
        // deterministic pre-trade check refuses it before it reaches the
        // broker.
        //
        // This test is why `a_sized_proposal_is_signed_by_two_controls_and_released_as_orders`
        // uses a smaller book rather than a larger limit. Both paths are real
        // and both are asserted; relaxing the limit would have deleted this
        // one silently.
        let mut platform = platform();
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        let now = Timestamp::from_secs(1_760_000_100);
        let correlation = CorrelationId::from_string("corr-refused");
        platform.stage_decide(now);
        let sized = platform.proposals.last().expect("a proposal is recorded");
        assert!(
            !sized.is_empty(),
            "the premise failed: nothing was sized, so nothing could be refused"
        );

        let outcome = platform.stage_act(now, &correlation);
        assert_eq!(
            outcome.produced, 0,
            "an order over the notional limit reached the broker: {}",
            outcome.detail
        );
        assert!(
            outcome
                .problems
                .iter()
                .any(|problem| problem.contains("order-notional")),
            "the refusal did not name the control that made it: {:?}",
            outcome.problems
        );
        // Refused, not lost: nothing filled, and the platform is still paper.
        assert!(platform.orders.fills().is_empty());
        assert!(!platform.orders.has_live_fills());
    }

    #[test]
    fn a_second_proposal_is_sized_against_what_the_first_still_holds() {
        // Gap-matrix item 10's headline, at the seam it lives on. Before the
        // reservation wiring, `construct_from` handed every proposal the full
        // tracked equity: two proposals sized before either resolved each
        // passed against the same free balance, and the second was a
        // double-spend wearing a passing capital check.
        let mut platform = small_book_platform();
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);
        let first = platform
            .proposals
            .last()
            .cloned()
            .expect("the first proposal is recorded");
        // The premise: the first proposal holds real capital, or the second
        // one's budget below is trivially the whole book.
        assert!(
            first.traded_notional().is_positive(),
            "the premise failed: the first proposal holds nothing"
        );
        let equity = platform.capital.equity();

        // A second decision arrives before the first resolves — no act stage
        // has run, so the first proposal's hold is still active. The cycle
        // counter advances as `run_cycle` would advance it, because the
        // proposal id is derived from it and the reservation is keyed by the
        // proposal id.
        platform.cycle += 1;
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));
        platform.stage_decide(Timestamp::from_secs(1_760_000_160));
        let second = platform
            .proposals
            .last()
            .cloned()
            .expect("the second proposal is recorded");

        // The exact property: the second proposal was budgeted the equity
        // minus the first one's hold, not the equity. `proposal.equity` is
        // the budget `construct` was handed, so the assertion reads the seam
        // directly rather than inferring it from sizes.
        assert_eq!(
            second.equity.amount,
            equity - first.traded_notional(),
            "the second proposal was sized against capital the first still \
             holds — the double-spend the reservation ledger exists to refuse"
        );
    }

    #[test]
    fn a_released_proposal_commits_its_hold_and_a_refused_one_returns_it() {
        // The resolution half of item 10. Committed capital does not return
        // to free — it is in the book now — while a refusal hands the hold
        // back, because a control saying no must not strand the capital it
        // said no to.
        let now = Timestamp::from_secs(1_760_000_100);
        let correlation = CorrelationId::from_string("corr-reserve");

        // Released: every leg placed, the hold commits.
        let mut placed = small_book_platform();
        feed_history(&mut placed, "AAPL", 30);
        feed_history(&mut placed, "MSFT", 30);
        placed.pending_theses.push(thesis("AAPL", 0.6));
        placed.pending_theses.push(thesis("MSFT", -0.4));
        placed.stage_decide(now);
        let notional = placed
            .proposals
            .last()
            .expect("a proposal")
            .traded_notional();
        assert!(notional.is_positive(), "the premise failed: nothing sized");
        placed.stage_act(now, &correlation);
        assert_eq!(
            placed.reservations.reserved_total(),
            Decimal::ZERO,
            "a hold survived the act stage that resolved its proposal"
        );
        assert_eq!(
            placed.reservations.committed_total(),
            notional,
            "the released proposal's hold did not commit"
        );

        // Refused: the over-limit book, every leg refused, the hold releases.
        let mut refused = platform();
        feed_history(&mut refused, "AAPL", 30);
        feed_history(&mut refused, "MSFT", 30);
        refused.pending_theses.push(thesis("AAPL", 0.6));
        refused.pending_theses.push(thesis("MSFT", -0.4));
        refused.stage_decide(now);
        assert!(
            refused
                .proposals
                .last()
                .expect("a proposal")
                .traded_notional()
                .is_positive(),
            "the premise failed: nothing sized, so nothing could be refused"
        );
        let outcome = refused.stage_act(now, &correlation);
        assert_eq!(outcome.produced, 0, "the over-limit book placed an order");
        assert_eq!(
            refused.reservations.reserved_total(),
            Decimal::ZERO,
            "a refused proposal left its hold in place, stranding the capital"
        );
        assert_eq!(
            refused.reservations.committed_total(),
            Decimal::ZERO,
            "a refused proposal committed capital nothing spent"
        );
    }

    #[test]
    fn a_released_proposal_is_not_released_a_second_time() {
        // The duplicate-order failure. An approved proposal left in `Approved`
        // is re-offered on every later cycle, so one sizing decision pyramids
        // into a position nobody chose. The idempotent order id would collide
        // downstream, but relying on that would be a second control covering
        // for a missing first one.
        let mut platform = small_book_platform();
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        let now = Timestamp::from_secs(1_760_000_100);
        let correlation = CorrelationId::from_string("corr-twice");
        platform.stage_decide(now);
        let first = platform.stage_act(now, &correlation);
        assert!(
            first.produced > 0,
            "the premise failed: nothing was released on the first pass, so a \
             second pass cannot demonstrate anything"
        );

        // A second ACT with no new proposal must release nothing.
        let later = now.saturating_add(Duration::from_secs(60));
        let second = platform.stage_act(later, &correlation);
        assert_eq!(
            second.produced, 0,
            "the same proposal was released twice: {}",
            second.detail
        );
    }

    #[test]
    fn an_approved_thesis_becomes_a_draft_proposal_with_legs_and_is_drained() {
        let mut platform = platform();
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        // The premise: before this change, stage_decide constructed the empty
        // proposal unconditionally — the audit's first-ranked finding. If this
        // assertion starts failing with zero legs, the stub is back.
        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);
        let proposal = platform.proposals.last().expect("a proposal is recorded");
        assert!(
            !proposal.is_empty(),
            "two approved theses with thirty closes of shared history produced no legs; \
             the decide stage has regressed to the unconditional empty proposal"
        );
        assert!(
            !proposal.status.is_releasable(),
            "a freshly constructed proposal must still need its governed approval; \
             construction is not permission"
        );
        assert!(
            platform.pending_theses.is_empty(),
            "the queue was not drained; the same thesis would pyramid next cycle"
        );
    }

    #[test]
    fn too_little_shared_history_is_a_named_refusal_and_not_a_guessed_covariance() {
        let mut platform = platform();
        feed_history(&mut platform, "AAPL", 5);
        platform.pending_theses.push(thesis("AAPL", 0.6));

        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);
        let proposal = platform.proposals.last().expect("a proposal is recorded");
        assert_eq!(
            proposal.len(),
            0,
            "five closes produced a sized proposal; the covariance was a costume"
        );
        assert!(
            proposal.rationale.contains("too little history"),
            "the quiet proposal does not say why nothing was sized: {}",
            proposal.rationale
        );
        assert!(
            platform.pending_theses.is_empty(),
            "an unsizeable thesis was requeued; it will not size better against the same history"
        );
    }

    /// A drawdown that leaves the active holds above equity is counted where
    /// the alerts look, under the name the registry exports.
    ///
    /// The site used to count a bare string literal. Nothing checked that the
    /// literal matched `names::RESERVATION_SHORTFALL`, so a rename in either
    /// place would have split one fact into two series, one of them empty and
    /// alerted on. The test drives the real failure — equity reserved in full,
    /// then a fill that realises a loss — and reads the counter back by the
    /// constant, so the literal and the constant cannot drift apart unseen.
    #[test]
    fn a_reservation_shortfall_is_counted_under_the_registered_name() {
        let mut platform = platform();
        let now = Timestamp::from_secs(1_760_000_000);
        let equity = platform.capital.equity();
        assert!(equity.is_positive(), "the book starts with equity");

        platform
            .reservations
            .resync_free(equity, now)
            .expect("holds are zero, so free is the whole equity");
        platform
            .reservations
            .reserve("hold-1", equity, now, Duration::from_hours(1))
            .expect("the whole equity is free to hold");
        assert_eq!(platform.reservations.reserved_total(), equity);

        // Buy one lot at 100 and close it at 50: a realised loss of 50, so
        // equity is now below the hold that was taken against it.
        platform.capital.apply_fill(
            "AAA",
            Side::Buy,
            Decimal::from_int(100),
            Decimal::from_int(1),
            Decimal::ZERO,
        );
        platform.capital.apply_fill(
            "AAA",
            Side::Sell,
            Decimal::from_int(50),
            Decimal::from_int(1),
            Decimal::ZERO,
        );
        assert!(
            platform.capital.equity() < equity,
            "the loss must have reached the tracked equity"
        );
        let shortfall = labels([("reason", "holds_exceed_equity")]);
        assert_eq!(
            platform
                .telemetry
                .metrics
                .snapshot()
                .counter(names::RESERVATION_SHORTFALL, &shortfall),
            0,
            "nothing has been counted before the decide stage resyncs"
        );

        let _ = platform.stage_decide(now);

        let snapshot = platform.telemetry.metrics.snapshot();
        assert_eq!(
            snapshot.counter(names::RESERVATION_SHORTFALL, &shortfall),
            1,
            "one resync found the holds above equity; series: {:?}",
            snapshot.series.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert_eq!(
            platform.reservations.free(now),
            Decimal::ZERO,
            "the free balance is floored, so no new reservation can be taken"
        );
        // The description is registered at assembly, not at the first count.
        // Without it the series exports with an empty `# HELP`, which is
        // valid exposition and unreadable documentation.
        let help = snapshot
            .series
            .iter()
            .find(|s| s.name == names::RESERVATION_SHORTFALL)
            .map(|s| s.help.clone())
            .unwrap_or_default();
        assert!(
            help.contains("holds exceeding equity"),
            "the series exports without its description: {help:?}"
        );
    }
}

#[cfg(test)]
mod retention_tests {
    //! The working sets that grow as the process runs, held at their bounds.
    //!
    //! Unit tests, because the bounds guard private fields at their single
    //! point of retention. The failure each prevents is not hypothetical: the
    //! deployed fastbrain's cycle grew from 2.4ms at cycle 255 to 310ms at
    //! cycle 16,728 — six times its 50ms ceiling — because these series held
    //! every observation since assembly and the DISCOVER stage rescans all of
    //! them every cycle.

    // The series carry closes and volumes these tests construct as exact small
    // integers, so an equality is exactly the assertion intended: the newest
    // value survived and the oldest was the one evicted. An epsilon here would
    // pretend to an imprecision that does not exist and would pass a bound
    // that evicted the wrong end by less than the tolerance.
    #![allow(clippy::float_cmp)]

    use super::*;
    use qip_financial::quality::DataQuality;
    use qip_financial::universe::Universe;
    use qip_market::bar::Interval;
    use qip_market::quote::Quote;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = qip_core::Context::deterministic(start(), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    /// Bars whose closes count upward, so a test can tell exactly which
    /// observations survived eviction.
    fn counting_bars(count: usize) -> Vec<SensedRecord> {
        (0..count)
            .map(|index| {
                let close = 100.0 + index as f64;
                let at = start().saturating_sub(Duration::from_days((count - index) as i64));
                SensedRecord::Bar(Box::new(Bar {
                    object_id: ObjectId::from_string("obj-AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: at,
                    open: Decimal::from_f64(close).unwrap(),
                    high: Decimal::from_f64(close + 1.0).unwrap(),
                    low: Decimal::from_f64(close - 1.0).unwrap(),
                    close: Decimal::from_f64(close).unwrap(),
                    volume: Decimal::from_int(1_000 + index as i64),
                    trade_count: 100,
                    vwap: Decimal::from_f64(close),
                    quality: DataQuality::default(),
                }))
            })
            .collect()
    }

    /// Quotes whose spread widens by one basis point of price per quote, so
    /// the newest spread observation is distinguishable from every other.
    fn counting_quotes(count: usize) -> Vec<SensedRecord> {
        (0..count)
            .map(|index| {
                let half_spread = 0.01 * (1.0 + index as f64);
                SensedRecord::Quote(Quote {
                    object_id: ObjectId::from_string("obj-AAA"),
                    venue: "XNYS".to_string(),
                    at: start().saturating_sub(Duration::from_secs((count - index) as i64)),
                    bid: Decimal::from_f64(100.0 - half_spread).unwrap(),
                    ask: Decimal::from_f64(100.0 + half_spread).unwrap(),
                    bid_size: Decimal::from_int(500),
                    ask_size: Decimal::from_int(500),
                    quality: DataQuality::default(),
                })
            })
            .collect()
    }

    #[test]
    fn the_price_and_volume_series_stop_growing_at_the_bound_and_keep_the_newest_bars() {
        let mut platform = platform();
        let fed = SERIES_HISTORY + 100;
        assert!(
            fed > SERIES_HISTORY,
            "the premise: more bars are fed than the series may keep"
        );

        let absorbed = platform.observe(counting_bars(fed));
        assert_eq!(absorbed, fed, "the premise: every bar was absorbed");

        let prices = platform
            .price_history
            .get("obj-AAA")
            .expect("the series exists");
        let volumes = platform
            .volume_history
            .get("obj-AAA")
            .expect("the series exists");
        assert_eq!(
            prices.len(),
            SERIES_HISTORY,
            "the price series grew past its bound; on a long-running process the DISCOVER \
             stage's cost grows with it until the cycle breaches its ceiling"
        );
        assert_eq!(
            volumes.len(),
            SERIES_HISTORY,
            "the volume series grew past its bound"
        );
        // Oldest-first eviction: the newest bar survives and the survivors are
        // exactly the most recent `SERIES_HISTORY` closes. A bound that evicted
        // the newest would pass a length check while feeding the detectors a
        // tape frozen at assembly.
        assert_eq!(
            *prices.last().expect("non-empty"),
            100.0 + (fed - 1) as f64,
            "the newest close did not survive eviction"
        );
        assert_eq!(
            prices[0],
            100.0 + (fed - SERIES_HISTORY) as f64,
            "the oldest retained close is not the one the bound implies; eviction is not \
             oldest-first"
        );
    }

    #[test]
    fn the_spread_series_stops_growing_at_the_bound_and_keeps_the_newest_quotes() {
        let mut platform = platform();
        let fed = SERIES_HISTORY + 50;
        assert!(
            fed > SERIES_HISTORY,
            "the premise: more quotes are fed than the series may keep"
        );

        let absorbed = platform.observe(counting_quotes(fed));
        assert_eq!(absorbed, fed, "the premise: every quote was absorbed");

        let spreads = platform
            .spread_history
            .get("obj-AAA")
            .expect("the series exists");
        assert_eq!(
            spreads.len(),
            SERIES_HISTORY,
            "the spread series grew past its bound; it is fed roughly ten times as often as \
             the bar series, which is how the live process reached 120k entries in seven hours"
        );
        // The newest quote's spread survives: quote `fed - 1` has half-spread
        // 0.01 * fed, so its spread in basis points is 2 * 0.01 * fed / 100 * 10_000.
        let newest = *spreads.last().expect("non-empty");
        let expected = 2.0 * 0.01 * fed as f64 / 100.0 * 10_000.0;
        assert!(
            (newest - expected).abs() < 1.0,
            "the newest spread observation did not survive eviction: {newest} against {expected}"
        );
    }

    #[test]
    fn the_prediction_set_stops_growing_at_the_bound_and_keeps_the_newest_claims() {
        let mut platform = platform();
        let recorded = PREDICTION_HISTORY + 10;
        assert!(
            recorded > PREDICTION_HISTORY,
            "the premise: more claims are recorded than the set may keep"
        );

        for cycle in 0..recorded {
            let proposition = Proposition::new(
                "close is above the reference by the horizon",
                ResolutionCriteria::Threshold {
                    metric: "close:obj-AAA".to_string(),
                    comparison: Comparison::GreaterThan,
                    value: Decimal::from_int(100),
                },
                ResolutionSource::new(
                    "platform-market-data",
                    SourceKind::Official,
                    vec!["close:obj-AAA".to_string()],
                ),
                start().saturating_add(Duration::from_days(1)),
                SettlementRule::unit(UndeterminedRule::RollForward),
                Duration::from_days(1),
            )
            .expect("a valid proposition");
            platform.keep_prediction(RecordedPrediction {
                hypothesis: format!("hyp-{cycle}"),
                cycle: cycle as u64,
                proposition,
                recorded_at: start(),
                verdict: None,
                scored_at: None,
            });
        }

        assert_eq!(
            platform.predictions.len(),
            PREDICTION_HISTORY,
            "the prediction set grew past its bound; every unsettled claim rolls forward by \
             design, so on the live process this set reached 16,674 open claims in seven hours"
        );
        // Oldest-first: the survivors are the newest claims, because the claim
        // worth keeping is the one whose horizon can still arrive.
        assert_eq!(
            platform.predictions.first().expect("non-empty").cycle,
            (recorded - PREDICTION_HISTORY) as u64,
            "eviction is not oldest-first"
        );
        assert_eq!(
            platform.predictions.last().expect("non-empty").cycle,
            (recorded - 1) as u64,
            "the newest claim did not survive eviction"
        );
    }
}
