//! The adapter port: everything a venue is asked to do, in one trait.
//!
//! [`VenueAdapter`] extends [`qip_execution_engine::broker::Broker`] rather than
//! competing with it. `Broker` is the narrow surface the order management
//! system needs — name, simulation flag, availability, submit, cancel — and it
//! stays exactly that, so every adapter here drops into the existing OMS
//! unchanged and inherits its controls. `VenueAdapter` adds what a real
//! connection has and a broker port does not: a session that has to be opened
//! and authenticated, market data, replaces, queries against the venue's own
//! books, a heartbeat, and a way to hang up.
//!
//! Three rules are load-bearing:
//!
//! * **There is no live venue.** Every adapter reports an [`AdapterClass`],
//!   and the enum has no `Live` variant — not disabled, not feature-gated,
//!   absent. The platform's autonomy ceiling is paper trading, and this is
//!   where that stops being a policy somebody enforces and becomes a thing
//!   that cannot be written down.
//! * **Order entry needs proof of readiness.** [`VenueAdapter::submit_order`] and
//!   [`VenueAdapter::replace_order`] take a [`ReadyTicket`], which only
//!   [`ConnectionState::ready_ticket`] can mint and only from a ready session.
//!   [`VenueAdapter::cancel_order`] does not, because cancelling reduces risk and the
//!   safe direction never waits for permission.
//! * **The simulation flag comes from the adapter, not the fill.** Every fill
//!   an adapter reports passes through [`stamp_simulated`], which overwrites
//!   whatever the venue said with what the adapter is. The order management
//!   system does the same thing again on the way past, and the belt and the
//!   braces are both deliberate: this bit decides whether a number is real
//!   money, and it is not something a venue message gets a vote on.

use crate::connection::{ConnectionPhase, ConnectionState, ReadyTicket, SessionHealth};
use crate::credential::{VenueCredential, VenueRequirement, describe_missing};
use qip_contracts::venue::VenueId;
use qip_core::error::Result;
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Currency, Decimal};
use qip_execution_engine::broker::Broker;
use qip_execution_engine::order::{Fill, Order, Side};
use qip_market::book::OrderBook;
use qip_market::quote::Quote;
use serde::{Deserialize, Serialize};

/// What kind of venue an adapter speaks to.
///
/// There are two variants and there is deliberately no third. A live venue is
/// not disabled here, not behind a feature flag and not guarded by a runtime
/// check that somebody could stop calling — it is *absent*, so the code that
/// would send a real order to a real exchange cannot be written against this
/// crate at all. `AdapterClass::Live` does not compile, and no configuration
/// file deserialises into it.
///
/// That is the whole control. A boolean `is_live` would have been one negation
/// away from being wrong; a variant that does not exist is not.
///
/// Should a real venue ever be added, [`crate::credential::standard_requirements`]
/// already names what it would take, including an operator enablement that is
/// separate from the credential — because holding a secret is not the same as
/// having decided to trade — and the refusal when any of it is missing is a
/// clean error naming every outstanding item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterClass {
    /// Matches in process, against a book this crate owns. No egress, no
    /// counterparty, no money.
    Simulated,
    /// The venue's own test environment: a real protocol against a real
    /// endpoint, settling nothing. Still not real money, and still not a
    /// variant that can become one.
    Sandbox,
}

impl AdapterClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Simulated => "simulated",
            Self::Sandbox => "sandbox",
        }
    }

    /// Whether fills from this class of adapter are paper.
    ///
    /// True for both variants, and it is not an oversight that nothing returns
    /// false: there is no class of adapter in this crate whose fills are real
    /// money. The method exists so the invariant is stated somewhere a test can
    /// read it rather than only implied by the enum being short.
    pub const fn is_paper(&self) -> bool {
        matches!(self, Self::Simulated | Self::Sandbox)
    }
}

/// The venue's answer to a heartbeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub venue: String,
    /// When the answer landed, which is the request time plus the round trip.
    pub at: Timestamp,
    pub round_trip: Duration,
    pub phase: ConnectionPhase,
    /// The session the heartbeat belongs to. A change here means the session
    /// was rebuilt, and anything held across it is stale.
    pub session: u64,
    /// Monotonic count of answers on this adapter, for gap detection.
    pub sequence: u64,
}

/// What the venue says an order is doing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VenueOrderState {
    /// At the venue, nothing filled.
    Working,
    /// At the venue, some filled.
    PartiallyFilled { filled: Decimal },
    /// Done.
    Filled,
    /// Withdrawn, by request or by the venue's own rules.
    Cancelled { reason: String },
    /// Refused. Nothing traded and nothing rests.
    Rejected { reason: String },
}

impl VenueOrderState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::PartiallyFilled { .. } => "partially_filled",
            Self::Filled => "filled",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
        }
    }

    /// Whether the venue could still trade it.
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Working | Self::PartiallyFilled { .. })
    }

    pub const fn is_terminal(&self) -> bool {
        !self.is_open()
    }
}

/// The venue's record of an order.
///
/// Identity survives a replace: `order_id` is the client's and never changes,
/// while `revision` counts the amendments and `priority` shows what they cost.
/// A venue that issued a new identifier on every amendment would make a
/// replaced order indistinguishable from a cancel and a resubmit, and the two
/// have very different fills.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueOrder {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    pub venue: String,
    pub side: Side,
    pub quantity: Decimal,
    pub filled: Decimal,
    pub limit: Option<Decimal>,
    pub state: VenueOrderState,
    /// Amendments applied. Zero for an order that was never replaced.
    pub revision: u32,
    /// The quantity the order was originally sent with.
    pub original_quantity: Decimal,
    pub submitted_at: Timestamp,
    pub updated_at: Timestamp,
    /// Queue sequence at the venue: lower is earlier. A replace that adds
    /// quantity or moves the price loses priority, and it shows here.
    pub priority: u64,
    pub simulated: bool,
}

impl VenueOrder {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// The venue's answer to an instruction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderAck {
    pub order_id: OrderId,
    pub venue: String,
    /// When the answer landed: the instruction time plus the round trip.
    pub at: Timestamp,
    pub latency: Duration,
    pub state: VenueOrderState,
    /// Fills this instruction produced, already stamped with the adapter's own
    /// simulation flag.
    pub fills: Vec<Fill>,
    pub remaining: Decimal,
    pub simulated: bool,
    pub detail: String,
}

impl OrderAck {
    pub fn filled_quantity(&self) -> Decimal {
        self.fills.iter().map(|fill| fill.quantity).sum()
    }

    pub fn total_costs(&self) -> Decimal {
        self.fills.iter().map(|fill| fill.costs).sum()
    }
}

/// A position as the venue sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub object_id: ObjectId,
    pub symbol: String,
    /// Signed: negative is short.
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub realised_pnl: Decimal,
    pub costs: Decimal,
    /// `None` when nothing has traded and there is no mark. A missing price is
    /// missing information; valuing it at zero understates risk exactly when it
    /// matters.
    pub mark: Option<Decimal>,
    pub market_value: Option<Decimal>,
    pub as_of: Timestamp,
}

/// Cash as the venue sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CashBalance {
    pub account: String,
    pub currency: Currency,
    /// Settled cash now.
    pub settled: Decimal,
    /// Cash before any fill, so a reconciliation has both ends of the identity.
    pub opening: Decimal,
    /// Commission and fees charged so far.
    pub costs: Decimal,
    pub realised_pnl: Decimal,
    /// The last fill or adjustment this balance accounts for.
    pub as_of: Timestamp,
}

impl CashBalance {
    /// Everything that has happened to cash since the account opened.
    pub fn movement(&self) -> Decimal {
        self.settled - self.opening
    }
}

/// Margin as the venue's risk desk computes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginState {
    pub at: Timestamp,
    pub cash: Decimal,
    pub position_value: Decimal,
    pub equity: Decimal,
    pub initial_requirement: Decimal,
    pub maintenance_requirement: Decimal,
    /// Equity less the initial requirement. Negative means no new risk.
    pub excess_liquidity: Decimal,
    /// Equity less the maintenance requirement. Negative is a margin call.
    pub maintenance_excess: Decimal,
    pub gross_exposure: Decimal,
    /// Gross exposure over equity — a statistic, so `f64`. Every amount above
    /// it is `Decimal`, because a margin call is decided to the cent.
    pub leverage: f64,
    /// Positions with no mark, excluded from the requirement rather than
    /// treated as free.
    pub unpriced: Vec<String>,
}

impl MarginState {
    /// Whether the account is under water on maintenance margin.
    pub fn is_call(&self) -> bool {
        self.maintenance_excess.is_negative()
    }

    /// Whether new risk may be opened.
    pub fn permits_new_risk(&self) -> bool {
        !self.excess_liquidity.is_negative()
    }
}

/// What the venue is showing for an instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketData {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    /// Depth, built from what is actually resting rather than asserted.
    pub book: OrderBook,
    /// The touch, when both sides are present.
    pub quote: Option<Quote>,
    /// Last traded price at this venue, if anything has traded.
    pub last_trade: Option<Decimal>,
    pub simulated: bool,
}

/// Overwrite every fill's simulation flag with the adapter's own.
///
/// The venue does not get a vote. A fill's `simulated` flag decides whether a
/// number is real money in every report and reconciliation downstream, and
/// taking it from the message would mean a mislabelled message could reclassify
/// paper as real. This is the same rule the order management system applies on
/// the way past; applying it at both ends is deliberate.
pub fn stamp_simulated(fills: &mut [Fill], simulated: bool) {
    for fill in fills.iter_mut() {
        fill.simulated = simulated;
    }
}

/// A venue, in full: session, market data, order entry, and the books.
pub trait VenueAdapter: Broker {
    /// The venue this adapter speaks for.
    fn venue_id(&self) -> &VenueId;

    /// What kind of venue this is. Never live; see [`AdapterClass`].
    fn class(&self) -> AdapterClass;

    /// The account orders are sent for, once authenticated.
    fn account(&self) -> Option<&str>;

    /// The live session and its health.
    fn connection(&self) -> &ConnectionState;

    /// Everything a production deployment of this venue has to supply.
    fn requirements(&self) -> Vec<VenueRequirement>;

    /// The subset still outstanding. Never empty for an adapter in this crate:
    /// a simulator is complete for what it is and incomplete as a venue.
    fn missing_requirements(&self) -> Vec<VenueRequirement>;

    /// Open the transport.
    fn connect(&mut self, at: Timestamp) -> Result<()>;

    /// Log on with the credential supplied. Never with a default.
    fn authenticate(&mut self, credential: &VenueCredential, at: Timestamp) -> Result<()>;

    /// Ask the venue whether it is still there.
    ///
    /// Answering promotes an authenticated session to ready and recovers a
    /// degraded one. Not answering is an error, and the session degrades once
    /// the gap exceeds the interval.
    fn heartbeat(&mut self, at: Timestamp) -> Result<Heartbeat>;

    /// What the venue is showing for an instrument.
    fn market_data(&self, object_id: &ObjectId, at: Timestamp) -> Result<MarketData>;

    /// Send an order. Requires proof the venue was ready at `at`.
    fn submit_order(
        &mut self,
        ticket: &ReadyTicket,
        order: &Order,
        at: Timestamp,
    ) -> Result<OrderAck>;

    /// Pull a working order. No ticket: cancelling reduces risk, and a degraded
    /// session is exactly when it is most needed.
    fn cancel_order(&mut self, order_id: &OrderId, at: Timestamp) -> Result<OrderAck>;

    /// Amend a working order, keeping its identity.
    ///
    /// `limit` of `None` leaves the price alone. Requires a ticket: an
    /// amendment can add quantity, which is new risk.
    fn replace_order(
        &mut self,
        ticket: &ReadyTicket,
        order_id: &OrderId,
        quantity: Decimal,
        limit: Option<Decimal>,
        at: Timestamp,
    ) -> Result<OrderAck>;

    /// The venue's record of one order.
    fn query_order(&self, order_id: &OrderId) -> Result<VenueOrder>;

    /// Every position on the account.
    fn query_positions(&self) -> Result<Vec<PositionSnapshot>>;

    /// Settled cash on the account.
    fn query_cash(&self) -> Result<CashBalance>;

    /// The margin position as of `at`, marked at the venue's own prices.
    fn query_margin(&self, at: Timestamp) -> Result<MarginState>;

    /// Fills since `since`, or all of them.
    fn query_fills(&self, since: Option<Timestamp>) -> Result<Vec<Fill>>;

    /// Hang up, with a reason.
    fn disconnect(&mut self, reason: &str, at: Timestamp) -> Result<()>;

    /// Proof the venue is ready at `at`, or the reason it is not.
    ///
    /// The only mint. There is no other constructor for a [`ReadyTicket`], so
    /// an order cannot be submitted from a non-ready venue by any code path.
    fn ready(&self, at: Timestamp) -> Result<ReadyTicket> {
        self.connection().ready_ticket(at)
    }

    /// The session as of `at`, including what is still missing.
    fn health(&self, at: Timestamp) -> SessionHealth {
        let connection = self.connection();
        let missing = self.missing_requirements();
        SessionHealth {
            venue: connection.venue().as_str().to_string(),
            at,
            phase: connection.effective_phase(at),
            session: connection.session(),
            since: connection.since(),
            last_heartbeat: connection.last_heartbeat(),
            heartbeat_gap: connection.heartbeat_gap(at),
            heartbeat_interval: connection.heartbeat_interval(),
            accepts_orders: connection.effective_phase(at).accepts_orders(),
            is_available: self.is_available(),
            simulated: self.is_simulated(),
            missing: missing.iter().map(VenueRequirement::describe).collect(),
            detail: connection.detail().to_string(),
        }
    }

    /// One sentence naming everything a deployment still owes this venue.
    fn requirement_summary(&self) -> String {
        describe_missing(self.venue_id(), &self.missing_requirements())
    }

    /// Connect, authenticate and heartbeat, in the only order that works.
    ///
    /// Provided rather than left to each adapter so the sequence is the same
    /// everywhere and a new adapter cannot invent a shorter one.
    fn bring_up(&mut self, credential: &VenueCredential, at: Timestamp) -> Result<ReadyTicket> {
        self.connect(at)?;
        self.authenticate(credential, at)?;
        self.heartbeat(at)?;
        self.ready(at)
    }
}
