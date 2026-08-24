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
use qip_cost_router::{ComputeLedger, CostEngine, DataCostModel, DataReads, IntelligenceTier};
use qip_data_finder::finder::{DataFinder, FinderConfig};
use qip_data_finder::probe::SourceProbe;
use qip_data_finder::source::SourceCandidate;
use qip_data_finder::{RegisteredSource, RegistrationDecision};
use qip_events::log::EventLog;
use qip_events::{EventBody, EventFilter, Topic};
use qip_execution_engine::broker::{Broker, SimulatedBroker, SimulationSettings};
use qip_execution_engine::oms::{OrderManager, RefusalReason, SubmissionResult};
use qip_execution_engine::order::{Order, OrderType, Side};
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
use qip_opportunity_engine::catalyst::MarketEvent;
use qip_opportunity_engine::detector::{DetectionContext, DetectorRegistry};
use qip_opportunity_engine::engine::{EngineConfig, OpportunityEngine};
use qip_opportunity_engine::opportunity::Opportunity;
use qip_optimization_engine::router::ComputeRouter;
use qip_portfolio::portfolio::Portfolio;
use qip_portfolio_engine::construction::PortfolioConstructor;
use qip_portfolio_engine::proposal::Proposal;
use qip_prediction::resolution::{
    Comparison, Observations, Proposition, ResolutionCriteria, ResolutionSource, SettlementRule,
    SourceKind, UndeterminedRule, Verdict,
};
use qip_quantum::provider::SimulatedProvider;
use qip_reasoning_engine::engine::{ReasoningEngine, ReasoningOutcome};
use qip_reasoning_engine::hypothesis::Claim;
use qip_risk::limits::{LimitSet, RiskState};
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

        Ok(Self {
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
                    self.price_history
                        .entry(key.clone())
                        .or_default()
                        .push(bar.close.to_f64());
                    self.volume_history
                        .entry(key)
                        .or_default()
                        .push(bar.volume.to_f64());
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
                        self.spread_history
                            .entry(quote.object_id.as_str().to_string())
                            .or_default()
                            .push(bps);
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
                        self.spread_history
                            .entry(book.object_id.as_str().to_string())
                            .or_default()
                            .push(spread.to_f64() / mid.to_f64() * 10_000.0);
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
                        self.observation_history
                            .entry(format!("{}:{}", update.entity_id, update.metric))
                            .or_default()
                            .push(surprise);
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
        // The stages run in order and every one of them runs: a cycle that
        // fails at REASON still reaches LEARN, because LEARN is what would
        // eventually notice that REASON keeps failing.
        let mut stages = vec![
            self.stage_sense(now),
            self.stage_understand(now),
            self.stage_discover(now),
            self.stage_reason(now, &lineage),
            self.stage_simulate(now),
            self.stage_decide(now),
            self.stage_act(now, &correlation_id),
            self.stage_learn(now),
        ];

        // Charge what the cycle consumed. A ledger per cycle rather than one
        // per process: the agent budget inside it is what refuses the next
        // rung, and a ledger that never resets would refuse every rung after
        // the first few hours of uptime. The running total is kept separately
        // and is monotone.
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

        // The journal is written last, so it records the cycle that happened
        // rather than the one that was about to. A journal failure is a problem
        // on the cycle and not the end of it: a process that stopped trading
        // because it could not write its own diary would be a worse outcome
        // than one that traded and said it could not write it down.
        if let Err(error) = self.journal_cycle(&report, now) {
            if let Some(learn) = report.stages.last_mut() {
                learn.problems.push(format!(
                    "the cycle journal was not written: {}",
                    error.message()
                ));
            }
        }
        report.events_logged = self.event_log.len();
        report
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
                // The organisation is a panel of agents reasoning against each
                // other, and only when it produced something.
                Stage::Reason if outcome.produced > 0 => {
                    tiers.push(IntelligenceTier::MultiAgentReasoning);
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
        &self,
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

        self.constructor.construct(
            theses,
            &covariance,
            &current,
            Money::new(equity, Currency::USD),
            now,
            now,
            ProposalId::from_string(format!("prop-{}", self.cycle)),
        )
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
        self.predictions.push(RecordedPrediction {
            hypothesis: reasoned.hypothesis.hypothesis_id.as_str().to_string(),
            cycle: self.cycle,
            proposition,
            recorded_at: now,
            verdict: None,
            scored_at: None,
        });
        Ok(true)
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
            return outcome;
        }

        StageOutcome::ran(
            Stage::Act,
            releasable,
            format!("{releasable} proposal(s) ready to release"),
        )
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
            .attribute(&periods, total, self.capital.equity(), now, now)
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
        RiskState {
            equity: self.capital.equity(),
            cash: self.capital.cash,
            gross_exposure: gross,
            net_exposure: net,
            position_notionals,
            drawdown: self.capital.drawdown(),
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
}
