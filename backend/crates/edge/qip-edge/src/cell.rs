//! The cell: one region's hot execution path, assembled.
//!
//! Bytes arrive on a feed and leave as orders, without a network hop to the
//! central plane anywhere in between. That is the whole point of the cell and
//! the reason every safety property here has to be local: there is nobody to
//! ask.
//!
//! What makes it safe is that the cell never decides *how much* it may risk.
//! It receives a [`crate::VerifiedEnvelope`] — signed, bounded, venue-scoped,
//! expiring — and the worst it can do while cut off is spend an amount
//! somebody already approved, for as long as the envelope has left to run.

use crate::arbitrage::ArbitrageDesk;
use crate::dropcopy::{CellFill, Discrepancy, DropCopyFill, DropCopyReconciler};
use crate::envelope::VerifiedEnvelope;
use crate::feasibility::{self, VenueModel};
use crate::journal::{Decision, Journal, Mirror};
use crate::mesh::{CellStateDelta, DeltaOrder, DeltaRefusal, StrategyUtilisation};
use crate::policy::{VerifiedHalt, VerifiedPolicy};
use crate::seam::CellLiquidity;
use crate::telemetry::CellMetrics;
use qip_arbitrage::scan::{Opportunity, RejectionStage};
use qip_contracts::capital::{CapitalGrant, Utilisation};
use qip_contracts::degradation::{DegradationState, StrategyClass};
use qip_contracts::intent::{Contributor, CycleLeg, Intent, NetIntent, net, netting_ratio};
use qip_contracts::message::{BookSide, MarketMessage};
use qip_contracts::signal::{Signal, SignalKind, StrategyId};
use qip_contracts::venue::{VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_feature_dag::engine::FeatureEngine;
use qip_orderbook::venue::VenueState;
use qip_protocols::registry::{FeedKey, ProtocolRegistry};
use qip_risk_engine::autonomy::{AutonomyController, AutonomyLevel};
use qip_sequencing::tracker::{ReorderPolicy, Sequencer};
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::program::Program;
use qip_strategy::runtime::StrategyRuntime;
use std::collections::{BTreeMap, VecDeque};

/// How a cell is identified and what it is allowed to reach.
#[derive(Clone, Debug)]
pub struct CellConfig {
    pub cell_id: String,
    pub region: String,
    /// Venues this cell may trade. A venue absent here is unreachable to it
    /// whatever an envelope says — the two are independent bounds, and an
    /// order must clear both.
    pub venues: Vec<VenueId>,
    /// How long a book may go unrefreshed before its prices stop counting.
    pub max_staleness: Duration,
    /// The runtime node budget a strategy may not exceed.
    pub strategy_budget: usize,
    /// What the cell knows about executing at each venue, keyed by venue id
    /// (blueprint §18.1). A venue absent here is judged for depth alone —
    /// see [`crate::feasibility`] for why that is stated rather than
    /// defaulted.
    pub feasibility: BTreeMap<String, VenueModel>,
    /// The interval §27.1's forty percent crossing cap is measured over, if
    /// the owner of the cap has chosen one.
    ///
    /// `None` — the default — measures the cap against each net on its own,
    /// which is what this cell has always done and is the safe reading:
    /// under it a net that cancels completely is always over the cap and is
    /// never crossed (see [`Cell::cross_internally`] for the arithmetic).
    /// The blueprint writes the cap "per instrument per interval" and never
    /// says how long the interval is; the length decides when a safety
    /// control fires, so the default does not guess one, and setting this is
    /// the owner's decision (completion plan D3), not this crate's.
    pub crossing_interval: Option<CrossingInterval>,
}

/// The rolling window §27.1's crossing cap is evaluated against.
///
/// Both forms are "trailing, this pass included": the cap compares the
/// crossed size the window has admitted plus the one proposed against the
/// gross intent the window has seen plus this net's. Neither form lets a
/// cross be trimmed to fit — the cap still refuses whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingInterval {
    /// The last `n` passes of [`Cell::work`], counting the current one.
    /// `Passes(1)` is the per-net reading with the accounting switched on.
    Passes(u32),
    /// Every net evaluated within the trailing span of wall time.
    Span(Duration),
}

/// How many nets one instrument's crossing window may hold.
///
/// One sample per net per pass, so `Passes(n)` holds at most `n` and is
/// refused above this at configuration. A `Span` window holds as many as
/// arrive; at this bound the history is truncated *and the cap refuses every
/// cross* until it drains, because a window whose oldest gross has been
/// dropped cannot be measured, and a cap measured against part of its window
/// is a cap that fires late.
pub const MAX_CROSSING_WINDOW_SAMPLES: usize = 1_024;

impl CellConfig {
    pub fn new(cell_id: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            cell_id: cell_id.into(),
            region: region.into(),
            venues: Vec::new(),
            max_staleness: Duration::from_secs(5),
            strategy_budget: 4_096,
            feasibility: BTreeMap::new(),
            crossing_interval: None,
        }
    }

    /// Measure the crossing cap over `interval` rather than per net.
    ///
    /// Refused rather than clamped when the interval is empty or longer
    /// than the history can hold: a zero-pass window would make the cap
    /// compare a cross against nothing and admit everything, and a window
    /// longer than the bound would be silently shortened to it — the safety
    /// parameter the operator wrote replaced by one they did not.
    pub fn with_crossing_interval(mut self, interval: CrossingInterval) -> Result<Self> {
        match interval {
            CrossingInterval::Passes(0) => {
                return Err(Error::invalid(
                    "a crossing interval of zero passes measures the cap against nothing; \
                     leave it unset to measure per net, or name at least one pass",
                ));
            }
            CrossingInterval::Passes(passes)
                if usize::try_from(passes).is_ok_and(|n| n > MAX_CROSSING_WINDOW_SAMPLES) =>
            {
                return Err(Error::invalid(format!(
                    "a crossing interval of {passes} passes exceeds the {MAX_CROSSING_WINDOW_SAMPLES} \
                     the history holds per instrument, and would be measured over fewer than \
                     configured"
                )));
            }
            CrossingInterval::Span(span) if span.as_nanos() <= 0 => {
                return Err(Error::invalid(format!(
                    "a crossing interval of {} nanoseconds measures the cap against nothing; \
                     leave it unset to measure per net, or name a positive span",
                    span.as_nanos()
                )));
            }
            CrossingInterval::Passes(_) | CrossingInterval::Span(_) => {}
        }
        self.crossing_interval = Some(interval);
        Ok(self)
    }

    pub fn with_venue(mut self, venue: VenueId) -> Self {
        self.venues.push(venue);
        self
    }

    /// Install the feasibility model for a venue.
    ///
    /// The model is keyed by the venue's id and read on every intent for that
    /// venue; installing one for a venue the cell cannot reach is harmless
    /// and installing none for a venue it can is the depth-only case.
    #[must_use]
    pub fn with_feasibility(mut self, venue: &VenueId, model: VenueModel) -> Self {
        self.feasibility.insert(venue.as_str().to_string(), model);
        self
    }
}

/// What one pass of the cell's work produced.
#[derive(Clone, Debug, Default)]
pub struct WorkReport {
    pub signals: Vec<Signal>,
    /// Orders the venue accepted this pass. Accepted, not filled: an entry
    /// here is a resting or working order until a [`Self::fills`] entry names
    /// it, and nothing downstream may read it as a position.
    pub orders: Vec<PlacedOrder>,
    /// Fills the venue reported this pass, on orders from this pass or an
    /// earlier one, each attributed to its contributors. These — and only
    /// these — are what the cell has traded.
    pub fills: Vec<ConfirmedFill>,
    /// Nets that cancelled to zero: strategies that wanted opposite things,
    /// whose disagreement never reached a venue. Recorded because a
    /// cancellation is an outcome the platform should be able to explain, not
    /// an absence.
    pub cancelled: Vec<NetIntent>,
    /// Gross intent over net order volume, per blueprint §27 — the single
    /// best summary of whether the strategy set has genuine diversity. `None`
    /// when everything cancelled, because the ratio is unbounded there and a
    /// sentinel would be a number nobody computed.
    pub netting_ratio: Option<f64>,
    /// Every gate that said no, and why. A cell must answer "why did nothing
    /// trade" as precisely as "why did this trade".
    pub refusals: Vec<(String, String)>,
    /// Every internal cross booked this pass (§27.1). A cross is a trade
    /// between two of the platform's own strategies; it is reported rather
    /// than merely journaled so a caller can see it without replaying the
    /// chain.
    pub crosses: Vec<InternalCross>,
    pub halted: bool,
}

/// One offsetting portion crossed inside the cell rather than at a venue.
///
/// §27.1: the price is the prevailing mid at the netting instant, "never a
/// price either side chose", and both sides are named because the blueprint
/// treats a cross as a ledger entry and a regulatory expectation rather than
/// an optimisation detail.
#[derive(Clone, Debug, PartialEq)]
pub struct InternalCross {
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The matched size — the smaller of the buying and selling sides, which
    /// is exactly how much never needed a venue.
    pub quantity: Decimal,
    /// The prevailing mid at the netting instant, read from the book rather
    /// than taken from any intent's own reference price.
    pub price: Decimal,
    pub bought: Vec<StrategyId>,
    pub sold: Vec<StrategyId>,
}

/// An order the cell actually sent.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedOrder {
    pub order_id: String,
    /// The largest contributor by absolute intended size, kept so every
    /// existing reader of this field still sees a strategy. It is no longer
    /// the whole truth once an order carries more than one — `contributors`
    /// is — and it is retained rather than removed so the change is additive
    /// at every seam that already reads it.
    pub strategy: StrategyId,
    /// Every strategy whose intent this order carries, and how much each
    /// wanted. This is the mechanism by which a fill remains traceable to the
    /// strategies that caused it after netting has collapsed them into one
    /// order.
    pub contributors: Vec<Contributor>,
    pub object_id: ObjectId,
    pub venue: VenueId,
    /// The side of the book the order takes: `Ask` is a buy, `Bid` is a sell.
    /// Every gateway reads it this way, and so does the sign on each
    /// contributor below — a positive share bought. Stated here because the
    /// enum's own names do not say which reading an *order* carries.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Set by the cell from the gateway's own answer, never taken from the
    /// order. A paper fill counted as real is the single most consequential
    /// bit in the execution path.
    pub simulated: bool,
}

/// The order-entry session's report that part of an order traded.
///
/// This is the channel the order went out on answering — the venue's
/// acknowledgement, or a later execution report on an order that rested. It
/// is the only thing that turns a sent order into a fill inside the cell.
/// The drop copy is the *other* channel and is never read for this; it is
/// what the fills confirmed here are checked against.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionReport {
    pub order_id: String,
    pub venue: VenueId,
    pub quantity: Decimal,
    pub price: Decimal,
    pub at: Timestamp,
}

/// A fill the venue reported, attributed to the strategies whose intent the
/// order carried (§43.4: the chain starts at the fill).
///
/// `shares` is the pro-rata split of `quantity` across the order's
/// contributors by [`NetIntent::split_fill`], so the shares sum to the fill
/// exactly, per fill. It is computed from what the venue reported traded and
/// never from what the cell sent: an order that filled in three parts is
/// attributed three times, each summing to its own part.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedFill {
    pub order_id: String,
    pub venue: VenueId,
    pub object_id: ObjectId,
    /// The side of the book the order took: `Ask` bought, `Bid` sold.
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    /// From the gateway's answer at the time the order was sent.
    pub simulated: bool,
    pub at: Timestamp,
    pub shares: Vec<(StrategyId, Decimal)>,
}

/// An order the venue accepted, as the cell holds it until it is settled.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenOrder {
    pub order_id: String,
    pub venue: VenueId,
    pub object_id: ObjectId,
    pub side: BookSide,
    /// What was sent.
    pub quantity: Decimal,
    /// The limit it was sent with.
    pub price: Decimal,
    /// What the venue has reported traded, summed over every report.
    pub filled: Decimal,
    pub simulated: bool,
    pub sent_at: Timestamp,
    /// When the cell withdraws what has not filled, for an order sent under
    /// [`PricingPolicy::RestAtMid`]. `None` for a marketable order, which
    /// either filled on acceptance or was cancelled by the venue.
    pub expires_at: Option<Timestamp>,
    /// Why the cell has finished with it, once it has: `filled` when the
    /// reports sum to the quantity sent, `expired` when the cell withdrew
    /// the remainder. `None` is an order still working at the venue — which
    /// is not a position, and not a break.
    pub closed: Option<String>,
}

impl OpenOrder {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// An open order and the net it was made from, for attributing its fills.
#[derive(Clone, Debug)]
struct Working {
    order: OpenOrder,
    net: NetIntent,
}

/// How many orders the cell will hold open at once.
///
/// An order leaves the set when it is settled — closed and agreed with the
/// venue — so this bounds the working memory of the fill path by the number
/// of orders the venue has not finished with. At the bound the cell refuses
/// to send under the `open_orders` gate rather than sending an order it
/// could not attribute a fill on; the refusal is counted and journaled like
/// every other, so a cell that stopped for this reason says so.
pub const MAX_OPEN_ORDERS: usize = 256;

/// How a strategy's intents are priced when they reach a venue.
///
/// Stated at deployment, per strategy, and read when the net order is
/// placed — never defaulted. A strategy deployed with no policy has its
/// intents refused under the `pricing` gate, because the alternative is a
/// cell deciding on its own whether to cross a spread, and a cell that
/// crosses spreads nobody asked it to is paying for liquidity nobody
/// budgeted. Until this existed every order was a limit at the mid: an
/// order that, against a real two-sided book, rests — and, with nothing to
/// withdraw it, rests forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricingPolicy {
    /// Take the touch: a buy is sent at the best ask, a sell at the best bid,
    /// so it fills on acceptance against what rests there. The net's size
    /// is checked against the size at the touch when it is placed, and a net
    /// that would walk past the touch is refused rather than reduced — the
    /// feasibility gate's rule, applied once more at the size that actually
    /// goes out, because two feasible contributors can net to more than the
    /// touch holds.
    Marketable,
    /// Rest at the prevailing mid, and have the cell withdraw whatever has
    /// not filled once `time_to_live` has elapsed.
    ///
    /// The withdrawal is the cell's own, through [`Placer::cancel`], which is
    /// the venue's cancel path and nothing invented here; a gateway that
    /// cannot withdraw refuses to rest at all, because an order nothing can
    /// withdraw is a position the cell has promised to take at a price the
    /// market has since left. The simulated venue offers no venue-side
    /// expiry, so the cell's own clock is the only one there is.
    RestAtMid { time_to_live: Duration },
}

impl PricingPolicy {
    /// A resting policy, refusing a time to live that could not elapse.
    ///
    /// Zero would withdraw the order on the pass after it was sent, which
    /// is a marketable order that pays to rest for nothing; negative would
    /// never withdraw it. Neither is what anybody meant.
    pub fn rest_at_mid(time_to_live: Duration) -> Result<Self> {
        if time_to_live.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "a resting order needs a positive time to live and {} nanoseconds is not one; \
                 name how long the order may rest, or price it marketable",
                time_to_live.as_nanos()
            )));
        }
        Ok(Self::RestAtMid { time_to_live })
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Marketable => "marketable",
            Self::RestAtMid { .. } => "rest_at_mid",
        }
    }
}

/// A deployed strategy, the arena its plan indexes into, and the capital it
/// runs under.
///
/// The runtime is per-strategy rather than per-cell. One shared arena would
/// mean a plan compiled against one program being evaluated against another,
/// and the failure mode is not a crash: `NodeRef` is an index, so the run
/// would read whatever node happened to sit at that position and emit a signal
/// derived from a different strategy's arithmetic. Giving each deployment the
/// program it was compiled against costs a few kilobytes and removes the
/// aliasing entirely.
#[derive(Debug)]
struct Deployed {
    strategy: CompiledStrategy,
    runtime: StrategyRuntime,
    envelope: VerifiedEnvelope,
    utilisation: Utilisation,
    /// Which of the degradation table's pause rules apply to this strategy.
    /// Everything deployed through [`Cell::deploy`] is `PriceOnly`, which is
    /// true of every strategy this platform ships today: nothing at the edge
    /// consumes world events, so an ingestion or episodic loss must not pause
    /// it.
    class: StrategyClass,
    /// How this strategy's intents are priced at the venue, if the
    /// deployment said. `None` refuses every intent under `pricing`.
    pricing: Option<PricingPolicy>,
}

/// One edge cell.
#[derive(Debug)]
pub struct Cell {
    config: CellConfig,
    protocols: ProtocolRegistry,
    sequencer: Sequencer,
    liquidity: CellLiquidity,
    features: FeatureEngine,
    deployed: BTreeMap<String, Deployed>,
    autonomy: AutonomyController,
    /// The last verified policy payload applied, if any ever was.
    ///
    /// `None` is not a neutral state: with no payload every payload-fed
    /// capability reads as unavailable and the cell sizes at its conservative
    /// floor. A cell nobody ships policy to trades small, not blind.
    policy: Option<VerifiedPolicy>,
    /// Whether the centre has halted this cell through policy. Separate from
    /// the local kill switch on purpose: the switch clears only with an
    /// operator credential, while this clears only with a newer verified
    /// payload saying it is over. Two halts, two release disciplines, and
    /// neither can release the other.
    policy_halted: bool,
    /// The instant of the newest halt applied. A payload releases the policy
    /// halt only if it was issued *after* this, so a pre-halt payload still in
    /// flight cannot un-halt the cell it was racing.
    policy_halt_barrier: Option<Timestamp>,
    /// The second halt wire (§46.2): the reason the polled flag gave, while
    /// it is engaged. Independent of the two above in both directions — it
    /// is set only by [`Self::apply_polled_halt`], which reads a flag the
    /// node polls from a file and not a frame off the mesh, and it is
    /// released only by that flag reading absent or released; no policy
    /// payload, however new, and no operator credential on the kill switch
    /// touches it. Two wires that shared a release would share a failure.
    polled_halt: Option<String>,
    dropcopy: DropCopyReconciler,
    /// The arbitrage desk, if the composition root installed one. `None` is
    /// a cell that runs strategy programs and scans no graph, which is every
    /// cell before this field existed and every test that does not ask for
    /// one.
    desk: Option<ArbitrageDesk>,
    journal: Journal,
    /// Orders the venue accepted and the cell has not settled, by order id.
    /// Bounded by [`MAX_OPEN_ORDERS`]; see the constant for the refusal at
    /// the bound.
    working: BTreeMap<String, Working>,
    /// Every fill the order-entry channel has confirmed on an order still in
    /// `working`. This is the cell's side of reconciliation. Until this
    /// existed the cell wrote a fill here the moment the venue *accepted* an
    /// order, so an order that rested unfilled was a position the cell
    /// believed in and the venue did not — and the reconciler, doing its
    /// job, halted the cell on the first strategy that fired against a real
    /// two-sided book. Retired with its order on settlement; the journal
    /// keeps the record.
    confirmed: Vec<ConfirmedFill>,
    /// Signed quantity held per venue and instrument, from confirmed fills
    /// alone. Keyed like the crossing history — by what the cell holds books
    /// for — so it is bounded by the instrument set, and it survives
    /// settlement because a position does not stop existing when the order
    /// that built it is agreed.
    positions: BTreeMap<String, Decimal>,
    /// Every disagreement between this cell's fills and the venue's own
    /// account, kept so the centre hears about it in the state delta as well as
    /// in the journal.
    ///
    /// Bounded, like everything else that grows on a signal from outside. It
    /// can only grow while an operator keeps resuming a cell that a break has
    /// already halted, and past the bound the count is what travels: a
    /// truncation nobody can see would understate an incident.
    breaks: Vec<String>,
    breaks_omitted: u32,
    order_sequence: u64,
    /// Passes of [`Self::work`] so far, counting the halted ones. What a
    /// [`CrossingInterval::Passes`] window is measured in.
    pass: u64,
    /// What each instrument's crossing window has seen, oldest first, keyed
    /// by venue, instrument and representation — the same key `net` groups
    /// on. Empty forever when no interval is configured, so the per-net
    /// reading costs nothing.
    ///
    /// Bounded twice: the key set by the instruments the cell holds books
    /// for, since a net exists only for an instrument a strategy could price
    /// here; and each history by [`MAX_CROSSING_WINDOW_SAMPLES`], past which
    /// the oldest sample is dropped and the cap refuses until the window
    /// drains — see the constant for why refusing is the only honest answer.
    crossing_history: BTreeMap<String, VecDeque<CrossingSample>>,
    /// Where the cell's facts go.
    ///
    /// Given, never reached for: a cell assembled without one records into a
    /// registry nobody reads, which is what every test in the tree does. See
    /// [`crate::telemetry`] for why nothing here can block or fail the pass.
    metrics: CellMetrics,
}

/// How many reconciliation breaks a cell keeps for reporting.
const MAX_RETAINED_BREAKS: usize = 32;

impl Cell {
    /// Assemble a cell.
    ///
    /// The autonomy ceiling is paper trading and there is no constructor that
    /// takes another. A cell cannot raise its own ceiling; a live-capable cell
    /// is a differently-assembled deployment the central plane signs off, and
    /// the absence of that constructor here is what makes the claim true
    /// rather than merely intended.
    pub fn new(config: CellConfig, features: FeatureEngine) -> Result<Self> {
        Ok(Self {
            protocols: ProtocolRegistry::new(),
            sequencer: Sequencer::new(ReorderPolicy::default()),
            liquidity: CellLiquidity::new(),
            features,
            deployed: BTreeMap::new(),
            autonomy: AutonomyController::new(),
            policy: None,
            policy_halted: false,
            policy_halt_barrier: None,
            polled_halt: None,
            dropcopy: DropCopyReconciler::new(),
            desk: None,
            journal: Journal::new(),
            working: BTreeMap::new(),
            confirmed: Vec::new(),
            positions: BTreeMap::new(),
            breaks: Vec::new(),
            breaks_omitted: 0,
            order_sequence: 0,
            pass: 0,
            crossing_history: BTreeMap::new(),
            metrics: CellMetrics::silent(),
            config,
        })
    }

    pub fn config(&self) -> &CellConfig {
        &self.config
    }

    /// Record into the composition root's registry rather than the silent one
    /// this cell was built with.
    ///
    /// Called once, in `qip-edge-node`, with the handle taken from the
    /// telemetry before it is used anywhere else — exactly as `qip-fastbrain`
    /// and `qip-deepbrain` install theirs. Taking a second registry here would
    /// produce a scrape surface that answers empty forever while the cell
    /// records diligently into one nothing can reach, which is the defect this
    /// seam exists to close rebuilt one level up.
    ///
    /// The halt gauge is written immediately so a cell that starts halted, and
    /// is scraped before its first pass, does not read as running.
    #[must_use]
    pub fn with_metrics(mut self, metrics: std::sync::Arc<qip_observability::Metrics>) -> Self {
        self.metrics = CellMetrics::new(metrics, &self.config.cell_id, &self.config.region);
        self.record_halt();
        self
    }

    /// Install the arbitrage desk this cell scans with.
    ///
    /// A builder rather than a constructor argument, like [`Self::with_metrics`],
    /// so [`Self::new`] stays the one way to assemble a cell and stays
    /// paper-only. Refused when the desk's envelope names another cell, or
    /// when its graph reaches a venue this cell may not: a cycle is priced
    /// against the cell's own books, and a venue absent from the cell's list
    /// has no book here to price against and no gateway here to send to.
    pub fn with_arbitrage(mut self, desk: ArbitrageDesk) -> Result<Self> {
        self.install_arbitrage(desk)?;
        Ok(self)
    }

    /// Install the desk into a cell that is already running.
    ///
    /// What a composition root needs, because the desk's two inputs arrive
    /// after the cell is assembled: the whitelist rides a policy payload and
    /// the desk's capital rides a grant, and neither is known at start-up.
    /// The same refusals as [`Self::with_arbitrage`], plus one: a cell that
    /// already holds a desk refuses a second, because replacing one would
    /// discard the utilisation the first has spent and hand the strategy its
    /// gross limit again.
    pub fn install_arbitrage(&mut self, desk: ArbitrageDesk) -> Result<()> {
        if self.desk.is_some() {
            return Err(Error::denied(
                "this cell already holds an arbitrage desk; a second would reset the capital \
                 the first has committed",
            ));
        }
        if desk.envelope().cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot fund the arbitrage desk at {}",
                desk.envelope().cell(),
                self.config.cell_id
            )));
        }
        for edge in desk.graph().edges() {
            for venue in [&edge.from.venue, &edge.to.venue] {
                if !self.config.venues.contains(venue) {
                    return Err(Error::denied(format!(
                        "conversion {} reaches {}, which this cell may not trade; a cycle \
                         through a venue the cell holds no book for cannot be priced here",
                        edge.label(),
                        venue.as_str()
                    )));
                }
            }
        }
        self.desk = Some(desk);
        Ok(())
    }

    /// The cycle whitelist the applied policy carries, while it is fresh.
    ///
    /// Fresh only: the slot's own time-to-live is a minute, and a desk built
    /// from a whitelist the centre has stopped republishing would price a
    /// graph the centre may since have withdrawn. Stale reads as none.
    pub fn cycle_whitelist(
        &self,
        now: Timestamp,
    ) -> Option<&qip_contracts::policy::CycleWhitelist> {
        let policy = self.policy.as_ref()?;
        if policy
            .payload()
            .freshness(qip_contracts::policy::PolicyItem::CycleWhitelist, now)
            != qip_contracts::degradation::Freshness::Fresh
        {
            return None;
        }
        policy.payload().cycle_whitelist.value()
    }

    /// The compiled plan the last applied payload names (§41.5 item 2),
    /// while that slot is fresh.
    ///
    /// Fresh only, exactly as [`Self::cycle_whitelist`]: a stale plan is a
    /// plan the centre has stopped vouching for, and a node that deployed
    /// from it would be running strategies on the strength of a payload
    /// whose every other slot has already narrowed the cell. The slot names
    /// the plan by digest and count and carries no strategy itself; whoever
    /// holds the plan's bytes checks them against this before deploying.
    pub fn compiled_plan(&self, now: Timestamp) -> Option<&qip_contracts::policy::PlanDigest> {
        let policy = self.policy.as_ref()?;
        if policy
            .payload()
            .freshness(qip_contracts::policy::PolicyItem::CompiledPlan, now)
            != qip_contracts::degradation::Freshness::Fresh
        {
            return None;
        }
        policy.payload().compiled_plan.value()
    }

    /// The installed arbitrage desk, if any.
    pub fn arbitrage(&self) -> Option<&ArbitrageDesk> {
        self.desk.as_ref()
    }

    /// Publish the halt state as it now stands.
    ///
    /// Called wherever either halt can change, rather than once per pass: a
    /// cell halted by a reconciliation break stops running passes, so a gauge
    /// written only inside `work` would never report the halt that stopped it.
    fn record_halt(&self) {
        self.metrics.halt(
            self.autonomy.kill_switch().is_globally_tripped(),
            self.policy_halted,
            self.polled_halt.is_some(),
        );
    }

    pub fn protocols_mut(&mut self) -> &mut ProtocolRegistry {
        &mut self.protocols
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The registry every series this cell records lands in.
    ///
    /// Exposed so a composition root's test can prove, by pointer identity,
    /// that it is the same registry the scrape surface serves. A cell that
    /// records into one registry while the health thread serves another
    /// answers every scrape empty forever, and nothing at runtime reports it.
    pub fn metrics_registry(&self) -> &std::sync::Arc<qip_observability::Metrics> {
        self.metrics.registry()
    }

    pub fn autonomy(&self) -> &AutonomyController {
        &self.autonomy
    }

    pub fn autonomy_mut(&mut self) -> &mut AutonomyController {
        &mut self.autonomy
    }

    pub fn liquidity(&self) -> &CellLiquidity {
        &self.liquidity
    }

    pub fn dropcopy_mut(&mut self) -> &mut DropCopyReconciler {
        &mut self.dropcopy
    }

    /// Fills the venue has confirmed and the cell has not yet settled.
    ///
    /// Confirmed means reported by the order-entry channel; an order the
    /// venue accepted and has not filled is in [`Self::open_orders`] and not
    /// here. Settled fills leave this list and stay in the journal.
    pub fn fills(&self) -> &[ConfirmedFill] {
        &self.confirmed
    }

    /// Orders the venue accepted and the cell has not settled, in order-id
    /// order — including closed ones awaiting a clean reconciliation.
    pub fn open_orders(&self) -> Vec<OpenOrder> {
        self.working
            .values()
            .map(|working| working.order.clone())
            .collect()
    }

    /// The signed quantity confirmed filled on one instrument at one venue:
    /// bought positive, sold negative. Zero for an instrument nothing has
    /// filled on, however much is resting there.
    pub fn position(&self, venue: &VenueId, object_id: &ObjectId) -> Decimal {
        self.positions
            .get(&Self::position_key(venue, object_id))
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    fn position_key(venue: &VenueId, object_id: &ObjectId) -> String {
        format!("{}/{}", venue.as_str(), object_id.as_str())
    }

    /// Whether the cell is stopped, by any of its three halts.
    pub fn is_halted(&self) -> bool {
        self.autonomy.kill_switch().is_globally_tripped()
            || self.policy_halted
            || self.polled_halt.is_some()
    }

    /// The reason the polled halt wire is engaged, while it is.
    pub fn polled_halt(&self) -> Option<&str> {
        self.polled_halt.as_deref()
    }

    /// Apply what the polled halt flag read as, this poll.
    ///
    /// The flag is the state: engaged or unreadable halts, absent or
    /// released does not, and every poll re-applies it. That is the
    /// opposite discipline from [`Self::apply_halt`], whose broadcast is
    /// engage-only and released by a newer signed payload, and the
    /// difference is the point. §46.2 asks for two paths that do not share
    /// a failure: the broadcast fails when the mesh does, and a mesh
    /// failure cannot reach this one, because this one is a file on the
    /// node that nothing on the mesh writes or clears. Neither halt can
    /// release the other.
    ///
    /// Unreadable halts. A flag that exists but cannot be read — the mount
    /// is gone, the permission is wrong, the content is not one of the two
    /// words — is a wire whose state is unknown, and a kill switch whose
    /// state is unknown must read as engaged ("stale is treated as engaged",
    /// §46.2). Reading it as absent would let the failure of the mount be
    /// the release of the halt.
    pub fn apply_polled_halt(&mut self, reading: PolledHalt, now: Timestamp) {
        let halting = reading.halts();
        match (self.polled_halt.is_some(), halting) {
            (false, true) => {
                let reason = format!("polled halt: {}", reading.describe());
                self.journal.record(
                    Decision::HaltChanged {
                        halted: true,
                        reason: reason.clone(),
                    },
                    now,
                );
                self.polled_halt = Some(reason);
            }
            (true, false) => {
                self.polled_halt = None;
                // `halted` names the cell, not the wire: the other two halts
                // may still hold it, and a reader of the chain must not take
                // this entry for a cell that resumed.
                let halted = self.is_halted();
                self.journal.record(
                    Decision::HaltChanged {
                        halted,
                        reason: format!(
                            "the polled halt flag {}; the cell is {}",
                            reading.describe(),
                            if halted {
                                "still halted by another wire"
                            } else {
                                "released"
                            }
                        ),
                    },
                    now,
                );
            }
            // Idempotent in both steady states: a flag re-read as engaged is
            // one halt, and a flag re-read as absent is no event.
            (true, true) | (false, false) => {}
        }
        self.record_halt();
    }

    /// The degradation narrowing currently in force, derived from the applied
    /// policy payload.
    ///
    /// With no policy this is [`DegradationState::nothing_known`], which reads
    /// every payload-fed capability as unavailable — the fail-closed floor.
    /// Ingestion is deliberately not observed here: the cell's book-staleness
    /// seam already refuses to route on a stale book, per book, which is
    /// stricter than the capability-level pause would be.
    pub fn narrowing(&self, now: Timestamp) -> DegradationState {
        match &self.policy {
            Some(policy) => policy.payload().narrowing(now),
            None => DegradationState::nothing_known(),
        }
    }

    /// The sequence of the applied policy, if any.
    pub fn policy_sequence(&self) -> Option<u64> {
        self.policy.as_ref().map(VerifiedPolicy::sequence)
    }

    /// Apply a verified halt command.
    ///
    /// Engage-only and idempotent: there is no release command, because
    /// release is a fresh policy decision and rides a newer signed payload
    /// issued after the barrier this records. Applying the same halt twice is
    /// one halt.
    pub fn apply_halt(&mut self, halt: VerifiedHalt, now: Timestamp) {
        // A halt at or behind the barrier of one already resolved is a
        // replay: a captured frame re-delivered after a legitimate release
        // would otherwise re-halt the cell in the gaps between publishes — a
        // bounded denial of service in the safe direction, but free to
        // remove. The asymmetry is preserved with care: a *fresh* halt is
        // accepted unconditionally, and an already-halted cell is never
        // released by this path — refusing the replay below leaves it exactly
        // as halted as it was.
        if !self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| halt.issued_at() <= barrier)
        {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_replay".to_string(),
                    reason: format!(
                        "a halt issued at {} is at or behind the resolved barrier and does not \
                         re-halt this cell",
                        halt.issued_at()
                    ),
                },
                now,
            );
            return;
        }
        let barrier = match self.policy_halt_barrier {
            Some(existing) if existing >= halt.issued_at() => existing,
            _ => halt.issued_at(),
        };
        self.policy_halt_barrier = Some(barrier);
        if !self.policy_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("central halt: {}", halt.reason()),
                },
                now,
            );
        }
        self.policy_halted = true;
        self.record_halt();
    }

    /// Apply a verified policy payload by atomic swap.
    ///
    /// "Atomic" in a single-threaded cell means one assignment and never a
    /// partial application: the payload was verified whole before this became
    /// callable — [`VerifiedPolicy`]'s only constructor recomputes the
    /// signature — and nothing below reads a slot before the swap. Trading is
    /// never paused; the previous policy serves until the assignment.
    ///
    /// Sequence discipline lives here because this is where "last applied" is
    /// a fact: a payload at or below the applied sequence is refused, which is
    /// what stops a replayed old payload from un-halting or re-widening the
    /// cell.
    pub fn apply_policy(&mut self, verified: VerifiedPolicy, now: Timestamp) -> Result<()> {
        if verified.payload().cell != self.config.cell_id {
            return Err(Error::denied(format!(
                "a policy payload for cell {} cannot apply to {}",
                verified.payload().cell,
                self.config.cell_id
            )));
        }
        if let Some(applied) = self.policy_sequence()
            && verified.sequence() <= applied
        {
            return Err(Error::denied(format!(
                "policy sequence {} is not newer than the applied {applied}; an old payload \
                 cannot re-widen or un-halt this cell",
                verified.sequence()
            )));
        }

        // A payload that would release the halt must postdate the halt it
        // releases. `halting` is what the cell will actually do, which may be
        // stricter than what the payload says.
        let releasing_too_early = !verified.halted()
            && self.policy_halted
            && self
                .policy_halt_barrier
                .is_some_and(|barrier| verified.payload().issued_at <= barrier);
        let halting = verified.halted() || releasing_too_early;
        let sequence = verified.sequence();
        let was_halted = self.policy_halted;
        let narrowed: Vec<String> = verified
            .payload()
            .narrowing(now)
            .narrowed()
            .iter()
            .map(|(capability, freshness)| {
                format!("{}:{}", capability.as_str(), freshness.as_str())
            })
            .collect();

        self.journal.record(
            Decision::PolicyApplied {
                sequence: verified.sequence(),
                halted: halting,
                narrowed,
            },
            now,
        );
        if releasing_too_early {
            self.journal.record(
                Decision::Refused {
                    gate: "halt_release".to_string(),
                    reason: format!(
                        "policy sequence {} was issued at or before the halt barrier and cannot \
                         release it",
                        verified.sequence()
                    ),
                },
                now,
            );
        }
        if halting != was_halted {
            self.journal.record(
                Decision::HaltChanged {
                    halted: halting,
                    reason: if halting {
                        "the centre halted this cell through policy".to_string()
                    } else {
                        format!(
                            "policy sequence {} released the central halt",
                            verified.sequence()
                        )
                    },
                },
                now,
            );
        }
        if verified.halted() {
            // A halt carried by policy is a halt decision like any other, and
            // it raises the same release barrier: whatever releases it must
            // postdate it, whatever its sequence says.
            self.policy_halt_barrier = Some(match self.policy_halt_barrier {
                Some(existing) if existing >= verified.payload().issued_at => existing,
                _ => verified.payload().issued_at,
            });
        }
        self.policy_halted = halting;
        self.policy = Some(verified);
        self.record_halt();
        // The sequence the cell has *applied*, recorded once the swap has
        // happened. Recording it before would publish a payload the cell might
        // still have refused.
        self.metrics.policy_applied(sequence);
        Ok(())
    }

    /// Track an instrument at a venue.
    pub fn track(&mut self, state: VenueState) {
        self.liquidity.insert(state);
    }

    /// Deploy a strategy, the program its plan indexes into, and the verified
    /// capital envelope it runs under.
    ///
    /// The envelope is the verified type, so a strategy cannot be deployed
    /// against a grant nobody signed. The program is taken here rather than at
    /// assembly for the reason that made this call worth changing: a cell used
    /// to be constructed with an empty arena, so a strategy whose plan pointed
    /// into a real one could be deployed, accepted, and then refuse on every
    /// pass of `work` — a cell that looked healthy, held a strategy, and could
    /// not evaluate it. Every reason that can be established without the market
    /// is established here instead, and a deployment that returns `Ok` is one
    /// the cell can actually run:
    ///
    /// * the envelope names this cell,
    /// * the program is internally consistent,
    /// * every node the plan names exists in that program,
    /// * the strategy fits the cell's evaluation budget.
    ///
    /// What deliberately is *not* checked here is whether the feature engine
    /// will produce the inputs the strategy reads. That depends on the market —
    /// a feature can be registered and still undefined for want of a quote —
    /// so it stays a per-pass judgement the runtime makes against the vector it
    /// was actually handed.
    ///
    /// Deployed this way the strategy names no [`PricingPolicy`], and every
    /// intent it raises is refused under the `pricing` gate until it is
    /// deployed through [`Self::deploy_with_pricing`]. The refusal is the
    /// safe default: a cell that guessed a pricing would either cross
    /// spreads nobody budgeted or rest orders nothing withdraws.
    pub fn deploy(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
    ) -> Result<()> {
        self.install(strategy, program, envelope, None)
    }

    /// [`Self::deploy`], naming how the strategy's intents are priced.
    ///
    /// A `RestAtMid` policy is validated here as it is in
    /// [`PricingPolicy::rest_at_mid`], so a literal built around the
    /// constructor is refused at the same seam.
    pub fn deploy_with_pricing(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
        pricing: PricingPolicy,
    ) -> Result<()> {
        let pricing = match pricing {
            PricingPolicy::Marketable => pricing,
            PricingPolicy::RestAtMid { time_to_live } => PricingPolicy::rest_at_mid(time_to_live)?,
        };
        self.install(strategy, program, envelope, Some(pricing))
    }

    fn install(
        &mut self,
        strategy: CompiledStrategy,
        program: Program,
        envelope: VerifiedEnvelope,
        pricing: Option<PricingPolicy>,
    ) -> Result<()> {
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot deploy into {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        if envelope.strategy() != strategy.id() {
            return Err(Error::denied(format!(
                "an envelope for strategy {} cannot deploy {}",
                envelope.strategy().as_str(),
                strategy.id().as_str()
            )));
        }

        // `NodeRef` is an index. A plan naming a node the arena does not hold
        // is the case where an out-of-range read would be the *lucky* outcome:
        // in a larger arena the index resolves, to a node belonging to some
        // other strategy, and the cell emits a signal computed from arithmetic
        // nobody wrote for it.
        program.validate()?;
        for node in strategy.plan() {
            if program.node(*node).is_none() {
                return Err(Error::invalid(format!(
                    "strategy {} plans node {} and the program it was deployed \
                     with holds {} node(s); the plan and the program do not \
                     belong together",
                    strategy.id().as_str(),
                    node.index(),
                    program.len()
                )));
            }
        }

        // `with_budget` refuses a program it could not evaluate in bounded
        // time. Doing it here means an over-budget strategy is refused by the
        // deployment that shipped it rather than silently, later, by a market
        // that moved.
        let runtime = StrategyRuntime::with_budget(program, self.config.strategy_budget)?;
        if strategy.cost() > runtime.budget() {
            return Err(Error::guard(format!(
                "strategy {} needs {} nodes and this cell evaluates at most {}",
                strategy.id().as_str(),
                strategy.cost(),
                runtime.budget()
            )));
        }

        self.deployed.insert(
            envelope.strategy().as_str().to_string(),
            Deployed {
                strategy,
                runtime,
                envelope,
                utilisation: Utilisation::default(),
                class: StrategyClass::PriceOnly,
                pricing,
            },
        );
        Ok(())
    }

    /// The pricing policy a deployed strategy was given, if any.
    pub fn pricing_of(&self, strategy: &str) -> Option<PricingPolicy> {
        self.deployed
            .get(strategy)
            .and_then(|deployed| deployed.pricing)
    }

    /// Declare which pause rules govern an already-deployed strategy.
    ///
    /// Separate from [`Self::deploy`] so that classification is an explicit
    /// act rather than a defaulted parameter nobody reads. `PriceOnly` is the
    /// deploy-time default because it is true of everything shipped today; a
    /// strategy that consumes world events must say so, and saying so is what
    /// makes the degradation table able to pause it.
    pub fn classify(&mut self, strategy: &str, class: StrategyClass) -> Result<()> {
        match self.deployed.get_mut(strategy) {
            Some(deployed) => {
                deployed.class = class;
                Ok(())
            }
            None => Err(Error::invalid(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 classify"
            ))),
        }
    }

    pub fn deployed_strategies(&self) -> Vec<&str> {
        self.deployed.keys().map(String::as_str).collect()
    }

    /// Withdraw a deployed strategy, handing back the envelope it ran under.
    ///
    /// The path a node takes when a fresh plan no longer names a strategy,
    /// or names it differently. Refused — nothing withdrawn — while an
    /// order carrying the strategy's intent is still open at a venue: the
    /// fill that order may yet report is attributed through the strategy's
    /// contributor share, and a strategy withdrawn out from under a resting
    /// order would leave a fill the cell could book but not explain. The
    /// caller tries again once the order has filled or expired; a resting
    /// order has a time to live somebody chose, and this does not shorten it.
    ///
    /// The envelope is returned rather than dropped because it is capital
    /// the centre signed for this strategy at this cell. A plan that renames
    /// nothing but changes a rule redeploys under the same grant; a plan
    /// that drops the strategy leaves the caller holding a grant it must not
    /// spend on anything else, which `renew_capital` already refuses.
    pub fn withdraw(&mut self, strategy: &str, now: Timestamp) -> Result<VerifiedEnvelope> {
        if !self.deployed.contains_key(strategy) {
            return Err(Error::not_found(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 withdraw"
            )));
        }
        let open: Vec<&str> = self
            .working
            .values()
            .filter(|working| {
                working.order.closed.is_none()
                    && working
                        .net
                        .contributors
                        .iter()
                        .any(|contributor| contributor.strategy.as_str() == strategy)
            })
            .map(|working| working.order.order_id.as_str())
            .collect();
        if !open.is_empty() {
            return Err(Error::denied(format!(
                "strategy {strategy} has {} open order(s) at the venue ({}); it is withdrawn once \
                 they have filled or expired, not while a fill on them could still arrive",
                open.len(),
                open.join(", ")
            )));
        }
        let Some(deployed) = self.deployed.remove(strategy) else {
            return Err(Error::not_found(format!(
                "no strategy named {strategy} is deployed in this cell, so there is nothing to \
                 withdraw"
            )));
        };
        self.journal.record(
            Decision::StrategyWithdrawn {
                strategy: strategy.to_string(),
            },
            now,
        );
        Ok(deployed.envelope)
    }

    // --- the hot path -------------------------------------------------------

    /// Bytes in: decode, sequence, apply, and mark features dirty.
    ///
    /// Does no file or network I/O. The mirror is drained by [`Cell::flush`]
    /// precisely so that this call's cost is arithmetic and memory, never a
    /// storage system's availability.
    pub fn on_bytes(&mut self, feed: &FeedKey, bytes: &[u8], now: Timestamp) -> Result<usize> {
        let (decoded, skipped) = {
            let decoder = self.protocols.decoder_mut(&feed.venue, &feed.feed)?;
            let decoded = decoder.decode(bytes, now)?;
            // Read the counter after decoding: it is cumulative, and the
            // difference is what this call actually skipped.
            let skipped = usize::try_from(decoder.diagnostics().messages_skipped).unwrap_or(0);
            (decoded, skipped)
        };
        let count = decoded.len();
        self.journal.record(
            Decision::Ingested {
                feed: format!("{}/{}", feed.venue.as_str(), feed.feed),
                decoded: count,
                skipped,
            },
            now,
        );

        let batch = self.sequencer.accept(decoded, now);
        self.apply_batch(batch.released, now)?;
        for event in &batch.events {
            if let Some(detail) = gap_detail(event) {
                self.journal.record(
                    Decision::GapDetected {
                        stream: detail.0,
                        detail: detail.1,
                    },
                    now,
                );
            }
        }
        Ok(count)
    }

    /// Apply released messages to books and the feature graph.
    fn apply_batch(&mut self, messages: Vec<MarketMessage>, _now: Timestamp) -> Result<()> {
        for message in &messages {
            let venue = message.origin.venue.clone();
            if let Some(state) = self.liquidity.get_mut(&venue, &message.object_id) {
                // A message the book refuses is a book that would be wrong if
                // it accepted it; the refusal is recorded by the reset path,
                // not swallowed here.
                state.apply(message)?;
            }
            self.features.ingest(message)?;
        }
        Ok(())
    }

    /// One pass of decide-and-act.
    ///
    /// Every gate that refuses records why, in order, so the reason a cell was
    /// quiet is reconstructable without re-running it.
    pub fn work(&mut self, now: Timestamp, gateway: &mut dyn Placer) -> Result<WorkReport> {
        let mut report = WorkReport {
            halted: self.is_halted(),
            ..WorkReport::default()
        };
        // Recorded before the halt check, so a halted cell still counts its
        // passes. A refusal count with no pass count underneath it cannot tell
        // "nothing was refused" from "the cell never ran".
        self.metrics.work_pass();
        // Counted before the halt check too, for the crossing window: a
        // `Passes` interval that skipped halted passes would stretch over
        // more wall time the longer the cell was stopped.
        self.pass = self.pass.saturating_add(1);
        self.record_halt();

        // What the venue has done with the orders already out, before the
        // halt check: a halted cell sends nothing, and still has to learn
        // what filled, because a fill it does not confirm is a fill the
        // reconciler will read as unknown to it.
        report.fills = self.confirm_execution_reports(gateway, now);
        // And what has rested long enough. Also before the halt check:
        // withdrawing is not sending, and a halted cell with orders resting
        // at a price the market has left is exactly the cell that should
        // withdraw them.
        self.withdraw_expired(gateway, now);

        if report.halted {
            // Books keep absorbing and the journal keeps recording while
            // halted. A cell that stops seeing the market cannot tell whether
            // it is safe to resume. The gate names which halt is in force,
            // because the two release disciplines are different and an
            // operator staring at a quiet cell needs to know which door to
            // knock on.
            let gate = if self.autonomy.kill_switch().is_globally_tripped() {
                "kill_switch"
            } else if self.policy_halted {
                "policy_halt"
            } else {
                "polled_halt"
            };
            self.refuse(&mut report, gate, "the cell is halted", now);
            return Ok(report);
        }

        // The degradation table, consulted once per pass. Everything below
        // reads the same narrowing, so a payload applied mid-pass changes the
        // next pass, never half of this one.
        let narrowing = self.narrowing(now);
        let multiplier = narrowing.sizing_multiplier();
        // Freshness is a function of `now`, so this is the instant it becomes
        // known and the only instant at which the recorded value is what the
        // cell actually sized against. Before this the whole table was
        // formatted into a journal string and discarded.
        self.metrics.narrowing(&narrowing);

        let vector = self.features.evaluate(now)?;
        let strategy_ids: Vec<String> = self.deployed.keys().cloned().collect();
        // Phase one collects; phase two nets; phase three sends. The split is
        // the blueprint's, and §28 is why the per-strategy gates stay in phase
        // one rather than moving onto the net.
        let mut intents: Vec<Intent> = Vec::new();

        for id in strategy_ids {
            // A paused strategy does not evaluate at all. Refusing before the
            // run rather than after keeps the journal honest about why the
            // cell was quiet: no signal existed, because the capability the
            // strategy depends on is gone.
            if let Some(deployed) = self.deployed.get(&id)
                && narrowing.pauses(deployed.class)
            {
                self.refuse(
                    &mut report,
                    "degradation_pause",
                    &format!("strategy {id} pauses while its capability is degraded"),
                    now,
                );
                continue;
            }
            // Each deployment evaluates against the arena it was compiled
            // with. `runtime` and `strategy` are disjoint fields of the same
            // deployment, so the borrow ends with the call and the refusal
            // path below can take `&mut self` to journal why it refused.
            let outcome = match self.deployed.get_mut(&id) {
                Some(deployed) => deployed.runtime.run(&deployed.strategy, &vector, now),
                None => continue,
            };
            let signal = match outcome {
                Ok(Some(signal)) => signal,
                Ok(None) => continue,
                Err(error) => {
                    self.refuse(&mut report, "strategy_runtime", error.message(), now);
                    continue;
                }
            };

            self.journal.record(
                Decision::SignalRaised {
                    strategy: signal.strategy.as_str().to_string(),
                    object: signal.object_id.as_str().to_string(),
                    kind: signal.kind.as_str().to_string(),
                    conviction_shrunk_f64: signal.conviction.shrunk(),
                },
                now,
            );
            self.metrics.signal(signal.kind);
            report.signals.push(signal.clone());

            if let Some(intent) = self.intent_for(&signal, multiplier, now, &mut report)? {
                intents.push(intent);
            }
        }

        // The arbitrage desk, at the seam §27.2 names: after the strategies
        // have asked and before anything is netted, so that legs and
        // directional intents meet at one place — and part company there,
        // because a leg is never netted. Every leg is judged by the same
        // feasibility gate the directional intents meet below, then held
        // until the nets have gone out.
        let cycles = self.scan_cycles(now, multiplier, &narrowing, &mut report)?;

        // The feasibility gate, between collection and netting (§18.1). An
        // intent that cannot execute at its size never enters the netting
        // set: a net built from an infeasible contributor would carry that
        // contributor's share to the venue inside an order whose other
        // contributors were feasible, and the venue's rejection — or the
        // fee's bite — would land on all of them. `retain` judges in place,
        // so the gate allocates nothing per pass.
        intents.retain(|intent| self.admit_feasible(intent, now, &mut report));

        // Phase two. Everything the strategies asked for collapses onto one
        // intent per instrument, venue and representation — so two strategies
        // buying the same thing send one order and pay the spread once, and
        // two wanting opposite things cancel without either reaching the
        // venue. Before this, each strategy placed its own order and the two
        // could cross each other, which is a self-trade: a regulatory problem
        // and a pure loss at the same time.
        let nets = net(intents);
        report.netting_ratio = netting_ratio(&nets);
        // `None` when everything cancelled: the ratio is unbounded there, and
        // observing a sentinel would put a number nobody computed into the
        // distribution. The cancellation is counted in `place_net` instead.
        if let Some(ratio) = report.netting_ratio {
            self.metrics.netting_ratio(ratio);
        }
        for net_intent in &nets {
            if let Some(order) = self.place_net(net_intent, now, gateway, &mut report)? {
                report.orders.push(order);
            }
        }

        // Cycles go out after the nets. Never through `net`: each leg is
        // sent by the same order path a net intent uses, one leg after
        // another in the plan's order, least reversible first.
        for cycle in &cycles {
            self.place_cycle(cycle, now, gateway, &mut report)?;
        }

        Ok(report)
    }

    /// Take one signal through every per-strategy gate to an intent, or
    /// refuse it.
    ///
    /// Phase one of the two the blueprint separates. §28 is explicit that
    /// strategy-level limits are checked *before* netting, "because a strategy
    /// that has exhausted its budget must not contribute to a net intent at
    /// all" — so expiry, venue, book staleness, pricing, the degradation
    /// multiplier and the capital envelope all run here, per strategy, exactly
    /// as they did when this function placed an order directly. No gate was
    /// removed and none was reordered; what changed is that the admitted size
    /// becomes an intent instead of an order.
    fn intent_for(
        &mut self,
        signal: &Signal,
        multiplier: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Result<Option<Intent>> {
        if !signal.is_live(now) {
            self.refuse(report, "signal_expiry", "the signal is no longer live", now);
            return Ok(None);
        }

        let Some(venue) = self.venue_for(&signal.object_id) else {
            self.refuse(
                report,
                "venue_selection",
                "no venue this cell may reach quotes the instrument",
                now,
            );
            return Ok(None);
        };

        // A stale or unpriceable book routes nothing. The book already refuses
        // to serve a mid; routing against one anyway would use a price from
        // before the gap that made it stale.
        // Read everything needed from the book in one borrow, so the refusal
        // path below can take `&mut self` to journal why it refused.
        let assessment = self.liquidity.get(&venue, &signal.object_id).map(|state| {
            (
                state.is_stale(),
                state
                    .reset_reason()
                    .unwrap_or("the book is awaiting resynchronisation")
                    .to_string(),
                state.status(),
                state.mid(),
            )
        });
        let Some((stale, reset_reason, status, mid)) = assessment else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument",
                now,
            );
            return Ok(None);
        };
        if stale {
            self.refuse(report, "stale_book", &reset_reason, now);
            return Ok(None);
        }
        if !status.accepts_orders() {
            self.refuse(
                report,
                "venue_status",
                &format!("the venue is {}", status.as_str()),
                now,
            );
            return Ok(None);
        }
        let Some(price) = mid else {
            self.refuse(report, "pricing", "the book serves no usable price", now);
            return Ok(None);
        };
        // The price the intent is *reasoned* at is the mid; the price it is
        // *sent* at is decided by the strategy's policy when the net is
        // placed, and a strategy that stated none is refused here, before
        // it can contribute to a net that another strategy's policy would
        // then price.
        if self.pricing_of(signal.strategy.as_str()).is_none() {
            self.refuse(
                report,
                "pricing",
                &format!(
                    "strategy {} was deployed with no pricing policy; deploy it with \
                     deploy_with_pricing naming marketable or rest-at-mid with a time to live, \
                     because an intent with no stated pricing is never sent",
                    signal.strategy.as_str()
                ),
                now,
            );
            return Ok(None);
        }

        let side = match signal.kind {
            SignalKind::Enter => BookSide::Ask,
            SignalKind::Exit | SignalKind::Hedge => BookSide::Bid,
            SignalKind::Stand => {
                self.refuse(report, "signal_kind", "the signal asks for no action", now);
                return Ok(None);
            }
        };

        // Confidence-weighted sizing, §6.2's consumer. The multiplier narrows
        // the *ask* before the envelope bounds it, so utilisation accounting
        // sees the quantity that will actually be requested. It is exact
        // arithmetic — this scales a position — and a multiply that cannot be
        // represented narrows to nothing rather than widening, the same
        // asymmetry the degradation table itself keeps.
        let desired = signal
            .desired_quantity
            .checked_mul(multiplier)
            .unwrap_or(Decimal::ZERO);
        if !desired.is_positive() {
            self.refuse(
                report,
                "degradation_sizing",
                "the degradation multiplier narrowed the size to nothing",
                now,
            );
            return Ok(None);
        }
        let notional = desired * price;
        let key = signal.strategy.as_str().to_string();
        let Some(deployed) = self.deployed.get(&key) else {
            self.refuse(
                report,
                "deployment",
                "the strategy is not deployed here",
                now,
            );
            return Ok(None);
        };

        // Expiry is checked at every use rather than once at verification:
        // it is the backstop bounding a cell that lost contact, and a backstop
        // consulted only on arrival is not one.
        if !deployed.envelope.is_live(now) {
            self.refuse(
                report,
                "envelope_expiry",
                "the capital envelope has expired; the cell stops rather than continues",
                now,
            );
            return Ok(None);
        }

        let quantity = match deployed
            .envelope
            .admit(&venue, notional, &deployed.utilisation, now)
        {
            CapitalGrant::Full => desired,
            CapitalGrant::Reduced(cap) => {
                let reduced = cap.checked_div(price).unwrap_or(Decimal::ZERO);
                self.refuse(
                    report,
                    "capital_reduced",
                    &format!("reduced to {reduced} by the capital envelope"),
                    now,
                );
                reduced
            }
            CapitalGrant::Refused(reason) => {
                self.refuse(report, "capital", &reason, now);
                return Ok(None);
            }
        };
        if !quantity.is_positive() {
            self.refuse(
                report,
                "capital",
                "the permitted size rounded to nothing",
                now,
            );
            return Ok(None);
        }

        if self.autonomy.level() == AutonomyLevel::Observation {
            self.refuse(
                report,
                "autonomy",
                "the cell is at observation and sends nothing",
                now,
            );
            return Ok(None);
        }

        // Signed, because netting is addition: a buy is positive, a sell is
        // negative, and two opposing intents of equal size sum to nothing
        // without anybody writing a conditional that could be got backwards.
        //
        // `side` names the side of the book the order *takes* — the one
        // convention every seam past this point shares (`Placer`, the node's
        // gateways, `sweep_cost`) — so taking the ask is the buy and is the
        // positive one. This line once read the other way round, and
        // `place_net` read `is_buy` the other way round to match, so an
        // `Enter` still reached the venue as a buy while every fact computed
        // from the sign in between — the cross ledger's `bought` and `sold`,
        // the contributor shares shipped to the centre, `NetIntent::is_buy`
        // itself — named the buyer as the seller. Two inversions that cancel
        // at the venue are not a convention; they are a defect the venue
        // happens not to see.
        let signed = if matches!(side, BookSide::Ask) {
            quantity
        } else {
            -quantity
        };
        let intent = Intent::new(
            signal.strategy.clone(),
            signal.object_id.clone(),
            venue,
            signed,
            price,
            signal.valid_until,
        )?
        // The revisions travel with the intent because this is the last point
        // that has them: after netting, several strategies' shares share one
        // order, and a fill can only be traced back to the values that caused
        // it if each contributor kept its own.
        .with_inputs(signal.inputs.clone());
        Ok(Some(intent))
    }

    /// Send one net intent as one order, or record that it cancelled.
    ///
    /// Phase three. A net of zero is not a refusal: it is two strategies that
    /// wanted opposite things, cancelled internally, and the venue never sees
    /// either — which is the self-trade this whole mechanism exists to
    /// prevent. It is recorded so the cell can still explain what happened.
    fn place_net(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<Option<PlacedOrder>> {
        // Evaluated before the zero-net guard below so that a net cancelling
        // to nothing is still assessed rather than skipped. It is *not* true,
        // as this comment previously claimed, that doing so lets the cap catch
        // its flagship case: see `cross_internally`, where the arithmetic that
        // makes a full cancellation permanently out of cap is set out.
        let crossed = self.cross_internally(net_intent, now, report);

        let Some(is_buy) = net_intent.is_buy() else {
            // Nothing reached the venue, so the cross — if the cap allowed one
            // — is final at this point and safe to seal into the chain.
            self.settle_cross(net_intent, crossed, now, report);
            self.journal.record(
                Decision::Refused {
                    gate: "internal_cross".to_string(),
                    reason: format!(
                        "{} intents on {} at {} cancelled to zero; nothing reached the venue",
                        net_intent.contributors.len(),
                        net_intent.object_id.as_str(),
                        net_intent.venue.as_str()
                    ),
                },
                now,
            );
            self.metrics.intent_cancelled();
            report.cancelled.push(net_intent.clone());
            return Ok(None);
        };
        // A buy takes the ask. See `intent_for` for why this must agree with
        // the sign there rather than compensate for it.
        let side = if is_buy { BookSide::Ask } else { BookSide::Bid };
        let quantity = net_intent.order_quantity();
        let venue = net_intent.venue.clone();

        // Every refusal from here to the send drops the cross unsealed, like
        // the send-error path below: nothing of this net happened, and a
        // cross booked beside a refused order would be a trade between two
        // strategies that the chain could not pair with the order it was the
        // residual of.
        if self.would_self_trade(net_intent, side) {
            self.refuse(
                report,
                "self_trade",
                &format!(
                    "an order of the cell's own is resting on the other side of {} at {}; a {} \
                     now would trade with it, so the net is refused until that order fills or \
                     expires",
                    net_intent.object_id.as_str(),
                    venue.as_str(),
                    if is_buy { "buy" } else { "sell" }
                ),
                now,
            );
            return Ok(None);
        }
        if !self.has_open_capacity(1) {
            self.refuse_for_capacity(report, now);
            return Ok(None);
        }
        let Some((price, expires_at)) =
            self.resolve_pricing(net_intent, side, quantity, now, gateway, report)
        else {
            return Ok(None);
        };

        let (order_id, simulated) = self.send(
            &net_intent.object_id,
            &venue,
            side,
            quantity,
            price,
            now,
            gateway,
        )?;

        // Only now, past the call that can fail. `gateway.place` propagates its
        // error out of `work`, and the caller loses the report with it — so a
        // cross sealed into the hash-chained journal before this line would
        // assert that two strategies traded during a pass that produced
        // nothing at all. The chain is the record; it may not carry a trade
        // the pass did not make.
        self.settle_cross(net_intent, crossed, now, report);

        // Utilisation is charged per contributor, pro-rata on what each
        // wanted, so a netted order still spends each strategy's own envelope
        // rather than one strategy's. The split sums exactly to the order, so
        // the envelopes together are charged what was actually sent.
        for (strategy, share) in net_intent.split_fill(quantity) {
            if let Some(deployed) = self.deployed.get_mut(strategy.as_str()) {
                deployed.utilisation.gross_committed += share * price;
                deployed.utilisation.orders_sent += 1;
            }
        }
        self.record_sent(
            Working {
                order: OpenOrder {
                    order_id: order_id.clone(),
                    venue: venue.clone(),
                    object_id: net_intent.object_id.clone(),
                    side,
                    quantity,
                    price,
                    filled: Decimal::ZERO,
                    simulated,
                    sent_at: now,
                    expires_at,
                    closed: None,
                },
                net: net_intent.clone(),
            },
            now,
        );
        // The venue may have filled some of it on acceptance. Those reports
        // are confirmed now, against the record just written, so a fill on
        // this pass is attributed on this pass.
        let confirmed = self.confirm_execution_reports(gateway, now);
        report.fills.extend(confirmed);

        let largest = net_intent
            .contributors
            .iter()
            .max_by(|left, right| {
                left.signed_size
                    .abs()
                    .cmp(&right.signed_size.abs())
                    .then_with(|| right.strategy.as_str().cmp(left.strategy.as_str()))
            })
            .map_or_else(|| StrategyId::new("unknown"), |c| c.strategy.clone());

        Ok(Some(PlacedOrder {
            order_id,
            strategy: largest,
            contributors: net_intent.contributors.clone(),
            object_id: net_intent.object_id.clone(),
            venue,
            side,
            quantity,
            price,
            simulated,
        }))
    }

    /// Number an order and hand it to the venue.
    ///
    /// The one place a `Placer` is called. Both the net path and the cycle
    /// path go through it, so an order that reaches a venue has been numbered
    /// by the cell's own sequence whichever seam produced it, and a second
    /// route to `gateway.place` — the shape of a control being bypassed —
    /// would have to be written in the open.
    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        now: Timestamp,
        gateway: &mut dyn Placer,
    ) -> Result<(String, bool)> {
        self.order_sequence += 1;
        let order_id = format!("{}-{}", self.config.cell_id, self.order_sequence);
        let simulated = gateway.is_simulated();
        gateway.place(&order_id, object_id, venue, side, quantity, price, now)?;
        Ok((order_id, simulated))
    }

    /// Record an order the venue accepted: the open order its fills will be
    /// confirmed against, the chain entry, and the series.
    ///
    /// Nothing here is a fill. An accepted order is a resting one until the
    /// order-entry channel says otherwise, and this function once wrote the
    /// sent quantity straight into the list the reconciler compares with the
    /// venue — which is how the platform came to record trades that had not
    /// happened.
    fn record_sent(&mut self, working: Working, now: Timestamp) {
        let order = &working.order;
        self.journal.record(
            Decision::OrderSent {
                order_id: order.order_id.clone(),
                venue: order.venue.as_str().to_string(),
                quantity: order.quantity.to_string(),
                simulated: order.simulated,
            },
            now,
        );
        self.metrics.order_placed(&order.venue);
        self.working.insert(order.order_id.clone(), working);
    }

    /// Whether `orders` more can be held open under [`MAX_OPEN_ORDERS`].
    fn has_open_capacity(&self, orders: usize) -> bool {
        self.working.len().saturating_add(orders) <= MAX_OPEN_ORDERS
    }

    fn refuse_for_capacity(&mut self, report: &mut WorkReport, now: Timestamp) {
        self.refuse(
            report,
            "open_orders",
            &format!(
                "the cell holds {} open order(s), the most it will track; nothing more is sent \
                 until fills or expiries settle some, because an order the cell could not hold \
                 is an order whose fill it could not attribute",
                self.working.len()
            ),
            now,
        );
    }

    // --- pricing -------------------------------------------------------------

    /// The price a net goes out at, and when the cell withdraws it, under
    /// the policy its contributors share — or a refusal and `None`.
    ///
    /// The contributors must agree: a net is one order and one order has
    /// one price, and choosing between two strategies' policies would be
    /// the cell deciding what one of them pays. The touch and the mid are
    /// read from the book now, at the instant the order goes out, because
    /// the reference price the net carries is where the size was reasoned
    /// and not where the venue will match it.
    fn resolve_pricing(
        &mut self,
        net_intent: &NetIntent,
        side: BookSide,
        quantity: Decimal,
        now: Timestamp,
        gateway: &dyn Placer,
        report: &mut WorkReport,
    ) -> Option<(Decimal, Option<Timestamp>)> {
        let mut policy: Option<PricingPolicy> = None;
        for contributor in &net_intent.contributors {
            let Some(theirs) = self.pricing_of(contributor.strategy.as_str()) else {
                // `intent_for` refuses an unpriced strategy before it can
                // contribute, so this is a contributor that was undeployed
                // between phases. Refused, not defaulted.
                self.refuse(
                    report,
                    "pricing",
                    &format!(
                        "contributor {} names no pricing policy at the instant the net is placed",
                        contributor.strategy.as_str()
                    ),
                    now,
                );
                return None;
            };
            match policy {
                None => policy = Some(theirs),
                Some(agreed) if agreed == theirs => {}
                Some(agreed) => {
                    self.refuse(
                        report,
                        "pricing_conflict",
                        &format!(
                            "the net on {} at {} carries {} contributors and they do not agree \
                             how to price it ({} and {}); one order has one price, and the \
                             cell does not choose whose",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str(),
                            net_intent.contributors.len(),
                            agreed.as_str(),
                            theirs.as_str()
                        ),
                        now,
                    );
                    return None;
                }
            }
        }
        let policy = policy?;

        let book = self
            .liquidity
            .get(&net_intent.venue, &net_intent.object_id)
            .map(|state| (state.best_bid(), state.best_ask(), state.mid()));
        let Some((bid, ask, mid)) = book else {
            self.refuse(
                report,
                "book",
                "the cell holds no book for the instrument at the instant the net is placed",
                now,
            );
            return None;
        };

        match policy {
            PricingPolicy::Marketable => {
                let touch = match side {
                    BookSide::Ask => ask,
                    BookSide::Bid => bid,
                };
                let Some(touch) = touch else {
                    self.refuse(
                        report,
                        feasibility::GATE_DEPTH,
                        &format!(
                            "nothing rests at the touch on the side the net on {} would take at {}",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                };
                if quantity > touch.size {
                    self.refuse(
                        report,
                        feasibility::GATE_DEPTH,
                        &format!(
                            "the net of {quantity} on {} exceeds the {} resting at the touch at {}; \
                             the net is refused rather than reduced or walked deeper, because a \
                             reduced order is a size nobody reasoned about and a deeper one is a \
                             price nobody did",
                            net_intent.object_id.as_str(),
                            touch.size,
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                }
                Some((touch.price, None))
            }
            PricingPolicy::RestAtMid { time_to_live } => {
                if !gateway.can_cancel() {
                    self.refuse(
                        report,
                        "pricing",
                        &format!(
                            "the net on {} at {} would rest and this gateway cannot withdraw an \
                             order; a resting order nothing can withdraw is refused rather than \
                             left to fill at a price the market has since left",
                            net_intent.object_id.as_str(),
                            net_intent.venue.as_str()
                        ),
                        now,
                    );
                    return None;
                }
                let Some(mid) = mid else {
                    self.refuse(report, "pricing", "the book serves no mid to rest at", now);
                    return None;
                };
                // The mid is between two grid prices and need not be on the
                // grid itself; the venue would refuse it, and the cell says
                // so first under the gate the feasibility rule names.
                let tick = feasibility::tick_for(
                    self.config.feasibility.get(net_intent.venue.as_str()),
                    self.feasibility_constraints(),
                    net_intent.venue.as_str(),
                    &net_intent.object_id,
                );
                match tick {
                    Err(infeasible) => {
                        self.refuse(report, infeasible.gate, &infeasible.reason, now);
                        return None;
                    }
                    Ok(Some(tick)) if mid.floor_to_step(tick) != mid => {
                        self.refuse(
                            report,
                            feasibility::GATE_TICK,
                            &format!(
                                "the mid {mid} is not on the {tick} tick grid for {} at {}; an order \
                                 cannot rest there and the price is refused rather than rounded",
                                net_intent.object_id.as_str(),
                                net_intent.venue.as_str()
                            ),
                            now,
                        );
                        return None;
                    }
                    Ok(_) => {}
                }
                Some((mid, Some(now.saturating_add(time_to_live))))
            }
        }
    }

    /// Whether an order of the cell's own rests on the other side of this
    /// net's instrument at its venue.
    ///
    /// Netting prevents two strategies crossing each other within a pass;
    /// a resting order from an earlier pass is the same self-trade one pass
    /// later, and the venue would match it. Refused, not withdrawn: the
    /// resting order has a time to live somebody chose.
    fn would_self_trade(&self, net_intent: &NetIntent, side: BookSide) -> bool {
        self.working.values().any(|working| {
            let order = &working.order;
            order.closed.is_none()
                && order.venue == net_intent.venue
                && order.object_id == net_intent.object_id
                && order.side != side
                && order.remaining().is_positive()
        })
    }

    /// Withdraw every resting order whose time to live has elapsed.
    ///
    /// Returns the ids withdrawn. The cancel goes through the gateway to
    /// the venue and the venue's answer — what was still open — closes the
    /// order as `expired`; a cancel the venue refuses leaves an order whose
    /// state the cell does not know, which is a break and halts the cell.
    /// A fill that landed between the last report and the cancel is
    /// confirmed straight afterwards, so the order settles with everything
    /// the venue did to it.
    pub fn withdraw_expired(&mut self, gateway: &mut dyn Placer, now: Timestamp) -> Vec<String> {
        let due: Vec<String> = self
            .working
            .values()
            .filter(|working| {
                working.order.closed.is_none()
                    && working
                        .order
                        .expires_at
                        .is_some_and(|expires_at| expires_at <= now)
            })
            .map(|working| working.order.order_id.clone())
            .collect();
        let mut withdrawn = Vec::new();
        for order_id in due {
            let Some(working) = self.working.get(&order_id) else {
                continue;
            };
            let venue = working.order.venue.clone();
            let object_id = working.order.object_id.clone();
            match gateway.cancel(&order_id, &object_id, &venue, now) {
                Ok(remaining) => {
                    if let Some(working) = self.working.get_mut(&order_id) {
                        working.order.closed = Some("expired".to_string());
                    }
                    self.journal.record(
                        Decision::OrderExpired {
                            order_id: order_id.clone(),
                            venue: venue.as_str().to_string(),
                            withdrawn: remaining.to_string(),
                        },
                        now,
                    );
                    self.metrics.order_expired(&venue);
                    withdrawn.push(order_id);
                }
                Err(error) => {
                    self.break_on(
                        format!(
                            "order {order_id} on {} passed its time to live and the venue refused \
                             to withdraw it: {}; whether it is still working is unknown",
                            venue.as_str(),
                            error.message()
                        ),
                        now,
                    );
                }
            }
        }
        if !withdrawn.is_empty() {
            self.confirm_execution_reports(gateway, now);
        }
        withdrawn
    }

    // --- fills: the venue's facts ------------------------------------------

    /// Absorb what the order-entry channel has reported since the last call.
    ///
    /// Returns the fills confirmed, attributed. Safe to call on a halted
    /// cell and meant to be: a halted cell learns what filled so the
    /// reconciler is comparing a record and not a memory. Every report is
    /// judged against the open-order record and a report that names an
    /// order the cell never sent, or fills one past its size, is a break —
    /// the venue's channel disagreeing with the cell's own record is the
    /// same failure the drop copy exists to catch, arriving on the other
    /// channel.
    pub fn confirm_execution_reports(
        &mut self,
        gateway: &mut dyn Placer,
        now: Timestamp,
    ) -> Vec<ConfirmedFill> {
        let mut confirmed = Vec::new();
        for execution in gateway.execution_reports() {
            if let Some(fill) = self.confirm(execution, now) {
                confirmed.push(fill);
            }
        }
        confirmed
    }

    fn confirm(&mut self, execution: ExecutionReport, now: Timestamp) -> Option<ConfirmedFill> {
        if !execution.quantity.is_positive() || !execution.price.is_positive() {
            self.break_on(
                format!(
                    "the order-entry channel reports {} at {} on order {}; a fill needs both \
                     positive, and one that is not is a record the cell cannot book",
                    execution.quantity, execution.price, execution.order_id
                ),
                now,
            );
            return None;
        }
        let Some(working) = self.working.get_mut(&execution.order_id) else {
            self.break_on(
                format!(
                    "the order-entry channel reports a fill of {} on order {} at {} and the cell \
                     has no open order under that id",
                    execution.quantity,
                    execution.order_id,
                    execution.venue.as_str()
                ),
                now,
            );
            return None;
        };
        if working.order.venue != execution.venue {
            let detail = format!(
                "the order-entry channel reports order {} filled at {} and the cell sent it to {}",
                execution.order_id,
                execution.venue.as_str(),
                working.order.venue.as_str()
            );
            self.break_on(detail, now);
            return None;
        }

        // The fill is booked whatever the size check below says: the venue
        // reports it traded, and a position the cell refuses to believe in
        // is the position nobody is watching.
        working.order.filled += execution.quantity;
        let overfilled = working.order.filled > working.order.quantity;
        if working.order.filled >= working.order.quantity {
            working.order.closed = Some("filled".to_string());
        }
        let shares = working.net.split_fill(execution.quantity);
        let fill = ConfirmedFill {
            order_id: execution.order_id.clone(),
            venue: execution.venue.clone(),
            object_id: working.order.object_id.clone(),
            side: working.order.side,
            quantity: execution.quantity,
            price: execution.price,
            simulated: working.order.simulated,
            at: execution.at,
            shares,
        };
        let overfill_detail = overfilled.then(|| {
            format!(
                "order {} was sent for {} and the order-entry channel has now reported {} filled",
                fill.order_id, working.order.quantity, working.order.filled
            )
        });

        let signed = if matches!(fill.side, BookSide::Ask) {
            fill.quantity
        } else {
            -fill.quantity
        };
        *self
            .positions
            .entry(Self::position_key(&fill.venue, &fill.object_id))
            .or_insert(Decimal::ZERO) += signed;
        self.journal.record(
            Decision::Filled {
                order_id: fill.order_id.clone(),
                venue: fill.venue.as_str().to_string(),
                object: fill.object_id.as_str().to_string(),
                quantity: fill.quantity.to_string(),
                price: fill.price.to_string(),
                simulated: fill.simulated,
                shares: fill
                    .shares
                    .iter()
                    .map(|(strategy, share)| (strategy.as_str().to_string(), share.to_string()))
                    .collect(),
            },
            now,
        );
        self.metrics.fill_confirmed(&fill.venue);
        self.confirmed.push(fill.clone());
        if let Some(detail) = overfill_detail {
            self.break_on(detail, now);
        }
        Some(fill)
    }

    /// Record a disagreement between the cell's record and a venue channel,
    /// and halt on the first.
    ///
    /// Tripping needs no authority, which is why a break can act immediately:
    /// the cost of a false stop is minutes of missed opportunity, the cost of
    /// trading on a book that disagrees with the venue is unbounded.
    fn break_on(&mut self, detail: String, now: Timestamp) {
        if self.breaks.len() < MAX_RETAINED_BREAKS {
            self.breaks.push(detail.clone());
        } else {
            self.breaks_omitted = self.breaks_omitted.saturating_add(1);
        }
        self.metrics.reconciliation_break();
        self.journal
            .record(Decision::ReconciliationBreak { detail }, now);
        if !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "drop-copy",
                "the cell's fills disagree with the venue's own account",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: "reconciliation break".to_string(),
                },
                now,
            );
            // A break halts the cell, which stops it running passes. Without
            // this the gauge would keep reporting the state of the last pass
            // that ran, which is the one before the break.
            self.record_halt();
        }
    }

    /// Retire every closed order after a clean comparison.
    ///
    /// Only after a clean one: a closed order whose fills the venue has not
    /// yet matched is exactly the order the next comparison has to see. The
    /// journal already holds every fill retired here.
    fn settle(&mut self) {
        let closed: Vec<String> = self
            .working
            .iter()
            .filter(|(_, working)| working.order.closed.is_some())
            .map(|(order_id, _)| order_id.clone())
            .collect();
        for order_id in closed {
            self.working.remove(&order_id);
            self.confirmed.retain(|fill| fill.order_id != order_id);
            self.dropcopy.retire(&order_id);
        }
    }

    // --- the arbitrage desk --------------------------------------------------

    /// Re-quote the graph from the books, scan it, and admit what survives.
    ///
    /// Returns the cycles to send once the nets have gone, each already past
    /// the feasibility gate and the desk's capital envelope. Everything the
    /// scan refused is journaled under the stage that refused it, every
    /// opportunity found is journaled as priced whether or not it is taken,
    /// and everything past the cap is refused and counted — a scan that
    /// found nothing and said nothing is indistinguishable from one that did
    /// not run.
    ///
    /// Two narrowings stop the scan outright. The degradation table's pause
    /// applies to the desk as to any price-only strategy. And a sizing
    /// multiplier below one opens no cycle at all, rather than a smaller one:
    /// the scanner prices at the size policy's size, edge is not linear in
    /// size, and a cycle re-priced narrower is a different cycle whose legs
    /// no longer close on what was priced. Stopping is the fail-closed
    /// reading of §6.2 for a family whose trades cannot be scaled after the
    /// fact.
    fn scan_cycles(
        &mut self,
        now: Timestamp,
        multiplier: Decimal,
        narrowing: &DegradationState,
        report: &mut WorkReport,
    ) -> Result<Vec<AdmittedCycle>> {
        let Some((cap, validity, strategy)) = self.desk.as_ref().map(|desk| {
            (
                desk.max_cycles_per_pass(),
                desk.leg_validity(),
                desk.strategy().clone(),
            )
        }) else {
            return Ok(Vec::new());
        };
        if narrowing.pauses(StrategyClass::PriceOnly) {
            self.refuse(
                report,
                "degradation_pause",
                "the arbitrage desk pauses while its capability is degraded",
                now,
            );
            return Ok(Vec::new());
        }
        if multiplier < Decimal::ONE {
            self.refuse(
                report,
                "degradation_sizing",
                "the arbitrage desk opens no cycle while sizing is narrowed: a cycle re-priced \
                 at a narrower size is a different cycle, and the scanner priced this one at \
                 the policy's size",
                now,
            );
            return Ok(Vec::new());
        }

        let scanned = {
            // Two fields of `self`, borrowed disjointly: the desk re-quotes
            // its graph from the liquidity it is handed and never reaches
            // for it.
            let Some(desk) = self.desk.as_mut() else {
                return Ok(Vec::new());
            };
            desk.refresh(&self.liquidity)?;
            desk.scan(&self.liquidity, now)
        };

        for rejection in &scanned.rejections {
            self.refuse(
                report,
                scan_gate(rejection.stage),
                &format!(
                    "{} cycle over edges {:?}: {}",
                    rejection.candidate.kind.as_str(),
                    rejection.candidate.edges,
                    rejection.detail
                ),
                now,
            );
        }

        // Bounded by the cap, and by what the scan found if that is fewer.
        let mut cycles: Vec<AdmittedCycle> =
            Vec::with_capacity(cap.min(scanned.opportunities.len()));
        // Notional admitted against the desk's envelope so far this pass, so
        // the second cycle is judged against what the first will spend.
        let mut pending = Decimal::ZERO;
        for (position, opportunity) in scanned.opportunities.iter().enumerate() {
            let cycle_id = opportunity.cycle_id(now);
            self.journal.record(
                Decision::EdgePriced {
                    opportunity: cycle_id.clone(),
                    net: opportunity.net().to_string(),
                    positive: true,
                },
                now,
            );
            if position >= cap {
                self.refuse(
                    report,
                    "arbitrage_cap",
                    &format!(
                        "cycle {cycle_id} is opportunity {} of this pass and the cap is {cap}; \
                         refused and counted rather than dropped, and the next pass will find \
                         it again if it is still there",
                        position + 1
                    ),
                    now,
                );
                continue;
            }
            if self.autonomy.level() == AutonomyLevel::Observation {
                self.refuse(
                    report,
                    "autonomy",
                    "the cell is at observation and sends nothing",
                    now,
                );
                continue;
            }
            let legs = match opportunity.cycle_legs(&strategy, now, now.saturating_add(validity)) {
                Ok(legs) => legs,
                Err(error) => {
                    self.refuse(report, "arbitrage_legs", error.message(), now);
                    continue;
                }
            };
            if let Some(admitted) =
                self.admit_cycle(opportunity, &cycle_id, legs, pending, now, report)
            {
                pending += admitted.notional;
                cycles.push(admitted);
            }
        }
        Ok(cycles)
    }

    /// Take every leg of one cycle through the feasibility gate and the
    /// desk's capital envelope, or veto the cycle whole.
    ///
    /// Whole, because a cycle is an atomic set: a leg that cannot execute at
    /// its size leaves the rest as a position rather than a smaller cycle,
    /// and a leg the envelope would reduce is the same position by another
    /// route. The leg's own refusal is recorded by the gate that found it,
    /// and then the cycle's, so the series counts which rule bound and the
    /// journal says which cycle it bound.
    fn admit_cycle(
        &mut self,
        opportunity: &Opportunity,
        cycle_id: &str,
        legs: Vec<CycleLeg>,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<AdmittedCycle> {
        let mut intents: Vec<Intent> = Vec::with_capacity(legs.len());
        let mut fixed_cost = Decimal::ZERO;
        let mut on_chain = false;
        let mut notional = Decimal::ZERO;
        for leg in legs {
            let intent: Intent = leg.into();
            if !self.admit_feasible(&intent, now, report) {
                self.veto_cycle(cycle_id, &intent, "is infeasible at its size", now, report);
                return None;
            }
            let cost = {
                let model = self.config.feasibility.get(intent.venue.as_str());
                on_chain |= model.is_some_and(|model| {
                    matches!(model.class(), VenueClass::DecentralisedExchange)
                });
                feasibility::fixed_cost_fraction(model, self.feasibility_constraints(), &intent)
            };
            match cost {
                Ok(fraction) => fixed_cost += fraction,
                Err(infeasible) => {
                    self.refuse(report, infeasible.gate, &infeasible.reason, now);
                    self.veto_cycle(
                        cycle_id,
                        &intent,
                        "has no notional to charge against",
                        now,
                        report,
                    );
                    return None;
                }
            }
            let Some(admitted) = self.admit_leg(&intent, pending + notional, now, report) else {
                self.veto_cycle(
                    cycle_id,
                    &intent,
                    "is not admitted by the capital envelope",
                    now,
                    report,
                );
                return None;
            };
            notional += admitted;
            intents.push(intent);
        }

        // The edge as a fraction of the start size, the unit the summed leg
        // costs are in. A start quantity the scanner priced at is positive by
        // construction; a division that fails anyway is a refusal, not a
        // pass.
        let edge_fraction = opportunity
            .net()
            .checked_div(opportunity.pricing.start_quantity);
        let Some(edge_fraction) = edge_fraction else {
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its edge cannot be stated per unit of start size"),
                now,
            );
            return None;
        };
        if let Err(infeasible) = feasibility::assess_cycle_cost(fixed_cost, edge_fraction, on_chain)
        {
            self.refuse(report, infeasible.gate, &infeasible.reason, now);
            self.refuse(
                report,
                "arbitrage_cycle",
                &format!("cycle {cycle_id} is refused whole: its fixed costs consume its edge"),
                now,
            );
            return None;
        }
        Some(AdmittedCycle {
            cycle_id: cycle_id.to_string(),
            net: opportunity.net(),
            legs: intents,
            notional,
        })
    }

    fn veto_cycle(
        &mut self,
        cycle_id: &str,
        leg: &Intent,
        why: &str,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle",
            &format!(
                "cycle {cycle_id} is refused whole: its leg {} of {} at {} {why}, and a cycle \
                 short one leg is a position rather than a smaller cycle",
                leg.signed_size,
                leg.object_id.as_str(),
                leg.venue.as_str()
            ),
            now,
        );
    }

    /// One leg through the gates a directional intent meets in `intent_for`
    /// after pricing: the venue's status, the envelope's life, and capital.
    ///
    /// Returns the leg's notional on admission. `pending` is what this pass
    /// has already admitted against the desk's envelope, added to its
    /// utilisation for the check so a cycle cannot be admitted leg by leg
    /// into more than the envelope holds.
    ///
    /// A `Reduced` grant is a refusal here where `intent_for` takes the
    /// reduction: a directional order at a smaller size is a smaller
    /// position, and a cycle leg at a smaller size is a cycle that no longer
    /// closes.
    fn admit_leg(
        &mut self,
        intent: &Intent,
        pending: Decimal,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<Decimal> {
        let status = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .map(VenueState::status);
        match status {
            None => {
                self.refuse(
                    report,
                    "book",
                    "the cell holds no book for the instrument",
                    now,
                );
                return None;
            }
            Some(status) if !status.accepts_orders() => {
                self.refuse(
                    report,
                    "venue_status",
                    &format!("the venue is {}", status.as_str()),
                    now,
                );
                return None;
            }
            Some(_) => {}
        }
        let Some(notional) = intent.signed_size.abs().checked_mul(intent.reference_price) else {
            self.refuse(
                report,
                "capital",
                "the leg's notional cannot be represented",
                now,
            );
            return None;
        };

        let grant = self.desk.as_ref().map(|desk| {
            if !desk.envelope().is_live(now) {
                return None;
            }
            let mut used = desk.utilisation().clone();
            used.gross_committed += pending;
            Some(desk.envelope().admit(&intent.venue, notional, &used, now))
        });
        match grant {
            None => {
                self.refuse(report, "deployment", "no arbitrage desk is installed", now);
                None
            }
            Some(None) => {
                self.refuse(
                    report,
                    "envelope_expiry",
                    "the desk's capital envelope has expired; the cell stops rather than continues",
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Full)) => Some(notional),
            Some(Some(CapitalGrant::Reduced(cap))) => {
                self.refuse(
                    report,
                    "arbitrage_capital",
                    &format!(
                        "the envelope would reduce the leg to {cap} notional and a cycle leg cannot \
                         be reduced: a reduced leg is a position, not a smaller cycle"
                    ),
                    now,
                );
                None
            }
            Some(Some(CapitalGrant::Refused(reason))) => {
                self.refuse(report, "capital", &reason, now);
                None
            }
        }
    }

    /// Send every leg of an admitted cycle, in plan order.
    ///
    /// # What this cannot promise, and what it does instead
    ///
    /// The blueprint's cycle is atomic-or-cancelled. This cell's
    /// [`Placer`] can place and cannot cancel, and no fill reaches the cell
    /// until the drop-copy is reconciled, so there is nothing here a
    /// `LegGroup` could act on: the coordinator in
    /// `qip-execution-engine::multileg` decides what to unwind from fills it
    /// is told about, and this seam is told nothing. Building one here would
    /// be a control with no input.
    ///
    /// What the cell can do is refuse to carry on. A leg the venue refuses
    /// after an earlier leg went out leaves the cell holding a position it
    /// did not decide to take, which is the state the multi-leg module calls
    /// the one that "looks, to every downstream report, exactly like a
    /// position somebody chose". So the break is journaled naming the cycle
    /// and how many legs were sent, and the kill switch is tripped as a
    /// reconciliation break trips it: the cell stops until an operator has
    /// looked, and the error propagates so the caller knows the pass did not
    /// complete.
    fn place_cycle(
        &mut self,
        cycle: &AdmittedCycle,
        now: Timestamp,
        gateway: &mut dyn Placer,
        report: &mut WorkReport,
    ) -> Result<()> {
        // Room for every leg before the first is sent: a cycle refused for
        // capacity between legs would be a broken cycle, and a broken cycle
        // is a position nobody chose.
        if !self.has_open_capacity(cycle.legs.len()) {
            self.refuse_for_capacity(report, now);
            return Ok(());
        }
        let mut orders: Vec<String> = Vec::with_capacity(cycle.legs.len());
        for (position, leg) in cycle.legs.iter().enumerate() {
            // A buy takes the ask; the sign was fixed where the leg was made.
            let side = if leg.signed_size.is_positive() {
                BookSide::Ask
            } else {
                BookSide::Bid
            };
            let quantity = leg.signed_size.abs();
            let price = leg.reference_price;
            let sent = self.send(
                &leg.object_id,
                &leg.venue,
                side,
                quantity,
                price,
                now,
                gateway,
            );
            let (order_id, simulated) = match sent {
                Ok(sent) => sent,
                Err(error) => {
                    self.break_cycle(
                        &cycle.cycle_id,
                        position,
                        cycle.legs.len(),
                        &error,
                        now,
                        report,
                    );
                    return Err(error);
                }
            };
            // A leg is attributed like a net of one: `net` over a single
            // no-net intent yields one contributor, which is the leg's
            // strategy at the leg's full size.
            let leg_net = net(vec![leg.clone()]).into_iter().next();
            let Some(leg_net) = leg_net else {
                let error = Error::invalid(format!(
                    "leg {position} of cycle {} nets to nothing and cannot be attributed",
                    cycle.cycle_id
                ));
                self.break_cycle(
                    &cycle.cycle_id,
                    position.saturating_add(1),
                    cycle.legs.len(),
                    &error,
                    now,
                    report,
                );
                return Err(error);
            };
            self.record_sent(
                Working {
                    order: OpenOrder {
                        order_id: order_id.clone(),
                        venue: leg.venue.clone(),
                        object_id: leg.object_id.clone(),
                        side,
                        quantity,
                        price,
                        filled: Decimal::ZERO,
                        simulated,
                        sent_at: now,
                        // A leg is priced at the touch the scanner quoted it
                        // from and takes it on acceptance; nothing rests.
                        expires_at: None,
                        closed: None,
                    },
                    net: leg_net,
                },
                now,
            );
            let confirmed = self.confirm_execution_reports(gateway, now);
            report.fills.extend(confirmed);
            if let Some(desk) = self.desk.as_mut() {
                let utilisation = desk.utilisation_mut();
                utilisation.gross_committed += quantity * price;
                utilisation.orders_sent += 1;
            }
            orders.push(order_id.clone());
            report.orders.push(PlacedOrder {
                order_id,
                strategy: leg.strategy.clone(),
                contributors: vec![Contributor {
                    strategy: leg.strategy.clone(),
                    signed_size: leg.signed_size,
                    inputs: leg.inputs.clone(),
                }],
                object_id: leg.object_id.clone(),
                venue: leg.venue.clone(),
                side,
                quantity,
                price,
                simulated,
            });
        }
        self.journal.record(
            Decision::CycleCommitted {
                cycle_id: cycle.cycle_id.clone(),
                orders,
                net: cycle.net.to_string(),
            },
            now,
        );
        Ok(())
    }

    /// A cycle stopped between legs. Record it and stop the cell.
    fn break_cycle(
        &mut self,
        cycle_id: &str,
        sent: usize,
        total: usize,
        error: &Error,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        self.refuse(
            report,
            "arbitrage_cycle_broken",
            &format!(
                "cycle {cycle_id} stopped after {sent} of {total} legs: {}; the legs already sent \
                 are a position nobody chose, and the cell halts until an operator has looked",
                error.message()
            ),
            now,
        );
        if sent > 0 && !self.is_halted() {
            self.autonomy.kill_switch_mut().trip_global(
                now,
                "arbitrage",
                "a cycle stopped between legs and left a position the cell did not decide to take",
            );
            self.journal.record(
                Decision::HaltChanged {
                    halted: true,
                    reason: format!("cycle {cycle_id} broke after {sent} of {total} legs"),
                },
                now,
            );
            self.record_halt();
        }
    }

    /// Work out the offsetting part of a net that should be crossed between its
    /// own contributors, or refuse it and say why (§27.1).
    ///
    /// Returns the cross rather than recording it: the caller seals it into the
    /// journal once the pass has an outcome, because a venue call can fail
    /// afterwards and take the whole report with it.
    ///
    /// The matched size is the smaller of the buying and selling sides, which
    /// is exactly the quantity that never needed a venue. It is computed as a
    /// minimum rather than as `(gross - |net|) / 2` so that no division enters
    /// a money path: the two are equal, and only one of them can introduce a
    /// remainder.
    ///
    /// **The cap refuses; it does not clamp.** Above forty percent of gross
    /// intent the blueprint's objection is that a persistent internal market
    /// forms whose marks drift from reality — and crossing the permitted forty
    /// percent and abandoning the rest would build exactly that market, just
    /// more slowly. Refusing the whole cross leaves the offsetting intents
    /// netted as before: nothing extra reaches the venue, and nothing is
    /// booked between strategies.
    ///
    /// # What the per-net cap cannot do, stated because the arithmetic is not obvious
    ///
    /// With no [`CrossingInterval`] configured the cap is measured against
    /// this net alone. The matched size is `min(buy, sell)` and the
    /// denominator is `buy + sell`, so the ratio can never exceed one half,
    /// and it reaches one half exactly when the two sides cancel completely.
    /// A forty percent cap therefore fires only in the narrow band above two
    /// fifths — and **a net that cancels to zero is always refused**, which
    /// is §27.1's own flagship case: "strategies that disagree cost nothing
    /// to run together because their disagreement never reaches a venue".
    /// Under the per-net measure that disagreement is never booked as a
    /// cross at all.
    ///
    /// That is the default, and it is deliberate. §27.1 caps crossing at
    /// forty percent of gross intent "per instrument **per interval**", and
    /// never says how long an interval is. The window length decides when a
    /// safety control fires, so this crate does not choose one: unset, the
    /// cap reads per net, which is safe and less than §27.1 asks for, and
    /// `a_fully_offsetting_net_is_out_of_cap_by_arithmetic_and_is_never_crossed`
    /// holds that default in place. Choosing the interval is the owner's
    /// decision (completion plan D3), taken by setting
    /// [`CellConfig::crossing_interval`].
    ///
    /// # With an interval
    ///
    /// The cap is then §27.1's: crossed size over gross intent, per
    /// instrument, accumulated over the trailing window — what the window
    /// has already crossed plus this cross, against what it has already
    /// seen plus this net. A full cancellation can sit inside a larger
    /// instrument-level gross and be admitted, and a run of them is refused
    /// once they are two fifths of the window, which is the persistent
    /// internal market the cap exists to prevent. The window's samples are
    /// written by [`Self::settle_cross`] only once the pass has an outcome,
    /// for the same reason the cross itself is.
    ///
    /// Crossing changes nothing about what is sent. It is a booking decision
    /// on top of netting, which has already decided what a venue sees.
    fn cross_internally(
        &mut self,
        net_intent: &NetIntent,
        now: Timestamp,
        report: &mut WorkReport,
    ) -> Option<InternalCross> {
        let mut bought = Vec::new();
        let mut sold = Vec::new();
        let mut buy_size = Decimal::ZERO;
        let mut sell_size = Decimal::ZERO;
        for contributor in &net_intent.contributors {
            if contributor.signed_size.is_positive() {
                buy_size += contributor.signed_size;
                bought.push(contributor.strategy.clone());
            } else if contributor.signed_size.is_negative() {
                sell_size -= contributor.signed_size;
                sold.push(contributor.strategy.clone());
            }
        }
        // Nothing offset, so there is nothing to cross. The common case, and
        // not a refusal: a net every contributor agreed on has no internal
        // trade in it to record. It is journaled anyway, because a chain that
        // explains a cross but is silent about its absence leaves a reader
        // unable to tell "they agreed" from "the crossing step never ran".
        if buy_size.is_zero() || sell_size.is_zero() {
            self.journal.record(
                Decision::CrossedInternally {
                    object: net_intent.object_id.as_str().to_string(),
                    venue: net_intent.venue.as_str().to_string(),
                    quantity: Decimal::ZERO.to_string(),
                    price: String::new(),
                    bought: bought.iter().map(|id| id.as_str().to_string()).collect(),
                    sold: sold.iter().map(|id| id.as_str().to_string()).collect(),
                },
                now,
            );
            return None;
        }
        let crossed = if buy_size < sell_size {
            buy_size
        } else {
            sell_size
        };

        // What the window has already seen for this instrument. Nothing,
        // exactly, when no interval is configured — so the per-net reading
        // below is byte-for-byte the arithmetic this cell always had.
        let window = self.crossing_window(net_intent, now);
        if window.full {
            self.refuse(
                report,
                "internal_cross_window",
                &format!(
                    "the crossing window for {} on {} holds {MAX_CROSSING_WINDOW_SAMPLES} nets \
                     and its oldest gross has been dropped; the cap cannot be measured against \
                     a partial window and refuses until it drains",
                    net_intent.object_id.as_str(),
                    net_intent.venue.as_str()
                ),
                now,
            );
            return None;
        }

        // Forty percent of gross intent, compared without dividing: the cap is
        // two fifths, so `crossed * 5 > gross * 2` asks the same question in
        // exact arithmetic. The totals are the window's plus this net's. A
        // sum or multiply that cannot be represented refuses, because a cap
        // that silently answered "under" on overflow would be a control that
        // cannot fire.
        let over_cap = match (
            window
                .crossed
                .checked_add(crossed)
                .and_then(|total| total.checked_mul(Decimal::from_int(5))),
            window
                .gross
                .checked_add(net_intent.gross_size)
                .and_then(|total| total.checked_mul(Decimal::from_int(2))),
        ) {
            (Some(five_crossed), Some(two_gross)) => five_crossed > two_gross,
            _ => true,
        };
        if over_cap {
            self.refuse(
                report,
                "internal_cross_cap",
                &format!(
                    "crossing {crossed} of {} gross intent on {} exceeds the forty percent cap \
                     ({} already crossed of {} gross in the window); the cross is refused whole \
                     rather than trimmed to the cap, because a cross repeated at the cap every \
                     interval is the persistent internal market the cap exists to prevent",
                    net_intent.gross_size,
                    net_intent.object_id.as_str(),
                    window.crossed,
                    window.gross
                ),
                now,
            );
            return None;
        }

        // The prevailing mid at the netting instant, read from the book now
        // rather than taken from `reference_price` — that is the largest
        // contributor's own stamped price, and §27.1 requires a price neither
        // side chose. A book that serves no mid refuses the cross instead of
        // falling back to one, because the fallback is precisely the price the
        // rule forbids.
        let mid = self
            .liquidity
            .get(&net_intent.venue, &net_intent.object_id)
            .and_then(|state| state.mid());
        let Some(price) = mid else {
            self.refuse(
                report,
                "internal_cross_price",
                "the book serves no mid at the netting instant, and a cross has no price either \
                 side may choose",
                now,
            );
            return None;
        };

        Some(InternalCross {
            object_id: net_intent.object_id.clone(),
            venue: net_intent.venue.clone(),
            quantity: crossed,
            price,
            bought,
            sold,
        })
    }

    /// Seal a cross into the chain, report it, and let the crossing window
    /// see the net it came from.
    ///
    /// One call for the two records because they must agree: a window that
    /// counted a cross the chain never sealed would refuse later crosses
    /// against a trade that did not happen, and one that missed a sealed
    /// cross would admit the persistent internal market the cap exists to
    /// prevent. Called only once the pass has an outcome, after the venue
    /// call that can fail — see `place_net`.
    fn settle_cross(
        &mut self,
        net_intent: &NetIntent,
        crossed: Option<InternalCross>,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        let quantity = crossed
            .as_ref()
            .map_or(Decimal::ZERO, |cross| cross.quantity);
        self.record_cross(crossed, now, report);
        self.observe_crossing(net_intent, quantity, now);
    }

    /// The instrument key the crossing window is kept by: what `net` groups
    /// on, so one window per net key.
    fn crossing_key(net_intent: &NetIntent) -> String {
        format!(
            "{}/{}/{}",
            net_intent.venue.as_str(),
            net_intent.object_id.as_str(),
            net_intent.representation.as_str()
        )
    }

    /// Whether a sample is inside the configured window at `now`, this pass.
    fn in_crossing_window(&self, sample: &CrossingSample, now: Timestamp) -> bool {
        match self.config.crossing_interval {
            None => false,
            // The last `n` passes, this one included: a sample from pass
            // `p` is in while `p + n > current`.
            Some(CrossingInterval::Passes(passes)) => {
                sample.pass.saturating_add(u64::from(passes)) > self.pass
            }
            Some(CrossingInterval::Span(span)) => sample.at >= now.saturating_sub(span),
        }
    }

    /// What the window has seen for this net's instrument, before this net.
    ///
    /// Samples that have left the window are dropped here, so a history is
    /// as long as its window and no longer. With no interval configured the
    /// history is never written and this is zero, zero, not full.
    fn crossing_window(&mut self, net_intent: &NetIntent, now: Timestamp) -> CrossingWindow {
        let mut window = CrossingWindow::default();
        if self.config.crossing_interval.is_none() {
            return window;
        }
        let key = Self::crossing_key(net_intent);
        let Some(history) = self.crossing_history.get(&key) else {
            return window;
        };
        let live: Vec<CrossingSample> = history
            .iter()
            .filter(|sample| self.in_crossing_window(sample, now))
            .copied()
            .collect();
        if let Some(history) = self.crossing_history.get_mut(&key) {
            history.clear();
            history.extend(live.iter().copied());
        }
        // A history at the bound has had its oldest sample dropped by
        // `observe_crossing`, so what it holds is not the whole window.
        window.full = live.len() >= MAX_CROSSING_WINDOW_SAMPLES;
        for sample in &live {
            // Saturating in the safe direction: a total that cannot be
            // summed reads as the largest representable, so the cap's own
            // checked sum over it overflows and refuses rather than
            // trusting a wrapped number.
            window.gross = window
                .gross
                .checked_add(sample.gross)
                .unwrap_or(Decimal::MAX);
            window.crossed = window
                .crossed
                .checked_add(sample.crossed)
                .unwrap_or(Decimal::MAX);
        }
        window
    }

    /// Record one settled net in its instrument's window.
    ///
    /// Written only when an interval is configured, so the per-net default
    /// allocates nothing. At the bound the oldest sample is dropped rather
    /// than the newest: the newest is the one the next evaluation must see,
    /// and `crossing_window` reports the truncation as `full`.
    fn observe_crossing(&mut self, net_intent: &NetIntent, crossed: Decimal, now: Timestamp) {
        if self.config.crossing_interval.is_none() {
            return;
        }
        let history = self
            .crossing_history
            .entry(Self::crossing_key(net_intent))
            .or_default();
        if history.len() >= MAX_CROSSING_WINDOW_SAMPLES {
            history.pop_front();
        }
        history.push_back(CrossingSample {
            pass: self.pass,
            at: now,
            gross: net_intent.gross_size,
            crossed,
        });
    }

    /// Seal a cross into the hash-chained journal and report it.
    ///
    /// Separate from working the cross out, so that the record is written only
    /// once the pass it belongs to has actually produced its outcome.
    fn record_cross(
        &mut self,
        crossed: Option<InternalCross>,
        now: Timestamp,
        report: &mut WorkReport,
    ) {
        let Some(cross) = crossed else {
            return;
        };
        self.journal.record(
            Decision::CrossedInternally {
                object: cross.object_id.as_str().to_string(),
                venue: cross.venue.as_str().to_string(),
                quantity: cross.quantity.to_string(),
                price: cross.price.to_string(),
                bought: cross
                    .bought
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
                sold: cross
                    .sold
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
            },
            now,
        );
        self.metrics.internal_cross(&cross.venue);
        report.crosses.push(cross);
    }

    /// Judge one intent against the feasibility gate, refusing and counting
    /// it under the rule that bound.
    ///
    /// The book is read here, at the netting instant, and the gate itself is
    /// a pure function of what it is handed — so the fact judged is the one
    /// the journal can replay. The size resting at the touch is read on the
    /// side the intent *takes*: a buy takes the ask, so it is the ask's size
    /// that bounds it.
    fn admit_feasible(&mut self, intent: &Intent, now: Timestamp, report: &mut WorkReport) -> bool {
        let touch = self
            .liquidity
            .get(&intent.venue, &intent.object_id)
            .and_then(|state| {
                if intent.signed_size.is_positive() {
                    state.best_ask()
                } else {
                    state.best_bid()
                }
            })
            .map(|level| level.size);
        let verdict = feasibility::assess(
            self.config.feasibility.get(intent.venue.as_str()),
            self.feasibility_constraints(),
            intent,
            touch,
        );
        match verdict {
            Ok(()) => true,
            Err(infeasible) => {
                self.refuse(report, infeasible.gate, &infeasible.reason, now);
                false
            }
        }
    }

    /// Item 11 of the applied policy payload, if the centre has produced it.
    ///
    /// Read whatever its freshness: a venue's minimum order and tick change
    /// on the order of months, the slot's own time-to-live is a day, and a
    /// constraint that has gone stale is still the last thing the centre
    /// knew rather than nothing. The degradation table already narrows the
    /// cell's sizing on the slot's staleness; refusing to read the slot as
    /// well would be a second control on the same fact with a different
    /// threshold.
    fn feasibility_constraints(&self) -> Option<&qip_contracts::policy::FeasibilityConstraints> {
        self.policy
            .as_ref()
            .and_then(|policy| policy.payload().feasibility_constraints.value())
    }

    fn venue_for(&self, object: &ObjectId) -> Option<VenueId> {
        self.config
            .venues
            .iter()
            .find(|venue| {
                self.liquidity
                    .get(venue, object)
                    .is_some_and(|state| state.status() != VenueStatus::Unreachable)
            })
            .cloned()
    }

    fn refuse(&mut self, report: &mut WorkReport, gate: &str, reason: &str, now: Timestamp) {
        // Every gate a *pass* can refuse at funnels through here, so one
        // recording site covers all of them. `gate` is a string literal at
        // each call, and that is what bounds this series' cardinality. The
        // three refusals that journal directly — a replayed halt, a release
        // that predates its barrier, and a net that cancelled to zero — are
        // not pass-time gates and are deliberately not counted here: the
        // first two are control-plane events with no "why was the cell
        // quiet" reading, and the third is counted as a cancellation.
        self.metrics.refusal(gate);
        report.refusals.push((gate.to_string(), reason.to_string()));
        self.journal.record(
            Decision::Refused {
                gate: gate.to_string(),
                reason: reason.to_string(),
            },
            now,
        );
    }

    // --- the mesh seam ------------------------------------------------------

    /// Describe this cell to the central plane.
    ///
    /// Assembled from what the cell already holds rather than accumulated as it
    /// goes, so a delta is a *view* and building one twice with the same report
    /// produces the same value. That matters because the transport underneath
    /// is at-least-once: a delta that is rebuilt and re-sent after a failed
    /// attempt has to be the same fact, not a second one.
    ///
    /// `cell`, `region` and `sequence` are filled in by
    /// [`crate::mesh::CellUplink::publish`], which owns the stream's numbering.
    /// A cell that numbered its own deltas would eventually skip one, and the
    /// centre cannot tell a skipped sequence from a lost delta.
    pub fn state_delta(&self, report: &WorkReport, at: Timestamp) -> CellStateDelta {
        let mut delta = CellStateDelta {
            cell: self.config.cell_id.clone(),
            region: self.config.region.clone(),
            sequence: 0,
            at,
            halted: self.is_halted(),
            utilisation: self
                .deployed
                .values()
                .map(|deployed| StrategyUtilisation {
                    strategy: deployed.envelope.strategy().clone(),
                    utilisation: deployed.utilisation.clone(),
                    envelope_expires_at: deployed.envelope.expires_at(),
                })
                // The desk spends an envelope too, and the centre that issued
                // it hears how much the same way.
                .chain(self.desk.as_ref().map(|desk| StrategyUtilisation {
                    strategy: desk.strategy().clone(),
                    utilisation: desk.utilisation().clone(),
                    envelope_expires_at: desk.envelope().expires_at(),
                }))
                .collect(),
            orders: report
                .orders
                .iter()
                .map(|order| DeltaOrder {
                    order_id: order.order_id.clone(),
                    strategy: order.strategy.clone(),
                    object_id: order.object_id.clone(),
                    venue: order.venue.clone(),
                    side: order.side,
                    quantity: order.quantity,
                    price: order.price,
                    simulated: order.simulated,
                    contributors: order.contributors.clone(),
                })
                .collect(),
            // From the fills the venue reported this pass and nothing else.
            // The list above is what was sent; deriving a fill from it is the
            // reading that charged the centre for resting orders.
            fills: report
                .fills
                .iter()
                .map(|fill| qip_contracts::wire::FillRecord {
                    order_id: fill.order_id.clone(),
                    object_id: fill.object_id.clone(),
                    venue: fill.venue.clone(),
                    side: fill.side,
                    quantity: fill.quantity,
                    price: fill.price,
                    simulated: fill.simulated,
                    at: fill.at,
                    shares: fill
                        .shares
                        .iter()
                        .map(|(strategy, quantity)| qip_contracts::wire::FillShare {
                            strategy: strategy.clone(),
                            quantity: *quantity,
                        })
                        .collect(),
                })
                .collect(),
            // Set by `bound_refusals` below, like the other two counters.
            fills_omitted: 0,
            refusals: report
                .refusals
                .iter()
                .map(|(gate, reason)| DeltaRefusal {
                    gate: gate.clone(),
                    reason: reason.clone(),
                })
                .collect(),
            // Set by `bound_refusals` below; the caller does not get to claim
            // a truncation that did not happen.
            refusals_omitted: 0,
            reconciliation_breaks: self.breaks.clone(),
            reconciliation_breaks_omitted: self.breaks_omitted,
            crosses: report
                .crosses
                .iter()
                .map(|cross| qip_contracts::wire::CrossRecord {
                    object_id: cross.object_id.clone(),
                    venue: cross.venue.clone(),
                    quantity: cross.quantity,
                    price: cross.price,
                    bought: cross.bought.clone(),
                    sold: cross.sold.clone(),
                })
                .collect(),
            // Set by `bound_refusals` below, like the refusal counter: the
            // caller does not get to claim a truncation that did not happen.
            crosses_omitted: 0,
        };
        delta.bound_refusals();
        delta
    }

    /// Install a capital envelope the centre issued for a strategy already
    /// deployed here.
    ///
    /// Takes the verified type, so there is no path from a frame off the wire
    /// to a live grant that does not go through
    /// [`crate::VerifiedEnvelope::verify`]. Arriving over the mesh buys an
    /// envelope nothing; this signature is what says so.
    ///
    /// Three things this deliberately does not do:
    ///
    /// * **It does not deploy.** A grant names a strategy; it does not carry
    ///   the compiled strategy or the program its plan indexes into, and a cell
    ///   that started running something because capital arrived for it would be
    ///   promoting its own strategy — the thing ADR 0008 says a cell never
    ///   does. An envelope for a strategy that is not deployed is refused.
    /// * **It does not reset utilisation.** What a strategy has committed is
    ///   measured against positions that are still open, and a renewal that
    ///   zeroed it would hand the strategy its whole gross limit again while
    ///   the previous commitment was still live. Carrying it across is the
    ///   conservative direction, and it is the one that is right.
    /// * **It does not widen anything by itself.** The new envelope replaces
    ///   the old one entirely — wider or narrower — because that is what the
    ///   centre signed. A cell that merged the two would be constructing a
    ///   grant nobody approved.
    pub fn renew_capital(&mut self, envelope: VerifiedEnvelope, now: Timestamp) -> Result<()> {
        // `verify` has already checked the cell, and this checks it again
        // against the cell's own identity rather than against the string a
        // caller passed to the verifier. The two are the same today; a
        // downlink misconfigured with another cell's name is the case where
        // they would not be, and that is exactly the case worth catching.
        if envelope.cell() != self.config.cell_id {
            return Err(Error::denied(format!(
                "an envelope for cell {} cannot renew capital at {}",
                envelope.cell(),
                self.config.cell_id
            )));
        }
        let key = envelope.strategy().as_str().to_string();
        let approver = envelope.approver().to_string();
        let expires_at = envelope.expires_at();
        if let Some(desk) = self.desk.as_mut()
            && desk.strategy().as_str() == key
        {
            // The desk is renewed by the same rules as a strategy: the grant
            // replaces the old one whole and utilisation carries across.
            desk.replace_envelope(envelope);
            self.journal.record(
                Decision::CapitalRenewed {
                    strategy: key,
                    approver,
                    expires_at,
                },
                now,
            );
            return Ok(());
        }
        let Some(deployed) = self.deployed.get_mut(&key) else {
            return Err(Error::not_found(format!(
                "no strategy {key} is deployed at this cell, so there is nothing for the grant to \
                 fund; a cell does not deploy a strategy because capital arrived for it"
            )));
        };
        deployed.envelope = envelope;
        self.journal.record(
            Decision::CapitalRenewed {
                strategy: key,
                approver,
                expires_at,
            },
            now,
        );
        Ok(())
    }

    /// Reconciliation breaks this cell has recorded, oldest first.
    pub fn reconciliation_breaks(&self) -> &[String] {
        &self.breaks
    }

    // --- reconciliation and the mirror --------------------------------------

    /// Absorb a fill from the independent drop-copy channel.
    pub fn observe_drop_copy(&mut self, fill: DropCopyFill) {
        self.dropcopy.observe(fill);
    }

    /// Compare the two records and halt on any disagreement.
    ///
    /// The cell's side is its *confirmed* fills — what the order-entry
    /// channel reported — never what it sent. An order resting unfilled is
    /// on neither side and is not a break; a fill on either side alone is.
    /// A clean comparison settles every closed order, which is what keeps
    /// both records bounded by the orders still working.
    pub fn reconcile(&mut self, now: Timestamp) -> Vec<Discrepancy> {
        let fills: Vec<CellFill> = self
            .confirmed
            .iter()
            .map(|fill| CellFill {
                order_id: fill.order_id.clone(),
                venue: fill.venue.clone(),
                quantity: fill.quantity,
                price: fill.price,
            })
            .collect();
        let breaks = self.dropcopy.reconcile(&fills);
        for discrepancy in &breaks {
            self.break_on(discrepancy.describe(), now);
        }
        if breaks.is_empty() {
            self.settle();
        }
        breaks
    }

    /// Ship the journal to durable storage.
    ///
    /// The only call in the cell that may block, and deliberately outside both
    /// [`Cell::on_bytes`] and [`Cell::work`].
    pub fn flush(&mut self, mirror: &mut dyn Mirror, now: Timestamp) -> Result<usize> {
        let watermarks = self
            .sequencer
            .watermarks()
            .into_iter()
            .map(|mark| (mark.stream, mark.position))
            .collect();
        crate::journal::ship(
            &mut self.journal,
            mirror,
            &self.config.cell_id,
            watermarks,
            now,
        )
    }
}

/// What the polled halt flag read as, on one poll (§46.2's second wire).
///
/// Built from the flag's bytes by [`Self::from_content`] and from the
/// failure to obtain them by the node, which is the only thing that touches
/// the file. The cell never reads a path: it is handed the reading, so the
/// same seam is driven by a test with no file at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolledHalt {
    /// No flag exists. Not halted: the deployment shape is a file the
    /// operator creates to halt and removes to release, and a node whose
    /// operator has never written one is running. A *missing mount* is not
    /// this — see [`crate::cell::PolledHalt::Unreadable`].
    Absent,
    /// The flag exists and reads `released`. Not halted: the managed-store
    /// shape keeps a key that always exists, and this is its off state.
    Released,
    /// The flag is engaged, with the reason it carried.
    Engaged(String),
    /// The flag could not be read or could not be understood: a permission
    /// error, a missing directory, more bytes than a flag may hold, bytes
    /// that are not text, or text that is neither word. Halted, because a
    /// wire whose state is unknown is a wire that has failed, and a kill
    /// switch fails engaged.
    Unreadable(String),
}

impl PolledHalt {
    /// The most bytes a flag may hold. A flag is a word and a short reason;
    /// a file larger than this is not the flag, whatever put it there.
    pub const MAX_CONTENT_BYTES: usize = 256;

    /// Read the flag's bytes.
    ///
    /// Two words are understood: `released`, and `engaged` optionally
    /// followed by a colon and a reason. An empty file is engaged — its
    /// presence is the signal in the file-per-halt shape. Anything else
    /// halts as unreadable; the content is not echoed into the reason,
    /// because whatever ended up in the file is not a fact the chain should
    /// carry.
    pub fn from_content(bytes: &[u8]) -> Self {
        if bytes.len() > Self::MAX_CONTENT_BYTES {
            return Self::Unreadable(format!(
                "the flag holds {} bytes and a flag may hold at most {}",
                bytes.len(),
                Self::MAX_CONTENT_BYTES
            ));
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Self::Unreadable("the flag is not text".to_string());
        };
        let text = text.trim();
        if text.is_empty() || text == "engaged" {
            return Self::Engaged("the flag is present".to_string());
        }
        if let Some(reason) = text.strip_prefix("engaged:") {
            let reason = reason.trim();
            return Self::Engaged(if reason.is_empty() {
                "the flag is present".to_string()
            } else {
                reason.to_string()
            });
        }
        if text == "released" {
            return Self::Released;
        }
        Self::Unreadable("the flag holds text that is neither `engaged` nor `released`".to_string())
    }

    /// Whether this reading stops the cell.
    pub const fn halts(&self) -> bool {
        matches!(self, Self::Engaged(_) | Self::Unreadable(_))
    }

    /// One bounded word per arm, for a health body or a label.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Released => "released",
            Self::Engaged(_) => "engaged",
            Self::Unreadable(_) => "unreadable",
        }
    }

    /// The reading, for the journal.
    pub fn describe(&self) -> String {
        match self {
            Self::Absent => "is absent".to_string(),
            Self::Released => "reads released".to_string(),
            Self::Engaged(reason) => format!("is engaged: {reason}"),
            Self::Unreadable(reason) => format!("is unreadable and reads as engaged: {reason}"),
        }
    }
}

/// One net as the crossing window saw it: when, and how much of its gross
/// was crossed.
#[derive(Clone, Copy, Debug)]
struct CrossingSample {
    pass: u64,
    at: Timestamp,
    gross: Decimal,
    crossed: Decimal,
}

/// The window's totals for one instrument, before the net being judged.
#[derive(Clone, Copy, Debug, Default)]
struct CrossingWindow {
    gross: Decimal,
    crossed: Decimal,
    /// The history was truncated at its bound, so these totals understate
    /// the window and the cap must refuse rather than measure.
    full: bool,
}

/// A cycle past every gate and waiting for the nets to go out first.
#[derive(Clone, Debug)]
struct AdmittedCycle {
    cycle_id: String,
    /// The scanner's net edge, in units of the start instrument.
    net: Decimal,
    /// In plan order. Every one carries `NettingPolicy::NoNet` by
    /// construction, which is what `CycleLeg` exists to guarantee.
    legs: Vec<Intent>,
    /// The sum of the legs' notionals, admitted against the desk's envelope.
    notional: Decimal,
}

/// The gate literal a scan rejection is counted under.
///
/// One literal per stage of the scanner, so §30.1's question — which stage
/// refuses most of what the search proposes — is a series rather than a
/// grep of the journal. Bounded by the enum.
const fn scan_gate(stage: RejectionStage) -> &'static str {
    match stage {
        RejectionStage::Unsized => "arbitrage_scan_unsized",
        RejectionStage::ExactArithmetic => "arbitrage_scan_exact_arithmetic",
        RejectionStage::Unpriceable => "arbitrage_scan_unpriceable",
        RejectionStage::Depth => "arbitrage_scan_depth",
        RejectionStage::Book => "arbitrage_scan_book",
        RejectionStage::NetEdge => "arbitrage_scan_net_edge",
        RejectionStage::Plan => "arbitrage_scan_plan",
    }
}

/// Where a cell sends an order.
///
/// Narrower than the routing crate's `Gateway` on purpose: the cell needs to
/// place and to know whether the venue is simulated, and a wider surface here
/// would be a wider surface to get wrong.
pub trait Placer: std::fmt::Debug {
    /// Whether this is a simulated venue. The cell sets every order's
    /// `simulated` flag from this rather than from anything the caller says.
    fn is_simulated(&self) -> bool;

    fn place(
        &mut self,
        order_id: &str,
        object_id: &ObjectId,
        venue: &VenueId,
        side: BookSide,
        quantity: Decimal,
        price: Decimal,
        at: Timestamp,
    ) -> Result<()>;

    /// What a production deployment must supply, empty when usable as is.
    fn required_configuration(&self) -> Vec<String> {
        Vec::new()
    }

    /// Everything the order-entry channel has reported filled since the
    /// last call: the fills the venue returned on acceptance, and later
    /// reports on orders that rested.
    ///
    /// Defaults to nothing, which is the honest answer for a gateway that
    /// has no such channel — its orders are then accepted and never filled,
    /// and the cell holds them open rather than assuming. A gateway must not
    /// synthesise a report from the order it was handed; the report is the
    /// venue's answer or it is nothing.
    fn execution_reports(&mut self) -> Vec<ExecutionReport> {
        Vec::new()
    }

    /// Whether [`Self::cancel`] reaches the venue. The cell reads this
    /// before it lets an order rest; a gateway answering `false` gets no
    /// resting orders at all.
    fn can_cancel(&self) -> bool {
        false
    }

    /// Withdraw what remains of an order, returning the quantity the venue
    /// says was still open. The default refuses, and a gateway that has a
    /// venue cancel path overrides both this and [`Self::can_cancel`]
    /// together: one without the other is a promise the cell would act on.
    fn cancel(
        &mut self,
        order_id: &str,
        _object_id: &ObjectId,
        venue: &VenueId,
        _at: Timestamp,
    ) -> Result<Decimal> {
        Err(Error::denied(format!(
            "this gateway cannot withdraw order {order_id} from {}; it has no cancel path to the \
             venue",
            venue.as_str()
        )))
    }
}

/// The gap events worth journalling, and what to say about each.
///
/// An opened gap may still fill, so it is recorded as an observation. An
/// abandoned one has already produced a reset and invalidated a book, which is
/// the event an incident review is looking for.
fn gap_detail(event: &qip_sequencing::tracker::SequenceEvent) -> Option<(String, String)> {
    use qip_sequencing::tracker::SequenceEvent;
    match event {
        SequenceEvent::GapOpened {
            stream,
            missing_from,
            missing_to,
        } => Some((
            stream.clone(),
            format!("sequences {missing_from}..={missing_to} are missing; holding for reorder"),
        )),
        SequenceEvent::GapAbandoned {
            stream,
            missing_from,
            missing_to,
            reason,
        } => Some((
            stream.clone(),
            format!(
                "sequences {missing_from}..={missing_to} will not arrive ({reason:?}); \
                 the affected books are reset"
            ),
        )),
        SequenceEvent::StreamStarted { .. }
        | SequenceEvent::Duplicate { .. }
        | SequenceEvent::GapFilled { .. } => None,
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod crossing_tests {
    //! §27.1's crossing price, tested where the two candidate prices differ.
    //!
    //! The behavioural tests in `qip-edge-node` cannot tell the book's mid from
    //! `NetIntent::reference_price`, because a cell prices every intent off the
    //! same mid in the same pass and the two numbers are equal there. A
    //! mutation that priced crosses from the reference price survived those
    //! tests for exactly that reason. These drive the private seam with a net
    //! intent whose reference price is deliberately nothing like the book, so
    //! "the prevailing mid at the netting instant, never a price either side
    //! chose" becomes an assertion instead of a coincidence.

    use super::*;
    use qip_contracts::message::{MarketMessage, MessageBody};
    use qip_contracts::venue::{Origin, VenueStatus};
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;
    use qip_orderbook::venue::VenueState;

    const CELL: &str = "london-1";

    fn object() -> ObjectId {
        ObjectId::from_string("ACME")
    }

    fn venue() -> VenueId {
        VenueId::new("XLON")
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000).saturating_add(qip_core::Duration::from_secs(seconds))
    }

    /// A book quoting 99 / 101, so the mid is 100.
    fn book() -> VenueState {
        let mut state = VenueState::aggregated(object(), venue(), VenueStatus::Open);
        for (index, (side, price, size)) in
            [(BookSide::Bid, "99", "900"), (BookSide::Ask, "101", "300")]
                .iter()
                .enumerate()
        {
            let message = MarketMessage::new(
                object(),
                Origin::new(venue(), "feed-a", 0, index as u64),
                MessageBody::LevelSet {
                    side: *side,
                    price: Decimal::parse(price).expect("a decimal literal"),
                    quantity: Decimal::parse(size).expect("a decimal literal"),
                    order_count: None,
                },
                at(index as i64),
                at(index as i64),
            );
            state.apply(&message).expect("a well-formed level");
        }
        state
    }

    fn cell_with_book() -> Result<Cell> {
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(book());
        Ok(cell)
    }

    /// Deploy `id` with a marketable pricing policy, so a net it contributes
    /// to can be priced and reach the venue. The nets these tests build by
    /// hand name strategies; a net whose contributors are not deployed is
    /// refused under `pricing` before any venue is called, which is right,
    /// and not what a test of the venue path is looking at.
    fn deploy_marketable(cell: &mut Cell, id: &str) -> Result<()> {
        use qip_contracts::capital::CapitalEnvelope;
        use qip_strategy::catalogue::FeatureCatalogue;
        use qip_strategy::compile::StrategyCompiler;
        use qip_strategy::ir::{Expr, Rule, StrategySpec};

        let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
        let spec = StrategySpec::new(
            StrategyId::new(id),
            object(),
            qip_core::Duration::from_secs(30),
        )
        .with_rule(Rule::new(
            "always",
            SignalKind::Enter,
            Expr::Flag(true),
            Expr::Exact(Decimal::ONE),
            Expr::Statistic(0.5),
            10,
        ));
        let compiled = compiler.compile(&spec)?;
        let key = b"a-unit-test-envelope-key";
        let build = |signature: &str| {
            CapitalEnvelope::new(
                StrategyId::new(id),
                CELL,
                Decimal::from_int(1_000_000),
                Decimal::from_int(100_000),
                Decimal::from_int(50_000),
                vec![venue()],
                at(0),
                at(3600),
                "alice@example.com",
                signature,
            )
        };
        let unsigned = build("unsigned")?;
        let signature = crate::envelope::sign_payload(key, &unsigned.signing_payload());
        let envelope = VerifiedEnvelope::verify(build(&signature)?, key, CELL, at(1))?;
        cell.deploy_with_pricing(
            compiled,
            compiler.into_program(),
            envelope,
            PricingPolicy::Marketable,
        )
    }

    /// Net the given `(strategy, signed size)` pairs on the fixture instrument
    /// through [`net`] itself, every intent stamped with `reference_price`.
    ///
    /// Not a literal. `NetIntent` is sealed to `net` so that nobody can
    /// assemble a vector of contributors `net` would have refused, and these
    /// tests were the one caller still forging one by hand — a fixture that
    /// bypasses the seam it is meant to drive is a second construction path
    /// with a friendlier name. Going through `net` also means the net's own
    /// reference price is whatever `net` chose, which is what the crossing
    /// test needs to be sure of before it can claim the mid was chosen over
    /// it.
    fn netted(sizes: &[(&str, &str)], reference_price: Decimal) -> NetIntent {
        let intents = sizes
            .iter()
            .map(|(strategy, size)| {
                Intent::new(
                    StrategyId::new(*strategy),
                    object(),
                    venue(),
                    Decimal::parse(size).expect("a decimal literal"),
                    reference_price,
                    at(60),
                )
                .expect("a fixture size is never zero")
            })
            .collect();
        let mut nets = net(intents);
        assert_eq!(
            nets.len(),
            1,
            "directional intents on one instrument and venue net to one group"
        );
        nets.pop().expect("exactly one net was just asserted")
    }

    /// A net of a 100 buy against a 20 sell: 20 crosses, which is a sixth of
    /// the 120 gross and so comfortably under the forty percent cap.
    fn offsetting_net(reference_price: Decimal) -> NetIntent {
        netted(&[("alpha", "100"), ("beta", "-20")], reference_price)
    }

    #[test]
    fn a_cross_is_priced_at_the_book_mid_and_not_at_a_price_either_side_chose() -> Result<()> {
        let mut cell = cell_with_book()?;
        // A reference price nothing in the book could produce. If the cross
        // were priced from the net intent, this is the number that would
        // appear — and it is one contributor's own stamped price, which §27.1
        // forbids by name.
        let chosen = Decimal::parse("12345").expect("a decimal literal");
        let net_intent = offsetting_net(chosen);
        // The premise, in two halves: the net `net` built really carries the
        // chosen price — otherwise the assertion below that the cross did not
        // take it would be true of any implementation — and the two candidate
        // prices really do differ here, which is the whole reason this test
        // exists rather than the behavioural one.
        assert_eq!(
            net_intent.reference_price, chosen,
            "the fixture net does not carry the price it was built from"
        );
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(|state| state.mid())
            .expect("the fixture book serves a mid");
        assert_ne!(mid, chosen, "the fixture cannot distinguish the two prices");

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&net_intent, at(10), &mut report);
        cell.settle_cross(&net_intent, crossed, at(10), &mut report);

        assert_eq!(report.crosses.len(), 1, "nothing was crossed: {report:?}");
        assert_eq!(
            report.crosses[0].price, mid,
            "the cross was priced at {} rather than at the mid",
            report.crosses[0].price
        );
        assert_ne!(
            report.crosses[0].price, chosen,
            "the cross took the reference price, which is a price one side chose"
        );
        assert_eq!(
            report.crosses[0].quantity,
            Decimal::parse("20").expect("a decimal literal"),
            "the matched size is the smaller side, not the net or the gross"
        );
        Ok(())
    }

    /// A venue that refuses everything, so `place_net` returns `Err` and the
    /// caller loses the report.
    #[derive(Debug)]
    struct RefusingGateway;

    impl Placer for RefusingGateway {
        fn is_simulated(&self) -> bool {
            true
        }

        fn place(
            &mut self,
            _order_id: &str,
            _object_id: &ObjectId,
            _venue: &VenueId,
            _side: BookSide,
            _quantity: Decimal,
            _price: Decimal,
            _at: Timestamp,
        ) -> Result<()> {
            Err(qip_core::error::Error::io("the venue refused the order"))
        }
    }

    #[test]
    fn a_venue_that_fails_leaves_no_cross_in_the_chain() -> Result<()> {
        // The journal is hash-chained and is the record. `gateway.place`
        // propagates its error out of `place_net` and out of `work`, and the
        // caller loses the report with it — so a cross written before that call
        // would leave the chain asserting that two strategies traded during a
        // pass that produced nothing at all. Nobody can unwrite it afterwards.
        let mut cell = cell_with_book()?;
        deploy_marketable(&mut cell, "alpha")?;
        deploy_marketable(&mut cell, "beta")?;
        let net_intent = offsetting_net(Decimal::parse("100").expect("a decimal literal"));
        let mut report = WorkReport::default();

        let before = cell.journal().entries().len();
        let outcome = cell.place_net(&net_intent, at(10), &mut RefusingGateway, &mut report);

        // Premise: the venue really did fail, so what follows is about the
        // failure path and not about a quiet success.
        assert!(
            outcome.is_err(),
            "the premise failed: the refusing gateway placed an order"
        );
        let crosses: Vec<_> = cell
            .journal()
            .entries()
            .iter()
            .skip(before)
            .filter(|entry| entry.decision.kind() == "crossed_internally")
            .collect();
        assert!(
            crosses.is_empty(),
            "the chain records a cross for a pass that placed nothing: {crosses:?}"
        );
        Ok(())
    }

    #[test]
    fn a_fully_offsetting_net_is_out_of_cap_by_arithmetic_and_is_never_crossed() -> Result<()> {
        // §27.1's flagship case, and the one this cap cannot admit. The matched
        // size is `min(buy, sell)` over a denominator of `buy + sell`, so the
        // ratio tops out at one half and hits it exactly when the two sides
        // cancel — always above forty percent. The test exists so the
        // divergence is asserted rather than merely described in a comment
        // somebody may later delete as stale.
        let mut cell = cell_with_book()?;
        let opposed = netted(
            &[("alpha", "100"), ("beta", "-100")],
            Decimal::parse("100").expect("a decimal literal"),
        );
        // Premise: the sides really do cancel, so this is the full-offset case
        // and not merely a large partial one.
        assert!(
            opposed.net_size.is_zero(),
            "the premise needs a total offset"
        );

        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(&opposed, at(10), &mut report);
        cell.settle_cross(&opposed, crossed, at(10), &mut report);

        assert!(
            report.crosses.is_empty(),
            "a fully offsetting net was crossed, so the cap arithmetic has \
             changed and the comment explaining it is now wrong"
        );
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_cap"),
            "the full offset was neither crossed nor refused by the cap: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn a_book_with_no_mid_refuses_the_cross_rather_than_pricing_it_from_the_intent() -> Result<()> {
        // The fallback §27.1 forbids is exactly the one a careless
        // implementation reaches for when the book is silent. There is no
        // price neither side chose available, so there is no cross.
        let config = CellConfig::new(CELL, "europe-west2").with_venue(venue());
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(VenueState::aggregated(object(), venue(), VenueStatus::Open));
        // Premise: this book genuinely serves no mid, so the refusal below is
        // about the price and not about something else.
        assert!(
            cell.liquidity()
                .get(&venue(), &object())
                .and_then(|state| state.mid())
                .is_none(),
            "the fixture book serves a mid, so nothing would be refused"
        );

        let mut report = WorkReport::default();
        let net_intent = offsetting_net(Decimal::parse("12345").expect("a decimal literal"));
        let crossed = cell.cross_internally(&net_intent, at(10), &mut report);
        cell.settle_cross(&net_intent, crossed, at(10), &mut report);

        assert!(report.crosses.is_empty(), "a cross was priced with no mid");
        assert!(
            report
                .refusals
                .iter()
                .any(|(gate, _)| gate == "internal_cross_price"),
            "the refusal did not name the pricing gate: {:?}",
            report.refusals
        );
        Ok(())
    }

    // --- the interval (§27.1 "per instrument per interval") -----------------

    fn cell_with_interval(interval: CrossingInterval) -> Result<Cell> {
        let config = CellConfig::new(CELL, "europe-west2")
            .with_venue(venue())
            .with_crossing_interval(interval)?;
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        let mut cell = Cell::new(config, features)?;
        cell.track(book());
        Ok(cell)
    }

    fn price() -> Decimal {
        Decimal::parse("100").expect("a decimal literal")
    }

    /// Run one net through the crossing seam as `place_net` would, on a
    /// fresh pass, and return what was crossed.
    fn judge(cell: &mut Cell, net_intent: &NetIntent, now: Timestamp) -> WorkReport {
        cell.pass = cell.pass.saturating_add(1);
        let mut report = WorkReport::default();
        let crossed = cell.cross_internally(net_intent, now, &mut report);
        cell.settle_cross(net_intent, crossed, now, &mut report);
        report
    }

    fn refused_under(report: &WorkReport, gate: &str) -> bool {
        report.refusals.iter().any(|(g, _)| g == gate)
    }

    #[test]
    fn with_an_interval_two_strategies_cancelling_completely_inside_a_larger_window_both_fill_at_the_mid()
    -> Result<()> {
        // §27.1's flagship case, reachable once the cap is measured per
        // interval. Pass one is a one-sided 400 on the instrument; pass two
        // is a 100 against a 100. Over the two-pass window that is 100
        // crossed of 600 gross — a sixth, well under two fifths — so the
        // full cancellation crosses, at the mid, and both sides are named.
        let mut cell = cell_with_interval(CrossingInterval::Passes(3))?;
        let mid = cell
            .liquidity()
            .get(&venue(), &object())
            .and_then(|state| state.mid())
            .expect("the fixture book serves a mid");

        let one_sided = netted(&[("alpha", "400")], price());
        let first = judge(&mut cell, &one_sided, at(10));
        assert!(
            first.crosses.is_empty(),
            "a one-sided net has nothing to cross"
        );

        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        // Premise: the sides really cancel, and without the interval this
        // very net is refused — the default the cited test holds — so what
        // admits it below is the window and nothing else.
        assert!(
            opposed.net_size.is_zero(),
            "the premise needs a total offset"
        );
        let mut per_net = cell_with_book()?;
        let refused = judge(&mut per_net, &opposed, at(11));
        assert!(
            refused.crosses.is_empty() && refused_under(&refused, "internal_cross_cap"),
            "the premise failed: the per-net default admitted a full cancellation"
        );

        let second = judge(&mut cell, &opposed, at(11));
        assert_eq!(
            second.crosses.len(),
            1,
            "the full cancellation was not crossed inside the window: {:?}",
            second.refusals
        );
        let cross = &second.crosses[0];
        assert_eq!(
            cross.quantity,
            Decimal::parse("100").expect("a decimal literal")
        );
        assert_eq!(cross.price, mid, "the cross was not priced at the mid");
        assert_eq!(cross.bought, vec![StrategyId::new("alpha")]);
        assert_eq!(cross.sold, vec![StrategyId::new("beta")]);
        Ok(())
    }

    #[test]
    fn a_run_of_full_cancellations_is_refused_once_it_is_two_fifths_of_the_window() -> Result<()> {
        // The persistent internal market the cap exists to prevent, built
        // one pass at a time. Passes one to three: 100 against 100 each. The
        // first is refused (100 of 200), the second admitted (100 of 400),
        // the third admitted (200 of 600). The fourth sees passes two to
        // four only — 300 crossed of 600 gross, half — and is refused. If
        // pass one were still counted the fourth would read 300 of 800 and
        // pass, so this also holds that a `Passes` window forgets.
        let mut cell = cell_with_interval(CrossingInterval::Passes(3))?;
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        let outcomes: Vec<bool> = (1..=4)
            .map(|pass| {
                let report = judge(&mut cell, &opposed, at(pass));
                !report.crosses.is_empty()
            })
            .collect();
        assert_eq!(
            outcomes,
            vec![false, true, true, false],
            "crossed-per-pass did not follow the rolling cap"
        );
        Ok(())
    }

    #[test]
    fn a_span_interval_forgets_nets_older_than_the_span() -> Result<()> {
        let mut cell =
            cell_with_interval(CrossingInterval::Span(qip_core::Duration::from_secs(10)))?;
        let one_sided = netted(&[("alpha", "400")], price());
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        judge(&mut cell, &one_sided, at(0));
        // Premise: inside the span the one-sided gross admits the cross.
        let inside = judge(&mut cell, &opposed, at(5));
        assert_eq!(
            inside.crosses.len(),
            1,
            "the premise failed: {:?}",
            inside.refusals
        );
        // Thirty seconds on, both earlier nets are outside the ten-second
        // span and the same cancellation is judged on its own again.
        let outside = judge(&mut cell, &opposed, at(35));
        assert!(
            outside.crosses.is_empty() && refused_under(&outside, "internal_cross_cap"),
            "a net outside the span still counted towards the window: {outside:?}"
        );
        Ok(())
    }

    #[test]
    fn with_no_interval_the_window_is_never_written() -> Result<()> {
        // The default must be today's behaviour exactly, and "exactly"
        // includes allocating nothing: a history that accumulated while
        // unread would be a bound with nothing behind it.
        let mut cell = cell_with_book()?;
        assert!(
            cell.config().crossing_interval.is_none(),
            "the premise is the default"
        );
        judge(&mut cell, &offsetting_net(price()), at(10));
        judge(&mut cell, &offsetting_net(price()), at(11));
        assert!(
            cell.crossing_history.is_empty(),
            "the per-net default kept a crossing history: {:?}",
            cell.crossing_history
        );
        Ok(())
    }

    #[test]
    fn a_window_that_hit_its_bound_refuses_rather_than_measuring_part_of_itself() -> Result<()> {
        // A `Span` long enough to hold everything, fed one net per pass past
        // the bound. The 1,025th net finds a truncated history and is
        // refused under the window gate — not admitted against a gross that
        // is missing its oldest sample, and not silently trimmed.
        let mut cell =
            cell_with_interval(CrossingInterval::Span(qip_core::Duration::from_hours(1)))?;
        let one_sided = netted(&[("alpha", "400")], price());
        for pass in 0..MAX_CROSSING_WINDOW_SAMPLES {
            judge(&mut cell, &one_sided, at(pass as i64));
        }
        let opposed = netted(&[("alpha", "100"), ("beta", "-100")], price());
        let report = judge(&mut cell, &opposed, at(MAX_CROSSING_WINDOW_SAMPLES as i64));
        assert!(
            report.crosses.is_empty(),
            "a cross was admitted against a partial window"
        );
        assert!(
            refused_under(&report, "internal_cross_window"),
            "the refusal did not name the window gate: {:?}",
            report.refusals
        );
        Ok(())
    }

    #[test]
    fn an_empty_or_oversized_interval_is_refused_at_configuration() {
        for interval in [
            CrossingInterval::Passes(0),
            CrossingInterval::Span(qip_core::Duration::ZERO),
            CrossingInterval::Span(qip_core::Duration::from_secs(-1)),
            CrossingInterval::Passes(u32::try_from(MAX_CROSSING_WINDOW_SAMPLES + 1).expect("fits")),
        ] {
            assert!(
                CellConfig::new(CELL, "europe-west2")
                    .with_crossing_interval(interval)
                    .is_err(),
                "{interval:?} was accepted, and would measure the cap against nothing or \
                 against less than it names"
            );
        }
        assert!(
            CellConfig::new(CELL, "europe-west2")
                .with_crossing_interval(CrossingInterval::Passes(1))
                .is_ok(),
            "a one-pass interval is the per-net reading with accounting on, and is valid"
        );
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod polled_halt_tests {
    //! §46.2's second wire, at the seam the node drives: what each reading
    //! of the flag does to the cell, and that no other wire releases it.

    use super::*;
    use qip_feature_dag::engine::FeatureEngine;
    use qip_feature_dag::state::MarketState;

    fn cell() -> Result<Cell> {
        let config = CellConfig::new("london-1", "europe-west2");
        let features = FeatureEngine::new(MarketState::default(), qip_core::Duration::from_secs(5));
        Cell::new(config, features)
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_secs(1_700_000_000).saturating_add(qip_core::Duration::from_secs(seconds))
    }

    #[test]
    fn every_content_the_flag_can_hold_reads_the_way_the_wire_needs() {
        // The two words, the empty file, and everything else. Everything
        // else halts: a flag whose content cannot be understood is a wire
        // whose state is unknown, and a kill switch fails engaged.
        assert_eq!(
            PolledHalt::from_content(b""),
            PolledHalt::Engaged("the flag is present".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b"engaged\n"),
            PolledHalt::Engaged("the flag is present".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b"engaged: drill 7\n"),
            PolledHalt::Engaged("drill 7".to_string())
        );
        assert_eq!(
            PolledHalt::from_content(b" released \n"),
            PolledHalt::Released
        );
        assert!(
            matches!(
                PolledHalt::from_content(b"release"),
                PolledHalt::Unreadable(_)
            ),
            "a near-miss of the release word must not release"
        );
        assert!(matches!(
            PolledHalt::from_content(b"\xff\xfe"),
            PolledHalt::Unreadable(_)
        ));
        let oversized = vec![b'e'; PolledHalt::MAX_CONTENT_BYTES + 1];
        assert!(matches!(
            PolledHalt::from_content(&oversized),
            PolledHalt::Unreadable(_)
        ));
        for (reading, halts) in [
            (PolledHalt::Absent, false),
            (PolledHalt::Released, false),
            (PolledHalt::Engaged("x".to_string()), true),
            (PolledHalt::Unreadable("x".to_string()), true),
        ] {
            assert_eq!(reading.halts(), halts, "{reading:?}");
        }
    }

    #[test]
    fn an_unreadable_flag_halts_and_an_absent_one_releases_and_the_chain_says_which() -> Result<()>
    {
        let mut cell = cell()?;
        assert!(!cell.is_halted(), "the premise is a running cell");

        cell.apply_polled_halt(
            PolledHalt::Unreadable("permission denied".to_string()),
            at(1),
        );
        assert!(cell.is_halted(), "an unreadable flag did not halt the cell");
        assert!(
            cell.polled_halt()
                .is_some_and(|reason| reason.contains("permission denied")),
            "the halt does not carry the read failure: {:?}",
            cell.polled_halt()
        );
        // Re-reading the same state is one halt, not a second chain entry.
        let entries = cell.journal().entries().len();
        cell.apply_polled_halt(PolledHalt::Engaged("still".to_string()), at(2));
        assert_eq!(
            cell.journal().entries().len(),
            entries,
            "a re-read was journaled as a new halt"
        );

        cell.apply_polled_halt(PolledHalt::Absent, at(3));
        assert!(
            !cell.is_halted(),
            "an absent flag did not release the polled halt"
        );
        let last = cell
            .journal()
            .entries()
            .last()
            .expect("the release was journaled");
        assert_eq!(last.decision.kind(), "halt_changed");
        assert!(
            format!("{:?}", last.decision).contains("polled halt flag is absent"),
            "the release entry does not name the wire: {:?}",
            last.decision
        );
        Ok(())
    }

    #[test]
    fn the_polled_wire_and_the_kill_switch_release_each_other_never() -> Result<()> {
        // Two wires that shared a release would share a failure. With both
        // engaged, clearing one leaves the cell exactly as halted, and the
        // chain entry for the polled release says so rather than reading as
        // a resumed cell.
        let mut cell = cell()?;
        cell.apply_polled_halt(PolledHalt::Engaged("drill".to_string()), at(1));
        cell.autonomy_mut()
            .kill_switch_mut()
            .trip_global(at(2), "operator", "drill");
        assert!(cell.is_halted(), "the premise needs both wires engaged");

        cell.apply_polled_halt(PolledHalt::Released, at(3));
        assert!(
            cell.is_halted(),
            "releasing the polled wire released a kill switch it does not own"
        );
        let last = cell.journal().entries().last().expect("journaled");
        assert!(
            matches!(last.decision, Decision::HaltChanged { halted: true, .. }),
            "the polled release entry claims the cell resumed: {:?}",
            last.decision
        );
        Ok(())
    }
}
