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

use crate::central::{
    AbsorbedFill, CellIngestion, CellOutcome, CellReport, CentralPlane, DispositionOutcome,
    LearningReport, WhitelistIssue,
};
use crate::config::PlatformConfig;
use crate::cycle::{CycleReport, Stage, StageOutcome};
use qip_agents::memory::ResearchMemory;
use qip_agents::runtime::{Reading, Upstream};
use qip_agents::{Budget, RunStatus};
use qip_ai::language::{DeterministicModel, LanguageModel};
use qip_ai::memory::{
    AnalystStance, ClaimRecord, DecisionTaken, Episode, EpisodeOutcome, EpisodeQuery,
    EpisodicMemory, FindingsSummary, PrecedentDigest, Recall, RegimeLabel, StanceDirection,
};
use qip_ai::retrieval::SearchIndex;
use qip_capital::ledger::{
    AttributedFill, DecidedBy, EligibilityDecision, EligibilityRecord, EligibilityRegistry, UserId,
    UserLedger, UserShare,
};
use qip_capital::reservation::ReservationLedger;
use qip_capital::{AllocationLimits, CapitalAllocator, DrawdownSchedule};
use qip_capital_fabric::journal::{
    FabricCommand, FabricJournal, FabricRecord, FabricState, PRODUCER as FABRIC_PRODUCER,
    WalletCommand,
};
use qip_capital_fabric::wallet::{
    Asset, HoldingObservation, LedgerView, Provenance as HoldingProvenance, TolerancePolicy,
    VenueAsset,
};
use qip_capital_fabric::{
    CapitalLocation, DemandForecast, DemandForecaster, DemandKind, DemandObservation, FundingCurve,
    FxRates, LocationBalance, PlanScore, PrePositioningPlan, PrePositioningPlanner,
    PrePositioningRequest, RealisedDemand, Region as CapitalRegion, SettlementCalendar,
    SettlementConvention, TransferCostModel,
};
use qip_chain::{
    BridgeFailure, BridgeLedger, BridgeTransfer, ChainState, ChainUpdate, Confirmations,
    ConfirmedView,
};
use qip_contracts::edge::Deduction;
use qip_contracts::governance::Usage;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::{VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionKind, EventKind, ObjectId, OrderId, ProposalId};
use qip_core::kv::KeyValueStore;
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
use qip_financial::universe::{CatalogueOrigin, Universe};
use qip_investment_agents::Organisation;
use qip_investment_agents::desk::{BookView, ComplianceView, Desk, MarketView, RiskView};
use qip_learning_engine::attribution::Attributor;
use qip_learning_engine::evaluation::{
    Evaluation, Outcome as ThesisOutcome, ThesisClaim, ThesisEvaluator,
};
use qip_learning_engine::feedback::{CalibrationReport, FeedbackEngine, FeedbackReport};
use qip_learning_engine::self_model::{ComponentKey, SelfModel};
use qip_lifecycle::trials::TrialBook;
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
    Comparison, Observation, Observations, Proposition, ResolutionCriteria, ResolutionSource,
    SettlementRule, SourceKind, UndeterminedRule, Verdict,
};
use qip_quantum::provider::SimulatedProvider;
use qip_reasoning_engine::engine::{ReasoningEngine, ReasoningOutcome};
use qip_reasoning_engine::hypothesis::Claim;
use qip_risk::aggregate::{AggregateFigures, RiskAggregates};
use qip_risk::limits::{LimitSet, RiskState};
use qip_risk_engine::autonomy::{AutonomyController, OperatorIdentity};
use qip_risk_engine::monitor::RiskMonitor;
use qip_risk_engine::pretrade::PreTradeChecker;
use qip_simulation_engine::costs::CostModel;
use qip_streaming::durable::DurableLogTransport;
use qip_streaming::envelope::{EventFacts, StreamEnvelope};
use qip_streaming::ports::Publisher;
use qip_streaming::provenance::{
    Region as StreamRegion, SourceId, SourceIdentity, SourceType, Subject,
};
use qip_twin::asof::TwinMarket;
use qip_twin::capture::{Action, Decision, OutcomeCapture, RealisedOutcome};
use qip_twin::counterfactual::{
    ActualTrade, AlternativeMenu, Counterfactual, CounterfactualEngine, CounterfactualSet,
};
use qip_twin::value::Simulated;
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
    /// The per-user, per-strategy books (blueprint §43.3), holding the
    /// mandate registry: the desk's mandate as the ceiling and every user
    /// mandate the configuration enrolled under it. Every attributed fill
    /// the centre settles is booked here — split pro rata across the users
    /// with capital at work at the strategy, or to the desk whole when no
    /// user mandate is registered — so the §43.4 chain terminates in a
    /// mandate rather than in a strategy lot.
    user_ledger: UserLedger,
    /// The fabric journal: every wallet, corridor, destination and gate
    /// decision as the command and its outcome, replayable. Its working
    /// copy of the log is process-local; the platform's own event log
    /// carries the same record ([`Platform::decide_fabric`]), and it is the
    /// platform's log a replay reads. Held rather than rebuilt per query
    /// because the state is what the records built, and a state rebuilt on
    /// each read would be a second reading of the log that could disagree
    /// with the one the cycle acted on.
    fabric: FabricJournal,
    /// Holdings observed through a statement, latest per venue-asset,
    /// bounded by [`MAX_OBSERVED_VENUE_ASSETS`]. What the wallet is
    /// assembled from; empty until a statement is handed in, and while
    /// empty no wallet is assembled — a wallet showing zero holdings would
    /// read as an empty account rather than an unobserved one.
    holdings_observed: BTreeMap<VenueAsset, HoldingObservation>,
    /// The reconciliation tolerance per observed asset, supplied with the
    /// statement by whoever holds the rates; the wallet refuses to reconcile
    /// an asset it has no tolerance for rather than guessing one.
    wallet_tolerances: TolerancePolicy,
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
    /// Cross-chain transfers in flight, and what became of them.
    ///
    /// Held here so a reorganisation the chain state reports reaches the
    /// transfers whose deposits sat in the withdrawn blocks. Until this
    /// existed `BridgeLedger::on_reorg` had no caller: a transfer waiting for
    /// finality on a block that stopped existing kept waiting, and the value
    /// it was supposed to move stayed on the books as in flight.
    bridges: BridgeLedger,
    /// Falsifiable claims the REASON stage has made, and their verdicts.
    predictions: Vec<RecordedPrediction>,
    /// Resolved episodes, for precedent (blueprint §10): what this platform
    /// reasoned in situations like the one in front of it, and what
    /// followed. Bounded by the memory's own capacity, and readable only
    /// from each episode's `known_at`, so REASON cannot recall a resolution
    /// LEARN has not yet seen.
    episodes: EpisodicMemory,
    /// Episodes REASON formed whose claim has not resolved, oldest first,
    /// bounded by [`PREDICTION_HISTORY`] like the claims they stand behind.
    /// Held apart from memory rather than in it with an empty outcome,
    /// because a precedent with no outcome is not a precedent, and a memory
    /// that returned one would report "no evidence" as a neighbour.
    pending_episodes: Vec<Episode>,
    /// The precedent recorded beside each hypothesis, most recent last,
    /// bounded by [`PREDICTION_HISTORY`].
    precedents: Vec<HypothesisPrecedent>,
    /// Theses scored against what was published, oldest first, bounded by
    /// [`PREDICTION_HISTORY`] like the claims they came from.
    ///
    /// The window the calibration is computed over. Kept because "when it
    /// says seventy percent, does it happen seventy percent" is a question
    /// about many resolved claims, and a Brier score recomputed from only the
    /// claims that resolved this cycle would be a different number every
    /// cycle and a statistic on none of them.
    evaluations: Vec<Evaluation>,
    /// The most recent calibration, for the health surfaces and the tests.
    last_calibration: Option<CalibrationReport>,
    /// What the platform has measured of its own components — each detector
    /// kind and each analyst — from the theses they produced that resolved.
    /// Fed by [`Platform::learn_from`]; read by the REASON stage through the
    /// per-origin factors it hands the reasoning engine (blueprint §13.1).
    self_model: SelfModel,
    /// What the LEARN stage calibrated this cycle, for the journal. Cleared
    /// as each cycle's LEARN begins so a cycle that scored nothing journals
    /// nothing rather than the previous cycle's figure.
    cycle_calibration: Option<CalibrationJournal>,
    /// Bars as observed, per instrument, bounded by [`SERIES_HISTORY`] like
    /// the price series beside them. The twin prices a declined path against
    /// these — the bars the platform actually saw, at the instants it saw
    /// them — rather than against a market rebuilt from the float series,
    /// which has no timestamps and would be a fabricated tape.
    bar_history: BTreeMap<String, Vec<Bar>>,
    /// Orders a control refused and the twin has not yet priced, oldest
    /// first, bounded by [`DECLINED_HISTORY`].
    declined: Vec<DeclinedPath>,
    /// What each priced refusal would have earned, most recent last, bounded
    /// by [`DECLINED_HISTORY`].
    declined_scores: Vec<DeclinedScore>,
    /// What the LEARN stage priced this cycle, for the journal. Cleared as
    /// each cycle's LEARN begins.
    cycle_counterfactuals: Option<CounterfactualJournal>,
    /// What the LEARN stage's strategy review did this cycle, for the
    /// journal. Cleared as each cycle's LEARN begins.
    cycle_strategy_review: Option<StrategyReviewJournal>,
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
    /// Instruments in the assembled universe that may not drive a capital
    /// decision, with the reason, in object-id order. Empty is the only
    /// state a production universe should show.
    universe_not_decision_grade: Vec<(String, String)>,
    /// What the event log's first record says about the universe this
    /// platform was assembled from. Kept so the overview can say it without
    /// decoding the log; the log is the record.
    universe_assembled: UniverseAssembled,
    /// The last log sequence this platform inherited rather than wrote. See
    /// `inherited_through`.
    inherited_through: u64,
    /// The asset class of every instrument this platform was assembled to
    /// trade, taken from the universe at assembly.
    ///
    /// A projection of reference data rather than a second copy of a facility.
    /// The universe itself lives in the market view behind the desk's
    /// `read_market_data` gate; the platform is that view's upstream and
    /// could read it without a context, but reference data does not change
    /// under the platform, so a projection taken once at assembly cannot
    /// drift from it the way absorbed state would — and absorbed state is
    /// now shared rather than projected, see the `world` field.
    asset_classes: BTreeMap<String, AssetClass>,
    /// The exposure buckets every instrument this platform was assembled to
    /// trade belongs to, keyed by object id then axis — the same projection
    /// of reference data as `asset_classes`, taken at the same moment for
    /// the same reason.
    ///
    /// Four axes are fed, and only where the record carries a value:
    /// `sector` (the GICS-style sector), `country` (the ISO code of primary
    /// risk), `asset_class` and `venue` (the primary listing). A record with
    /// an empty venue feeds no venue bucket rather than an invented one.
    /// Nothing else the blueprint's gate names — factor, family, causal
    /// driver — is carried by the instrument record, so nothing else is fed;
    /// a bucket the data cannot fill is reported as absent, not guessed.
    /// An instrument the universe holds no record for reaches no bucket at
    /// all, so a `MaxConcentration` or `MaxBucketExposure` limit sees only
    /// what the reference data can vouch for.
    exposure_axes: BTreeMap<String, BTreeMap<String, String>>,
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

    /// The world model — the one [`Platform::observe`] feeds, held as the
    /// writing end of the slot the agents' desk reads through its
    /// `read_world_model` gate.
    ///
    /// One instance, not a copy. Until this was an [`Upstream`] the desk held
    /// a `WorldModel::new()` taken at assembly and nothing ever wrote to it:
    /// the platform absorbed three hundred and twenty periods of a tape into
    /// *this* field while every analyst read an empty model, answered
    /// `no_data`, and left each hypothesis resting on the single anomaly
    /// origin — whose concentration penalty caps effective confidence at
    /// 0.36 against a 0.50 bar. No running binary had ever produced an order,
    /// and the reason was a cold copy, not a control. The platform writes
    /// between cycles, in `observe`, and never while a dispatch is reading.
    world: Upstream<WorldModel>,
    /// The market view the agents read through the desk's `read_market_data`
    /// gate — the snapshot [`Platform::observe`] applies every bar, quote,
    /// trade and book to, beside the reference universe.
    ///
    /// The snapshot's bar series is trimmed to [`SERIES_HISTORY`] on every
    /// push past it, so what the desk holds per instrument is exactly the
    /// platform's own `bar_history`: the same bars, the same bound, the same
    /// newest-first retention. A `BarSeries` is unbounded on its own, and a
    /// desk that grew with uptime would be the failure [`SERIES_HISTORY`]
    /// documents, reached by a second road.
    market: Upstream<MarketView>,
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
    /// The same fills as running counters — the figures every risk check
    /// reads. `qip_risk::aggregate` holds a check O(1) in strategy count
    /// only if the check is handed counters kept per fill rather than a
    /// walk it performs itself, and this is where the kernel keeps them.
    /// Fed at the one seam a desk fill becomes known
    /// ([`Platform::capture_submission`]) with the change in the position's
    /// at-cost notional, so its gross and net are the walk over
    /// [`TrackedCapital`]'s lots to the cent, and marked with that book's
    /// equity, cash and drawdown after every fill.
    aggregates: RiskAggregates,
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

/// The window a `volatility:<subject>` claim is settled over, in log returns.
///
/// The volatility-shift detector's own default window, restated here because
/// the claim it raises is about *that* statistic: a realised volatility over
/// any other window would settle the claim against a number nobody claimed
/// anything about. `DetectorRegistry::standard` constructs the detector with
/// its default, and the registry test that pins this pairing is the one that
/// would fail if either side moved.
const VOLATILITY_CLAIM_WINDOW: usize = 20;

/// How many declined paths the LEARN stage prices per cycle.
///
/// A cap, and a visible one: the paths it leaves are counted under
/// `qip_counterfactuals_deferred_total` and priced on a later cycle rather
/// than dropped. Eight because each evaluation resamples a market of up to
/// [`SERIES_HISTORY`] bars through every alternative on the menu, and the
/// stage runs inside the cycle's own latency budget; a backlog is a fact an
/// operator should see, not a stall the cycle should absorb.
const COUNTERFACTUALS_PER_CYCLE: usize = 8;

/// How many declined paths wait to be priced, and how many scores are kept.
///
/// A working window like [`PROPOSAL_HISTORY`]: a path declined this many
/// refusals ago and still not priceable is one whose bars never arrived. A
/// refusal that arrives while the window is full is *not* queued — it is
/// counted under `qip_counterfactuals_unscored_total{reason="capacity"}` and
/// reported on the cycle, because evicting the oldest waiting path would
/// silently choose which veto goes unexamined.
const DECLINED_HISTORY: usize = 256;

/// The budget holder every desk fill is charged to in the risk aggregate.
///
/// The desk's own orders implement proposals and carry hypotheses; they do
/// not belong to a foundry strategy, and the aggregate refuses a fill that
/// names none. One fixed name keeps the aggregate's strategy set bounded by
/// a source-file literal for the desk, with the foundry's strategies
/// arriving beside it as cell fills are carried across.
const DESK_STRATEGY: &str = "central-desk";

/// The one user the per-user ledger books to until users exist: the desk,
/// under [`qip_capital::ledger::Mandate::desk`]. A literal, like
/// [`DESK_STRATEGY`], so the ledger's user set is bounded by the source
/// until a mandate registry enrols anyone else.
const DESK_USER: &str = "desk";

/// How recently an operator must have authenticated to decide a user's
/// eligibility — the same fifteen minutes `AutonomyController` holds an
/// autonomy change to, because the two are the same kind of act: a person
/// widening what the platform may do with capital. A session token from
/// this morning is not evidence that anyone is at the keyboard now.
const ELIGIBILITY_CREDENTIAL_AGE: Duration = Duration::from_mins(15);

/// The producer every eligibility record in the event log carries, and the
/// one [`Platform::replay_eligibility`] selects on. Distinct from the fabric
/// journal's producer on the topic they share, so each replay passes over
/// the other's records rather than refusing them as undecodable.
const ELIGIBILITY_ORIGIN: &str = "kernel/eligibility";

/// The most venue-assets the platform keeps a statement about.
///
/// A statement names one balance at one venue, and an operator hands them
/// in; the bound is what keeps the wallet's working set a working set rather
/// than the unbounded history the retention rule forbids. A statement for a
/// venue-asset already held replaces it; a statement for a new one past the
/// bound is refused by name.
const MAX_OBSERVED_VENUE_ASSETS: usize = 256;

/// How old a statement may be when the wallet is assembled against it.
///
/// A custodian's, bank's or administrator's statement is a daily document,
/// so a day is the bound a statement is judged fresh within; one older than
/// that at assembly makes the wallet's assembly a refused, journalled
/// decision rather than a reconciliation against yesterday's figure — the
/// break the wallet would otherwise manufacture itself.
const STATEMENT_FRESHNESS: Duration = Duration::from_days(1);

/// The correlation the fabric journal's own working-copy records carry.
///
/// A literal, because the journal is constructed before any cycle has a
/// correlation of its own and its working copy is not the record: the
/// platform's event log holds each fabric record under the correlation the
/// decision was made in, which is where a reader looks.
const FABRIC_CORRELATION: &str = "capital-fabric-journal";

/// The exposure buckets one instrument record vouches for.
///
/// Axis names are the ones the default limit set looks up — `sector` and
/// `country` are what `LimitSet::conservative_default` names, and a limit
/// keyed on a spelling the fill never writes is a limit that cannot fire.
/// A value the record leaves blank feeds no bucket: the builder defaults the
/// venue to an empty string, and an empty-string bucket would be a real
/// counter under a name nobody chose.
fn exposure_axes_of(object: &qip_financial::object::FinancialObject) -> BTreeMap<String, String> {
    let mut axes = BTreeMap::new();
    for (axis, bucket) in [
        ("sector", object.sector.as_str().to_string()),
        ("country", object.geography.clone()),
        ("asset_class", object.asset_class.as_str().to_string()),
        ("venue", object.venue.clone()),
    ] {
        if !bucket.trim().is_empty() {
            axes.insert(axis.to_string(), bucket);
        }
    }
    axes
}

/// The feasibility grid one instrument record vouches for, in the shape the
/// execution engine's central gate judges an order against.
///
/// The lot and the tick are the record's own — the value the catalogue
/// stated, or `qip_financial`'s builder default of one lot and a hundredth of
/// a tick where the record stated none, which is a fact about the reference
/// data and not a number invented here. The two minimums are zero, which the
/// engine defines as "the venue states none": the catalogue carries no
/// minimum quantity or notional, and `qip_execution_engine::feasibility`
/// states no numeric constants of its own, only the gate literals, so a
/// positive minimum here would be a guess wearing a refusal's clothes. Until
/// this existed the kernel constructed its order manager bare, so the
/// central path judged no order against any grid: every off-lot or off-tick
/// order rode the kill switch, the autonomy gate and pre-trade risk to the
/// venue, and the platform's test universes — lot one, tick a hundredth,
/// stated on every record — protected nothing. Fallible because a record
/// with a non-positive lot or tick is one `ObjectBuilder::build` already
/// refuses, and a universe that reached here carrying one anyway should stop
/// assembly rather than be checked for nothing.
fn instrument_grid_of(
    object: &qip_financial::object::FinancialObject,
) -> Result<qip_execution_engine::VenueFeasibility> {
    qip_execution_engine::VenueFeasibility::new(
        object.lot_size,
        Some(object.tick_size),
        Decimal::ZERO,
        Decimal::ZERO,
    )
    .map_err(|error| {
        Error::invalid(format!(
            "{} carries a grid the execution engine cannot judge against: {}",
            object.object_id.as_str(),
            error.message()
        ))
    })
}

/// Bars the twin estimates liquidity over when pricing a declined path — the
/// same window the platform's own counterfactual tests price with, so a path
/// priced here and one priced in a test are priced by the same law.
const COUNTERFACTUAL_IMPACT_WINDOW: usize = 20;

/// The venue a refused order is recorded against for the twin.
///
/// A refusal reached no venue, and the twin's menu compares venues only to
/// decide whether an alternative venue differs from the one used. A name
/// that says so beats borrowing the venue the order would have gone to,
/// which the refusal path never learned.
const UNROUTED_VENUE: &str = "unrouted";

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
    /// What the hypothesis claimed, in the shape the learning engine grades:
    /// direction, magnitude, horizon and the confidence it was stated at.
    ///
    /// The proposition above says what would have to be published for the
    /// claim to be wrong; this says how confident the platform was, which is
    /// the number calibration is about. `None` on a record written before the
    /// field existed — such a claim can still be settled, and is still not
    /// gradeable, because its confidence was never written down.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim: Option<ThesisClaim>,
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

/// How many prior episodes REASON recalls for a situation.
///
/// Five is what a brief can carry and a reviewer can read; the memory ranks
/// by exact cosine, so these are the five nearest of the bounded candidate
/// set, not the first five found.
const PRECEDENT_K: usize = 5;

/// The precedent as the panel is briefed on it, from what REASON recalled.
///
/// `None` where nothing was recalled: an empty brief field is "no
/// precedent", which is not the same statement as a digest of zeros. The
/// direction the digest is taken against is the claim the anomaly implies,
/// the same one the recall query carried, so the panel reads the agreement
/// share the record beside the hypothesis will show. The age is measured
/// from the nearest episode's `known_at`, which the store guarantees is
/// strictly before `now`; `BriefPrecedent::new` refuses anything else, and
/// that refusal reaching a caller means the store's rule was bypassed.
fn brief_precedent(
    query: &EpisodeQuery,
    recall: &Recall,
    now: Timestamp,
) -> Result<Option<qip_agents::finding::BriefPrecedent>> {
    let Some(nearest) = recall.nearest.first() else {
        return Ok(None);
    };
    let direction = query.claim.as_ref().map_or(0.0, |claim| claim.direction);
    let digest = PrecedentDigest::of(&recall.nearest, direction);
    let prior_outcome = nearest
        .episode
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.agrees_with(direction));
    qip_agents::finding::BriefPrecedent::new(
        digest,
        nearest.similarity,
        prior_outcome,
        now.since(nearest.episode.known_at),
    )
    .map(Some)
}

/// The precedent recorded beside a hypothesis: the resolved episodes nearest
/// to the situation when REASON asked, and how their outcomes sat against
/// the claim's direction.
///
/// Evidence context, not an input. `confidence` is a copy of what review
/// produced, kept here so a reader sees the two side by side and a test can
/// prove the digest did not move it. The route by which precedent could
/// later bear on confidence is ADR 0005's evidence-weighted update — a
/// digest entering as an `Evidence` item with a stated diagnosticity — and
/// not a multiplier applied after review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HypothesisPrecedent {
    pub hypothesis_id: String,
    pub cycle: u64,
    /// The hypothesis's effective confidence after review, as recorded.
    pub confidence: f64,
    /// Candidates the index examined before re-ranking; never above the
    /// memory's bound.
    pub examined: usize,
    /// Episodes in memory when the question was asked.
    pub memory_size: usize,
    /// The nearest, best first.
    pub nearest: Vec<PrecedentEntry>,
    /// The share of the nearest whose outcome went the claim's way.
    pub digest: PrecedentDigest,
}

/// One recalled episode, as the precedent record keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrecedentEntry {
    pub episode_id: String,
    pub instrument: String,
    /// When the earlier situation was true, and when its outcome became
    /// knowable — the second is what made it recallable now.
    pub at: Timestamp,
    pub known_at: Timestamp,
    pub similarity: f32,
    pub claim: String,
    pub decision: DecisionTaken,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub realised_move_bps: Option<f64>,
    /// Whether the outcome went the current claim's way; `None` where
    /// either side has no sign.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agreed: Option<bool>,
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
    /// Bridge transfers failed because a reorganisation withdrew the block
    /// their deposit sat in. Defaulted so an older record replays.
    #[serde(default)]
    pub bridged_transfers_failed: usize,
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
        let bridged = if self.bridged_transfers_failed > 0 {
            format!(
                "; {} bridge transfer(s) failed on a withdrawn deposit",
                self.bridged_transfers_failed
            )
        } else {
            String::new()
        };
        format!(
            "{} block(s) applied, {} on a side branch, {} reorg(s) (deepest {}); {confirmed}{bridged}",
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
    /// The belief calibration as LEARN left it, on a cycle that scored at
    /// least one thesis. Absent on a cycle that scored none — an entry that
    /// restated the previous figure would read as a measurement this cycle
    /// made. Defaulted so a journal written before the field existed replays.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub calibration: Option<CalibrationJournal>,
    /// The declined paths LEARN priced this cycle, and how many it left for
    /// want of capacity. Absent on a cycle that priced nothing and deferred
    /// nothing. Defaulted so an older journal replays.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub counterfactuals: Option<CounterfactualJournal>,
    /// The strategies LEARN reviewed this cycle on the sessions their cells
    /// realised, and what became of them. Absent on a cycle in which no cell
    /// had closed a session since its strategy's baseline. Defaulted so an
    /// older journal replays.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub strategy_review: Option<StrategyReviewJournal>,
}

/// What the LEARN stage's counterfactual pass left in the journal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualJournal {
    /// Declined paths priced this cycle.
    pub scored: usize,
    /// Of those, how many would have beaten standing aside.
    pub regrets: usize,
    /// Declined paths due for pricing and left for a later cycle by the cap.
    pub deferred: usize,
}

/// What the LEARN stage's strategy review left in the journal: how many
/// strategies the demotion monitor judged on the sessions their cells
/// realised, and what became of them.
///
/// Counts rather than the verdicts themselves, because the verdicts are
/// already in the record — a move is in the lifecycle ledger with its
/// rationale, and a retirement's disposition is journaled by
/// [`Platform::learn_from_cells`] in the call that retired it. What the entry
/// adds is the fact that the review *ran* this cycle and over how much: a
/// cycle whose entry carries no review is one in which no cell had closed a
/// session, and that is distinguishable from a cycle in which the review
/// found nothing to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyReviewJournal {
    /// Strategies judged against their pilot baseline this cycle.
    pub reviewed: usize,
    /// Of those, pushed down a rung and still on the ladder.
    pub demoted: usize,
    /// Of those, retired — sustained decay at the floor, without a human.
    pub retired: usize,
    /// Retirements whose positions were scheduled for unwinding.
    pub dispositioned: usize,
    /// Retirements whose positions the centre refused to guess at, because
    /// the attribution and a cell's own book disagree.
    pub dispositions_refused: usize,
    /// Observations with no baseline to be judged against.
    pub skipped: usize,
}

/// One refused order, kept until the twin can price what refusing it cost.
#[derive(Clone, Debug, PartialEq)]
struct DeclinedPath {
    /// The captured refusal, as the twin evaluates against it.
    decision: Decision,
    order_id: OrderId,
    object_id: ObjectId,
    side: BookSide,
    quantity: Decimal,
    /// The control that refused, in the vocabulary `gate_of` gives.
    gate: String,
}

/// What a refused order would have done, once the twin has priced it.
///
/// Every money figure here is [`Simulated`] and stays that way: a declined
/// path's earnings are what an alternative world produced, and the type is
/// what keeps them out of the P&L they are reported next to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeclinedScore {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    /// The control that refused it — the rule the score is attributed to.
    pub gate: String,
    pub declined_at: Timestamp,
    pub scored_at: Timestamp,
    /// What the trade as proposed would have earned over the twin's horizon,
    /// net of the costs the twin charges.
    pub would_have_earned: Simulated<Decimal>,
    /// Whether the trade would have beaten standing aside. The bit blueprint
    /// §12.3 accumulates per rule: a rule that vetoes mostly profitable paths
    /// is too tight, one that vetoes mostly losing paths is earning its place.
    pub regret: bool,
    /// How many alternatives the twin priced.
    pub alternatives: usize,
}

/// What the LEARN stage's calibration pass left in the journal.
///
/// The Brier score and the adjustment are the two numbers the blueprint's
/// "single most important metric" comes down to; the counts say how much
/// they rest on, which is what stops a score from three theses reading as a
/// track record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibrationJournal {
    /// Theses scored this cycle.
    pub evaluated_this_cycle: usize,
    /// Informative evaluations in the window the figures below cover.
    pub evaluations_in_window: usize,
    pub brier_score: f64,
    pub confidence_adjustment: f64,
    pub is_overconfident: bool,
}

/// What one call to [`Platform::learn_from`] produced.
#[derive(Clone, Debug, PartialEq)]
pub struct LearningOutcome {
    /// The theses scored on this call, verdicts attached.
    pub evaluations: Vec<Evaluation>,
    /// Claims that could not be scored — no outcome, or a horizon that has
    /// not passed — so an unscored thesis is visible rather than absent.
    pub skipped: Vec<String>,
    /// The calibration and lessons over the whole window, or `None` where
    /// nothing in the window was informative.
    pub report: Option<FeedbackReport>,
    /// Theses that were graded and joined the window but could not be
    /// charged to a component — a class the self-model cannot key, a
    /// confidence it cannot score — one line each, so the self-model's
    /// silence about them is visible rather than the whole pass aborting.
    pub problems: Vec<String>,
}

/// The universe a platform was assembled from — the first record on its
/// hash-chained event log, written at assembly and before any cycle.
///
/// A replay of a run is built from the log, and the first thing it needs is
/// the instrument set the run saw: which catalogue, by hash. Until this
/// existed the roots could only journal that hash in a key-value namespace
/// *beside* the log (the kernel had no seam a root could append through), so
/// the fact a replay needs first was the one fact the log did not hold. It is
/// now written by [`Platform::new`] itself, from the origin the universe
/// carries, which makes "a cycle over an unrecorded universe" unrepresentable
/// rather than refused: no cycle can run on a platform that does not exist,
/// and the platform does not exist until this has been appended.
///
/// Recorded under `Topic::ReferenceDataUpdated` — reference data is what a
/// catalogue is — which is an observation-class topic: a file-backed log
/// keeps it for good, and the in-memory index evicts it only after every
/// replaceable record has gone. A topic of its own with permanent retention
/// belongs in `qip-events` and is named as remaining work rather than
/// borrowed from a group it does not belong to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniverseAssembled {
    /// The catalogue, by the hash `qip_financial::catalogue::load` computed
    /// over its text — or `None` for a universe assembled in-process from no
    /// catalogue. `None` is stated rather than the log staying silent: such a
    /// run cannot be reproduced from the log alone, and the log now says so
    /// in its first record instead of leaving a replay to discover it.
    pub catalogue: Option<CatalogueOrigin>,
    /// How many instruments the universe held at assembly.
    pub instruments: usize,
    /// SHA-256 over the object ids in id order, newline-separated: the
    /// membership of the universe as assembled. Distinct from the catalogue
    /// hash on purpose — that names a file, this names a set — so a replay
    /// that rebuilt a universe by hand can still check it holds the same
    /// instruments, and a catalogue whose loader changed what it admits is
    /// caught by the two disagreeing.
    pub members_sha256: String,
    /// How many of them `Universe::not_decision_grade` named.
    pub not_decision_grade: usize,
    pub assembled_at: Timestamp,
}

impl UniverseAssembled {
    /// One line for a start-up banner or an overview.
    pub fn describe(&self) -> String {
        match &self.catalogue {
            Some(origin) => format!(
                "universe of {} instrument(s) from catalogue {} (sha256 {}), {} not \
                 decision-grade",
                self.instruments, origin.version, origin.sha256, self.not_decision_grade
            ),
            None => format!(
                "universe of {} instrument(s) from no catalogue (members {}), {} not \
                 decision-grade; a replay cannot rebuild it from the log",
                self.instruments, self.members_sha256, self.not_decision_grade
            ),
        }
    }
}

/// How an attributed fill reached the user books, on the record.
///
/// One variant per choice the kernel can make, so a reader of the log can
/// group on the choice rather than parse a sentence for it: the desk booked
/// whole, with why; a pro-rata split, with every share and where the
/// rounding unit went; or a refusal, with the ledger's own reason and no
/// book moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum BookingBasis {
    /// Booked to the desk whole.
    DeskWhole { user: UserId, reason: String },
    /// Split across users in proportion to what each had at work.
    ProRata {
        shares: Vec<UserShare>,
        entitlement_total: Decimal,
        remainder: Decimal,
        remainder_to: UserId,
    },
    /// Not booked. The fill has happened and the strategy lot holds it; the
    /// user books do not, and this record says so.
    Refused { reason: String },
}

/// One entry the kernel made in the per-user ledger, as the event log keeps
/// it: a funding of a user's book from their mandate, or the booking of an
/// attributed fill under a [`BookingBasis`].
///
/// Journalled so the per-user books are reproducible from the log alone
/// (`rules/10-product-direction.md`): a booking that lived only in the
/// ledger's memory would be a second source of truth for who was attributed
/// what, and the one that survives a restart would be the strategy lot,
/// which does not know.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum LedgerEntry {
    /// [`UserLedger::fund`]: capital moved from a mandate into one book.
    Funded {
        user: UserId,
        strategy: StrategyId,
        currency: Currency,
        amount: Decimal,
    },
    /// An attributed fill booked, or refused, under the stated basis.
    Booked {
        strategy: StrategyId,
        source: String,
        currency: Currency,
        amount: Decimal,
        basis: BookingBasis,
    },
    /// A funding the eligibility registry refused before any book moved.
    /// `gate` is the [`qip_capital::ledger::Ineligible`] name — the stable
    /// token — and `reason` the sentence; on the record because a refusal
    /// that leaves no trace is indistinguishable from a request never made.
    FundingRefused {
        user: UserId,
        strategy: StrategyId,
        amount: Decimal,
        gate: String,
        reason: String,
    },
}

impl EventBody for LedgerEntry {
    /// The last link of the attribution chain: what the centre's exact
    /// attribution said a strategy realised, booked to whose capital it was.
    /// The topic sits in the Learn group, which the log never evicts, so a
    /// user's booking cannot be dropped from the working set to make room
    /// for a tick.
    const TOPIC: Topic = Topic::AttributionCompleted;
    const SCHEMA_VERSION: u32 = 1;
}

impl EventBody for UniverseAssembled {
    // Reference data is what an instrument catalogue is. See the type's
    // comment for the retention consequence of the topic.
    const TOPIC: Topic = Topic::ReferenceDataUpdated;
    const SCHEMA_VERSION: u32 = 1;
}

/// Where an eligibility decision came from. Two sources and no third: the
/// committed configuration, or an authenticated operator at runtime. ADR
/// 0021 names what may never be one — a model output, an agent finding, a
/// blueprint revision — and there is no variant for any of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibilitySource {
    Configuration,
    Operator,
}

/// One eligibility decision as the event log keeps it: the record the
/// registry applied, where it came from, and the reason stated.
///
/// Journalled so the registry is reproducible from the log alone:
/// [`Platform::replay_eligibility`] rebuilds it from these records and
/// nothing else, and a decision that lived only in the registry's memory
/// would be a second source of truth for who was allowed to be funded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EligibilityEntry {
    pub record: EligibilityRecord,
    pub source: EligibilitySource,
    pub reason: String,
}

impl EventBody for EligibilityEntry {
    /// A compliance decision about a person, in the Decide group the log
    /// never evicts. Shared with the fabric journal's records, which are
    /// told apart by producer — see [`ELIGIBILITY_ORIGIN`].
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;
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
    ///
    /// Returns the signed change in the instrument's at-cost notional, which
    /// is what the risk aggregate is fed: the fill's own notional would be
    /// wrong for it, because a partial close at a profit moves the position
    /// at cost by less than the cash it brought in, and an aggregate fed cash
    /// would report a long book short after enough of them.
    fn apply_fill(
        &mut self,
        object_id: &str,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        costs: Decimal,
    ) -> Decimal {
        let at_cost = |positions: &BTreeMap<String, PositionLot>| {
            positions
                .get(object_id)
                .map_or(Decimal::ZERO, |lot| lot.quantity * lot.average_price)
        };
        let before = at_cost(&self.positions);
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
        at_cost(&self.positions) - before
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
        Self::with_language_model(
            config,
            context,
            telemetry,
            universe,
            limits,
            Arc::new(DeterministicModel::new()),
        )
    }

    /// Assemble a platform whose organisation narrates through the given
    /// language model.
    ///
    /// The seam ADR 0037 needs: the deep brain — and only the deep brain —
    /// hands in a `FallbackChain` with a hosted adapter ahead of the
    /// deterministic model, so a provider outage degrades to templates rather
    /// than stopping reasoning. Every other root takes [`Self::new`], which is
    /// this with the deterministic model alone. The model is not on
    /// `PlatformConfig` because that is a serialisable record of how the
    /// platform was assembled, and a credential-bearing adapter must not be.
    pub fn with_language_model(
        config: PlatformConfig,
        context: Context,
        telemetry: Telemetry,
        universe: Universe,
        limits: LimitSet,
        language_model: Arc<dyn LanguageModel>,
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
        // The last sequence this process did not write.
        //
        // Taken here, between opening the log and appending anything to it,
        // because that is the only instant at which it is knowable: from the
        // next line on, the log holds records from a previous run and records
        // from this one and nothing in a `LogRecord` distinguishes them.
        // A caller that asks the log afterwards gets the wrong answer —
        // qip-deepbrain's did, and the consequence was precise: it read the
        // watermark after assembly, so the universe record `Platform::new`
        // had just appended looked inherited, and the first record of the
        // chain — the one a replay needs before it can read any cycle — was
        // the one record the durable archive never sealed.
        let inherited_through = event_log
            .records()
            .last()
            .map_or(0, |record| record.sequence);
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
        // The exposure buckets, taken here too. Until this existed the
        // aggregate was fed no axis at all, so `MaxConcentration` and
        // `MaxBucketExposure` — two limits in every default set — evaluated
        // against empty buckets on every cycle and could never fire: a
        // control that read as protection and was not.
        let exposure_axes: BTreeMap<String, BTreeMap<String, String>> = universe
            .iter()
            .map(|object| {
                (
                    object.object_id.as_str().to_string(),
                    exposure_axes_of(object),
                )
            })
            .collect();
        // The lot and tick grid of every instrument, taken here for the same
        // reason and installed on the order manager below, so the central
        // path refuses an order the venue could not express before any
        // downstream control treats its size as real (blueprint §18.1).
        let instrument_grids: BTreeMap<String, qip_execution_engine::VenueFeasibility> = universe
            .iter()
            .map(|object| {
                instrument_grid_of(object).map(|grid| (object.object_id.as_str().to_string(), grid))
            })
            .collect::<Result<_>>()?;
        // What in this universe may not drive a decision, and why, taken here
        // for the same reason: `Universe::not_decision_grade` said the kernel
        // logged it at start-up, and nothing did, so a universe assembled
        // entirely from research-only or synthetic instruments looked exactly
        // like one fit to trade. Kept as (object, reason) pairs for the
        // overview and recorded as a gauge once the registry exists below.
        let not_decision_grade: Vec<(String, String)> = universe
            .not_decision_grade()
            .into_iter()
            .map(|(object, reason)| (object.object_id.as_str().to_string(), reason))
            .collect();
        // What the log's first record will say. Taken here, before the
        // universe moves into the desk, and appended below once the log and
        // the journal both exist — but before this function returns, so no
        // cycle can precede it.
        let members: Vec<String> = universe.ids().map(|id| id.as_str().to_string()).collect();
        let universe_assembled = UniverseAssembled {
            catalogue: universe.origin().cloned(),
            instruments: universe.len(),
            members_sha256: qip_core::sha256_hex(members.join("\n").as_bytes()),
            not_decision_grade: not_decision_grade.len(),
            assembled_at: now,
        };

        // The two facilities the platform feeds, held as their writing ends.
        // The desk is built by `Desk::new` so the facility-to-capability
        // pairing stays where that constructor's documentation says it lives,
        // and the two gates are then re-pointed at these slots under the
        // capability and label `Desk::new` itself chose — not a second
        // spelling of either.
        let world = Upstream::new(WorldModel::new());
        let market = Upstream::new(MarketView {
            snapshot: MarketSnapshot::new(now),
            universe,
        });
        let mut desk = Desk::new(
            MarketView {
                snapshot: MarketSnapshot::new(now),
                universe: Universe::new(),
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
        );
        desk.world = world.gate(desk.world.required(), desk.world.label());
        desk.market = market.gate(desk.market.required(), desk.market.label());
        let desk = Arc::new(desk);

        let organisation = Organisation::standard(
            desk.clone(),
            now,
            now,
            config.seed,
            Some(language_model),
            config.licensed_datasets.clone(),
            config.quantum_enabled,
        )?;

        let mut router = ComputeRouter::classical(config.seed).with_policy(config.routing);
        if config.quantum_enabled {
            router = router.with_quantum(Arc::new(SimulatedProvider::new(config.seed)));
        }

        let mut central = CentralPlane::with_reproducible_key(
            &central_signing_secret(config.seed),
            config.central.clone(),
        )?;
        central.attach_metrics(Arc::clone(&telemetry.metrics));

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

        // The desk first, under a mandate sized to the book's opening equity
        // — the ceiling — and then every user mandate the configuration
        // names, each admitted against that ceiling. A refused enrolment
        // stops assembly with the term named: a platform that opened a book
        // under a mandate the registry refused would be promising capital
        // the desk does not have.
        let mut user_ledger =
            UserLedger::with_desk(UserId::new(DESK_USER)?, initial_equity, Currency::USD)?;
        for enrolment in &config.user_mandates {
            user_ledger.enrol(
                enrolment.user.clone(),
                enrolment.id.clone(),
                enrolment.mandate.clone(),
                now,
            )?;
        }
        let fabric = Self::resume_fabric(&event_log, config.seed)?;

        let mut platform = Self {
            central,
            insights: crate::central::insights::CellInsights::new(config.seed),
            data_finder,
            catalog: Catalog::new(),
            chain: None,
            confirmations: Confirmations::exactly(config.chain_confirmations),
            bridges: BridgeLedger::new(),
            predictions: Vec::new(),
            evaluations: Vec::new(),
            last_calibration: None,
            self_model: SelfModel::new(),
            cycle_calibration: None,
            bar_history: BTreeMap::new(),
            declined: Vec::new(),
            declined_scores: Vec::new(),
            cycle_counterfactuals: None,
            cycle_strategy_review: None,
            journal: DurableLogTransport::in_memory("kernel-journal"),
            outcomes: OutcomeCapture::new(),
            counterfactuals,
            forecaster: DemandForecaster::new(),
            pre_positioner,
            demand_history: BTreeMap::new(),
            cost_engine: CostEngine::new(DataCostModel::new()),
            cost_router: Router::default(),
            reason_routing: None,
            universe_not_decision_grade: not_decision_grade,
            universe_assembled,
            inherited_through,
            asset_classes,
            exposure_axes,
            cycle_ledger: None,
            compute_spend: Decimal::ZERO,
            data_reads: DataReads::new(),
            capture_problems: Vec::new(),
            last_correlation: None,
            constructor: PortfolioConstructor::new(config.mandate, router)?,
            pending_theses: Vec::new(),
            episodes: EpisodicMemory::default(),
            pending_episodes: Vec::new(),
            precedents: Vec::new(),
            reasoning: ReasoningEngine::new(config.review),
            opportunities: OpportunityEngine::new(
                DetectorRegistry::standard(),
                EngineConfig::default(),
            ),
            // Every instrument's grid installed at assembly, keyed on the
            // object id an order carries, so the feasibility gate is a
            // control that can fire rather than a module nothing reaches.
            orders: instrument_grids.into_iter().fold(
                OrderManager::new(PreTradeChecker::new(limits.clone())),
                |orders, (object_id, grid)| orders.with_instrument_feasibility(object_id, grid),
            ),
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
            user_ledger,
            fabric,
            holdings_observed: BTreeMap::new(),
            wallet_tolerances: TolerancePolicy::new(),
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
            world,
            market,
            liquidity: LiquidityTopology::default(),
            market_events: Vec::new(),
            capital: TrackedCapital::new(initial_equity),
            aggregates: RiskAggregates::new(initial_equity, initial_equity)?,
            queue: Vec::new(),
            proposals: Vec::new(),
            equity_history: Vec::new(),
            proposals_made: 0,
        };
        platform.describe_metrics();
        // Written once, at assembly, because the universe does not change
        // under a running platform. A count and not a per-instrument series:
        // the instrument list is unbounded and the reasons are for the
        // overview, which reads them from the platform.
        platform.telemetry.metrics.gauge(
            names::UNIVERSE_NOT_DECISION_GRADE,
            labels([]),
            platform.universe_not_decision_grade.len() as f64,
        );
        // The first record, appended last in assembly and before the platform
        // is handed to anything that could run a cycle. A log that cannot
        // take it is a platform that does not assemble: the alternative —
        // a platform running cycles whose log never says what universe they
        // ran over — is the state this record exists to end.
        platform.record_universe_assembled(now)?;
        // The committed eligibility decisions, after the universe record and
        // after every mandate is enrolled, through the same journaled path
        // an operator's runtime decision takes. A decision the registry
        // refuses — a user with no mandate, a blank operator — stops
        // assembly with the user named, because a platform that assembled
        // anyway would either fund that user on nobody's say-so or silently
        // hold a configuration it did not apply.
        for committed in platform.config.user_eligibilities.clone() {
            platform.apply_eligibility(
                EligibilityRecord {
                    user: committed.user,
                    decision: EligibilityDecision::Granted {
                        eligibility: committed.eligibility,
                    },
                    by: committed.decided_by,
                    decided_at: now,
                },
                EligibilitySource::Configuration,
                "committed in the deployment's configuration".to_string(),
                now,
            )?;
        }
        Ok(platform)
    }

    /// What the event log's first record says about the universe this
    /// platform was assembled from.
    pub fn universe_assembled(&self) -> &UniverseAssembled {
        &self.universe_assembled
    }

    /// Append the universe record to the event log and publish it to the
    /// journal, exactly as a cycle's entry is.
    fn record_universe_assembled(&mut self, now: Timestamp) -> Result<()> {
        // `context.ids()` is seeded purely from `config.seed`, so a process
        // resuming this log and a from-scratch process both starting at the
        // same instant would otherwise mint the exact same id for this
        // record — deliberate determinism turned into a genuine collision,
        // because the resumed run's record sits at a different position in
        // a longer chain than a from-scratch run's does. Advancing the
        // stream by what this run inherited moves it off that shared
        // starting point; a from-scratch assembly (`inherited_through == 0`)
        // draws nothing extra and is unaffected.
        self.context.ids().advance(self.inherited_through);
        let correlation_id = self
            .context
            .ids()
            .generate::<qip_core::lineage::CorrelationKind>(now);
        let facts = EventFacts::derived(
            SourceIdentity::new(
                SourceId::new("qip-kernel"),
                SourceType::Internal,
                StreamRegion::new(HOME_REGION),
            ),
            Subject::unattributed(),
            UniverseAssembled::TOPIC,
        );
        let envelope = StreamEnvelope::seal(
            self.context.ids().generate::<EventKind>(now),
            Lineage::root(correlation_id, "kernel/universe"),
            self.universe_assembled.clone(),
            now,
            now,
            facts,
        )?;
        self.event_log.append(&envelope.to_frame()?)?;
        self.journal.publish(envelope, now)?;
        Ok(())
    }

    /// Instruments in the assembled universe unfit to drive a capital
    /// decision, each with the reason `Universe::not_decision_grade` gave.
    pub fn universe_not_decision_grade(&self) -> &[(String, String)] {
        &self.universe_not_decision_grade
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
            names::AGENT_MANIFESTS_EXPIRED,
            "roster manifests past their review interval; non-zero means every agent run is refused",
        );
        metrics.describe(
            names::RESERVATION_SHORTFALL,
            "resyncs that found capital holds exceeding equity, by reason",
        );
        metrics.describe(
            names::CENTRAL_RECONCILIATION_BREAKS,
            "reconciliation breaks the central plane halted a cell for, by direction",
        );
        metrics.describe(
            names::CENTRAL_CELL_HALTS,
            "scoped halts the central plane placed on a cell, by cause",
        );
        metrics.describe(
            names::STRATEGY_PROMOTIONS,
            "strategies admitted to a rung by a gate, by the rungs left and entered",
        );
        metrics.describe(
            names::STRATEGY_DEMOTIONS,
            "strategies pushed down or retired, by the rungs left and entered",
        );
        metrics.describe(
            names::BELIEF_BRIER_SCORE,
            "Brier score over the window of resolved theses; when the platform said seventy \
             percent, how far from seventy percent it happened",
        );
        metrics.describe(
            names::BELIEF_CONFIDENCE_ADJUSTMENT,
            "factor stated confidences would be scaled by to match outcomes; one is calibrated",
        );
        metrics.describe(
            names::BELIEF_EVALUATIONS,
            "informative evaluations the calibration rests on",
        );
        metrics.describe(
            names::THESES_EVALUATED,
            "theses scored against what was published, by verdict",
        );
        metrics.describe(
            names::COUNTERFACTUALS_SCORED,
            "declined paths priced by the twin once their horizon passed, by the gate that \
             declined them",
        );
        metrics.describe(
            names::COUNTERFACTUAL_REGRETS,
            "declined paths that, priced, would have beaten standing aside, by gate",
        );
        metrics.describe(
            names::COUNTERFACTUALS_DEFERRED,
            "declined paths due for pricing and left for a later cycle by the per-cycle cap",
        );
        metrics.describe(
            names::COUNTERFACTUALS_UNSCORED,
            "declined paths that will never be priced, by reason",
        );
        metrics.describe(
            names::CENTRAL_FILLS_ATTRIBUTED,
            "cell fills attributed to strategies by the central plane, by the basis of the split",
        );
        metrics.describe(
            names::CENTRAL_CROSSES_SETTLED,
            "internal crosses settled to both contributors' books at the mid",
        );
        metrics.describe(
            names::CENTRAL_SETTLEMENTS_REFUSED,
            "orders and crosses the central plane refused to settle, by kind",
        );
        metrics.describe(
            names::CENTRAL_ATTRIBUTION_FAILURES,
            "settlements whose decomposition did not close; must stay at zero",
        );
        metrics.describe(
            names::BRIDGE_TRANSFERS_FAILED,
            "bridge transfers failed on the platform's own evidence, by failure",
        );
        metrics.describe(
            names::UNIVERSE_NOT_DECISION_GRADE,
            "instruments in the assembled universe unfit to drive a capital decision",
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

    /// Produce one cell's cycle whitelist and journal what was produced.
    ///
    /// The plane derives the whitelist (`CentralPlane::cycle_whitelist_for`);
    /// this is the entry point a shipper uses, because the journal is the
    /// platform's and a whitelist that reached a cell without a record here
    /// would be a permission reproducible from nothing. Recorded whether or
    /// not it carries anything: an empty whitelist shipped every few minutes
    /// is exactly the fact an operator asking why the desk never installs
    /// needs to find. A refusal is not journaled here, because nothing was
    /// shipped; it is returned, naming the entry, for the shipper to log.
    pub fn issue_cycle_whitelist(&mut self, cell: &str, now: Timestamp) -> Result<WhitelistIssue> {
        let issue = self.central.cycle_whitelist_for(cell, now)?;
        let correlation_id = self
            .context
            .ids()
            .generate::<qip_core::lineage::CorrelationKind>(now);
        let facts = EventFacts::derived(
            SourceIdentity::new(
                SourceId::new("qip-kernel"),
                SourceType::Internal,
                StreamRegion::new(HOME_REGION),
            ),
            Subject::unattributed(),
            WhitelistIssue::TOPIC,
        );
        let envelope = StreamEnvelope::seal(
            self.context.ids().generate::<EventKind>(now),
            Lineage::root(correlation_id, "kernel/whitelist"),
            issue.clone(),
            now,
            now,
            facts,
        )?;
        self.event_log.append(&envelope.to_frame()?)?;
        self.journal.publish(envelope, now)?;
        Ok(issue)
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

    pub fn set_central(&mut self, mut central: CentralPlane) {
        // The swapped-in plane is the one every deployment trades on, and a
        // plane that arrived without the registry would count its rungs into
        // nothing — the reproducible plane it replaces was wired, and the
        // silence would begin exactly when the real key arrived.
        central.attach_metrics(Arc::clone(&self.telemetry.metrics));
        // The same for the durable trial book, where one has been opened: a
        // plane swapped in after `open_trial_book` would otherwise arrive
        // with the factory's in-process default and forget every family's
        // lifetime count at the moment the operator's key was installed,
        // which is the per-run accounting the book exists to prevent — and
        // it would do so silently, because an in-memory book answers every
        // question a durable one does. Carrying it across makes the two
        // calls order-independent rather than leaving a trap in the roots.
        if let Some(book) = self
            .central
            .factory()
            .ledger()
            .trial_book()
            .filter(|book| book.is_durable())
            .cloned()
        {
            central.factory_mut().attach_trial_book(book);
        }
        self.central = central;
    }

    /// Open the durable trial book on `store` and charge every holdout
    /// evaluation from now on to it.
    ///
    /// The factory is built with an in-process book, whose lifetime counts
    /// are this process's — so until a composition root called this, every
    /// restart forgot every family's lifetime trial count, and a sweep split
    /// across two runs was two small sweeps as far as the deflated Sharpe
    /// gate could tell. That is the laundering cumulative accounting exists
    /// to refuse.
    ///
    /// Refuses, and the caller must not start, when the store's journal does
    /// not verify. `TrialBook::open` replays every family's hash chain and
    /// refuses a record that was altered, removed, reordered or backdated; a
    /// process that fell back to an empty book over that store would begin
    /// counting at zero on top of the very tampering the chain caught.
    /// `namespace` is the store's name as the root configured it, so the
    /// refusal says which store to restore; the journal key inside the
    /// inner message names the family.
    pub fn open_trial_book(
        &mut self,
        store: Arc<dyn KeyValueStore>,
        namespace: &str,
    ) -> Result<()> {
        let book = TrialBook::open(store).map_err(|error| {
            Error::invalid(format!(
                "the trial book in store `{namespace}` does not verify, and this process will \
                 not start over it: {}. A count rebuilt over a broken chain is the understated \
                 count the chain exists to catch; restore `{namespace}` from its last good copy, \
                 or open a new namespace and record why",
                error.message()
            ))
        })?;
        self.central.factory_mut().attach_trial_book(book);
        Ok(())
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
    ///
    /// The break and halt series are recorded by the plane itself, at the
    /// line after the switch is tripped, and deliberately not here on the
    /// returned ingestion: `ingest` can still refuse after the trip, and a
    /// count that waited for `Ok` was un-counted by that refusal — a cell
    /// halted, an incident raised, and no series moved.
    ///
    /// The report's venue fills are then charged into the platform's risk
    /// aggregate — the one the desk's pre-trade check reads — under the
    /// cell's id as the aggregate's strategy axis. Until this existed only
    /// desk fills reached the aggregate, so cells could carry the book past
    /// a gross, leverage or bucket limit while the centre's counters read
    /// clean and the next desk order was admitted against them. The fills
    /// charged are exactly the ones the plane settled, read off the
    /// settlement rather than off the report a second time; a report the
    /// plane refuses whole is charged nothing, and one refused after it
    /// settled (a recall the register would not take) returns the error
    /// without charging, which the caller sees as the failure it is.
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
        let ingestion = central.ingest(report, autonomy.kill_switch_mut(), now)?;
        self.charge_cell_fills(&ingestion.cell, &ingestion.settlement.absorbed);
        self.book_settlement(&ingestion.settlement, now);
        Ok(ingestion)
    }

    /// Book what the settlement's exact attribution said each strategy
    /// realised into the per-user books (blueprint §43.4: `Strategy →
    /// Mandate → User`), and journal how.
    ///
    /// Fed from the attribution and not from the report, so the user books
    /// carry only what the centre accepted and closed to the last unit (ADR
    /// 0007); a fill the centre refused reaches no user. A position graded
    /// under other than exactly one strategy is recorded as a capture
    /// problem rather than split, because a split the attribution did not
    /// state would be a guess in the one record that must not carry one.
    ///
    /// Whose books: with no user mandate registered the desk is the only
    /// holder and takes the fill whole. With users registered the fill is
    /// split pro rata across the users with capital at work at the strategy
    /// by [`UserLedger::journal_pro_rata`], every share and the rounding
    /// unit's destination on the record; where no user has capital at work
    /// at that strategy the strategy was trading the desk's capital and the
    /// desk takes it whole, explicitly — the ledger's own stated path for
    /// that case — under a basis that says so, so the desk-whole booking
    /// that used to be the only one never survives silently. A split the
    /// ledger refuses (overflow, a share nobody holds) books nothing and is
    /// journalled as refused. The fill has already happened, so a refusal is
    /// a problem on the record and not an error to the caller — the same
    /// reasoning as [`Self::charge_cell_fills`].
    fn book_settlement(&mut self, settlement: &crate::central::plane::Settlement, now: Timestamp) {
        let Some(attribution) = &settlement.attribution else {
            return;
        };
        let desk = self.user_ledger.desk().clone();
        for position in &attribution.positions {
            let [strategy] = position.hypotheses.as_slice() else {
                self.capture_problems.push(format!(
                    "the attributed position {} names {} strategies and was not journalled \
                     to the user books; the ledger books exactly one per position",
                    position.object_id,
                    position.hypotheses.len()
                ));
                continue;
            };
            let fill = AttributedFill {
                strategy: StrategyId::new(strategy.as_str()),
                source: position.object_id.clone(),
                currency: Currency::USD,
                amount: position.total,
            };
            let basis = self.book_fill(&desk, &fill, now);
            if let BookingBasis::Refused { reason } = &basis {
                self.capture_problems.push(format!(
                    "the attributed position {} was settled and not booked to any user: \
                     {reason}",
                    position.object_id
                ));
            }
            let entry = LedgerEntry::Booked {
                strategy: fill.strategy.clone(),
                source: fill.source.clone(),
                currency: fill.currency,
                amount: fill.amount,
                basis,
            };
            if let Err(error) = self.journal_record(entry, "kernel/ledger", now) {
                self.capture_problems.push(format!(
                    "the booking of the attributed position {} was made and not journalled: {}",
                    position.object_id,
                    error.message()
                ));
            }
        }
    }

    /// Book one attributed fill and say on what basis. See
    /// [`Self::book_settlement`] for the choice.
    fn book_fill(&mut self, desk: &UserId, fill: &AttributedFill, now: Timestamp) -> BookingBasis {
        let users_registered = self.user_ledger.mandates().keys().any(|user| user != desk);
        let desk_whole =
            |ledger: &mut UserLedger, reason: String| match ledger.journal_to(desk, fill, now) {
                Ok(()) => BookingBasis::DeskWhole {
                    user: desk.clone(),
                    reason,
                },
                Err(error) => BookingBasis::Refused {
                    reason: error.message().to_string(),
                },
            };
        if !users_registered {
            return desk_whole(
                &mut self.user_ledger,
                "no user mandate is registered; the desk is the only holder".to_string(),
            );
        }
        if !self.has_entitlement_at(&fill.strategy, fill.currency) {
            return desk_whole(
                &mut self.user_ledger,
                format!(
                    "no user has {} at work at {}; the strategy was trading the desk's capital",
                    fill.currency, fill.strategy
                ),
            );
        }
        match self.user_ledger.journal_pro_rata(fill, now) {
            Ok(split) => BookingBasis::ProRata {
                shares: split.shares,
                entitlement_total: split.entitlement_total,
                remainder: split.remainder,
                remainder_to: split.remainder_to,
            },
            Err(error) => BookingBasis::Refused {
                reason: error.message().to_string(),
            },
        }
    }

    /// Whether any user holds positive settled cash at the strategy in the
    /// currency — the entitlement [`UserLedger::pro_rata_shares`] splits on.
    /// Asked here, before the split, so the desk-whole fallback is a stated
    /// choice rather than a reaction to the ledger's refusal message.
    fn has_entitlement_at(&self, strategy: &StrategyId, currency: Currency) -> bool {
        self.user_ledger
            .books()
            .iter()
            .filter(|((_, at), _)| at == strategy)
            .any(|(_, book)| {
                book.cash(currency)
                    .is_some_and(|cash| cash.settled().is_positive())
            })
    }

    /// Move capital from a user's mandate into one strategy's book, and
    /// journal it.
    ///
    /// The one way a user comes to have capital at work — and so a share of
    /// what the strategy realises. The eligibility registry is consulted
    /// first, through [`UserLedger::eligibility_of`], and a user it does not
    /// admit is refused with the registry's own reason and the refusal
    /// journalled as [`LedgerEntry::FundingRefused`] — before any book is
    /// touched, which is where a limit belongs. Every other refusal is
    /// [`UserLedger::fund`]'s: a non-positive amount, or a total past the
    /// mandate's investable capital. `fund` consults the registry again on
    /// its own; the check here is what puts the refusal on the record, and
    /// the check there is what makes the gate hold for a caller that did
    /// not ask.
    ///
    /// Until the registry existed this method funded on the mandate alone,
    /// because [`UserLedger::admit`] evaluated a product's eligibility this
    /// process had no registry for and would have refused every request.
    /// The product gate is still not consulted here — no product-eligibility
    /// registry exists yet — and that is stated rather than passed by
    /// inventing one.
    pub fn fund_user(
        &mut self,
        user: &UserId,
        strategy: &StrategyId,
        amount: Decimal,
        now: Timestamp,
    ) -> Result<()> {
        let Some(mandate) = self.user_ledger.mandate(user) else {
            return Err(Error::denied(format!(
                "{user} holds no mandate; enrol one in the configuration before funding"
            )));
        };
        let currency = mandate.currency();
        if let Err(why) = self.user_ledger.eligibility_of(user, now) {
            let reason = why.describe(user);
            self.journal_record(
                LedgerEntry::FundingRefused {
                    user: user.clone(),
                    strategy: strategy.clone(),
                    amount,
                    gate: why.name().to_string(),
                    reason: reason.clone(),
                },
                "kernel/ledger",
                now,
            )?;
            return Err(Error::denied(reason));
        }
        self.user_ledger.fund(user, strategy, amount, now)?;
        self.journal_record(
            LedgerEntry::Funded {
                user: user.clone(),
                strategy: strategy.clone(),
                currency,
                amount,
            },
            "kernel/ledger",
            now,
        )
    }

    // --- eligibility ------------------------------------------------------------

    /// Decide a user's eligibility, as an authenticated operator.
    ///
    /// The one runtime path, and it takes the same identity type an autonomy
    /// change does, held to the same two conditions: a stated reason and a
    /// credential fresh within [`ELIGIBILITY_CREDENTIAL_AGE`]. There is no
    /// overload taking a subject string, a flag, or anything an agent or a
    /// model could produce — an [`OperatorIdentity`] is constructed only
    /// where a credential was verified, and that is the point of taking it.
    /// The decision is journalled before the registry adopts it, so the log
    /// never lacks a decision the platform is acting on.
    pub fn decide_eligibility(
        &mut self,
        user: &UserId,
        decision: EligibilityDecision,
        operator: &OperatorIdentity,
        reason: impl Into<String>,
        now: Timestamp,
    ) -> Result<EligibilityRecord> {
        let reason = reason.into();
        if reason.trim().len() < 10 {
            return Err(Error::denied(
                "an eligibility decision needs a stated reason; the audit trail is the point",
            ));
        }
        if !operator.is_fresh(now, ELIGIBILITY_CREDENTIAL_AGE) {
            return Err(Error::denied(format!(
                "operator {} authenticated more than {:?} ago; re-authenticate to decide \
                 eligibility",
                operator.subject(),
                ELIGIBILITY_CREDENTIAL_AGE
            )));
        }
        let mut by = DecidedBy::operator(operator.subject(), operator.method())?;
        if let Some(approver) = operator.second_approver() {
            by = by.with_second_approver(approver);
        }
        let record = EligibilityRecord {
            user: user.clone(),
            decision,
            by,
            decided_at: now,
        };
        self.apply_eligibility(record.clone(), EligibilitySource::Operator, reason, now)?;
        Ok(record)
    }

    /// Journal an eligibility decision and apply it — in that order, and
    /// only after the registry has been shown to accept it.
    ///
    /// Checked on a scratch copy first so a decision the registry would
    /// refuse is never written to the log as though it stood; then
    /// journalled; then applied to the live registry. The same discipline
    /// the fabric journal keeps: the log has the record before the state
    /// moves, and never a record of a state that did not.
    fn apply_eligibility(
        &mut self,
        record: EligibilityRecord,
        source: EligibilitySource,
        reason: String,
        now: Timestamp,
    ) -> Result<()> {
        let mut scratch = self.user_ledger.clone();
        scratch.decide_eligibility(record.clone())?;
        self.journal_record(
            EligibilityEntry {
                record: record.clone(),
                source,
                reason,
            },
            ELIGIBILITY_ORIGIN,
            now,
        )?;
        self.user_ledger.decide_eligibility(record)
    }

    /// Rebuild the eligibility registry from the event log alone.
    ///
    /// Selects this kernel's eligibility records by topic and producer and
    /// replays them in log order through [`EligibilityRegistry::replay`],
    /// which refuses one out of order. The result is what the log says
    /// stands; a caller compares it to [`UserLedger::eligibility`] to prove
    /// the platform acted on nothing the log does not hold.
    pub fn replay_eligibility(&self) -> Result<EligibilityRegistry> {
        let mut records = Vec::new();
        for record in self.event_log.records() {
            if record.event.topic != EligibilityEntry::TOPIC
                || record.event.lineage.producer != ELIGIBILITY_ORIGIN
            {
                continue;
            }
            // The kernel appends the stream envelope's frame, so the body
            // is one envelope down from the log record — the same path
            // `replay_journal` takes, read here from the log rather than
            // the transport because the log is the record.
            let envelope = StreamEnvelope::from_frame(&record.event)?;
            records.push(envelope.decode::<EligibilityEntry>()?.body.record);
        }
        EligibilityRegistry::replay(records)
    }

    // --- the fabric journal ---------------------------------------------------

    /// Resume the fabric journal from what the platform's log already holds.
    ///
    /// A fresh journal when the log holds no fabric record. Otherwise the
    /// log is replayed — chain, hashes and every recorded outcome checked by
    /// [`qip_capital_fabric::replay::replay`] — and each fabric command is
    /// decided again into a fresh journal, which must arrive at the replayed
    /// state or assembly is refused. Without this a process restarted on a
    /// file-backed log would journal a corridor's proposal a second time,
    /// and a replay from genesis would refuse that record as claiming an
    /// outcome the control does not produce — the log the platform kept
    /// would stop being one it could read.
    ///
    /// Refused, naming the alternative, when fabric records exist but the
    /// in-memory log no longer starts at genesis: the log evicts replaceable
    /// records at capacity, and a replay that stepped over the gap would
    /// produce a state that reads as rebuilt from the log and is not.
    fn resume_fabric(log: &EventLog, seed: u64) -> Result<FabricJournal> {
        let correlation = CorrelationId::from_string(FABRIC_CORRELATION);
        let mut fabric = FabricJournal::new(seed, correlation);
        let fabric_records: Vec<FabricRecord> = log
            .records()
            .iter()
            .filter(|record| Self::is_fabric_record(record))
            .map(|record| record.event.decode::<FabricRecord>().map(|e| e.body))
            .collect::<Result<_>>()?;
        if fabric_records.is_empty() {
            return Ok(fabric);
        }
        if log
            .records()
            .first()
            .is_some_and(|first| first.sequence != 1)
        {
            return Err(Error::invalid(format!(
                "the event log holds {} fabric record(s) but no longer starts at genesis, so \
                 the fabric journal cannot be resumed from it; archive the log and start a \
                 new one, or open it with a capacity that keeps every record",
                fabric_records.len()
            )));
        }
        let replayed = qip_capital_fabric::replay::replay(log.records())?;
        for record in fabric_records {
            fabric.decide(record.command)?;
        }
        if *fabric.state() != replayed.state {
            return Err(Error::invalid(
                "the fabric journal decided the log's commands again and arrived at a state the \
                 log's replay does not hold; the log was written by something other than this \
                 kernel's controls",
            ));
        }
        Ok(fabric)
    }

    /// Whether a log record is one of the fabric's, by the same rule the
    /// replay uses: the fabric topic and the fabric producer, both.
    fn is_fabric_record(record: &qip_events::log::LogRecord) -> bool {
        record.event.topic == FabricRecord::TOPIC
            && record.event.lineage.producer == FABRIC_PRODUCER
    }

    /// Route one fabric command through the journal and the platform's log.
    ///
    /// The journal decides — executing on a scratch copy of the state,
    /// writing its own record, then adopting — and the same record is then
    /// appended to the platform's event log in the plain envelope form
    /// [`qip_capital_fabric::replay::replay`] decodes, under the fabric
    /// producer, and published to the cycle journal. A refusal by the
    /// control is an `Outcome::Refused` inside an `Ok` record, because the
    /// refusal is a decision and belongs in the log; an `Err` here is the
    /// journal or the log refusing to take the record at all, and the
    /// caller sees it as the failure it is.
    ///
    /// Public so an operator, a composition root or a test can propose a
    /// destination or a corridor, step one through its life, or have the
    /// gate assess an intent — each a record, none a movement: the gate's
    /// admitted verdict carries no way to execute (ADR 0021), and nothing
    /// in this process consumes one.
    pub fn decide_fabric(
        &mut self,
        command: FabricCommand,
        now: Timestamp,
    ) -> Result<FabricRecord> {
        let at = command.at();
        let record = self.fabric.decide(command)?;
        let correlation_id = self
            .context
            .ids()
            .generate::<qip_core::lineage::CorrelationKind>(now);
        let event_id = self.context.ids().generate::<EventKind>(now);
        let lineage = Lineage::root(correlation_id, FABRIC_PRODUCER);
        // The plain envelope, not the stream frame: the replay decodes the
        // record straight out of the log's payload, and a frame wraps the
        // payload in the stream envelope's wire form.
        let plain =
            qip_events::Envelope::new(event_id.clone(), at, now, lineage.clone(), record.clone());
        self.event_log.append(&plain.erase()?)?;
        let facts = EventFacts::derived(
            SourceIdentity::new(
                SourceId::new("qip-kernel"),
                SourceType::Internal,
                StreamRegion::new(HOME_REGION),
            ),
            Subject::unattributed(),
            FabricRecord::TOPIC,
        );
        let sealed = StreamEnvelope::seal(event_id, lineage, record.clone(), at, now, facts)?;
        self.journal.publish(sealed, now)?;
        Ok(record)
    }

    /// Hand in a statement of one balance at one venue, with the tolerance
    /// its reconciliation is judged against.
    ///
    /// The one observation channel this process can honestly attest. The
    /// kernel holds no read-only API key, no watch-only address and no view
    /// key — structurally: no field could hold one — so the only provenance
    /// it can record is a statement a person handed it, and the provenance
    /// is fixed here rather than taken from the caller. A statement for a
    /// venue-asset already held replaces it; one for a new venue-asset past
    /// [`MAX_OBSERVED_VENUE_ASSETS`] is refused. The tolerance is refused
    /// unless strictly positive, by [`TolerancePolicy::with_tolerance`].
    /// Nothing is assembled here: the wallet is assembled in the LEARN
    /// stage, against the ledger as the cycle left it.
    pub fn observe_statement(
        &mut self,
        venue: VenueId,
        asset: &str,
        observed: Decimal,
        tolerance: Decimal,
        observed_at: Timestamp,
    ) -> Result<()> {
        let asset = Asset::new(asset)?;
        let key = VenueAsset {
            venue: venue.clone(),
            asset: asset.clone(),
        };
        if !self.holdings_observed.contains_key(&key)
            && self.holdings_observed.len() >= MAX_OBSERVED_VENUE_ASSETS
        {
            return Err(Error::denied(format!(
                "a statement for {key} would be the {}th venue-asset observed against a bound \
                 of {MAX_OBSERVED_VENUE_ASSETS}; retire one before adding another",
                self.holdings_observed.len() + 1
            )));
        }
        self.wallet_tolerances = self
            .wallet_tolerances
            .clone()
            .with_tolerance(asset.clone(), tolerance)?;
        self.holdings_observed.insert(
            key,
            HoldingObservation::new(
                venue,
                asset,
                observed,
                observed_at,
                HoldingProvenance::Statement,
            ),
        );
        Ok(())
    }

    /// Assemble the wallet from every statement held and the ledger's view,
    /// and reconcile it, through the fabric journal.
    ///
    /// Reached from the LEARN stage, after ACT has moved cash, so the
    /// ledger view is the book the cycle left. Nothing is assembled while no
    /// statement is held — a wallet of zero holdings reads as an empty
    /// account rather than an unobserved one. The ledger's view is one
    /// entry, the desk's cash at the broker's venue, with the capital
    /// ledger's reservations against it; it is supplied only when a
    /// statement names that venue-asset, because the wallet refuses a
    /// ledger view nobody has observed. In-flight is zero and stated so:
    /// this process instructs no transfer (ADR 0021), so nothing is ever in
    /// flight towards its book. A stale statement makes the assembly a
    /// refused record, which the journal keeps; reconciliation then finds
    /// no wallet and is a refused record too. Both are decisions the log
    /// shows rather than a stage failure.
    fn reconcile_wallet(&mut self, now: Timestamp) -> Result<()> {
        if self.holdings_observed.is_empty() {
            return Ok(());
        }
        let observations: Vec<HoldingObservation> =
            self.holdings_observed.values().cloned().collect();
        let desk_venue = VenueId::new(self.broker.name());
        let desk_key = VenueAsset {
            venue: desk_venue.clone(),
            asset: Asset::new(Currency::USD.to_string())?,
        };
        let ledger_views = if self.holdings_observed.contains_key(&desk_key) {
            vec![LedgerView::new(
                desk_venue,
                desk_key.asset,
                self.capital.cash,
                self.reservations.reserved_total(),
                Decimal::ZERO,
            )?]
        } else {
            Vec::new()
        };
        self.decide_fabric(
            FabricCommand::Wallet(WalletCommand::Assemble {
                observations,
                ledger_views,
                freshness: STATEMENT_FRESHNESS,
                now,
            }),
            now,
        )?;
        self.decide_fabric(
            FabricCommand::Wallet(WalletCommand::Reconcile {
                tolerances: self.wallet_tolerances.clone(),
                at: now,
            }),
            now,
        )?;
        Ok(())
    }

    /// The fabric's state as the records built it — the wallet as last
    /// assembled, the reconciliation outcomes, every corridor and
    /// destination, every gate assessment. Read-only; the way in is
    /// [`Platform::decide_fabric`].
    pub fn fabric_state(&self) -> &FabricState {
        self.fabric.state()
    }

    /// The fabric journal's own record count, for a caller checking that
    /// the platform's log and the journal agree.
    pub fn fabric_records(&self) -> usize {
        self.fabric.records().len()
    }

    /// The statements held, latest per venue-asset.
    pub fn holdings_observed(&self) -> &BTreeMap<VenueAsset, HoldingObservation> {
        &self.holdings_observed
    }

    /// Charge one cell's absorbed fills into the running risk counters.
    ///
    /// The cell's id is the strategy the aggregate charges, not the
    /// contributing foundry strategies: the aggregate must stay O(1) in
    /// strategy count, and a counter per cell is bounded by the deployment's
    /// cell list — a value fixed at deployment — while a counter per
    /// contributor would grow with the foundry. The strategy-level budgets
    /// the contributors are held to are the cell's own concern, checked
    /// before netting where the intents are. Each fill is charged to the
    /// instrument's exposure buckets exactly as a desk fill is, so a sector
    /// a cell has filled counts toward the same bucket the desk's orders
    /// are projected onto.
    ///
    /// Cash is re-marked from the desk's ledger afterwards. A cell's fills
    /// spend capital granted in its envelope, which the desk's ledger does
    /// not hold, so the aggregate's gross, net, positions and buckets carry
    /// the cells while its equity and cash remain the desk's — a ratio limit
    /// therefore compares cell-and-desk gross against desk-only equity,
    /// which errs toward refusing and is stated here rather than hidden.
    ///
    /// A refusal is recorded as a capture problem rather than returned, for
    /// the reason [`Self::aggregate_fill`] gives: the fill has happened and
    /// the strategy books hold it, and an error here would tell the caller
    /// the report failed when it did not.
    fn charge_cell_fills(&mut self, cell: &str, fills: &[AbsorbedFill]) {
        for fill in fills {
            let axes = self.exposure_axes_for(&fill.object_id);
            if let Err(error) =
                self.aggregates
                    .apply_fill(cell, &fill.object_id, &axes, fill.signed_notional)
            {
                self.capture_problems.push(format!(
                    "a fill in {} reported by {cell} was settled and not aggregated: {}",
                    fill.object_id,
                    error.message()
                ));
            }
        }
        self.aggregates.mark_cash(self.capital.cash);
    }

    /// Feed realised cell outcomes back into the ladder and the allocator.
    ///
    /// The learn edge for strategies, distinct from [`Platform::learn_from`],
    /// which scores resolved theses. A thesis resolves on its own horizon; a
    /// strategy is judged against the baseline it was promoted on, and the two
    /// answer different questions with different evidence.
    ///
    /// Reached every cycle from the LEARN stage, over the outcomes
    /// [`CentralPlane::live_outcomes`] derives from the sessions each cell's
    /// fills were attributed to. Public as well, for a caller holding an
    /// observation the sessions cannot express; the stage's call is the one
    /// a deployed process makes.
    pub fn learn_from_cells(
        &mut self,
        outcomes: &[CellOutcome],
        now: Timestamp,
    ) -> Result<LearningReport> {
        let report = self.central.learn(outcomes, None, now)?;
        // Blueprint §35.2. A retirement's disposition — or the refusal to
        // guess one — reaches the log in the same call that retired the
        // strategy, so the instruction is reproducible from the log and a
        // retirement with no disposition record is a retirement that did not
        // happen here. Written before the report is returned: a caller that
        // read the report and then failed to journal it would leave the
        // ledger saying retired and the log saying nothing about the lots.
        for disposition in &report.dispositions {
            match disposition {
                DispositionOutcome::Dispositioned(record) => {
                    self.journal_record(record.clone(), "kernel/retirement", now)?;
                }
                DispositionOutcome::Refused(refusal) => {
                    self.journal_record(refusal.clone(), "kernel/retirement", now)?;
                }
            }
        }
        Ok(report)
    }

    /// Append one record to the event log and publish it to the journal,
    /// exactly as a cycle's entry is: the same frame reaches both, so neither
    /// can hold a record the other does not.
    fn journal_record<B: EventBody>(
        &mut self,
        body: B,
        origin: &str,
        now: Timestamp,
    ) -> Result<()> {
        let correlation_id = self
            .context
            .ids()
            .generate::<qip_core::lineage::CorrelationKind>(now);
        let facts = EventFacts::derived(
            SourceIdentity::new(
                SourceId::new("qip-kernel"),
                SourceType::Internal,
                StreamRegion::new(HOME_REGION),
            ),
            Subject::unattributed(),
            B::TOPIC,
        );
        let envelope = StreamEnvelope::seal(
            self.context.ids().generate::<EventKind>(now),
            Lineage::root(correlation_id, origin),
            body,
            now,
            now,
            facts,
        )?;
        self.event_log.append(&envelope.to_frame()?)?;
        self.journal.publish(envelope, now)?;
        Ok(())
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

    /// The last log sequence this platform inherited rather than wrote.
    ///
    /// Zero for a log that opened empty. Everything after it is this run's to
    /// hand to a durable archive; everything up to it a previous run already
    /// did. This is the platform's own answer rather than one derived from
    /// the log afterwards, because after assembly the log's last record is
    /// this run's universe record and the derivation is wrong by one — see
    /// the field's comment in `new`.
    pub fn inherited_through(&self) -> u64 {
        self.inherited_through
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

    /// The per-user, per-strategy books — read-only; fills reach them
    /// through [`Platform::ingest_cell_report`] and nothing else.
    pub fn user_ledger(&self) -> &UserLedger {
        &self.user_ledger
    }

    /// What every enrolled user may do, evaluated fresh under the viewer
    /// role against every product the central factory has registered.
    ///
    /// Evaluated here rather than in the API because the API is forbidden
    /// the capital crate (`api_boundary.rs`, `FORBIDDEN_CRATES`) and so
    /// cannot name the evaluator; what it may do is read the result. The
    /// products are the registered strategies' families, each under an
    /// eligibility record cleared in no jurisdiction, because this process
    /// holds no product-eligibility registry — a family nobody has cleared
    /// is refused everywhere, which is the type's own default and the honest
    /// one. The viewer role is fixed rather than mapped from the caller's
    /// API role: an operator credential on the API is not an investor in the
    /// ledger, and the surface that reads this is the viewer's. Ordered by
    /// user then family, so a report renders the same on every machine.
    /// Empty when no strategy is registered, which the caller states rather
    /// than fills in.
    pub fn viewer_entitlements(&self, now: Timestamp) -> Vec<qip_capital::ledger::Entitlement> {
        let families: std::collections::BTreeSet<&str> = self
            .central()
            .factory()
            .candidates()
            .map(|candidate| candidate.family().as_str())
            .collect();
        self.user_ledger
            .mandates()
            .iter()
            .flat_map(|(user, mandate)| {
                families.iter().map(move |family| {
                    qip_capital::ledger::Entitlement::evaluate(
                        user,
                        mandate,
                        qip_capital::ledger::Role::Viewer,
                        &qip_capital::ledger::ProductEligibility::new(*family),
                        now,
                    )
                })
            })
            .collect()
    }

    /// The seven checks of the transfer gate, in the order it runs them.
    ///
    /// Passed through from the fabric so the API, which may not depend on
    /// the fabric, lists the checks the gate actually runs rather than a
    /// copy that would drift the day an eighth was added. The gate runs
    /// only when an intent is routed through [`Platform::decide_fabric`],
    /// and its assessments are read from [`Platform::fabric_state`]; this
    /// returns the roster, not an assessment.
    pub fn transfer_gate_checks() -> &'static [qip_capital_fabric::gate::GateCheck; 7] {
        &qip_capital_fabric::gate::GateCheck::ALL
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

    /// The platform's world model — the one [`Platform::observe`] feeds, and
    /// the one the agents read through the desk's `read_world_model` gate.
    ///
    /// Read-only: `observe` is the writer, and a second writer would be a
    /// second story about what the platform believes. The borrow is a read
    /// lock on the shared slot, so hold it for a statement, not a cycle.
    pub fn world(&self) -> Reading<'_, WorldModel> {
        self.world.read()
    }

    /// The market view the agents read through the desk's `read_market_data`
    /// gate, as [`Platform::observe`] has fed it.
    pub fn market_view(&self) -> Reading<'_, MarketView> {
        self.market.read()
    }

    /// Whether the desk the agents hold reads the platform's own world model
    /// and market view rather than a copy.
    ///
    /// Answered by pointer identity on the shared slots, so a regression to
    /// a cold copy — the wiring this platform ran with for its whole life
    /// before this seam existed — is a `false` here and not a `no_data`
    /// finding somebody has to notice in a cycle report.
    pub fn desk_is_fed(&self) -> bool {
        self.world.feeds(&self.desk.world) && self.market.feeds(&self.desk.market)
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

    /// The realised drawdown of the book from its running peak — the
    /// figure the allocator sizes under.
    ///
    /// Exposed so the API's policy producer reads the same number the
    /// allocator does: the region shares it ships and the envelopes the
    /// centre issues then come from one drawdown, rather than from two
    /// readings that could differ by a fill. A statistic, so `f64`; it
    /// scales nothing here, and the crossing to money happens where the
    /// allocator applies its schedule.
    pub fn drawdown(&self) -> f64 {
        self.capital.drawdown()
    }

    /// P&L realised by position-reducing fills, cumulative.
    pub fn realised_pnl(&self) -> Decimal {
        self.capital.realised_pnl
    }

    /// Commissions and fees paid across every fill, cumulative.
    pub fn trading_costs(&self) -> Decimal {
        self.capital.costs_paid
    }

    /// Score resolved theses and recompute the calibration over the window.
    ///
    /// Called by the LEARN stage on every cycle with whatever resolved since
    /// the last, and callable directly for claims and outcomes that arrived
    /// another way. Until the stage called it nothing did: the platform
    /// wrote down a confidence with every hypothesis, settled the claim
    /// against what was published, and never once asked whether its seventy
    /// percents happened seventy percent of the time — the one number the
    /// blueprint calls the most important metric it has, computed by a
    /// function with no caller.
    ///
    /// The evaluations join the bounded window and the feedback engine runs
    /// over the whole window, not over this batch alone: calibration is a
    /// property of many resolved claims, and a Brier score from the two that
    /// resolved this cycle would be a different number every cycle. The
    /// report is `None` rather than an error when nothing in the window is
    /// informative — every verdict inconclusive — because that is the honest
    /// state of a platform whose claims have not yet moved anything, not a
    /// failure of the stage.
    pub fn learn_from(
        &mut self,
        claims: &[ThesisClaim],
        outcomes: &[ThesisOutcome],
        now: Timestamp,
    ) -> Result<LearningOutcome> {
        let (evaluations, skipped) = self.evaluator.evaluate_all(claims, outcomes, now);
        for evaluation in &evaluations {
            self.telemetry.metrics.count(
                names::THESES_EVALUATED,
                labels([("verdict", evaluation.verdict.as_str())]),
            );
        }
        self.evaluations.extend(evaluations.iter().cloned());
        if self.evaluations.len() > PREDICTION_HISTORY {
            let excess = self.evaluations.len() - PREDICTION_HISTORY;
            self.evaluations.drain(..excess);
        }

        // Charge every graded outcome to the components that produced the
        // thesis: the detector whose kind is the hypothesis class, and each
        // analyst whose run is among the contributors. Then hand the REASON
        // stage the factors the record now supports. Replaced whole every
        // time, so an origin whose window rolled below the minimum sample
        // loses its factor rather than keeping a stale one.
        //
        // An evaluation the self-model cannot charge — a class that is empty
        // or carries the key separator, a confidence outside `[0, 1]` — is
        // skipped with a problem line and the rest are charged. It used to
        // abort the pass with `?`, which threw away every other thesis that
        // resolved this cycle, the calibration over the window and the
        // factors REASON was owed, because one class string was malformed;
        // and it aborted after the evaluations had already joined the
        // window, so the window and the self-model disagreed about what had
        // been graded.
        let roster: Vec<String> = self
            .organisation
            .roster()
            .iter()
            .map(|manifest| manifest.id.clone())
            .collect();
        let mut problems = Vec::new();
        for evaluation in &evaluations {
            let charged = match Self::components_of(evaluation, &roster) {
                Ok(components) => self.self_model.absorb(evaluation, &components),
                Err(error) => Err(error),
            };
            if let Err(error) = charged {
                problems.push(format!(
                    "thesis {} (class {:?}) was graded but charged to no component: {}",
                    evaluation.hypothesis_id,
                    evaluation.class,
                    error.message()
                ));
            }
        }
        self.reasoning
            .set_origin_factors(self.self_model.origin_factors());

        let informative = self
            .evaluations
            .iter()
            .any(|evaluation| evaluation.verdict.is_informative());
        let report = if informative {
            let report = self.feedback.process(&self.evaluations, now)?;
            // Statistics, and therefore `f64` end to end: the Brier score
            // and the adjustment are already floats in the report.
            self.telemetry.metrics.gauge(
                names::BELIEF_BRIER_SCORE,
                labels([]),
                report.calibration.brier_score,
            );
            self.telemetry.metrics.gauge(
                names::BELIEF_CONFIDENCE_ADJUSTMENT,
                labels([]),
                report.calibration.confidence_adjustment,
            );
            self.telemetry.metrics.gauge(
                names::BELIEF_EVALUATIONS,
                labels([]),
                report.calibration.evaluated as f64,
            );
            self.last_calibration = Some(report.calibration.clone());
            Some(report)
        } else {
            None
        };
        Ok(LearningOutcome {
            evaluations,
            skipped,
            report,
            problems,
        })
    }

    /// The most recent calibration, if any thesis has resolved informatively.
    pub fn calibration(&self) -> Option<&CalibrationReport> {
        self.last_calibration.as_ref()
    }

    /// What the platform has measured of its own components, for the API and
    /// the tests. Empty until a thesis has resolved informatively.
    pub fn self_model(&self) -> &SelfModel {
        &self.self_model
    }

    /// Every thesis evaluation in the calibration window, oldest first.
    pub fn evaluations(&self) -> &[Evaluation] {
        &self.evaluations
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
                        self.volume_history.entry(key.clone()).or_default(),
                        bar.volume.to_f64(),
                    );
                    push_bounded_bars(
                        self.bar_history.entry(key).or_default(),
                        bar.as_ref().clone(),
                    );
                    // The desk's series, from the same bar at the same
                    // instant. The guard is taken and released before the
                    // rebuild below asks for the write lock again.
                    let overflowed = self.market.update(|market| {
                        market.snapshot.apply_bar(bar.as_ref().clone());
                        market
                            .snapshot
                            .get(&bar.object_id)
                            .is_some_and(|state| state.bars.len() >= DESK_SERIES_REBUILD_AT)
                    });
                    if overflowed {
                        self.rebuild_desk_snapshot();
                    }
                    bars.push(bar);
                    absorbed += 1;
                }
                SensedRecord::Trade(trade) => {
                    self.ensure_world_object(trade.object_id.as_str(), trade.at);
                    // "Last traded price" is the feature store's own
                    // definition of `close`, and a trade is exactly that.
                    self.world.update(|world| {
                        world.features_mut().record(
                            "close",
                            trade.object_id.as_str(),
                            FeatureValue::new(trade.price.to_f64(), trade.at, trade.at),
                        );
                    });
                    self.market
                        .update(|market| market.snapshot.apply_trade(trade));
                    absorbed += 1;
                }
                SensedRecord::Tick(tick) => {
                    self.ensure_world_object(tick.object_id.as_str(), tick.at);
                    self.world.update(|world| {
                        world.features_mut().record(
                            "close",
                            tick.object_id.as_str(),
                            FeatureValue::new(tick.price.to_f64(), tick.at, tick.at),
                        );
                    });
                    absorbed += 1;
                }
                SensedRecord::Quote(quote) => {
                    self.ensure_world_object(quote.object_id.as_str(), quote.at);
                    self.market
                        .update(|market| market.snapshot.apply_quote(quote.clone()));
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
                    self.market
                        .update(|market| market.snapshot.apply_book(*book));
                    absorbed += 1;
                }
                SensedRecord::News(item) => {
                    // Resolves entities, indexes the document as evidence and
                    // records sentiment at the item's published instant; the
                    // context supplies only entity-resolution bookkeeping,
                    // never a knowability stamp.
                    let context = &self.context;
                    self.world.update(|world| world.absorb_news(&item, context));
                    for event in MarketEvent::from_news(&item) {
                        self.push_market_event(event);
                    }
                    absorbed += 1;
                }
                SensedRecord::Fundamental(update) => {
                    self.define_fundamental_features(&update.metric, &update.provenance.source);
                    self.world.update(|world| world.absorb_fundamental(&update));
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
                    self.world.update(|world| world.absorb_macro(&observation));
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
                    // The world model owns the name and the key, from the
                    // vocabulary the analyst reads by. A reading it refuses —
                    // a licensed metric from an unlicensed dataset — is not
                    // absorbed and is reported, for the same reason a refused
                    // depth observation is: a reading quietly dropped looks
                    // exactly like a dataset that never published.
                    let outcome = self
                        .world
                        .update(|world| world.absorb_alternative_data(&point));
                    match outcome {
                        Ok(()) => absorbed += 1,
                        Err(error) => self.capture_problems.push(format!(
                            "an alternative-data reading was refused: {}",
                            error.message()
                        )),
                    }
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
                        self.world.update(|world| {
                            if world.features().definition(&feature).is_none() {
                                world.features_mut().define(
                                    Feature::new(
                                        &feature,
                                        "reference data field",
                                        update.provenance.source.clone(),
                                    )
                                    // Reference values persist until
                                    // restated; ten years is "no staleness
                                    // bound" said with a number.
                                    .with_staleness(Duration::from_days(3_650)),
                                );
                            }
                            world.features_mut().record(
                                &feature,
                                &update.object_id,
                                FeatureValue::new(
                                    value,
                                    update.effective_from,
                                    update.provenance.ingestion_time,
                                ),
                            );
                        });
                    }
                    absorbed += 1;
                }
            }
        }
        if !bars.is_empty() {
            self.world.update(|world| {
                world.absorb_bars(bars.iter().map(|bar| (bar.as_ref(), bar.close_time())));
            });
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
        self.world.update(|world| {
            if world.graph().node(object_id).is_none() {
                world.graph_mut().add_node(Node::new(
                    object_id,
                    NodeKind::FinancialObject,
                    object_id,
                    recorded_at,
                ));
            }
        });
    }

    /// Hold the desk's bar series to the platform's own bound by rebuilding
    /// the snapshot from `bar_history`.
    ///
    /// `BarSeries` has no bound of its own and `MarketSnapshot` no mutable
    /// path to one instrument, so the whole snapshot is rebuilt: every
    /// instrument's latest quote, book and trade are re-applied from the old
    /// view, and its bars are re-applied from the platform's bounded history
    /// rather than from the old series — so what the desk reads afterwards is
    /// derived from the platform's record and not from its own past. Bars
    /// arrive in order, so each re-application lands at the end of its series
    /// and the rebuild is linear. The one figure not preserved is
    /// `session_volume`, which a re-applied last trade restarts at that
    /// trade's size; nothing on the desk reads it, and it is said here rather
    /// than left to be discovered. Without this the desk would grow with
    /// uptime — the failure [`SERIES_HISTORY`] exists to prevent, reached
    /// through the one series that constant did not cover.
    fn rebuild_desk_snapshot(&self) {
        let history = &self.bar_history;
        self.market.update(|market| {
            let as_of = market.snapshot.as_of;
            let old = std::mem::replace(&mut market.snapshot, MarketSnapshot::new(as_of));
            for (object_id, state) in old.instruments() {
                if let Some(quote) = &state.quote {
                    market.snapshot.apply_quote(quote.clone());
                }
                if let Some(book) = &state.book {
                    market.snapshot.apply_book(book.clone());
                }
                if let Some(trade) = &state.last_trade {
                    market.snapshot.apply_trade(trade.clone());
                }
                for bar in history.get(object_id).into_iter().flatten() {
                    market.snapshot.apply_bar(bar.clone());
                }
            }
            market.snapshot.advance_to(as_of);
        });
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
            self.world.update(|world| {
                if world.features().definition(name).is_none() {
                    world.features_mut().define(
                        Feature::new(name, description, source)
                            .with_lag(Duration::from_days(30))
                            .with_staleness(Duration::from_days(200)),
                    );
                }
            });
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
            calibration: self.cycle_calibration.clone(),
            counterfactuals: self.cycle_counterfactuals.clone(),
            strategy_review: self.cycle_strategy_review.clone(),
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
        let (state, documents) = {
            let world = self.world.read();
            (world.state_at(now, now), world.index().len())
        };
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

    /// The REASON stage: the organisation's authorisation first, then the
    /// queue.
    ///
    /// The authorisation is checked and recorded on every cycle, whether or
    /// not anything is in the queue, because it is a state of the
    /// organisation and not of the question: an operator who sees the gauge
    /// rise on a quiet cycle learns the same fact a day earlier than one who
    /// waits for the next opportunity to be refused eighteen times.
    fn stage_reason(&mut self, now: Timestamp, lineage: &Lineage) -> StageOutcome {
        let expired = self.expired_manifests(now);
        let roster = self.organisation.roster().len();
        self.telemetry.metrics.gauge(
            names::AGENT_MANIFESTS_EXPIRED,
            labels([]),
            expired.len() as f64,
        );
        let outcome = self.reason_about_the_queue(now, lineage);
        if expired.is_empty() {
            return outcome;
        }
        // Up to three addresses in the line, the rest as a count: the
        // journal keeps every problem of every cycle, and a roster's worth of
        // ids on every cycle of a ninety-day lapse is a record nobody reads.
        let named: Vec<&str> = expired.iter().take(3).map(String::as_str).collect();
        let others = expired.len().saturating_sub(named.len());
        let addresses = if others == 0 {
            named.join(", ")
        } else {
            format!("{} and {others} more", named.join(", "))
        };
        outcome.with_problem(format!(
            "the organisation is unauthorised: {} of {roster} agent manifest(s) are past \
             their review interval ({addresses}); every run is refused until an operator \
             re-reviews them, and nothing here renews one",
            expired.len()
        ))
    }

    /// Manifests on the roster whose review interval has lapsed at `now`, in
    /// roster order.
    ///
    /// Read from the manifests themselves rather than from the governance
    /// review's rule name, so a rewording of that review cannot silently
    /// leave this count at zero.
    fn expired_manifests(&self, now: Timestamp) -> Vec<String> {
        self.organisation
            .roster()
            .iter()
            .filter(|manifest| manifest.is_expired(now))
            .map(|manifest| manifest.id.clone())
            .collect()
    }

    fn reason_about_the_queue(&mut self, now: Timestamp, lineage: &Lineage) -> StageOutcome {
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

        // Precedent: the nearest resolved episodes memory holds for this
        // situation, as known before `now`. Recorded beside the hypothesis
        // as evidence context, and handed to the panel through the brief's
        // *typed* field only — never through `brief.context`, which is the
        // string the reviewer's lesson matcher substring-matches against. A
        // precedent block written there could change which objections are
        // raised and, through their count, the confidence, which is the one
        // thing this record must not do. `BriefPrecedent` cannot be passed
        // where a `&str` is, so the panel can cite it and cannot count it;
        // see `HypothesisPrecedent` and `qip_agents::finding::BriefPrecedent`.
        let precedent = self.recall_precedent(&opportunity, now);
        let mut brief = qip_agents::finding::AgentBrief::new(
            opportunity.headline.clone(),
            now,
            opportunity.horizon,
        )
        .with_context(opportunity.historical_context.clone())
        .about_objects(opportunity.affected_objects.clone())
        .about_entities(opportunity.affected_entities.clone());
        // A precedent the brief refuses is a kernel bug — the store already
        // filters to `known_at < now` — so it is reported as a problem and
        // the panel is convened without it, rather than with a precedent
        // that fails the point-in-time rule.
        let mut briefing_problem = None;
        match precedent
            .as_ref()
            .map(|(query, recall)| brief_precedent(query, recall, now))
        {
            Some(Ok(Some(briefed))) => brief = brief.with_precedent(briefed),
            Some(Ok(None)) | None => {}
            Some(Err(error)) => {
                briefing_problem = Some(format!(
                    "the recalled precedent could not be briefed to the panel: {}",
                    error.message()
                ));
            }
        }

        let report = self.organisation.dispatch(&brief, now, lineage);
        self.telemetry
            .metrics
            .increment(names::AGENT_RUNS, labels([]), report.runs.len() as u64);
        // A run the host refused before the agent ran is reported with the
        // host's reason, not as `failed`: an expired manifest and a bug in the
        // analyst are different problems with different owners, and for
        // ninety days after assembly they read identically.
        let refused: BTreeMap<&str, &str> = report
            .runs
            .iter()
            .filter_map(|run| match &run.status {
                RunStatus::Refused { reason } => Some((run.agent_id.as_str(), reason.as_str())),
                _ => None,
            })
            .collect();
        let mut problems: Vec<String> = report
            .failed
            .iter()
            .map(|agent| match refused.get(agent.as_str()) {
                Some(reason) => format!("{agent} refused to run: {reason}"),
                None => format!("{agent} failed"),
            })
            .collect();
        problems.extend(briefing_problem);
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
                let decision = if !approved {
                    outcome = outcome
                        .with_problem(format!("rejected on review: {}", reasoned.review.rationale));
                    DecisionTaken::RejectedOnReview
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
                            DecisionTaken::Approved
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
                            DecisionTaken::NotSizeable
                        }
                    }
                };
                // The episode this cycle is, written down with what was
                // decided, and the precedent it was decided beside. The
                // episode waits in `pending_episodes` until LEARN resolves
                // the claim; only then does it enter memory.
                match self.record_precedent(
                    &opportunity,
                    &reasoned,
                    &report,
                    precedent.as_ref(),
                    decision,
                    now,
                ) {
                    Ok(Some(digest)) => {
                        outcome.detail.push_str(&format!(
                            "; precedent: {} nearest, {} resolved, {} agreed",
                            digest.nearest, digest.resolved, digest.agreeing
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        outcome = outcome.with_problem(format!(
                            "the episode could not be recorded: {}",
                            error.message()
                        ));
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

    /// The regime in force for `subject`, as the labels memory encodes.
    fn regime_label(&self, subject: &str) -> RegimeLabel {
        RegimeLabel {
            market: self.market_regime(subject).as_str().to_string(),
            volatility: self.volatility_regime(subject).as_str().to_string(),
        }
    }

    /// The situation as REASON sees it before the panel reports — the
    /// instrument, the regime, the claim the anomaly implies and the
    /// horizon — and the precedent memory holds for it at `now`.
    ///
    /// The query carries no stances, no findings and a zero confidence,
    /// because none of those exist yet; the encoding leaves those blocks at
    /// zero rather than guessing them. `None` where the opportunity names no
    /// instrument, which is also where no hypothesis can be formed.
    fn recall_precedent(
        &self,
        opportunity: &Opportunity,
        now: Timestamp,
    ) -> Option<(EpisodeQuery, Recall)> {
        let subject = opportunity.affected_objects.first()?;
        let claim = opportunity.anomalies.first().and_then(|anomaly| {
            mechanism_for(anomaly).map(|(_, claim)| ClaimRecord {
                class: anomaly.kind.as_str().to_string(),
                claim: claim.as_str().to_string(),
                direction: claim.implied_sign().unwrap_or(0.0),
                confidence: 0.0,
            })
        });
        let query = EpisodeQuery {
            instrument: subject.as_str().to_string(),
            regime: self.regime_label(subject.as_str()),
            claim,
            findings: None,
            stances: Vec::new(),
            horizon: opportunity.horizon,
        };
        let recall = self.episodes.recall(&query, now, PRECEDENT_K);
        Some((query, recall))
    }

    /// Write the precedent beside the hypothesis and hold the episode until
    /// its claim resolves.
    ///
    /// Returns the digest where a precedent was recorded, `None` where the
    /// opportunity names no instrument, and an error where the episode would
    /// not validate — a finding with a conviction outside `[0, 1]`, say —
    /// which the stage reports rather than remembering a record the memory
    /// would refuse at resolution.
    fn record_precedent(
        &mut self,
        opportunity: &Opportunity,
        reasoned: &ReasoningOutcome,
        report: &qip_investment_agents::OrganisationReport,
        precedent: Option<&(EpisodeQuery, Recall)>,
        decision: DecisionTaken,
        now: Timestamp,
    ) -> Result<Option<PrecedentDigest>> {
        let Some(subject) = opportunity.affected_objects.first() else {
            return Ok(None);
        };
        let hypothesis = &reasoned.hypothesis;
        let direction = hypothesis.claim.implied_sign().unwrap_or(0.0);
        let claim = ClaimRecord {
            class: hypothesis.class.clone(),
            claim: hypothesis.claim.as_str().to_string(),
            direction,
            confidence: hypothesis.effective_confidence(),
        };
        // Agent-id order, so the same panel encodes identically whatever
        // order its runs returned in.
        let stances: Vec<AnalystStance> = report
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.agent_id.clone(),
                    AnalystStance {
                        agent_id: finding.agent_id.clone(),
                        direction: match finding.direction {
                            qip_agents::finding::Direction::Positive => StanceDirection::Positive,
                            qip_agents::finding::Direction::Negative => StanceDirection::Negative,
                            qip_agents::finding::Direction::Ambiguous => StanceDirection::Ambiguous,
                            qip_agents::finding::Direction::Neutral => StanceDirection::Neutral,
                        },
                        conviction: finding.conviction,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();

        let empty = Recall {
            examined: 0,
            probed: 0,
            nearest: Vec::new(),
        };
        let recall = precedent.map_or(&empty, |(_, recall)| recall);
        let digest = PrecedentDigest::of(&recall.nearest, direction);
        let nearest = recall
            .nearest
            .iter()
            .map(|recalled| PrecedentEntry {
                episode_id: recalled.episode.episode_id.clone(),
                instrument: recalled.episode.instrument.clone(),
                at: recalled.episode.at,
                known_at: recalled.episode.known_at,
                similarity: recalled.similarity,
                claim: recalled.episode.claim.claim.clone(),
                decision: recalled.episode.decision,
                realised_move_bps: recalled
                    .episode
                    .outcome
                    .as_ref()
                    .map(|outcome| outcome.realised_move_bps),
                agreed: recalled
                    .episode
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.agrees_with(direction)),
            })
            .collect();
        self.precedents.push(HypothesisPrecedent {
            hypothesis_id: hypothesis.hypothesis_id.as_str().to_string(),
            cycle: self.cycle,
            confidence: claim.confidence,
            examined: recall.examined,
            memory_size: self.episodes.len(),
            nearest,
            digest: digest.clone(),
        });
        if self.precedents.len() > PREDICTION_HISTORY {
            self.precedents
                .drain(..self.precedents.len() - PREDICTION_HISTORY);
        }

        let draft = Episode {
            episode_id: episode_id_for(hypothesis.hypothesis_id.as_str()),
            instrument: subject.as_str().to_string(),
            regime: self.regime_label(subject.as_str()),
            findings: FindingsSummary {
                runs: report.runs.len(),
                findings: report.findings.len(),
                coverage: report.coverage(),
                contested: report.is_contested(),
            },
            stances,
            claim,
            horizon: hypothesis.horizon,
            decision,
            outcome: None,
            // Knowable at formation for now; LEARN restamps `known_at` to
            // the resolution instant when the outcome arrives, and nothing
            // reads a pending episode before then.
            at: now,
            known_at: now,
        };
        draft.validate()?;
        self.pending_episodes.push(draft);
        if self.pending_episodes.len() > PREDICTION_HISTORY {
            self.pending_episodes
                .drain(..self.pending_episodes.len() - PREDICTION_HISTORY);
        }
        Ok(Some(digest))
    }

    /// Move each resolved thesis's episode from pending into memory, with its
    /// outcome and knowable from `now`.
    ///
    /// Called from the LEARN stage's resolve path, which is the only place an
    /// outcome exists. A thesis whose episode is no longer pending — evicted
    /// as stale, or formed before this field existed — is skipped: memory
    /// holds what was reasoned, and a record reconstructed from the claim
    /// alone would not be that. Returns how many entered memory.
    fn remember_resolved(&mut self, outcomes: &[ThesisOutcome], now: Timestamp) -> Result<usize> {
        let mut remembered = 0usize;
        for outcome in outcomes {
            let wanted = episode_id_for(&outcome.hypothesis_id);
            let Some(index) = self
                .pending_episodes
                .iter()
                .position(|episode| episode.episode_id == wanted)
            else {
                continue;
            };
            let mut episode = self.pending_episodes.remove(index);
            episode.outcome = Some(EpisodeOutcome {
                resolved_at: outcome.observed_at,
                realised_move_bps: outcome.realised_move_bps,
                realised_pnl: outcome.realised_pnl,
            });
            episode.known_at = now;
            self.episodes.remember(episode)?;
            remembered += 1;
        }
        Ok(remembered)
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
    /// The §6.2 degradation table as the centre reads it at `now`, for the
    /// central sizing path.
    ///
    /// Three rows are measured, each from the one fact its object records
    /// at the seam where it changes. Row 6, the self-model, from
    /// [`SelfModel::sample_facts`] against the engine's own minimum sample
    /// and [`SELF_MODEL_HORIZON`]: a model that has never absorbed an
    /// outcome reads unavailable, one thin or older than the horizon reads
    /// stale. Row 2, the causal graph, from the world model's
    /// `CausalGraph::last_updated` against [`CAUSAL_GRAPH_HORIZON`]: a graph
    /// that has never absorbed a claim reads unavailable and one whose
    /// newest claim is older than a quarter reads stale. Row 4, the belief
    /// state, from the reasoning engine's `BeliefState::last_updated`
    /// against [`BELIEF_HORIZON`]: before the first belief is formed in a
    /// process it reads unavailable, and one formed more than a session ago
    /// reads stale. Each narrows by its own multiplier, and every reading
    /// is the object's own fact rather than a constant. An earlier version
    /// of this method started rows 2 and 4 fresh, on the argument that the
    /// centre holds the live objects and has no shipped age to judge them
    /// on; that was true of the age and false of the freshness — a graph
    /// seeded from claims a year old is the live object and is stale, and
    /// the demo seed backdates every claim by a year, so the centre sized
    /// at full budget against relationships a year's evidence has not
    /// re-estimated. The edge cell's table is untouched: a cell never holds
    /// a self-model, and its floor is its own.
    ///
    /// Fallible on purpose: a record claiming outcomes with no newest
    /// instant, or an instant after `now`, is the reporter's bug, and sizing
    /// as though the row were fresh would hide it.
    pub fn central_degradation(
        &self,
        now: Timestamp,
    ) -> Result<qip_contracts::degradation::DegradationState> {
        use qip_contracts::degradation::{
            BELIEF_HORIZON, BeliefFreshness, CAUSAL_GRAPH_HORIZON, Capability,
            CausalGraphFreshness, DegradationState, SELF_MODEL_HORIZON, SelfModelFreshness,
        };
        let mut state = DegradationState::fully_available();
        let self_model = SelfModelFreshness::assess(
            self.self_model.sample_facts(),
            qip_learning_engine::self_model::MINIMUM_SAMPLE,
            SELF_MODEL_HORIZON,
            now,
        )?;
        state.observe(Capability::SelfModel, self_model.freshness());
        let causal = CausalGraphFreshness::assess(
            self.world.read().causal().last_updated(),
            CAUSAL_GRAPH_HORIZON,
            now,
        )?;
        state.observe(Capability::CausalGraph, causal.freshness());
        let belief =
            BeliefFreshness::assess(self.reasoning.beliefs().last_updated(), BELIEF_HORIZON, now)?;
        state.observe(Capability::BeliefState, belief.freshness());
        Ok(state)
    }

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
        // Narrowed by §6.2 as the centre reads it — the self-model row
        // measured from the learning engine's own record. A refused
        // assessment refuses the construction: nothing is sized against a
        // table that could not be read, rather than sized as though it read
        // fresh.
        let multiplier = self.central_degradation(now)?.central_sizing_multiplier();
        let budget = free.checked_mul(multiplier).ok_or_else(|| {
            Error::numeric(format!(
                "the free budget {free} narrowed by the degradation multiplier {multiplier} \
                 overflows; nothing is sized against a number that cannot be represented"
            ))
        })?;

        let outcome = self.constructor.construct(
            theses,
            &covariance,
            &current,
            Money::new(budget, Currency::USD),
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
        // A claim about a series standing at zero has no magnitude to be
        // graded against — every move is infinitely many basis points of it —
        // so it is not written down rather than written down ungradeable.
        if anomaly.observed.abs() <= f64::EPSILON {
            return Ok(false);
        }

        // What the hypothesis claimed, in the shape the learning engine
        // grades. Direction from the comparison the proposition tests;
        // magnitude as the reversion of the anomaly's measured displacement,
        // in basis points of the reference, which is the same quantity the
        // thesis is sized on; confidence as the review left it.
        let direction = match comparison {
            Comparison::GreaterThan => 1.0,
            Comparison::LessThan => -1.0,
            // Unreachable given the table above, which names only the two.
            // Listed rather than wildcarded so a third comparison added to
            // that table has to say which way it points.
            Comparison::AtLeast | Comparison::AtMost | Comparison::EqualTo => 0.0,
        };
        let expected_move_bps =
            (anomaly.expected - anomaly.observed).abs() / anomaly.observed.abs() * 10_000.0;
        let claim = ThesisClaim {
            hypothesis_id: reasoned.hypothesis.hypothesis_id.as_str().to_string(),
            class: reasoned.hypothesis.class.clone(),
            subject: anomaly.subject.clone(),
            formed_at: now,
            resolves_at: now.saturating_add(reasoned.hypothesis.horizon),
            direction,
            expected_move_bps: direction * expected_move_bps,
            confidence: reasoned.hypothesis.effective_confidence(),
            falsifiers: reasoned.hypothesis.falsifiers.clone(),
            contributors: reasoned.hypothesis.contributors.clone(),
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
            claim: Some(claim),
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
                if !matches!(proposal.status(), ProposalStatus::Draft) {
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
            .filter(|proposal| proposal.status().is_releasable())
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
        // Legs whose order carries fewer units than the leg sized, because the
        // instrument's lot does not divide the target. Counted into the
        // stage's summary so the residual is on the record rather than a
        // silent difference between the proposal and the book.
        let mut sized_to_lots = 0usize;
        let mut problems: Vec<String> = sign_off_problems.clone();

        let approved: Vec<Proposal> = self
            .proposals
            .iter()
            .filter(|proposal| proposal.status().is_releasable())
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

                // The leg's size is a continuous target the optimiser's
                // weight produced; the order's is a whole number of the
                // instrument's lots. Expressed here, where the leg becomes an
                // order, from the same grid the order manager judges it
                // against below — so a leg that rounds to nothing is stopped
                // before a control decision is spent on it, and one that
                // does not is submitted at a size the gate will admit rather
                // than refused on every cycle for a grid the sizer never saw.
                let quantity = self.whole_lots(leg.object_id.as_str(), leg.quantity);
                if !quantity.is_positive() {
                    refused += 1;
                    problems.push(format!(
                        "{} leg {index} was refused: {} of {} is less than one lot; nothing to \
                         release",
                        proposal.proposal_id.as_str(),
                        leg.quantity,
                        leg.object_id.as_str()
                    ));
                    continue;
                }
                if quantity != leg.quantity {
                    sized_to_lots += 1;
                }

                let order = Order::new(
                    order_id,
                    leg.object_id.clone(),
                    Self::release_side(leg.side),
                    quantity,
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
                "{released} order(s) released from {} approved proposal(s), {refused} refused, \
                 {sized_to_lots} sized down to whole lots; risk monitor says {}",
                approved.len(),
                action.as_str()
            ),
        );
        for problem in problems {
            outcome = outcome.with_problem(problem);
        }
        outcome
    }

    /// The largest whole number of the instrument's lots that does not exceed
    /// `quantity`, or `quantity` itself for an instrument with no grid.
    ///
    /// Read from the order manager's installed grid rather than a second copy
    /// of the lot, so the size released and the size judged are the same
    /// fact. Floors toward zero, as `Decimal::floor_to_step` does, because a
    /// sized leg is a ceiling the risk projection approved and a lot rounded
    /// up would be exposure nobody sized.
    fn whole_lots(&self, object_id: &str, quantity: Decimal) -> Decimal {
        match self.orders.instrument_feasibility(object_id) {
            Some(grid) => quantity.floor_to_step(grid.lot_size()),
            None => quantity,
        }
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
        self.cycle_calibration = None;
        self.cycle_counterfactuals = None;
        // The wallet, against the book ACT left. A refusal by the control is
        // a record the journal keeps; an error here is the journal or the
        // log refusing the record, which is a problem on the cycle's record
        // rather than a reason to skip scoring what resolved.
        if let Err(error) = self.reconcile_wallet(now) {
            self.capture_problems.push(format!(
                "the wallet was not journalled this cycle: {}",
                error.message()
            ));
        }
        let (outcome, by_hypothesis) = self.attribute(now);
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
        // Score whatever resolved, and say how calibrated the platform is on
        // everything that has. This is the seam where the fact becomes known:
        // the platform's own series are the only source it holds for the
        // observables its claims name, and the attribution above is the only
        // source of what each thesis earned.
        match self.calibrate_resolved(&by_hypothesis, now) {
            Ok(Some(pass)) => {
                let detail = format!("{}; {pass}", outcome.detail);
                outcome = StageOutcome { detail, ..outcome };
            }
            Ok(None) => {}
            Err(error) => {
                outcome = outcome.with_problem(format!(
                    "resolved theses could not be calibrated: {}",
                    error.message()
                ));
            }
        }
        // Price what the gates declined, now that the world has said what
        // would have happened. Blueprint §12: a platform that learns only
        // from the trades it took is learning from a heavily selected
        // sample, and every veto is a data point until something scores it.
        let (priced, problems) = self.score_declined(now);
        if let Some(priced) = priced {
            let detail = format!("{}; {priced}", outcome.detail);
            outcome = StageOutcome { detail, ..outcome };
        }
        for problem in problems {
            outcome = outcome.with_problem(problem);
        }
        // Judge every strategy on what its cells have realised since it was
        // promoted. Blueprint §20.3: retirement is as automated as promotion,
        // and until this call existed it was not — the review seam and the
        // triggers behind it were reached only by tests, so a strategy could
        // decay at the floor for a year in a deployed process and nothing
        // would notice.
        let (reviewed, problem) = self.review_strategies(now);
        if let Some(reviewed) = reviewed {
            let detail = format!("{}; {reviewed}", outcome.detail);
            outcome = StageOutcome { detail, ..outcome };
        }
        if let Some(problem) = problem {
            outcome = outcome.with_problem(problem);
        }
        for problem in std::mem::take(&mut self.capture_problems) {
            outcome = outcome.with_problem(problem);
        }
        outcome
    }

    /// Run the demotion monitor over the sessions every cell has realised,
    /// through [`Self::learn_from_cells`], and say what it did.
    ///
    /// Returns `(None, None)` when no cell has closed a session for any
    /// strategy since that strategy's baseline was established — most cycles
    /// on a platform whose central plane holds no live strategy, and every
    /// cycle before the first full session. A stage that said "0 reviewed"
    /// on those would make the additive property of the central plane false
    /// in the journal: a cycle on a platform with no cells must read exactly
    /// as it did before this existed.
    ///
    /// A failure is returned as a problem rather than an error for the reason
    /// every LEARN step gives: the cycle has happened, and a review that
    /// could not run is a fact about this cycle to record, not a reason to
    /// lose the rest of the stage's account.
    fn review_strategies(&mut self, now: Timestamp) -> (Option<String>, Option<String>) {
        self.cycle_strategy_review = None;
        let outcomes = self.central.live_outcomes(now);
        if outcomes.is_empty() {
            return (None, None);
        }
        let report = match self.learn_from_cells(&outcomes, now) {
            Ok(report) => report,
            Err(error) => {
                return (
                    None,
                    Some(format!(
                        "{} strategy observation(s) could not be reviewed: {}",
                        outcomes.len(),
                        error.message()
                    )),
                );
            }
        };
        let retired = report
            .learnings
            .iter()
            .filter(|learning| {
                learning.review.stage_after == qip_contracts::gate::GateStage::Retired
                    && learning.review.stage_before != qip_contracts::gate::GateStage::Retired
            })
            .count();
        let demoted = report
            .learnings
            .iter()
            .filter(|learning| {
                learning.review.moved()
                    && learning.review.stage_after != qip_contracts::gate::GateStage::Retired
            })
            .count();
        let (dispositioned, dispositions_refused) =
            report
                .dispositions
                .iter()
                .fold(
                    (0, 0),
                    |(scheduled, refused), disposition| match disposition {
                        DispositionOutcome::Dispositioned(_) => (scheduled + 1, refused),
                        DispositionOutcome::Refused(_) => (scheduled, refused + 1),
                    },
                );
        let journal = StrategyReviewJournal {
            reviewed: report.learnings.len(),
            demoted,
            retired,
            dispositioned,
            dispositions_refused,
            skipped: report.skipped.len(),
        };
        let detail = format!(
            "{} strategy(ies) reviewed on realised sessions ({} demoted, {} retired, {} \
             dispositioned, {} disposition(s) refused, {} skipped)",
            journal.reviewed,
            journal.demoted,
            journal.retired,
            journal.dispositioned,
            journal.dispositions_refused,
            journal.skipped
        );
        self.cycle_strategy_review = Some(journal);
        (Some(detail), None)
    }

    /// The components a graded thesis is charged to.
    ///
    /// The detector is the hypothesis class — [`Self::synthesise`] sets the
    /// class to the anomaly's kind, and the direct evidence's origin is the
    /// detector that raised it, so the class is the key the REASON factor is
    /// looked up under. Each analyst is recovered from its contributor run
    /// id, which the chief mints as `run-<manifest id>-<sequence>`, matched
    /// against the roster rather than parsed: an id with a hyphen in it —
    /// every analyst on this roster has one — would split wrong, and a run
    /// of an agent no longer on the roster is charged to nobody rather than
    /// to whichever prefix happened to fit.
    fn components_of(evaluation: &Evaluation, roster: &[String]) -> Result<Vec<ComponentKey>> {
        let mut components = vec![ComponentKey::detector(&evaluation.class)?];
        for contributor in &evaluation.contributors {
            let Some(rest) = contributor.strip_prefix("run-") else {
                continue;
            };
            let analyst = roster.iter().find(|id| {
                rest.strip_prefix(id.as_str())
                    .and_then(|tail| tail.strip_prefix('-'))
                    .is_some_and(|sequence| sequence.parse::<u64>().is_ok())
            });
            if let Some(analyst) = analyst {
                let key = ComponentKey::analyst(analyst)?;
                if !components.contains(&key) {
                    components.push(key);
                }
            }
        }
        Ok(components)
    }

    /// Settle the claims whose horizon has passed against the platform's own
    /// series, grade them, and recompute the calibration.
    ///
    /// Returns `Ok(None)` when nothing was due, which is most cycles: a thesis
    /// resolves on its own horizon, not the cycle's. The observations come
    /// from the same series the detectors read — the last close, the
    /// realised volatility over the volatility detector's own window, the
    /// last quoted spread — so a claim is settled by the quantity it was made
    /// about rather than by a neighbour of it. A claim naming a series the
    /// platform no longer holds stays open; resolving it as failure is how a
    /// system marks itself right by scoring the questions nobody answered.
    ///
    /// The realised move is measured from the same observation that settled
    /// the verdict, so the two cannot disagree about what was published. The
    /// realised P&L is the attribution's figure for the hypothesis, and zero
    /// where the thesis was never expressed as a trade — the honest answer
    /// for a claim the platform made and did not act on.
    fn calibrate_resolved(
        &mut self,
        by_hypothesis: &BTreeMap<String, Decimal>,
        now: Timestamp,
    ) -> Result<Option<String>> {
        let due: Vec<(String, Decimal)> = self
            .predictions
            .iter()
            .filter(|prediction| prediction.is_open() && prediction.proposition.resolves_at <= now)
            .filter_map(|prediction| match &prediction.proposition.criteria {
                ResolutionCriteria::Threshold { metric, value, .. } => {
                    Some((metric.clone(), *value))
                }
                _ => None,
            })
            .collect();
        if due.is_empty() {
            return Ok(None);
        }

        let observations = self.published_observations(now, due.iter().map(|(m, _)| m.as_str()));
        let scored = self.score_predictions(&observations, now);
        if scored.is_empty() {
            return Ok(None);
        }

        let mut claims = Vec::with_capacity(scored.len());
        let mut outcomes = Vec::with_capacity(scored.len());
        let mut ungradeable = 0usize;
        for (hypothesis, _) in &scored {
            let Some(prediction) = self
                .predictions
                .iter()
                .find(|prediction| &prediction.hypothesis == hypothesis)
            else {
                continue;
            };
            let Some(claim) = prediction.claim.clone() else {
                ungradeable += 1;
                continue;
            };
            let ResolutionCriteria::Threshold { metric, value, .. } =
                &prediction.proposition.criteria
            else {
                ungradeable += 1;
                continue;
            };
            let Some(Observation::Numeric(observed)) = observations.get(metric) else {
                ungradeable += 1;
                continue;
            };
            // Money to statistic: the move and the P&L are graded as
            // statistics, and this is where the exact figures become floats.
            let realised_move_bps = if value.is_zero() {
                0.0
            } else {
                (observed.to_f64() - value.to_f64()) / value.to_f64().abs() * 10_000.0
            };
            outcomes.push(ThesisOutcome {
                hypothesis_id: hypothesis.clone(),
                observed_at: now,
                realised_move_bps,
                realised_pnl: by_hypothesis
                    .get(hypothesis)
                    .map_or(0.0, |pnl| pnl.to_f64()),
                falsifiers_triggered: Vec::new(),
                // The platform holds no observation of a mechanism's own
                // observables, so it does not claim to have confirmed one.
                mechanism_confirmed: None,
            });
            claims.push(claim);
        }

        let learned = self.learn_from(&claims, &outcomes, now)?;
        // A thesis graded but charged to nobody is a stage problem, not a
        // stage failure: the pass went on without it, and the outcome says so
        // through the same drain every other LEARN-time problem reaches.
        self.capture_problems
            .extend(learned.problems.iter().cloned());
        let mut summary = format!(
            "{} thesis(es) resolved, {} graded",
            scored.len(),
            learned.evaluations.len()
        );
        if ungradeable > 0 {
            summary.push_str(&format!(", {ungradeable} ungradeable"));
        }
        if !learned.skipped.is_empty() {
            summary.push_str(&format!(", {} skipped", learned.skipped.len()));
        }
        match &learned.report {
            Some(report) => {
                summary.push_str(&format!("; calibration {}", report.calibration.summarise()));
                self.cycle_calibration = Some(CalibrationJournal {
                    evaluated_this_cycle: learned.evaluations.len(),
                    evaluations_in_window: report.calibration.evaluated,
                    brier_score: report.calibration.brier_score,
                    confidence_adjustment: report.calibration.confidence_adjustment,
                    is_overconfident: report.calibration.is_overconfident,
                });
            }
            None => summary.push_str("; nothing informative yet to calibrate on"),
        }
        // Each resolved thesis's episode enters memory here, knowable from
        // now, so the next REASON can recall it as precedent.
        let remembered = self.remember_resolved(&outcomes, now)?;
        if remembered > 0 {
            summary.push_str(&format!("; {remembered} episode(s) remembered"));
        }
        Ok(Some(summary))
    }

    /// What the platform's own series say, for the metrics named.
    ///
    /// The metric is `observable:subject`, as [`Platform::record_prediction`]
    /// spells it. Three observables are published, each from the series the
    /// detector that raised the claim read: `close` is the last close held for
    /// the subject; `volatility` is the standard deviation of log returns over
    /// the volatility-shift detector's window, computed with the same
    /// functions; `spread` is the last quoted spread in basis points. Anything
    /// else is left unpublished, so the claim stays open rather than being
    /// settled by a number the platform never measured.
    fn published_observations<'a>(
        &self,
        now: Timestamp,
        metrics: impl Iterator<Item = &'a str>,
    ) -> Observations {
        let mut observations = Observations::at(now);
        for metric in metrics {
            let Some((observable, subject)) = metric.split_once(':') else {
                continue;
            };
            let value = match observable {
                "close" => self
                    .price_history
                    .get(subject)
                    .and_then(|series| series.last().copied()),
                "volatility" => self.price_history.get(subject).and_then(|series| {
                    let returns = qip_numerics::stats::log_returns(series);
                    if returns.len() < VOLATILITY_CLAIM_WINDOW {
                        return None;
                    }
                    Some(qip_numerics::stats::stddev(
                        &returns[returns.len() - VOLATILITY_CLAIM_WINDOW..],
                    ))
                }),
                "spread" => self
                    .spread_history
                    .get(subject)
                    .and_then(|series| series.last().copied()),
                _ => None,
            };
            if let Some(value) = value.and_then(Decimal::from_f64) {
                observations = observations.with(metric, Observation::Numeric(value));
            }
        }
        observations
    }

    /// Attribute what the fills cost. The body of LEARN, without the capture
    /// reporting wrapped around it.
    ///
    /// Returns the stage outcome and what each hypothesis earned, which the
    /// calibration pass grades the resolved theses on.
    fn attribute(&mut self, now: Timestamp) -> (StageOutcome, BTreeMap<String, Decimal>) {
        let fills = self.orders.fills();
        if fills.is_empty() {
            return (
                StageOutcome::ran(
                    Stage::Learn,
                    0,
                    "no fills to attribute; nothing has resolved yet",
                ),
                BTreeMap::new(),
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
                let by_hypothesis = attribution.by_hypothesis();
                (
                    StageOutcome::ran(
                        Stage::Learn,
                        attribution.positions.len(),
                        format!(
                            "{} fill(s) attributed across {} hypothesis(es), {} of \
                             implementation cost, residual {}",
                            attribution.positions.len(),
                            by_hypothesis.len(),
                            attribution.implementation_cost(),
                            attribution.residual()
                        ),
                    ),
                    by_hypothesis,
                )
            }
            Err(error) => (
                StageOutcome::ran(Stage::Learn, 0, "attribution failed")
                    .with_problem(error.message().to_string()),
                BTreeMap::new(),
            ),
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
        self.risk_state_from(&self.aggregates)
    }

    /// The risk state the checks evaluate, from a set of aggregate figures.
    ///
    /// This is the read side of the O(1) contract, and it is a separate
    /// function taking the figures as a trait so a test can hand it a probe
    /// that counts every figure consulted: the property "reads the counters,
    /// never the strategies" is held by that test rather than by this
    /// comment. Production passes the platform's own aggregate; nothing else
    /// should. Until this existed the state was rebuilt here by a walk over
    /// the book's lots, which was O(1) in strategy count only because the
    /// desk had no strategies yet.
    ///
    /// The tail statistics the limits read are derived in the risk lib from
    /// each configured limit's own confidence — `RiskState::with_tail_risk`
    /// — so the key a limit looks up and the key the figure is filed under
    /// are formatted by one rule. This function used to carry a second copy
    /// of that rule; two copies of a key format are one rounding boundary
    /// away from a limit that silently never evaluates. The return series is
    /// the crossing from the book's `Decimal` equity to a statistic, made in
    /// `equity_returns`.
    #[doc(hidden)]
    pub fn risk_state_from(&self, figures: &impl AggregateFigures) -> RiskState {
        let returns = self.equity_returns();
        RiskState::from_figures(figures).with_tail_risk(self.monitor.limits(), &returns)
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
        // The order's own buckets, so the pre-trade projection adds this
        // order to the sector, country, class and venue it belongs to before
        // the limits read it. The projection used to be handed no axes, so
        // an order that would take a bucket over its limit was admitted and
        // the breach was discovered — if the aggregate had carried a bucket
        // at all — only by the monitor, one cycle late.
        let axes = self.exposure_axes_for(object_id.as_str());
        let result = self.orders.submit(
            order,
            self.broker.as_mut(),
            &self.autonomy,
            &risk_state,
            axes,
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
            let refused = self.capture(
                now,
                &correlation,
                object_id.clone(),
                Action::Rejected {
                    order_id: result.order_id.clone(),
                    gate: gate.clone(),
                    reason: reason.clone(),
                },
                RealisedOutcome::nothing_happened(now),
                reason,
            );
            // Kept for the twin. The refusal record above carries no side and
            // no size — it says which control said no — and pricing what the
            // veto cost needs the trade that was proposed. A full window is a
            // refusal to queue, counted, not an eviction: dropping the oldest
            // waiting path would silently choose which veto goes unexamined.
            if let Some(decision) = refused {
                if self.declined.len() >= DECLINED_HISTORY {
                    self.telemetry.metrics.count(
                        names::COUNTERFACTUALS_UNSCORED,
                        labels([("reason", "capacity")]),
                    );
                    self.capture_problems.push(format!(
                        "refused order {} will not be priced: {DECLINED_HISTORY} declined paths \
                         are already waiting to be",
                        result.order_id
                    ));
                } else {
                    self.declined.push(DeclinedPath {
                        decision,
                        order_id: result.order_id.clone(),
                        object_id: object_id.clone(),
                        side: book_side(side),
                        quantity,
                        gate,
                    });
                }
            }
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
            let moved = self.capital.apply_fill(
                object_id.as_str(),
                side,
                fill.price,
                fill.quantity,
                fill.costs,
            );
            self.aggregate_fill(object_id.as_str(), moved);
        }
    }

    /// Carry one desk fill into the running risk counters.
    ///
    /// `moved` is the change in the instrument's at-cost notional the fill
    /// produced; a fill that moved nothing at cost has nothing to aggregate,
    /// and the aggregate would refuse it as not a fill. The desk's own orders
    /// are charged to one budget holder, [`DESK_STRATEGY`], because a desk
    /// order carries hypotheses and a proposal rather than a foundry strategy
    /// and the aggregate refuses a fill that names no strategy.
    ///
    /// The fill is charged to the instrument's exposure buckets — sector,
    /// country, asset class and venue, as the universe's record carries them
    /// (see the `exposure_axes` field for what is fed and what is not) — so
    /// each bucket is a running aggregate that a limit reads in constant
    /// time, never a sum over positions. Until this passed the axes the
    /// aggregate held no bucket for any instrument, and the two bucket limits
    /// in every default set were controls that could not fire.
    ///
    /// A refusal is recorded as a capture problem rather than returned: the
    /// fill has already happened and been journalled, and an error here
    /// would tell the caller the order failed when the venue says it did
    /// not. The problem surfaces on the next cycle's report, and the
    /// aggregate's fill count falling behind the order manager's is the
    /// symptom an operator would see.
    fn aggregate_fill(&mut self, object_id: &str, moved: Decimal) {
        let axes = self.exposure_axes_for(object_id);
        if !moved.is_zero()
            && let Err(error) = self
                .aggregates
                .apply_fill(DESK_STRATEGY, object_id, &axes, moved)
        {
            self.capture_problems.push(format!(
                "a fill in {object_id} was booked and not aggregated: {}",
                error.message()
            ));
        }
        self.aggregates.mark_cash(self.capital.cash);
        if let Err(error) = self
            .aggregates
            .mark(self.capital.equity(), self.capital.drawdown())
        {
            self.capture_problems.push(format!(
                "the book's mark was refused by the risk aggregate, which keeps its last: {}",
                error.message()
            ));
        }
    }

    /// The running risk counters, as the pre-trade check reads them.
    pub fn risk_figures(&self) -> &RiskAggregates {
        &self.aggregates
    }

    /// The exposure buckets one instrument is charged to, by axis.
    ///
    /// Empty for an instrument the assembled universe holds no record for:
    /// a bucket has to come from reference data, and a fill in an unknown
    /// instrument is charged to the book's gross and net and to nothing
    /// narrower rather than to a bucket somebody guessed. The map is
    /// cloned because it is at most four short entries and the callers hand
    /// it across a seam that takes it by value.
    pub fn exposure_axes_for(&self, object_id: &str) -> BTreeMap<String, String> {
        self.exposure_axes
            .get(object_id)
            .cloned()
            .unwrap_or_default()
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
            bridged_transfers_failed: 0,
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
                    // The seam where a bridged deposit stops existing. Failed
                    // here, on the reorganisation the chain state reported,
                    // rather than noticed later by diffing snapshots: a
                    // transfer still waiting on a withdrawn block is value
                    // the destination could credit against nothing.
                    let failed = self.bridges.on_reorg(&reorg, self.context.now());
                    for _ in &failed {
                        self.telemetry.metrics.count(
                            names::BRIDGE_TRANSFERS_FAILED,
                            labels([("failure", BridgeFailure::SourceReorg.as_str())]),
                        );
                    }
                    absorption.bridged_transfers_failed += failed.len();
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

    /// Open a cross-chain transfer, so a reorganisation of its source block
    /// can fail it. Refuses a duplicate id, as the ledger does.
    pub fn open_bridge_transfer(&mut self, transfer: BridgeTransfer) -> Result<()> {
        self.bridges.open(transfer)
    }

    /// Every bridge transfer the platform has opened, in flight or settled.
    pub fn bridges(&self) -> &BridgeLedger {
        &self.bridges
    }

    // --- predictions --------------------------------------------------------

    /// The precedent recorded beside each hypothesis, most recent last.
    pub fn precedents(&self) -> &[HypothesisPrecedent] {
        &self.precedents
    }

    /// Every falsifiable claim the platform has made, open and resolved.
    pub fn predictions(&self) -> &[RecordedPrediction] {
        &self.predictions
    }

    /// How many claims [`Self::predictions`] can hold before the oldest is
    /// evicted.
    ///
    /// Published beside the slice so a reader of the working set can say how
    /// much of the record it is looking at. A count served without its bound
    /// reads as the whole history, and the bound exists precisely because the
    /// whole history is not what the process holds.
    pub const fn prediction_window() -> usize {
        PREDICTION_HISTORY
    }

    // --- the price tape -----------------------------------------------------

    /// Each instrument's closes as the platform absorbed them, oldest first,
    /// bounded by [`SERIES_HISTORY`].
    ///
    /// Read-only, and the only way out for the series the detectors and the
    /// simulate stage read. A consumer that wants a statistic over these —
    /// the API's correlation view is the one that exists — computes it from
    /// the same tape the cycle did, so the number it reports is reproducible
    /// against this process rather than against a copy it kept itself.
    pub fn price_history(&self) -> &BTreeMap<String, Vec<f64>> {
        &self.price_history
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

    /// Decode the journal back into the cycle entries that were written.
    ///
    /// Filtered to the cycle entry's own topic: the journal also carries the
    /// universe record written at assembly and every policy issue, and
    /// decoding those as cycle entries would fail the whole read the first
    /// time either was present.
    pub fn journal_entries(&self) -> Result<Vec<CycleJournalEntry>> {
        self.replay_journal(&EventFilter::new().topic(CycleJournalEntry::TOPIC))?
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

    /// Price every alternative to one order the platform sent, or to one a
    /// control refused.
    ///
    /// The market is the caller's, because the twin evaluates against history
    /// and the platform holds no bar store of its own beyond the bounded one
    /// the LEARN stage prices from. Everything the set reports is
    /// [`qip_twin::Simulated`] and stays that way: there is no conversion out
    /// of it, so no figure in here can reach
    /// [`qip_twin::capture::OutcomeCapture::realised_pnl`].
    ///
    /// For a refused order the "actual" is standing aside — nothing happened,
    /// nothing was earned — and the `trade` alternative is the order as it
    /// was proposed. That entry's difference is what the veto cost, which is
    /// the number blueprint §12 says is otherwise unknowable: whether the
    /// rule that fired was protective or merely expensive.
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
            });
        let (decision, actual, realised) = match placed {
            Some(placed) => {
                let Action::OrderPlaced {
                    venue,
                    side,
                    quantity,
                    ..
                } = &placed.decision.action
                else {
                    return Err(Error::invalid("the captured action is not an order"));
                };
                // What was realised is the fill's, not the placement's:
                // placing an order costs nothing on its own, and pricing an
                // alternative against a zero would make every alternative
                // look like a regret.
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
                (placed.decision.clone(), actual, realised)
            }
            None => {
                let declined = self
                    .declined
                    .iter()
                    .find(|declined| &declined.order_id == order_id)
                    .ok_or_else(|| {
                        Error::not_found(format!(
                            "no order {order_id} was captured, so there is nothing to \
                             counterfact"
                        ))
                    })?;
                let actual = ActualTrade::new(
                    declined.object_id.clone(),
                    declined.side,
                    declined.quantity,
                    VenueId::new(UNROUTED_VENUE),
                    HOME_REGION,
                    declined.decision.at,
                )?;
                (
                    declined.decision.clone(),
                    actual,
                    RealisedOutcome::nothing_happened(declined.decision.at),
                )
            }
        };
        self.counterfactuals
            .evaluate(market, &decision, &actual, &realised)
    }

    /// What each priced refusal would have done, most recent last.
    pub fn declined_scores(&self) -> &[DeclinedScore] {
        &self.declined_scores
    }

    /// How many refused orders are waiting for their horizon or their bars.
    pub fn declined_awaiting_score(&self) -> usize {
        self.declined.len()
    }

    /// Price the declined paths whose horizon has passed, up to the cap.
    ///
    /// The LEARN stage's counterfactual pass, and the production caller of
    /// [`Platform::evaluate_alternatives`]. A path is due once the twin's
    /// horizon has elapsed since the refusal *and* the platform has observed
    /// a bar closing after that instant, because the twin marks the
    /// alternative at the horizon and a market that ends before it has no
    /// price to mark at; a path whose bars have not arrived is left waiting,
    /// not scored on a guess. Anything the twin itself refuses is counted
    /// under `unscored{reason="refused"}`, reported on the cycle and dropped,
    /// because a path the twin refused once it will refuse every cycle.
    ///
    /// Bounded by [`COUNTERFACTUALS_PER_CYCLE`]. What the cap leaves is
    /// counted, journaled and priced on a later cycle: the count is what
    /// makes "the twin is falling behind the gates" a number rather than a
    /// silence.
    fn score_declined(&mut self, now: Timestamp) -> (Option<String>, Vec<String>) {
        let horizon = self.counterfactuals.horizon();
        let due: Vec<OrderId> = self
            .declined
            .iter()
            .filter(|declined| {
                let marks_at = declined.decision.at.saturating_add(horizon);
                marks_at <= now
                    && self
                        .bar_history
                        .get(declined.object_id.as_str())
                        .and_then(|bars| bars.iter().map(Bar::close_time).max())
                        .is_some_and(|last_close| last_close >= marks_at)
            })
            .map(|declined| declined.order_id.clone())
            .collect();
        if due.is_empty() {
            return (None, Vec::new());
        }

        let deferred = due.len().saturating_sub(COUNTERFACTUALS_PER_CYCLE);
        if deferred > 0 {
            self.telemetry.metrics.increment(
                names::COUNTERFACTUALS_DEFERRED,
                labels([]),
                deferred as u64,
            );
        }

        let mut scored = 0usize;
        let mut regrets = 0usize;
        let mut problems = Vec::new();
        for order_id in due.into_iter().take(COUNTERFACTUALS_PER_CYCLE) {
            let Some(index) = self
                .declined
                .iter()
                .position(|declined| declined.order_id == order_id)
            else {
                continue;
            };
            let (object_id, gate, declined_at) = {
                let declined = &self.declined[index];
                (
                    declined.object_id.clone(),
                    declined.gate.clone(),
                    declined.decision.at,
                )
            };
            let priced = self
                .bar_history
                .get(object_id.as_str())
                .cloned()
                .ok_or_else(|| Error::not_found(format!("no bars are held for {object_id}")))
                .and_then(|bars| {
                    TwinMarket::new(
                        bars,
                        CostModel::liquid_equity(),
                        COUNTERFACTUAL_IMPACT_WINDOW,
                    )
                })
                .and_then(|mut market| self.evaluate_alternatives(&order_id, &mut market));
            // Priced or refused, the path leaves the queue: what it would
            // have earned is now known, or the twin has said it cannot be.
            self.declined.remove(index);
            match priced {
                Ok(set) => {
                    let trade = set.by_kind("trade");
                    let regret = trade.is_some_and(Counterfactual::favours_the_alternative);
                    let would_have_earned = trade.map_or(Simulated::ZERO, |entry| {
                        entry.counterfactual_outcome.simulated_pnl()
                    });
                    self.telemetry.metrics.count(
                        names::COUNTERFACTUALS_SCORED,
                        labels([("gate", gate.as_str())]),
                    );
                    if regret {
                        regrets += 1;
                        self.telemetry.metrics.count(
                            names::COUNTERFACTUAL_REGRETS,
                            labels([("gate", gate.as_str())]),
                        );
                    }
                    scored += 1;
                    self.declined_scores.push(DeclinedScore {
                        order_id,
                        object_id,
                        gate,
                        declined_at,
                        scored_at: now,
                        would_have_earned,
                        regret,
                        alternatives: set.len(),
                    });
                    if self.declined_scores.len() > DECLINED_HISTORY {
                        let excess = self.declined_scores.len() - DECLINED_HISTORY;
                        self.declined_scores.drain(..excess);
                    }
                }
                Err(error) => {
                    self.telemetry.metrics.count(
                        names::COUNTERFACTUALS_UNSCORED,
                        labels([("reason", "refused")]),
                    );
                    problems.push(format!(
                        "refused order {order_id} could not be priced: {}",
                        error.message()
                    ));
                }
            }
        }

        self.cycle_counterfactuals = Some(CounterfactualJournal {
            scored,
            regrets,
            deferred,
        });
        let mut summary = format!("{scored} declined path(s) priced, {regrets} regret(s)");
        if deferred > 0 {
            summary.push_str(&format!(", {deferred} deferred by the per-cycle cap"));
        }
        (Some(summary), problems)
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
        // A feasibility veto is carried as `Malformed` and named by its own
        // gate literal — the same four the edge plane charts under — so an
        // off-lot order and an order tracing to no hypothesis are not one bar.
        RefusalReason::Malformed { .. } => refusal.feasibility_gate().unwrap_or("order-validation"),
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
/// [`push_bounded`] for the bar series, with the same bound and the same
/// single drain.
fn push_bounded_bars(series: &mut Vec<Bar>, bar: Bar) {
    series.push(bar);
    if series.len() > SERIES_HISTORY {
        series.drain(..series.len() - SERIES_HISTORY);
    }
}

/// The desk's bar series length at which the snapshot is rebuilt from the
/// platform's own bounded history.
///
/// Twice [`SERIES_HISTORY`] rather than the bound itself because a
/// `MarketSnapshot` offers no mutable path to one instrument's series — it
/// can only be rebuilt whole, at a cost linear in instruments times the
/// bound — so the rebuild is amortised over a bound's worth of bars. Between
/// rebuilds the desk holds at most this many bars per instrument; after each
/// it holds exactly `bar_history`.
const DESK_SERIES_REBUILD_AT: usize = 2 * SERIES_HISTORY;

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
/// The episode id a hypothesis's episode is kept under, on both sides of
/// the resolve seam.
fn episode_id_for(hypothesis_id: &str) -> String {
    format!("ep-{hypothesis_id}")
}

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

    /// A platform whose book is large enough that one sized leg exceeds the
    /// conservative single-order notional limit under the centre's own
    /// degradation table.
    ///
    /// The limit is untouched, as in [`small_book_platform`]. The default
    /// ten-million book used to serve this purpose, when the table started
    /// the causal-graph and belief-state rows fresh; now that both are
    /// measured, a platform that has seeded no world and formed no belief
    /// sizes against 0.1875 of its free balance, and a leg off ten million
    /// lands at 150k — inside the 250k cap. Forty million puts the same leg
    /// at 600k. Every test that uses this fixture asserts that premise
    /// before asserting the refusal, so a future narrowing cannot turn a
    /// refusal test into a release test silently.
    fn over_limit_order_notional() -> Decimal {
        Decimal::from_int(250_000)
    }

    fn over_limit_book_platform() -> Platform {
        let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(40_000_000));
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

    /// The small-book platform over a universe whose two names each state a
    /// lot, so the order manager holds a grid for them.
    ///
    /// The fixtures above assemble over `Universe::new()`, which installs no
    /// grid at all — honest for what they test, and exactly the state in
    /// which the release path could submit any fraction the optimiser
    /// produced.
    fn gridded_small_book_platform(lot: Decimal) -> Platform {
        use qip_financial::asset_class::{InstrumentType, Sector};
        use qip_financial::object::FinancialObject;
        use qip_financial::quality::Provenance;

        let now = Timestamp::from_secs(1_760_000_000);
        let mut universe = Universe::new();
        for symbol in ["AAPL", "MSFT"] {
            universe
                .insert(
                    FinancialObject::builder(
                        qip_core::ObjectId::from_string(symbol),
                        symbol,
                        InstrumentType::CommonStock,
                    )
                    .venue("XNAS")
                    .sector(Sector::InformationTechnology)
                    .price(Decimal::from_int(100))
                    .lot_size(lot)
                    .provenance(Provenance::synthetic("test", now))
                    .build(now)
                    .expect("valid object"),
                )
                .expect("insertable");
        }
        let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(200_000));
        let (context, _clock) = qip_core::Context::deterministic(now, config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            universe,
            LimitSet::conservative_default(),
        )
        .expect("the platform assembles")
    }

    #[test]
    fn a_sized_leg_is_released_as_whole_lots_and_the_installed_gate_admits_it() {
        // The optimiser sizes a leg as notional over price, a fraction; the
        // venue accepts whole lots. With the grid installed on the order
        // manager and nothing expressing the leg at it, every sized leg
        // would be refused on every cycle — a gate that fires on everything
        // reads as a platform that never trades, not as a control.
        let lot = Decimal::from_int(7);
        let mut platform = gridded_small_book_platform(lot);
        feed_history(&mut platform, "AAPL", 30);
        feed_history(&mut platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));

        let now = Timestamp::from_secs(1_760_000_100);
        platform.stage_decide(now);

        // The premise: at least one leg was sized off the grid, so the
        // assertion below is about expression and not about legs that
        // happened to land on it.
        let sized_legs = platform
            .proposals
            .last()
            .expect("a proposal is recorded")
            .legs
            .clone();
        assert!(
            !sized_legs.is_empty(),
            "the premise failed: no legs were sized"
        );
        let off_grid: Vec<_> = sized_legs
            .iter()
            .filter(|leg| leg.quantity.floor_to_step(lot) != leg.quantity)
            .cloned()
            .collect();
        assert!(
            !off_grid.is_empty(),
            "every sized leg sat on a lot of seven by chance; the test measures nothing"
        );
        let legs = sized_legs.len();

        let correlation = CorrelationId::from_string("corr-lots");
        let outcome = platform.stage_act(now, &correlation);

        assert_eq!(
            outcome.produced, legs,
            "{legs} leg(s) were sized and {} order(s) were released: {} :: problems={:?}",
            outcome.produced, outcome.detail, outcome.problems
        );
        assert!(
            platform.orders.refusals().is_empty(),
            "the gate refused a leg the stage was meant to have expressed at its lot: {:?}",
            platform.orders.refusals()
        );
        for order in platform.orders.orders() {
            assert_eq!(
                order.quantity.floor_to_step(lot),
                order.quantity,
                "order {} was released at {} — not a whole number of lots of {lot}",
                order.order_id,
                order.quantity
            );
            let leg = sized_legs
                .iter()
                .find(|leg| leg.object_id == order.object_id)
                .expect("an order names a sized leg");
            assert!(
                order.quantity <= leg.quantity && leg.quantity - order.quantity < lot,
                "order {} at {} is not the largest whole-lot size under the sized {}",
                order.order_id,
                order.quantity,
                leg.quantity
            );
        }
        // The residual is on the record: the summary counts the legs whose
        // order carries fewer units than were sized.
        assert!(
            outcome
                .detail
                .contains(&format!("{} sized down to whole lots", off_grid.len())),
            "the stage summary does not say how many legs were expressed at the lot: {}",
            outcome.detail
        );
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
            !sized.status().is_releasable(),
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
            .find(|proposal| matches!(proposal.status(), ProposalStatus::Released { .. }))
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
        // controls are not decoration: a leg sized against the over-limit
        // book is 600k against a 250k single-order notional cap, and the
        // deterministic pre-trade check refuses it before it reaches the
        // broker.
        //
        // This test is why `a_sized_proposal_is_signed_by_two_controls_and_released_as_orders`
        // uses a smaller book rather than a larger limit. Both paths are real
        // and both are asserted; relaxing the limit would have deleted this
        // one silently — and so would a narrowing of the budget that took
        // the leg under the cap, which is why the premise is asserted.
        let mut platform = over_limit_book_platform();
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
        assert!(
            sized
                .legs
                .iter()
                .any(|leg| leg.quantity * leg.reference_price > over_limit_order_notional()),
            "the premise failed: no leg exceeds the {} cap, so a release below would not be \
             the control failing",
            over_limit_order_notional()
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
        // directly rather than inferring it from sizes. The budget is the
        // free balance narrowed by the central degradation table — this
        // platform's self-model has never absorbed an outcome, so the table
        // reads it unavailable — and the multiplier is read from the same
        // table rather than restated, so the assertion stays about the
        // reservation and not about the constant.
        let multiplier = platform
            .central_degradation(Timestamp::from_secs(1_760_000_160))
            .expect("the table reads")
            .central_sizing_multiplier();
        assert!(
            multiplier < Decimal::ONE,
            "the premise: a never-absorbed self-model narrows, so the budget below is the \
             narrowed free balance and not the free balance by coincidence"
        );
        assert_eq!(
            second.equity.amount,
            (equity - first.traded_notional()) * multiplier,
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
        let mut refused = over_limit_book_platform();
        feed_history(&mut refused, "AAPL", 30);
        feed_history(&mut refused, "MSFT", 30);
        refused.pending_theses.push(thesis("AAPL", 0.6));
        refused.pending_theses.push(thesis("MSFT", -0.4));
        refused.stage_decide(now);
        let sized = refused.proposals.last().expect("a proposal");
        assert!(
            sized.traded_notional().is_positive(),
            "the premise failed: nothing sized, so nothing could be refused"
        );
        assert!(
            sized
                .legs
                .iter()
                .any(|leg| leg.quantity * leg.reference_price > over_limit_order_notional()),
            "the premise failed: no leg exceeds the {} cap",
            over_limit_order_notional()
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
            !proposal.status().is_releasable(),
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
                claim: None,
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

    // --- the desk -----------------------------------------------------------

    /// The desk the agents read is the platform's own world model and market
    /// view, not a copy taken at assembly.
    ///
    /// The failure this prevents ran in every deployed binary until this seam
    /// existed: `Platform::observe` absorbed three hundred and twenty tape
    /// periods into the platform's fields while the desk held a
    /// `WorldModel::new()` and an empty `MarketSnapshot`, so every analyst
    /// answered `no_data` and no hypothesis could gather a second origin.
    #[test]
    fn the_desk_reads_what_the_platform_absorbed_and_stays_within_the_bound() {
        let mut platform = platform();
        let object = ObjectId::from_string("obj-AAA");

        // The premise, structurally: the gates share the platform's slots.
        assert!(
            platform.desk_is_fed(),
            "the desk's world and market gates are not wired to the platform's upstreams"
        );
        // And before anything is absorbed the desk is honestly empty, so a
        // populated desk below is absorption and not assembly.
        assert!(platform.market_view().snapshot.get(&object).is_none());

        // Enough bars to cross the rebuild threshold and then some, so the
        // test sees the desk both before and after a rebuild.
        let extra = 5;
        platform.observe(counting_bars(DESK_SERIES_REBUILD_AT + extra));

        let history = platform
            .bar_history
            .get("obj-AAA")
            .expect("the platform holds the instrument's bars");
        assert_eq!(
            history.len(),
            SERIES_HISTORY,
            "the platform's own bound moved"
        );

        let market = platform.market_view();
        let state = market
            .snapshot
            .get(&object)
            .expect("the desk holds market state for the instrument the platform absorbed");
        // Bounded: never more than the rebuild threshold, and after the
        // rebuild exactly the platform's history plus what arrived since.
        assert!(
            state.bars.len() < DESK_SERIES_REBUILD_AT,
            "the desk holds {} bars; it grows with uptime",
            state.bars.len()
        );
        assert_eq!(
            state.bars.len(),
            SERIES_HISTORY + extra,
            "the rebuild did not leave the desk holding the platform's history plus the bars since"
        );
        // Derived from the platform's record: the newest SERIES_HISTORY
        // closes on the desk are the platform's, in order.
        let desk_closes = state.bars.closes();
        let platform_closes: Vec<f64> = history.iter().map(|bar| bar.close.to_f64()).collect();
        assert_eq!(
            desk_closes[desk_closes.len() - SERIES_HISTORY..],
            platform_closes[..],
            "the desk's newest bars are not the platform's bars"
        );
        // The last bar absorbed is the last bar the desk shows, and the
        // snapshot's clock followed it.
        let newest = 100.0 + (DESK_SERIES_REBUILD_AT + extra - 1) as f64;
        assert_eq!(
            state.bars.last().map(|bar| bar.close.to_f64()),
            Some(newest)
        );
        assert_eq!(
            market.snapshot.as_of,
            history[history.len() - 1].close_time()
        );
        drop(market);

        // The world model the agents read through the other gate saw the
        // same absorption: its `close` feature for the instrument is readable
        // at the newest bar's close.
        let world = platform.world();
        let closes = world.features().history("close", "obj-AAA", start());
        assert!(
            !closes.is_empty(),
            "the world model behind the desk's gate holds no close series"
        );
        assert_eq!(closes.last().map(|value| value.value), Some(newest));
    }

    /// The other market records reach the desk too, so an agent reading the
    /// book or the last trade is reading what the platform absorbed.
    #[test]
    fn quotes_reach_the_desk_snapshot_as_they_are_absorbed() {
        let mut platform = platform();
        let object = ObjectId::from_string("obj-AAA");
        assert!(platform.market_view().snapshot.get(&object).is_none());

        platform.observe(counting_quotes(3));

        let market = platform.market_view();
        let state = market
            .snapshot
            .get(&object)
            .expect("the desk holds the quoted instrument");
        let quote = state.quote.as_ref().expect("the desk holds the quote");
        // The newest quote of the three: half-spread 0.03 either side of 100.
        assert_eq!(quote.ask - quote.bid, Decimal::from_f64(0.06).unwrap());
    }
}

#[cfg(test)]
mod user_ledger_tests {
    //! The §43.4 chain reaching a user: a fill the centre settles is booked
    //! to the desk's per-strategy books. A unit test because the seam is
    //! `Platform::journal_to_desk`, which is private on purpose — the only
    //! road into the user books is a report the centre accepted.

    use super::*;
    use qip_contracts::intent::Contributor;
    use qip_contracts::wire::{FillRecord, FillShare};
    use qip_core::dec;
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_mesh::delta::DeltaOrder;
    use qip_observability::Telemetry;
    use qip_risk::limits::{Limit, LimitKind, LimitSet};

    const CELL: &str = "cell-lon-1";
    const INSTRUMENT: &str = "obj-AAA";

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let mut universe = Universe::new();
        universe
            .insert(
                FinancialObject::builder(
                    ObjectId::from_string(INSTRUMENT),
                    "AAA",
                    InstrumentType::CommonStock,
                )
                .venue("XNYS")
                .sector(Sector::InformationTechnology)
                .price(dec!("100"))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
        let limits = LimitSet::new("kernel-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        );
        let config = PlatformConfig::default();
        let (context, _clock) = qip_core::Context::deterministic(start(), config.seed);
        Platform::new(config, context, Telemetry::silent(), universe, limits)
            .expect("the platform assembles")
    }

    /// One order sent and filled whole for a single strategy, as the cell
    /// reports it: the order registered as sent and the venue's fill beside
    /// it, attributed entirely to `strategy`.
    fn report(order_id: &str, side: BookSide, quantity: Decimal, price: Decimal) -> CellReport {
        let strategy = StrategyId::new("alpha");
        let order = DeltaOrder {
            order_id: order_id.to_string(),
            strategy: strategy.clone(),
            object_id: ObjectId::from_string(INSTRUMENT),
            venue: VenueId::new("XNYS"),
            side,
            quantity,
            price,
            simulated: true,
            contributors: vec![Contributor {
                strategy: strategy.clone(),
                signed_size: quantity,
                inputs: vec![("alpha-feature".to_string(), 1)],
            }],
        };
        let fill = FillRecord {
            order_id: order_id.to_string(),
            object_id: ObjectId::from_string(INSTRUMENT),
            venue: VenueId::new("XNYS"),
            side,
            quantity,
            price,
            simulated: true,
            at: start(),
            shares: vec![FillShare { strategy, quantity }],
        };
        CellReport::new(CELL, start())
            .with_orders(vec![order])
            .with_fills(vec![fill])
    }

    #[test]
    fn a_fill_the_centre_settles_is_booked_to_the_desk_users_per_strategy_balance() {
        // The failure this closes: the platform's books were per strategy
        // and not per user, so the attribution chain stopped at the lot and
        // nothing could say whose capital `alpha` was trading. Premise
        // first: the desk holds a mandate and no book, and the round trip
        // below genuinely realises something — a buy at 50 and a sell at 60
        // is a thousand, which a ledger that booked nothing would not show.
        let mut platform = platform();
        let desk = UserId::new(DESK_USER).expect("the desk user id is valid");
        let alpha = StrategyId::new("alpha");
        assert!(
            platform.user_ledger().mandate(&desk).is_some(),
            "the premise is a desk with a mandate"
        );
        assert!(platform.user_ledger().book(&desk, &alpha).is_none());
        assert_eq!(platform.user_ledger().fills_journalled(), 0);

        let bought = platform
            .ingest_cell_report(
                report("ord-1", BookSide::Ask, dec!("100"), dec!("50")),
                start(),
            )
            .expect("the buy is ingested");
        assert_eq!(
            bought.settlement.fills_settled, 1,
            "the premise is a settled fill"
        );
        assert!(
            bought.settlement.refused.is_empty(),
            "nothing was refused: {:?}",
            bought.settlement.refused
        );
        let opened = platform
            .user_ledger()
            .book(&desk, &alpha)
            .expect("the buy opened the desk's book at alpha");
        assert_eq!(opened.entries(), 1, "the opening buy is one entry");
        assert_eq!(
            opened
                .cash(Currency::USD)
                .map(qip_capital::ledger::CashBalance::settled),
            Some(Decimal::ZERO),
            "an opening buy realises nothing yet"
        );

        let sold = platform
            .ingest_cell_report(
                report("ord-2", BookSide::Bid, dec!("100"), dec!("60")),
                start(),
            )
            .expect("the sell is ingested");
        assert_eq!(sold.settlement.fills_settled, 1);
        let attributed = sold
            .settlement
            .by_strategy()
            .get("alpha")
            .copied()
            .expect("the sell is attributed to alpha");
        assert_eq!(
            attributed,
            dec!("1000"),
            "the premise is a realised thousand"
        );

        let closed = platform
            .user_ledger()
            .book(&desk, &alpha)
            .expect("the desk's book at alpha persists");
        assert_eq!(closed.entries(), 2);
        assert_eq!(
            closed
                .cash(Currency::USD)
                .map(qip_capital::ledger::CashBalance::settled),
            Some(dec!("1000")),
            "the desk's per-strategy balance carries what alpha realised"
        );
        assert_eq!(platform.user_ledger().fills_journalled(), 2);
        assert!(
            platform.capture_problems.is_empty(),
            "nothing was left unjournalled: {:?}",
            platform.capture_problems
        );
    }
}

#[cfg(test)]
mod central_sizing_tests {
    //! §6.2 row 6 on the central path: the self-model's freshness narrows
    //! the budget every proposal is sized against. A unit test because the
    //! seam is `Platform::construct_from`, private on purpose, and the
    //! budget it hands the constructor is readable off the proposal.

    use super::*;
    use qip_core::dec;
    use qip_financial::universe::Universe;
    use qip_observability::Telemetry;
    use qip_portfolio_engine::construction::ApprovedThesis;
    use qip_risk::limits::LimitSet;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default().with_initial_equity(Decimal::from_int(200_000));
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

    fn thesis(object: &str, conviction: f64) -> ApprovedThesis {
        ApprovedThesis {
            hypothesis_id: format!("HYP-{object}"),
            object_id: qip_core::ObjectId::from_string(object),
            conviction,
            expected_return: 0.04 * conviction.signum(),
            price: Decimal::from_int(100),
        }
    }

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

    /// Size one proposal and return the budget `construct` was handed and
    /// the notional it traded.
    fn size(platform: &mut Platform, now: Timestamp) -> (Decimal, Decimal) {
        feed_history(platform, "AAPL", 30);
        feed_history(platform, "MSFT", 30);
        platform.pending_theses.push(thesis("AAPL", 0.6));
        platform.pending_theses.push(thesis("MSFT", -0.4));
        platform.stage_decide(now);
        let proposal = platform.proposals.last().expect("a proposal is recorded");
        (proposal.equity.amount, proposal.traded_notional())
    }

    /// Grade `count` vindicated theses on one detector, so the self-model
    /// carries a component at the minimum sample with a newest outcome at
    /// `resolves_at`.
    fn feed_self_model(platform: &mut Platform, count: usize, resolves_at: Timestamp) {
        for n in 1..=count {
            let id = format!("hyp-{n}");
            let claim = ThesisClaim {
                hypothesis_id: id.clone(),
                class: "price_dislocation".to_string(),
                subject: "obj-AAA".to_string(),
                formed_at: start(),
                resolves_at,
                direction: 1.0,
                expected_move_bps: 200.0,
                falsifiers: vec!["it reverts".to_string()],
                confidence: 0.7,
                contributors: Vec::new(),
            };
            let outcome = ThesisOutcome {
                hypothesis_id: id,
                observed_at: resolves_at,
                realised_move_bps: 180.0,
                realised_pnl: 0.0,
                falsifiers_triggered: Vec::new(),
                mechanism_confirmed: None,
            };
            platform
                .learn_from(&[claim], &[outcome], resolves_at)
                .expect("a resolvable claim grades");
        }
    }

    #[test]
    fn a_never_absorbed_self_model_sizes_at_the_unavailable_multiplier_and_a_fed_one_sizes_wider() {
        // The failure this guards: the centre sizing every proposal at full
        // budget while its self-model — the record of which of its own
        // components can be trusted — has never absorbed an outcome, so a
        // platform with no evidence about itself sized exactly as one with
        // a week of graded theses. Row 6 of §6.2 exists to narrow on that
        // absence, and until this seam it narrowed nothing at the centre.
        let now = Timestamp::from_secs(1_760_000_100);

        // Premise: the fresh platform's self-model is empty and the table
        // reads it unavailable — the reading the sizing below must follow.
        // The other two measured rows are unavailable on a fresh platform
        // too: the causal graph has absorbed no claim (this test seeds no
        // world) and no belief has been formed before the first `reason()`
        // in a process, so the shared multiplier is 0.75 × 0.5 = 0.375 and
        // the self-model's unavailable halving takes it to 0.1875. Until
        // rows 2 and 4 were measured this read 0.5, which was the table
        // reading two absent objects as fresh.
        let mut unfed = platform();
        assert!(
            unfed.self_model().is_empty(),
            "the premise is an empty self-model"
        );
        let unfed_multiplier = unfed
            .central_degradation(now)
            .expect("the table reads")
            .central_sizing_multiplier();
        assert_eq!(
            unfed_multiplier,
            dec!("0.1875"),
            "unavailable self-model halves the 0.375 that an unavailable causal graph and \
             belief state already narrowed to"
        );
        // With no hold active the free balance `stage_decide` anchors is
        // the whole equity.
        let free_before = unfed.capital.equity();
        let (unfed_budget, unfed_notional) = size(&mut unfed, now);
        assert!(
            unfed_notional.is_positive(),
            "the premise failed: nothing was sized on the unfed platform"
        );
        assert_eq!(
            unfed_budget,
            free_before * unfed_multiplier,
            "the unfed budget is the free balance at the unavailable multiplier"
        );

        // A platform whose self-model holds a component at the minimum
        // sample, graded within the horizon of the sizing instant.
        let mut fed = platform();
        let resolves_at = start().saturating_add(Duration::from_days(5));
        feed_self_model(
            &mut fed,
            qip_learning_engine::self_model::MINIMUM_SAMPLE,
            resolves_at,
        );
        assert!(
            !fed.self_model().is_empty(),
            "the premise is a fed self-model"
        );
        let sized_at = resolves_at.saturating_add(Duration::from_secs(100));
        let fed_multiplier = fed
            .central_degradation(sized_at)
            .expect("the table reads")
            .central_sizing_multiplier();
        // A fresh self-model narrows nothing of its own: the 0.375 is the
        // two rows this test leaves unavailable — no causal claim absorbed,
        // no belief formed — and is the same shared factor the unfed
        // platform carried, so the difference between the two multipliers
        // is the self-model row alone.
        assert_eq!(
            fed_multiplier,
            dec!("0.375"),
            "a fresh self-model narrows nothing beyond the unavailable causal graph and \
             belief state"
        );
        assert_eq!(
            fed_multiplier,
            unfed_multiplier * dec!("2"),
            "the self-model row is exactly the halving between the two platforms"
        );
        let fed_free = fed.capital.equity();
        let (fed_budget, fed_notional) = size(&mut fed, sized_at);
        assert_eq!(
            fed_budget,
            fed_free * fed_multiplier,
            "the fed budget is the free balance at the fed multiplier"
        );
        assert!(
            fed_notional > unfed_notional,
            "a fed self-model sized {fed_notional}, not wider than the unfed {unfed_notional}"
        );
    }

    #[test]
    fn a_backdated_causal_graph_narrows_the_central_multiplier_to_the_stale_value_and_a_fresh_one_does_not()
     {
        // The failure this closes: the centre's table started row 2 fresh
        // on the argument that the live graph has no shipped age, so a
        // graph whose every claim was a year old — which is what the demo
        // seed builds — sized at full budget. Premise: the fresh platform's
        // graph has absorbed nothing and reads unavailable, and the two
        // platforms below differ only in when their one claim was recorded.
        use qip_contracts::degradation::{Capability, Freshness};
        use qip_world_model::causal::{CausalEdge, Mechanism};

        let now = Timestamp::from_secs(1_760_000_100);
        let claim = |recorded_at: Timestamp| {
            CausalEdge::new(
                "ent-cause",
                "ent-effect",
                Mechanism::SupplyChain,
                0.5,
                Duration::from_days(1),
                recorded_at,
            )
        };
        let unseeded = platform();
        assert!(
            unseeded.world().causal().last_updated().is_none(),
            "the premise is a graph that has absorbed nothing"
        );
        let unseeded_state = unseeded.central_degradation(now).expect("the table reads");
        assert_eq!(
            unseeded_state.freshness(Capability::CausalGraph),
            Freshness::Unavailable
        );

        let fresh = platform();
        fresh
            .world
            .update(|world| world.claim_causal(claim(now.saturating_sub(Duration::from_days(1)))));
        let fresh_state = fresh.central_degradation(now).expect("the table reads");
        assert_eq!(
            fresh_state.freshness(Capability::CausalGraph),
            Freshness::Fresh,
            "a claim absorbed yesterday is inside the quarter horizon"
        );

        let stale = platform();
        stale.world.update(|world| {
            world.claim_causal(claim(now.saturating_sub(Duration::from_days(365))));
        });
        let stale_state = stale.central_degradation(now).expect("the table reads");
        assert_eq!(
            stale_state.freshness(Capability::CausalGraph),
            Freshness::Stale,
            "a claim absorbed a year ago is past the quarter horizon"
        );

        // The multipliers: the stale graph narrows by exactly the row 2
        // factor against the fresh one, and reads the same as no graph at
        // all — the table does not distinguish stale from unavailable on
        // this row, which is stated here so nobody reads it as a bug.
        let fresh_multiplier = fresh_state.central_sizing_multiplier();
        let stale_multiplier = stale_state.central_sizing_multiplier();
        assert_eq!(
            fresh_multiplier,
            dec!("0.25"),
            "belief unavailable (0.5) and self-model unavailable (0.5); the graph narrows nothing"
        );
        assert_eq!(
            stale_multiplier,
            fresh_multiplier * dec!("0.75"),
            "the stale graph narrows by the row 2 factor and nothing else"
        );
        assert_eq!(stale_multiplier, unseeded_state.central_sizing_multiplier());

        // And the demo seed itself, as a deployed process would carry it:
        // every claim backdated a year from the context's clock, so the
        // graph the demo reasons over reads stale and the centre narrows.
        let demo = platform();
        demo.world.update(|world| {
            qip_world_model::world::seed_demo_world(world, &demo.context).expect("the demo seeds")
        });
        let seeded_at = demo
            .world()
            .causal()
            .last_updated()
            .expect("the demo seed absorbed a claim");
        assert!(
            now.since(seeded_at) > qip_contracts::degradation::CAUSAL_GRAPH_HORIZON,
            "the premise: the demo's newest claim is older than the horizon"
        );
        assert_eq!(
            demo.central_degradation(now)
                .expect("the table reads")
                .freshness(Capability::CausalGraph),
            Freshness::Stale,
            "the demo seed's year-old claims read stale at the centre"
        );
    }
}

#[cfg(test)]
mod self_model_tests {
    //! The LEARN stage charging a graded thesis to the components that
    //! produced it. A unit test because the attribution seam is
    //! `Platform::components_of`, private on purpose: the only road into the
    //! self-model is an evaluation the learning engine graded.

    use super::*;
    use qip_financial::universe::Universe;
    use qip_learning_engine::self_model::ComponentKind;
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

    #[test]
    fn a_graded_thesis_is_charged_to_its_detector_and_to_every_roster_analyst_that_ran_on_it() {
        // The failure this guards: a thesis resolved, the calibration moved,
        // and no component was charged — so the self-model stayed empty and
        // the REASON stage kept weighting a detector that had been wrong
        // sixty times at full weight. The run id of an analyst no longer on
        // the roster, and a contributor that is not a run id at all, must
        // charge nobody rather than whichever roster prefix happened to fit.
        let mut platform = platform();
        let roster: Vec<String> = platform
            .organisation
            .roster()
            .iter()
            .map(|manifest| manifest.id.clone())
            .collect();
        // Premise: the roster carries the analyst the claim will name, and
        // its id contains a hyphen — the case a naive split gets wrong.
        let analyst = "macro-analyst";
        assert!(
            roster.iter().any(|id| id == analyst),
            "the fixture analyst is not on the roster: {roster:?}"
        );
        assert!(
            platform.self_model().is_empty(),
            "the premise is an empty self-model"
        );

        let resolves_at = start().saturating_add(Duration::from_days(5));
        let claim = ThesisClaim {
            hypothesis_id: "hyp-1".to_string(),
            class: "price_dislocation".to_string(),
            subject: "obj-AAA".to_string(),
            formed_at: start(),
            resolves_at,
            direction: 1.0,
            expected_move_bps: 200.0,
            confidence: 0.7,
            falsifiers: vec!["it reverts".to_string()],
            contributors: vec![
                format!("run-{analyst}-7"),
                "run-retired-analyst-3".to_string(),
                "not-a-run-id".to_string(),
            ],
        };
        let outcome = ThesisOutcome {
            hypothesis_id: "hyp-1".to_string(),
            observed_at: resolves_at,
            realised_move_bps: 180.0,
            realised_pnl: 0.0,
            falsifiers_triggered: Vec::new(),
            mechanism_confirmed: None,
        };
        let learned = platform
            .learn_from(&[claim], &[outcome], resolves_at)
            .expect("a resolvable claim grades");
        assert_eq!(
            learned.evaluations.len(),
            1,
            "the premise is one graded thesis"
        );
        assert!(
            learned.evaluations[0].verdict.is_informative(),
            "an informative verdict is the premise; got {:?}",
            learned.evaluations[0].verdict
        );

        let charged: Vec<String> = platform
            .self_model()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect();
        assert_eq!(
            charged,
            vec![
                "detector:price_dislocation".to_string(),
                format!("analyst:{analyst}"),
            ],
            "the wrong components were charged"
        );
        let detector = ComponentKey::new(ComponentKind::Detector, "price_dislocation")
            .expect("a named component");
        let record = platform
            .self_model()
            .get(&detector)
            .expect("the detector was charged");
        assert_eq!(record.sample_count(), 1);
        assert_eq!(record.hits(), 1, "a vindicated thesis is a hit");
        assert_eq!(record.last_updated(), Some(resolves_at));
        // One outcome is below the minimum sample, so the REASON stage was
        // handed no factor: an unmeasured component stays at full weight.
        assert!(
            platform.reasoning.origin_factors().is_empty(),
            "a factor was handed over on one outcome: {:?}",
            platform.reasoning.origin_factors()
        );

        // Nine more misses reach the minimum sample, and the factors the
        // record now supports reach the reasoning engine — the handover
        // that makes the self-model something REASON uses rather than
        // something LEARN reports. One hit in ten: (1 + 2) / (10 + 4).
        for n in 2..=qip_learning_engine::self_model::MINIMUM_SAMPLE {
            let id = format!("hyp-{n}");
            let claim = ThesisClaim {
                hypothesis_id: id.clone(),
                class: "price_dislocation".to_string(),
                subject: "obj-AAA".to_string(),
                formed_at: start(),
                resolves_at,
                direction: 1.0,
                expected_move_bps: 200.0,
                confidence: 0.7,
                falsifiers: vec!["it reverts".to_string()],
                contributors: vec![format!("run-{analyst}-{n}")],
            };
            let outcome = ThesisOutcome {
                hypothesis_id: id,
                observed_at: resolves_at,
                realised_move_bps: -180.0,
                realised_pnl: 0.0,
                falsifiers_triggered: Vec::new(),
                mechanism_confirmed: None,
            };
            platform
                .learn_from(&[claim], &[outcome], resolves_at)
                .expect("a resolvable claim grades");
        }
        let handed = platform.reasoning.origin_factors();
        let expected = 3.0 / 14.0;
        for origin in ["price_dislocation", analyst] {
            let factor = handed.get(origin).unwrap_or_else(|| {
                panic!("{origin} reached the minimum and REASON holds no factor for it: {handed:?}")
            });
            assert!(
                (factor - expected).abs() < 1e-12,
                "{origin} was handed {factor}, not the {expected} its record supports"
            );
        }
        assert_eq!(
            handed.len(),
            2,
            "an origin nobody measured was handed a factor: {handed:?}"
        );
    }

    #[test]
    fn a_thesis_the_self_model_cannot_key_is_skipped_with_a_problem_and_the_rest_are_charged() {
        // The failure this guards, found in review: `learn_from` charged
        // each graded thesis with `?`, so one whose class the self-model
        // could not key aborted the whole calibration pass — every other
        // thesis resolved that cycle went uncharged, the window and the
        // self-model disagreed about what had been graded, and the stage
        // reported a failure rather than which thesis was the problem.
        let mut platform = platform();
        assert!(
            platform.self_model().is_empty(),
            "the premise is an empty self-model"
        );
        let resolves_at = start().saturating_add(Duration::from_days(5));
        let claim = |id: &str, class: &str| ThesisClaim {
            hypothesis_id: id.to_string(),
            class: class.to_string(),
            subject: "obj-AAA".to_string(),
            formed_at: start(),
            resolves_at,
            direction: 1.0,
            expected_move_bps: 200.0,
            confidence: 0.7,
            falsifiers: vec!["it reverts".to_string()],
            contributors: Vec::new(),
        };
        let outcome = |id: &str| ThesisOutcome {
            hypothesis_id: id.to_string(),
            observed_at: resolves_at,
            realised_move_bps: 180.0,
            realised_pnl: 0.0,
            falsifiers_triggered: Vec::new(),
            mechanism_confirmed: None,
        };
        // Premise: the bad class is one `ComponentKey` refuses — it carries
        // the serialised key's separator — and the good one is accepted.
        let unkeyable = "price:dislocation";
        assert!(ComponentKey::detector(unkeyable).is_err());
        assert!(ComponentKey::detector("price_dislocation").is_ok());

        // The unkeyable thesis first, so a pass that aborted on it would
        // leave the good one uncharged.
        let learned = platform
            .learn_from(
                &[
                    claim("hyp-bad", unkeyable),
                    claim("hyp-good", "price_dislocation"),
                ],
                &[outcome("hyp-bad"), outcome("hyp-good")],
                resolves_at,
            )
            .expect("one unkeyable thesis must not abort the pass");
        assert_eq!(
            learned.evaluations.len(),
            2,
            "the premise is two graded theses: {:?}",
            learned.skipped
        );
        assert_eq!(
            learned.problems.len(),
            1,
            "one problem for the one unkeyable thesis: {:?}",
            learned.problems
        );
        assert!(
            learned.problems[0].contains("hyp-bad")
                && learned.problems[0].contains("charged to no component"),
            "the problem does not name the thesis it skipped: {}",
            learned.problems[0]
        );
        assert!(
            !learned.problems[0].contains("hyp-good"),
            "the problem names a thesis that was charged: {}",
            learned.problems[0]
        );
        // The good thesis was charged and the bad one was not.
        let charged: Vec<String> = platform
            .self_model()
            .iter()
            .map(|(key, _)| key.to_string())
            .collect();
        assert_eq!(charged, vec!["detector:price_dislocation".to_string()]);
        // And the window still holds both, as it did before the fix — the
        // self-model's silence about one is stated, not hidden.
        assert_eq!(platform.evaluations.len(), 2);
    }
}
