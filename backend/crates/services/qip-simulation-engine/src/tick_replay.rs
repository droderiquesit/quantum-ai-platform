//! Tick-level replay of recorded history, with the platform's decisions
//! executed against it (R12-TICK-2059, 2061, 2067, 2076, 2078).
//!
//! [`crate::market::MarketSimulator`] replays bars. A bar has no queue, no
//! book and no sequence, so a strategy replayed on bars is credited with
//! fills at prices nobody was offering, ahead of orders that were resting
//! there first. This module replays the **ticks** the lake recorded: every
//! venue message is applied to a rebuilt order book in a deterministic
//! order, the platform's orders meet that book after their latency, take
//! liquidity that was really there, wait in a queue behind the size that was
//! really resting, and pay fees and impact. Multi-leg plans execute leg by
//! leg, so a leg that fails part-way leaves the earlier legs open and the
//! replay unwinds them at the price the book then offered and charges the
//! cost — completion is never assumed.
//!
//! # Deterministic, and resumable from a checkpoint
//!
//! Ticks are sequenced by the instant they became knowable, then stream,
//! then the venue's own sequence, then object id; orders by arrival, then
//! id. Nothing here reads a clock or draws a random number. Every applied
//! step folds into a rolling SHA-256 (`ReplayRun::digest`), so two runs that
//! differ in one event's order, one integrity flag or one fill differ in
//! the digest. [`ReplaySession::checkpoint`] captures everything the session
//! holds — books, integrity state, working orders, plans, executions so
//! far — and [`ReplaySession::resume`] continues from it to the same final
//! run. A checkpoint names the digest of its input, and resuming against
//! different ticks, orders or execution settings is refused.
//!
//! # A span the replay cannot vouch for is flagged, on the output
//!
//! A sequence gap on a stream — of any width, including one message — or a
//! message arriving behind one already applied opens an [`UnreliableSpan`]
//! that closes only at that stream's next `Reset`. While it is open, book
//! edits on the stream are not applied (the book they would edit is already
//! wrong), every step is flagged, and **no order is filled against that
//! book**: a fill credited against a book known to be wrong is a fill that
//! never existed. Trades still apply; a gap in depth does not un-print a
//! trade. This closes the one-packet hole `qip_orderbook::replay` records as
//! an open defect, for every consumer of this module.
//!
//! # Paper, and only paper
//!
//! Nothing here can reach a venue. An execution in this module is a
//! simulated outcome over recorded history and is typed as one.

use crate::conditions::MessageBudget;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::VenueStatus;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Decimal, Duration, Timestamp};
use qip_orderbook::{BookKind, BookView, VenueCheckpoint, VenueState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most legs a plan may carry, matching `qip-mesh`'s `MAX_LEGS` and
/// `qip-arbitrage`'s ceiling: a cycle longer than this is refused whole
/// everywhere else, and a replay that accepted one would score a plan the
/// platform cannot send.
pub const MAX_LEGS: usize = 20;

/// The fewest legs a multi-leg plan may carry. One leg is an order.
pub const MIN_LEGS: usize = 2;

/// Levels walked when measuring the visible depth an order's impact is
/// scaled by. Deep enough for a sweep, bounded so a pathological book does
/// not make one step quadratic.
const IMPACT_DEPTH_LEVELS: usize = 50;

/// One recorded message and the instant it became knowable to the platform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayTick {
    pub message: MarketMessage,
    pub knowable_at: Timestamp,
}

/// Which way an order trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    /// The book side a marketable order consumes.
    pub const fn takes(self) -> BookSide {
        match self {
            Self::Buy => BookSide::Ask,
            Self::Sell => BookSide::Bid,
        }
    }

    /// The book side a resting order joins.
    pub const fn rests_on(self) -> BookSide {
        match self {
            Self::Buy => BookSide::Bid,
            Self::Sell => BookSide::Ask,
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

/// The execution realism a replay applies. Every field is required and none
/// may be zeroed where zero is a claim about the world nobody can make.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionModel {
    /// Decision to arrival at the venue. Refused at zero: an order that
    /// meets the book at the instant it was decided trades on a book the
    /// platform had not finished reading.
    pub latency: Duration,
    /// Fee charged on traded notional, in basis points. A statistic until it
    /// is applied to a `Decimal` notional by `checked_apply_bps`.
    pub fee_bps: f64,
    /// Adverse price move, in basis points, for taking the whole visible
    /// depth on the far side; scaled linearly by the share actually taken.
    pub impact_bps_at_full_depth: f64,
    /// How long a resting limit order works before it is withdrawn.
    pub limit_ttl: Duration,
    /// The venue's order-message rate limit. An order arriving when the
    /// trailing window is full is refused with a throttle reason rather than
    /// filled: a replay that ignored the limit would credit the burstiest
    /// strategy with fills the venue never gave. `None` states that no limit
    /// is modelled, and is omitted from the serialised model so the input
    /// digest of a run recorded before the field existed is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_budget: Option<MessageBudget>,
}

impl ExecutionModel {
    /// Refuse a model that would credit fills the market did not offer.
    pub fn validate(&self) -> Result<()> {
        if self.latency <= Duration::ZERO {
            return Err(Error::invalid(
                "replay latency must be positive; a zero latency trades on a book before the \
                 decision that reads it is finished",
            ));
        }
        if !self.fee_bps.is_finite() || self.fee_bps < 0.0 {
            return Err(Error::invalid(format!(
                "replay fee of {} bps is not a finite non-negative rate",
                self.fee_bps
            )));
        }
        if !self.impact_bps_at_full_depth.is_finite() || self.impact_bps_at_full_depth < 0.0 {
            return Err(Error::invalid(format!(
                "replay impact of {} bps is not a finite non-negative rate",
                self.impact_bps_at_full_depth
            )));
        }
        if self.limit_ttl <= Duration::ZERO {
            return Err(Error::invalid(
                "a resting order needs a positive time to live; zero withdraws it before it can \
                 queue",
            ));
        }
        if let Some(budget) = &self.message_budget {
            budget.validate()?;
        }
        Ok(())
    }
}

/// One order the platform decided, to be replayed against history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayOrder {
    pub id: String,
    /// The instrument's object id, as the recorded messages carry it.
    pub instrument: String,
    pub side: OrderSide,
    pub quantity: Decimal,
    /// `None` is a market order; a limit order that crosses the touch on
    /// arrival takes what is there up to its limit and rests the rest.
    pub limit: Option<Decimal>,
    /// The instant the decision was made. Arrival is this plus latency.
    pub decided_at: Timestamp,
}

impl ReplayOrder {
    fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.instrument.trim().is_empty() {
            return Err(Error::invalid(
                "a replayed order names its id and its instrument",
            ));
        }
        if !self.quantity.is_positive() {
            return Err(Error::invalid(format!(
                "replayed order {} has quantity {}; it must be positive",
                self.id, self.quantity
            )));
        }
        if self.limit.is_some_and(|limit| !limit.is_positive()) {
            return Err(Error::invalid(format!(
                "replayed order {} has a non-positive limit",
                self.id
            )));
        }
        Ok(())
    }
}

/// One leg of a multi-leg plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegPlan {
    pub instrument: String,
    pub side: OrderSide,
    pub quantity: Decimal,
    /// A leg is immediate-or-cancel: it takes what its limit allows on
    /// arrival and never rests, because a leg left resting is the unhedged
    /// exposure a multi-leg plan exists to avoid.
    pub limit: Option<Decimal>,
    /// The region whose capital the leg consumes.
    pub region: String,
}

/// A 2–20 leg plan, sent one leg at a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MultiLegPlan {
    pub id: String,
    pub decided_at: Timestamp,
    pub legs: Vec<LegPlan>,
    /// Time between one leg's arrival and the next leg's decision: the
    /// legging window in which the earlier legs are open and the later ones
    /// not yet sent.
    pub leg_interval: Duration,
}

impl MultiLegPlan {
    fn validate(&self) -> Result<()> {
        if self.legs.len() < MIN_LEGS || self.legs.len() > MAX_LEGS {
            return Err(Error::invalid(format!(
                "multi-leg plan {} has {} leg(s); a plan carries {MIN_LEGS} to {MAX_LEGS}",
                self.id,
                self.legs.len()
            )));
        }
        if self.leg_interval < Duration::ZERO {
            return Err(Error::invalid(format!(
                "multi-leg plan {} has a negative leg interval",
                self.id
            )));
        }
        for leg in &self.legs {
            if !leg.quantity.is_positive() || leg.limit.is_some_and(|l| !l.is_positive()) {
                return Err(Error::invalid(format!(
                    "multi-leg plan {} has a leg with a non-positive quantity or limit",
                    self.id
                )));
            }
            if leg.region.trim().is_empty() || leg.instrument.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "multi-leg plan {} has a leg naming no region or instrument; capital is \
                     charged by region",
                    self.id
                )));
            }
        }
        Ok(())
    }
}

/// How an execution ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ExecutionStatus {
    Filled,
    /// Some quantity filled and the remainder was cancelled or expired.
    PartiallyFilled,
    /// Nothing filled, and why.
    Rejected {
        reason: String,
    },
    /// A resting order's time to live elapsed with nothing filled.
    Expired,
}

/// One simulated execution over recorded history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayExecution {
    pub order_id: String,
    pub instrument: String,
    pub side: OrderSide,
    pub requested: Decimal,
    pub decided_at: Timestamp,
    /// When the order met the book; `None` if history ended first.
    pub arrived_at: Option<Timestamp>,
    pub filled: Decimal,
    /// Notional of the fills, impact included, before fees.
    pub notional: Decimal,
    pub fees: Decimal,
    /// The share of `notional` that is impact.
    pub impact_cost: Decimal,
    /// The mid when the order met the book.
    pub arrival_mid: Option<Decimal>,
    /// Size resting ahead of the order at its price when it joined the queue.
    pub queue_ahead_on_arrival: Option<Decimal>,
    pub status: ExecutionStatus,
}

impl ReplayExecution {
    /// Volume-weighted fill price, impact included. `None` when nothing
    /// filled — never zero, which would read as free.
    pub fn average_price(&self) -> Option<Decimal> {
        self.notional.checked_div(self.filled)
    }

    /// Cost against the arrival mid, in basis points, positive when worse
    /// for the platform. A statistic.
    pub fn slippage_bps(&self) -> Option<f64> {
        let mid = self.arrival_mid?.to_f64();
        let average = self.average_price()?.to_f64();
        if mid <= 0.0 {
            return None;
        }
        let signed = match self.side {
            OrderSide::Buy => average - mid,
            OrderSide::Sell => mid - average,
        };
        Some(signed / mid * 10_000.0)
    }
}

/// What a multi-leg plan came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MultiLegOutcome {
    pub plan_id: String,
    /// One execution per leg sent, in leg order.
    pub legs: Vec<ReplayExecution>,
    /// Whether every leg filled in full.
    pub completed: bool,
    /// The first leg that did not fill in full.
    pub failed_leg: Option<usize>,
    /// The orders that flattened the legs left open by a failure.
    pub unwinds: Vec<ReplayExecution>,
    /// Signed cash of entering and unwinding the open legs, fees included:
    /// negative is what the failure cost.
    pub unwind_pnl: Decimal,
    /// Quantity per instrument the unwinds could not flatten — exposure the
    /// replay ends holding, named rather than netted away.
    pub unhedged: BTreeMap<String, Decimal>,
    /// Adverse touch movement between the plan's decision and each sent
    /// leg's arrival, summed over legs, in basis points. A statistic.
    pub path_decay_bps: f64,
    /// Peak gross notional the plan held open, per region.
    pub capital_by_region: BTreeMap<String, Decimal>,
}

/// A span of history the replay could not vouch for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreliableSpan {
    pub stream: String,
    /// `gap` or `out_of_order`.
    pub fault: String,
    /// First missing sequence for a gap; the late sequence for out-of-order.
    pub first_sequence: u64,
    /// Last missing sequence for a gap; equal to `first_sequence` otherwise.
    pub last_sequence: u64,
    /// Knowable instant of the first step inside the span.
    pub from: Timestamp,
    /// Knowable instant of the `Reset` that closed it; `None` if history
    /// ended with the span still open.
    pub until: Option<Timestamp>,
    /// Steps replayed while it was open.
    pub steps: u64,
}

/// What a whole replay produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayRun {
    /// Digest of the input — ticks in replay order, orders, plans and the
    /// execution model.
    pub input_digest: String,
    /// Rolling digest of every step: order, stream, sequence and flag.
    pub digest: String,
    pub steps: u64,
    /// Indices (in replay order) of every step replayed inside an
    /// unreliable span.
    pub unreliable_steps: Vec<u64>,
    pub spans: Vec<UnreliableSpan>,
    /// Single orders, in the order they resolved.
    pub executions: Vec<ReplayExecution>,
    pub plans: Vec<MultiLegOutcome>,
    /// The final book of each instrument, as a snapshot digest.
    pub books: BTreeMap<String, String>,
}

/// Why a pending order exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Origin {
    Single,
    Leg { plan: usize, leg: usize },
    Unwind { plan: usize },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    order: ReplayOrder,
    arrival: Timestamp,
    origin: Origin,
    /// Legs and unwinds take liquidity and never rest.
    immediate: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Working {
    execution: ReplayExecution,
    limit: Decimal,
    remaining: Decimal,
    queue_ahead: Decimal,
    expires_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PlanState {
    plan: MultiLegPlan,
    /// The far touch of each leg's instrument when the plan was decided.
    touch_at_decision: BTreeMap<usize, Decimal>,
    legs: Vec<ReplayExecution>,
    unwinds: Vec<ReplayExecution>,
    failed_leg: Option<usize>,
    open_capital: BTreeMap<String, Decimal>,
    peak_capital: BTreeMap<String, Decimal>,
    path_decay_bps: f64,
    unwinds_outstanding: usize,
    done: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct OpenSpan {
    span: UnreliableSpan,
}

/// Everything a session holds, as plain data a replay can resume from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayCheckpoint {
    input_digest: String,
    position: usize,
    digest: String,
    books: BTreeMap<String, VenueCheckpoint>,
    last_sequence: BTreeMap<String, u64>,
    open: BTreeMap<String, OpenSpan>,
    spans: Vec<UnreliableSpan>,
    unreliable_steps: Vec<u64>,
    pending: Vec<Pending>,
    working: Vec<Working>,
    executions: Vec<ReplayExecution>,
    plans: Vec<PlanState>,
    next_seq: u64,
    #[serde(default)]
    admitted: Vec<Timestamp>,
}

impl ReplayCheckpoint {
    /// Steps already replayed when the checkpoint was taken.
    pub const fn position(&self) -> usize {
        self.position
    }
}

/// A replay in progress. See the module doc.
#[derive(Debug)]
pub struct ReplaySession {
    model: ExecutionModel,
    ticks: Vec<ReplayTick>,
    kinds: BTreeMap<String, BookKind>,
    input_digest: String,
    position: usize,
    digest: String,
    books: BTreeMap<String, VenueState>,
    last_sequence: BTreeMap<String, u64>,
    open: BTreeMap<String, OpenSpan>,
    spans: Vec<UnreliableSpan>,
    unreliable_steps: Vec<u64>,
    pending: Vec<Pending>,
    working: Vec<Working>,
    executions: Vec<ReplayExecution>,
    plans: Vec<PlanState>,
    next_seq: u64,
    /// Arrival instants of orders the venue's message budget admitted, inside
    /// the trailing window only; older ones are dropped as orders arrive, so
    /// this holds at most `per_window` entries.
    admitted: Vec<Timestamp>,
}

fn checked(value: Option<Decimal>, what: &str) -> Result<Decimal> {
    value.ok_or_else(|| Error::numeric(format!("{what} overflowed the fixed-point range")))
}

fn is_book_edit(body: &MessageBody) -> bool {
    matches!(
        body,
        MessageBody::OrderAdded { .. }
            | MessageBody::OrderReduced { .. }
            | MessageBody::OrderRemoved { .. }
            | MessageBody::OrderReplaced { .. }
            | MessageBody::LevelSet { .. }
            | MessageBody::Quote { .. }
    )
}

fn is_order_level(body: &MessageBody) -> bool {
    matches!(
        body,
        MessageBody::OrderAdded { .. }
            | MessageBody::OrderReduced { .. }
            | MessageBody::OrderRemoved { .. }
            | MessageBody::OrderReplaced { .. }
    )
}

/// Sort ticks into replay order: knowable instant, stream, venue sequence,
/// object id. A total order over the fields that identify a message, so two
/// replays of one history cannot disagree about it.
pub fn sequence_ticks(mut ticks: Vec<ReplayTick>) -> Vec<ReplayTick> {
    ticks.sort_by(|a, b| {
        a.knowable_at
            .cmp(&b.knowable_at)
            .then_with(|| {
                a.message
                    .origin
                    .stream_key()
                    .cmp(&b.message.origin.stream_key())
            })
            .then_with(|| a.message.origin.sequence.cmp(&b.message.origin.sequence))
            .then_with(|| {
                a.message
                    .object_id
                    .as_str()
                    .cmp(b.message.object_id.as_str())
            })
    });
    ticks
}

fn input_digest(
    ticks: &[ReplayTick],
    orders: &[ReplayOrder],
    plans: &[MultiLegPlan],
    model: &ExecutionModel,
) -> Result<String> {
    let value = serde_json::json!({
        "ticks": ticks,
        "orders": orders,
        "plans": plans,
        "model": model,
    });
    Ok(sha256_hex(
        qip_core::canonical::canonical_json(&value).as_bytes(),
    ))
}

impl ReplaySession {
    /// A session over `ticks` (any order; sequenced here) executing `orders`
    /// and `plans` under `model`.
    pub fn new(
        model: ExecutionModel,
        ticks: Vec<ReplayTick>,
        orders: Vec<ReplayOrder>,
        plans: Vec<MultiLegPlan>,
    ) -> Result<Self> {
        model.validate()?;
        let mut ids = BTreeSet::new();
        for order in &orders {
            order.validate()?;
            if !ids.insert(order.id.clone()) {
                return Err(Error::invalid(format!(
                    "two replayed orders share id {}; an execution must name one decision",
                    order.id
                )));
            }
        }
        for plan in &plans {
            plan.validate()?;
        }
        let ticks = sequence_ticks(ticks);
        let digest = input_digest(&ticks, &orders, &plans, &model)?;
        let mut kinds: BTreeMap<String, BookKind> = BTreeMap::new();
        for tick in &ticks {
            let key = tick.message.object_id.as_str().to_string();
            let entry = kinds.entry(key).or_insert(BookKind::Aggregated);
            if is_order_level(&tick.message.body) {
                *entry = BookKind::OrderByOrder;
            }
        }
        let mut session = Self {
            model,
            ticks,
            kinds,
            input_digest: digest.clone(),
            position: 0,
            digest: digest.clone(),
            books: BTreeMap::new(),
            last_sequence: BTreeMap::new(),
            open: BTreeMap::new(),
            spans: Vec::new(),
            unreliable_steps: Vec::new(),
            pending: Vec::new(),
            working: Vec::new(),
            executions: Vec::new(),
            plans: Vec::new(),
            next_seq: 0,
            admitted: Vec::new(),
        };
        let mut orders = orders;
        orders.sort_by(|a, b| a.decided_at.cmp(&b.decided_at).then(a.id.cmp(&b.id)));
        for order in orders {
            let arrival = order.decided_at.saturating_add(session.model.latency);
            session.pending.push(Pending {
                order,
                arrival,
                origin: Origin::Single,
                immediate: false,
            });
        }
        for (index, plan) in plans.into_iter().enumerate() {
            let first = Self::leg_order(&plan, 0, plan.decided_at);
            let arrival = plan.decided_at.saturating_add(session.model.latency);
            session.pending.push(Pending {
                order: first,
                arrival,
                origin: Origin::Leg {
                    plan: index,
                    leg: 0,
                },
                immediate: true,
            });
            session.plans.push(PlanState {
                plan,
                touch_at_decision: BTreeMap::new(),
                legs: Vec::new(),
                unwinds: Vec::new(),
                failed_leg: None,
                open_capital: BTreeMap::new(),
                peak_capital: BTreeMap::new(),
                path_decay_bps: 0.0,
                unwinds_outstanding: 0,
                done: false,
            });
        }
        Ok(session)
    }

    /// Continue a session from `checkpoint`, refusing an input that is not
    /// the one the checkpoint was taken over.
    pub fn resume(
        model: ExecutionModel,
        ticks: Vec<ReplayTick>,
        orders: Vec<ReplayOrder>,
        plans: Vec<MultiLegPlan>,
        checkpoint: &ReplayCheckpoint,
    ) -> Result<Self> {
        let mut session = Self::new(model, ticks, orders, plans)?;
        if session.input_digest != checkpoint.input_digest {
            return Err(Error::invalid(
                "the checkpoint was taken over different ticks, orders or execution settings; \
                 resuming it here would splice two histories",
            ));
        }
        if checkpoint.position > session.ticks.len() {
            return Err(Error::invalid(
                "the checkpoint is past the end of the history it names",
            ));
        }
        session.position = checkpoint.position;
        session.digest = checkpoint.digest.clone();
        session.books = checkpoint
            .books
            .iter()
            .map(|(k, c)| Ok((k.clone(), VenueState::restore(c)?)))
            .collect::<Result<_>>()?;
        session.last_sequence = checkpoint.last_sequence.clone();
        session.open = checkpoint.open.clone();
        session.spans = checkpoint.spans.clone();
        session.unreliable_steps = checkpoint.unreliable_steps.clone();
        session.pending = checkpoint.pending.clone();
        session.working = checkpoint.working.clone();
        session.executions = checkpoint.executions.clone();
        session.plans = checkpoint.plans.clone();
        session.next_seq = checkpoint.next_seq;
        session.admitted = checkpoint.admitted.clone();
        Ok(session)
    }

    /// Everything needed to resume from here.
    pub fn checkpoint(&self) -> ReplayCheckpoint {
        ReplayCheckpoint {
            input_digest: self.input_digest.clone(),
            position: self.position,
            digest: self.digest.clone(),
            books: self
                .books
                .iter()
                .map(|(k, s)| (k.clone(), s.checkpoint()))
                .collect(),
            last_sequence: self.last_sequence.clone(),
            open: self.open.clone(),
            spans: self.spans.clone(),
            unreliable_steps: self.unreliable_steps.clone(),
            pending: self.pending.clone(),
            working: self.working.clone(),
            executions: self.executions.clone(),
            plans: self.plans.clone(),
            next_seq: self.next_seq,
            admitted: self.admitted.clone(),
        }
    }

    /// Steps replayed so far.
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Steps in the history.
    pub fn len(&self) -> usize {
        self.ticks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ticks.is_empty()
    }

    /// The rebuilt state of `instrument` as of the last step, for a feature
    /// or label job reading the book the replay is holding.
    pub fn book(&self, instrument: &str) -> Option<&VenueState> {
        self.books.get(instrument)
    }

    /// Whether `instrument`'s stream is inside an unreliable span now.
    pub fn is_unreliable(&self, instrument: &str) -> bool {
        self.books.get(instrument).is_some_and(|_| {
            self.open
                .values()
                .any(|open| open.span.stream.ends_with(&format!("#{instrument}")))
        })
    }

    fn leg_order(plan: &MultiLegPlan, leg: usize, decided_at: Timestamp) -> ReplayOrder {
        let spec = &plan.legs[leg];
        ReplayOrder {
            id: format!("{}#leg{leg}", plan.id),
            instrument: spec.instrument.clone(),
            side: spec.side,
            quantity: spec.quantity,
            limit: spec.limit,
            decided_at,
        }
    }

    /// Apply one step. Returns `false` once history is exhausted.
    pub fn step(&mut self) -> Result<bool> {
        let Some(tick) = self.ticks.get(self.position).cloned() else {
            return Ok(false);
        };
        // Orders that arrived before this message became knowable meet the
        // book as it stood before it.
        self.release_pending(Some(tick.knowable_at))?;
        self.expire_working(tick.knowable_at)?;

        let instrument = tick.message.object_id.as_str().to_string();
        // A stream is keyed per instrument as well as per feed partition, so
        // a gap on one instrument's stream flags that instrument.
        let stream = format!("{}#{instrument}", tick.message.origin.stream_key());
        let sequence = tick.message.origin.sequence;
        let is_reset = matches!(tick.message.body, MessageBody::Reset { .. });

        if is_reset {
            if let Some(mut open) = self.open.remove(&stream) {
                open.span.until = Some(tick.knowable_at);
                self.spans.push(open.span);
            }
            self.last_sequence.insert(stream.clone(), sequence);
        } else {
            match self.last_sequence.get(&stream).copied() {
                Some(previous) if sequence > previous.saturating_add(1) => {
                    self.open.entry(stream.clone()).or_insert(OpenSpan {
                        span: UnreliableSpan {
                            stream: stream.clone(),
                            fault: "gap".to_string(),
                            first_sequence: previous + 1,
                            last_sequence: sequence - 1,
                            from: tick.knowable_at,
                            until: None,
                            steps: 0,
                        },
                    });
                    self.last_sequence.insert(stream.clone(), sequence);
                }
                Some(previous) if sequence < previous => {
                    self.open.entry(stream.clone()).or_insert(OpenSpan {
                        span: UnreliableSpan {
                            stream: stream.clone(),
                            fault: "out_of_order".to_string(),
                            first_sequence: sequence,
                            last_sequence: sequence,
                            from: tick.knowable_at,
                            until: None,
                            steps: 0,
                        },
                    });
                }
                _ => {
                    self.last_sequence.insert(stream.clone(), sequence);
                }
            }
        }
        let unreliable = if let Some(open) = self.open.get_mut(&stream) {
            open.span.steps += 1;
            true
        } else {
            false
        };

        let kind = self
            .kinds
            .get(&instrument)
            .copied()
            .unwrap_or(BookKind::Aggregated);
        let state = self.books.entry(instrument.clone()).or_insert_with(|| {
            let venue = tick.message.origin.venue.clone();
            let object = tick.message.object_id.clone();
            match kind {
                BookKind::OrderByOrder => {
                    VenueState::order_by_order(object, venue, VenueStatus::Open)
                }
                _ => VenueState::aggregated(object, venue, VenueStatus::Open),
            }
        });
        if !(unreliable && is_book_edit(&tick.message.body)) {
            state.apply(&tick.message)?;
        }
        if is_reset {
            state.resynchronised(tick.message.venue_time);
        }

        let index = self.position as u64;
        if unreliable {
            self.unreliable_steps.push(index);
        }
        let line = format!(
            "{}|{index}|{stream}|{sequence}|{}|{unreliable}",
            self.digest,
            tick.knowable_at.as_nanos()
        );
        self.digest = sha256_hex(line.as_bytes());

        self.work_resting(&tick, &instrument)?;
        self.position += 1;
        Ok(true)
    }

    /// Run to the end of history and produce the run.
    pub fn run_to_end(mut self) -> Result<ReplayRun> {
        while self.step()? {}
        self.finish()
    }

    fn finish(mut self) -> Result<ReplayRun> {
        // Whatever has not arrived when history ends never met a book.
        self.release_pending(None)?;
        let working = std::mem::take(&mut self.working);
        for mut work in working {
            work.execution.status = if work.execution.filled.is_positive() {
                ExecutionStatus::PartiallyFilled
            } else {
                ExecutionStatus::Expired
            };
            self.executions.push(work.execution);
        }
        for open in std::mem::take(&mut self.open).into_values() {
            self.spans.push(open.span);
        }
        let mut plans = Vec::new();
        for state in &self.plans {
            plans.push(Self::outcome_of(state)?);
        }
        let books = self
            .books
            .iter()
            .map(|(k, s)| (k.clone(), s.snapshot().digest()))
            .collect();
        Ok(ReplayRun {
            input_digest: self.input_digest,
            digest: self.digest,
            steps: self.position as u64,
            unreliable_steps: self.unreliable_steps,
            spans: self.spans,
            executions: self.executions,
            plans,
            books,
        })
    }

    fn outcome_of(state: &PlanState) -> Result<MultiLegOutcome> {
        let completed = state.legs.len() == state.plan.legs.len()
            && state
                .legs
                .iter()
                .all(|leg| leg.status == ExecutionStatus::Filled);
        // Cash of every leg and unwind: a buy pays, a sell receives, fees
        // are paid either way. Only meaningful for a failed plan; a
        // completed one is an open position the plan meant to hold.
        let mut unwind_pnl = Decimal::ZERO;
        let mut unhedged: BTreeMap<String, Decimal> = BTreeMap::new();
        if !completed {
            for execution in state.legs.iter().chain(state.unwinds.iter()) {
                let cash = match execution.side {
                    OrderSide::Buy => -execution.notional,
                    OrderSide::Sell => execution.notional,
                };
                unwind_pnl = checked(unwind_pnl.checked_add(cash), "unwind cash")?;
                unwind_pnl = checked(unwind_pnl.checked_sub(execution.fees), "unwind fees")?;
                let signed = match execution.side {
                    OrderSide::Buy => execution.filled,
                    OrderSide::Sell => -execution.filled,
                };
                let entry = unhedged
                    .entry(execution.instrument.clone())
                    .or_insert(Decimal::ZERO);
                *entry = checked(entry.checked_add(signed), "unhedged quantity")?;
            }
            unhedged.retain(|_, q| !q.is_zero());
        }
        Ok(MultiLegOutcome {
            plan_id: state.plan.id.clone(),
            legs: state.legs.clone(),
            completed,
            failed_leg: state.failed_leg,
            unwinds: state.unwinds.clone(),
            unwind_pnl,
            unhedged,
            path_decay_bps: state.path_decay_bps,
            capital_by_region: state.peak_capital.clone(),
        })
    }

    /// Release every pending order arriving at or before `until` (all of
    /// them when `None`), in arrival order, re-scanning because a leg's
    /// outcome can schedule the next leg or an unwind.
    fn release_pending(&mut self, until: Option<Timestamp>) -> Result<()> {
        loop {
            let due = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, p)| until.is_none_or(|t| p.arrival <= t))
                .min_by(|(_, a), (_, b)| {
                    a.arrival
                        .cmp(&b.arrival)
                        .then_with(|| a.order.id.cmp(&b.order.id))
                })
                .map(|(i, _)| i);
            let Some(index) = due else {
                return Ok(());
            };
            let pending = self.pending.remove(index);
            self.next_seq += 1;
            let execution = if until.is_none() {
                Some(Self::rejected(
                    &pending.order,
                    None,
                    "history ended before the order arrived at the venue",
                ))
            } else if let Some(reason) = self.throttled(pending.arrival) {
                Some(Self::rejected(
                    &pending.order,
                    Some(pending.arrival),
                    &reason,
                ))
            } else {
                self.meet_book(&pending)?
            };
            self.resolve(pending, execution)?;
        }
    }

    /// The venue's answer to an order message arriving at `arrival` under the
    /// model's message budget: `Some(reason)` when the trailing window is
    /// full, and otherwise the arrival is counted against the window.
    ///
    /// Arrivals are released in time order, so the window only moves forward
    /// and entries that fell out of it are dropped here, keeping the set
    /// bounded by the budget itself.
    fn throttled(&mut self, arrival: Timestamp) -> Option<String> {
        let budget = self.model.message_budget?;
        let floor = budget.trailing_floor(arrival);
        self.admitted.retain(|sent| sent.as_nanos() > floor);
        if budget.admits(&self.admitted, arrival) {
            self.admitted.push(arrival);
            return None;
        }
        Some(format!(
            "throttled: the venue's message budget of {} per {:?} was already spent in the \
             trailing window",
            budget.per_window, budget.window
        ))
    }

    fn rejected(
        order: &ReplayOrder,
        arrived_at: Option<Timestamp>,
        reason: &str,
    ) -> ReplayExecution {
        ReplayExecution {
            order_id: order.id.clone(),
            instrument: order.instrument.clone(),
            side: order.side,
            requested: order.quantity,
            decided_at: order.decided_at,
            arrived_at,
            filled: Decimal::ZERO,
            notional: Decimal::ZERO,
            fees: Decimal::ZERO,
            impact_cost: Decimal::ZERO,
            arrival_mid: None,
            queue_ahead_on_arrival: None,
            status: ExecutionStatus::Rejected {
                reason: reason.to_string(),
            },
        }
    }

    fn fee_on(&self, notional: Decimal) -> Result<Decimal> {
        checked(notional.checked_apply_bps(self.model.fee_bps), "fee")
    }

    /// The order meets the book as it stands. Marketable quantity is taken
    /// at once; what is left of a limit order rests unless it is immediate.
    fn meet_book(&mut self, pending: &Pending) -> Result<Option<ReplayExecution>> {
        let order = &pending.order;
        let arrived = pending.arrival;
        if self.is_unreliable(&order.instrument) {
            return Ok(Some(Self::rejected(
                order,
                Some(arrived),
                "the book's reconstruction is unreliable at arrival; no fill is credited \
                 against a book known to be wrong",
            )));
        }
        let Some(state) = self.books.get(&order.instrument) else {
            return Ok(Some(Self::rejected(
                order,
                Some(arrived),
                "no recorded book for the instrument at arrival",
            )));
        };
        if !state.prices_are_usable() {
            return Ok(Some(Self::rejected(
                order,
                Some(arrived),
                "the venue was not trading, or its book awaited a snapshot, at arrival",
            )));
        }
        let mid = state.mid();
        let taking = order.side.takes();
        let book = state.book();
        let available = match order.limit {
            None => order.quantity,
            Some(limit) => book.depth_to(taking, limit).min(order.quantity),
        };
        let visible: Decimal = book
            .levels(taking, IMPACT_DEPTH_LEVELS)
            .iter()
            .map(|level| level.size)
            .sum();
        let mut execution = ReplayExecution {
            order_id: order.id.clone(),
            instrument: order.instrument.clone(),
            side: order.side,
            requested: order.quantity,
            decided_at: order.decided_at,
            arrived_at: Some(arrived),
            filled: Decimal::ZERO,
            notional: Decimal::ZERO,
            fees: Decimal::ZERO,
            impact_cost: Decimal::ZERO,
            arrival_mid: mid,
            queue_ahead_on_arrival: None,
            status: ExecutionStatus::Filled,
        };
        if available.is_positive() {
            let sweep = book.sweep_cost(taking, available);
            if sweep.filled.is_positive() {
                // Statistics: the share of visible depth taken scales the
                // impact rate; the rate crosses back to Decimal through
                // checked_apply_bps on the swept notional.
                let share = if visible.is_positive() {
                    (sweep.filled.to_f64() / visible.to_f64()).min(1.0)
                } else {
                    1.0
                };
                let impact_bps = self.model.impact_bps_at_full_depth * share;
                let impact = checked(sweep.notional.checked_apply_bps(impact_bps), "impact")?;
                let notional = match order.side {
                    OrderSide::Buy => checked(sweep.notional.checked_add(impact), "notional")?,
                    OrderSide::Sell => checked(sweep.notional.checked_sub(impact), "notional")?,
                };
                execution.filled = sweep.filled;
                execution.notional = notional;
                execution.impact_cost = impact;
                execution.fees = self.fee_on(notional)?;
            }
        }
        let remaining = checked(
            order.quantity.checked_sub(execution.filled),
            "remaining quantity",
        )?;
        if !remaining.is_positive() {
            return Ok(Some(execution));
        }
        match (order.limit, pending.immediate) {
            (Some(limit), false) => {
                // Rest the remainder behind everything already at the price.
                let ahead = book
                    .walk(order.side.rests_on())
                    .find(|level| level.price == limit)
                    .map_or(Decimal::ZERO, |level| level.size);
                execution.queue_ahead_on_arrival = Some(ahead);
                self.working.push(Working {
                    execution,
                    limit,
                    remaining,
                    queue_ahead: ahead,
                    expires_at: arrived.saturating_add(self.model.limit_ttl),
                });
                Ok(None)
            }
            _ => {
                execution.status = if execution.filled.is_positive() {
                    ExecutionStatus::PartiallyFilled
                } else {
                    ExecutionStatus::Rejected {
                        reason: "the book offered nothing within the order's limit on arrival"
                            .to_string(),
                    }
                };
                Ok(Some(execution))
            }
        }
    }

    /// Advance resting orders on a trade print: a print through the limit
    /// fills what is left; a print at the limit first consumes the queue
    /// ahead. A level shrinking below the queue ahead advances the queue,
    /// and never further — cancellations are assumed to come from behind.
    fn work_resting(&mut self, tick: &ReplayTick, instrument: &str) -> Result<()> {
        if self.working.is_empty() {
            return Ok(());
        }
        let level_sizes: BTreeMap<(BookSide, Decimal), Decimal> = match self.books.get(instrument) {
            Some(state) => {
                let mut sizes = BTreeMap::new();
                for side in [BookSide::Bid, BookSide::Ask] {
                    for level in state.book().levels(side, IMPACT_DEPTH_LEVELS) {
                        sizes.insert((side, level.price), level.size);
                    }
                }
                sizes
            }
            None => BTreeMap::new(),
        };
        let mut still = Vec::new();
        for mut work in std::mem::take(&mut self.working) {
            if work.execution.instrument != instrument
                || work
                    .execution
                    .arrived_at
                    .is_some_and(|at| tick.knowable_at < at)
            {
                still.push(work);
                continue;
            }
            let side = work.execution.side;
            let level = level_sizes
                .get(&(side.rests_on(), work.limit))
                .copied()
                .unwrap_or(Decimal::ZERO);
            if level < work.queue_ahead {
                work.queue_ahead = level;
            }
            if let MessageBody::Trade {
                price, quantity, ..
            } = &tick.message.body
            {
                let through = match side {
                    OrderSide::Buy => *price < work.limit,
                    OrderSide::Sell => *price > work.limit,
                };
                let take = if through {
                    work.remaining
                } else if *price == work.limit {
                    let past_queue =
                        checked(quantity.checked_sub(work.queue_ahead), "queue arithmetic")?;
                    work.queue_ahead =
                        checked(work.queue_ahead.checked_sub(*quantity), "queue arithmetic")?
                            .max(Decimal::ZERO);
                    past_queue.max(Decimal::ZERO).min(work.remaining)
                } else {
                    Decimal::ZERO
                };
                if take.is_positive() {
                    let notional = checked(take.checked_mul(work.limit), "fill notional")?;
                    let fee = self.fee_on(notional)?;
                    work.execution.filled =
                        checked(work.execution.filled.checked_add(take), "filled")?;
                    work.execution.notional =
                        checked(work.execution.notional.checked_add(notional), "notional")?;
                    work.execution.fees = checked(work.execution.fees.checked_add(fee), "fees")?;
                    work.remaining = checked(work.remaining.checked_sub(take), "remaining")?;
                }
            }
            if work.remaining.is_positive() {
                still.push(work);
            } else {
                work.execution.status = ExecutionStatus::Filled;
                self.executions.push(work.execution);
            }
        }
        self.working = still;
        Ok(())
    }

    fn expire_working(&mut self, now: Timestamp) -> Result<()> {
        let mut still = Vec::new();
        for mut work in std::mem::take(&mut self.working) {
            if now > work.expires_at {
                work.execution.status = if work.execution.filled.is_positive() {
                    ExecutionStatus::PartiallyFilled
                } else {
                    ExecutionStatus::Expired
                };
                self.executions.push(work.execution);
            } else {
                still.push(work);
            }
        }
        self.working = still;
        Ok(())
    }

    /// File an execution where its origin says, scheduling the next leg or
    /// the unwinds a failed leg leaves behind.
    fn resolve(&mut self, pending: Pending, execution: Option<ReplayExecution>) -> Result<()> {
        let Some(execution) = execution else {
            // Resting; filed when it fills or expires.
            return Ok(());
        };
        match pending.origin {
            Origin::Single => self.executions.push(execution),
            Origin::Unwind { plan } => {
                let state = &mut self.plans[plan];
                state.unwinds.push(execution);
                state.unwinds_outstanding = state.unwinds_outstanding.saturating_sub(1);
                state.done = state.unwinds_outstanding == 0;
            }
            Origin::Leg { plan, leg } => {
                let latency = self.model.latency;
                // The far touch at the plan's decision is read when the
                // first leg arrives — the earliest the replay holds a book
                // for every leg — and each leg's arrival touch against it is
                // the path decay the plan paid for legging.
                if leg == 0 {
                    let touches: BTreeMap<usize, Decimal> = self.plans[plan]
                        .plan
                        .legs
                        .iter()
                        .enumerate()
                        .filter_map(|(i, spec)| {
                            let state = self.books.get(&spec.instrument)?;
                            let level = match spec.side.takes() {
                                BookSide::Ask => state.best_ask(),
                                BookSide::Bid => state.best_bid(),
                            }?;
                            Some((i, level.price))
                        })
                        .collect();
                    self.plans[plan].touch_at_decision = touches;
                }
                let spec = self.plans[plan].plan.legs[leg].clone();
                if let (Some(before), Some(state)) = (
                    self.plans[plan].touch_at_decision.get(&leg).copied(),
                    self.books.get(&spec.instrument),
                ) {
                    let now = match spec.side.takes() {
                        BookSide::Ask => state.best_ask(),
                        BookSide::Bid => state.best_bid(),
                    };
                    if let Some(now) = now
                        && before.is_positive()
                    {
                        // Statistics: a touch move in basis points.
                        let moved =
                            (now.price.to_f64() - before.to_f64()) / before.to_f64() * 10_000.0;
                        let adverse = match spec.side {
                            OrderSide::Buy => moved,
                            OrderSide::Sell => -moved,
                        };
                        self.plans[plan].path_decay_bps += adverse;
                    }
                }
                let complete = execution.status == ExecutionStatus::Filled;
                let arrived = execution.arrived_at.unwrap_or(pending.arrival);
                let gross = execution.notional;
                {
                    let state = &mut self.plans[plan];
                    let open = state
                        .open_capital
                        .entry(spec.region.clone())
                        .or_insert(Decimal::ZERO);
                    *open = checked(open.checked_add(gross), "capital")?;
                    let open = *open;
                    let peak = state
                        .peak_capital
                        .entry(spec.region.clone())
                        .or_insert(Decimal::ZERO);
                    *peak = (*peak).max(open);
                    state.legs.push(execution);
                }
                let last = leg + 1 == self.plans[plan].plan.legs.len();
                if complete && !last {
                    let decided = arrived.saturating_add(self.plans[plan].plan.leg_interval);
                    let order = Self::leg_order(&self.plans[plan].plan, leg + 1, decided);
                    self.pending.push(Pending {
                        order,
                        arrival: decided.saturating_add(latency),
                        origin: Origin::Leg { plan, leg: leg + 1 },
                        immediate: true,
                    });
                } else if complete {
                    self.plans[plan].done = true;
                } else {
                    // The leg did not fill in full: everything the plan
                    // holds open is flattened at market, decided now.
                    let state = &mut self.plans[plan];
                    state.failed_leg = Some(leg);
                    let mut held: BTreeMap<String, (OrderSide, Decimal)> = BTreeMap::new();
                    for execution in &state.legs {
                        if !execution.filled.is_positive() {
                            continue;
                        }
                        let entry = held
                            .entry(execution.instrument.clone())
                            .or_insert((execution.side, Decimal::ZERO));
                        let signed = match execution.side {
                            OrderSide::Buy => execution.filled,
                            OrderSide::Sell => -execution.filled,
                        };
                        let net = match entry.0 {
                            OrderSide::Buy => entry.1,
                            OrderSide::Sell => -entry.1,
                        };
                        let net = checked(net.checked_add(signed), "held quantity")?;
                        *entry = if net.is_negative() {
                            (OrderSide::Sell, -net)
                        } else {
                            (OrderSide::Buy, net)
                        };
                    }
                    let id = state.plan.id.clone();
                    let mut unwinds = Vec::new();
                    for (instrument, (side, quantity)) in held {
                        if !quantity.is_positive() {
                            continue;
                        }
                        unwinds.push(Pending {
                            order: ReplayOrder {
                                id: format!("{id}#unwind-{instrument}"),
                                instrument,
                                side: side.opposite(),
                                quantity,
                                limit: None,
                                decided_at: arrived,
                            },
                            arrival: arrived.saturating_add(latency),
                            origin: Origin::Unwind { plan },
                            immediate: true,
                        });
                    }
                    state.unwinds_outstanding = unwinds.len();
                    state.done = unwinds.is_empty();
                    self.pending.extend(unwinds);
                }
            }
        }
        Ok(())
    }
}

/// Replay `ticks` with `orders` and `plans` from the start to the end.
pub fn replay(
    model: ExecutionModel,
    ticks: Vec<ReplayTick>,
    orders: Vec<ReplayOrder>,
    plans: Vec<MultiLegPlan>,
) -> Result<ReplayRun> {
    ReplaySession::new(model, ticks, orders, plans)?.run_to_end()
}
