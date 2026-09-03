//! Orders and their lifecycle.
//!
//! An [`Order`] carries where it came from — the proposal, the hypotheses, the
//! pre-trade check that cleared it — so a fill can always be traced back to the
//! reasoning that produced it. An order that cannot be traced is one nobody can
//! explain after the fact, which is the state every part of this platform is
//! built to avoid.
//!
//! The state machine is deliberately strict. [`Order::transition`] is the only
//! way a state changes, and it refuses anything that is not a legal move —
//! including the ones that would be convenient, like filling a cancelled order
//! because a late report arrived.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{FillId, ObjectId, OrderId};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Which way an order goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
    pub const fn sign(&self) -> i32 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
    pub const fn opposite(&self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the order is to be worked.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrderType {
    /// Take whatever the book offers.
    Market,
    /// Trade at `limit` or better.
    Limit { price: Decimal },
    /// Spread evenly over the window.
    TimeWeighted { minutes: u32 },
    /// Track the volume profile over the window.
    VolumeWeighted { minutes: u32 },
    /// Take a fixed share of volume until done.
    Participation { rate: f64 },
}

impl OrderType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit { .. } => "limit",
            Self::TimeWeighted { .. } => "twap",
            Self::VolumeWeighted { .. } => "vwap",
            Self::Participation { .. } => "participation",
        }
    }

    /// Whether the order accepts whatever price the market gives.
    ///
    /// A market order in an illiquid name is how a small position becomes a
    /// large loss, so this is checked before one is sent.
    pub const fn is_unpriced(&self) -> bool {
        matches!(self, Self::Market)
    }
}

/// Where an order is in its life.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OrderState {
    /// Created, not yet risk-checked.
    Created,
    /// Passed pre-trade risk.
    RiskApproved { at: Timestamp },
    /// Sent to a venue.
    Working { at: Timestamp, venue: String },
    /// Partly filled and still working.
    PartiallyFilled { filled: Decimal },
    /// Done.
    Filled { at: Timestamp },
    /// Cancelled before completion.
    Cancelled { at: Timestamp, reason: String },
    /// Refused by risk, compliance or the venue.
    Rejected { at: Timestamp, reason: String },
    /// Ended without a fill because the day ended or the order expired.
    Expired { at: Timestamp },
}

impl OrderState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::RiskApproved { .. } => "risk_approved",
            Self::Working { .. } => "working",
            Self::PartiallyFilled { .. } => "partially_filled",
            Self::Filled { .. } => "filled",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
            Self::Expired { .. } => "expired",
        }
    }

    /// Whether the order can still trade.
    pub const fn is_open(&self) -> bool {
        matches!(
            self,
            Self::Created
                | Self::RiskApproved { .. }
                | Self::Working { .. }
                | Self::PartiallyFilled { .. }
        )
    }

    /// Whether the order has reached a state it cannot leave.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Filled { .. }
                | Self::Cancelled { .. }
                | Self::Rejected { .. }
                | Self::Expired { .. }
        )
    }

    /// Whether the order has been cleared to reach a venue.
    pub const fn is_risk_cleared(&self) -> bool {
        matches!(
            self,
            Self::RiskApproved { .. }
                | Self::Working { .. }
                | Self::PartiallyFilled { .. }
                | Self::Filled { .. }
        )
    }
}

/// One execution against an order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub fill_id: FillId,
    pub order_id: OrderId,
    pub at: Timestamp,
    pub quantity: Decimal,
    pub price: Decimal,
    /// Commission and fees.
    pub costs: Decimal,
    pub venue: String,
    /// Whether the fill came from a simulated venue.
    ///
    /// Carried on the fill itself rather than inferred from configuration, so
    /// a paper fill can never be mistaken for a real one in a report or a
    /// reconciliation.
    pub simulated: bool,
}

impl Fill {
    pub fn notional(&self) -> Decimal {
        self.quantity * self.price
    }
}

/// An order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub order_id: OrderId,
    pub object_id: ObjectId,
    pub side: Side,
    /// Total quantity to trade. Always positive.
    pub quantity: Decimal,
    pub order_type: OrderType,
    pub created_at: Timestamp,
    pub state: OrderState,
    /// The proposal this order implements.
    pub proposal_id: String,
    /// The hypotheses behind it. Required: an untraceable order is one nobody
    /// can explain after the fact.
    pub hypotheses: Vec<String>,
    /// The scope for kill-switch purposes.
    pub scope: String,
    /// Reference price at the time of creation, for slippage measurement.
    pub arrival_price: Decimal,
    pub fills: Vec<Fill>,
    /// Every state it has been in, for reconstruction.
    pub transitions: Vec<(Timestamp, String)>,
}

impl Order {
    pub fn new(
        order_id: OrderId,
        object_id: ObjectId,
        side: Side,
        quantity: Decimal,
        order_type: OrderType,
        arrival_price: Decimal,
        proposal_id: impl Into<String>,
        hypotheses: Vec<String>,
        scope: impl Into<String>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            order_id,
            object_id,
            side,
            quantity,
            order_type,
            created_at,
            state: OrderState::Created,
            proposal_id: proposal_id.into(),
            hypotheses,
            scope: scope.into(),
            arrival_price,
            fills: Vec::new(),
            transitions: vec![(created_at, "created".to_string())],
        }
    }

    pub fn filled_quantity(&self) -> Decimal {
        self.fills
            .iter()
            .map(|fill| fill.quantity)
            .fold(Decimal::ZERO, |a, b| a + b)
    }

    pub fn remaining_quantity(&self) -> Decimal {
        self.quantity - self.filled_quantity()
    }

    pub fn is_complete(&self) -> bool {
        self.remaining_quantity() <= Decimal::ZERO
    }

    /// Volume-weighted average fill price, if anything filled.
    pub fn average_price(&self) -> Option<Decimal> {
        let filled = self.filled_quantity();
        if filled <= Decimal::ZERO {
            return None;
        }
        let notional = self
            .fills
            .iter()
            .map(|fill| fill.notional())
            .fold(Decimal::ZERO, |a, b| a + b);
        Some(notional / filled)
    }

    /// Slippage against the arrival price, in basis points, signed so positive
    /// is worse for the order.
    pub fn slippage_bps(&self) -> Option<f64> {
        let average = self.average_price()?.to_f64();
        let arrival = self.arrival_price.to_f64();
        if arrival.abs() < 1e-12 {
            return None;
        }
        let raw = (average - arrival) / arrival * 10_000.0;
        Some(raw * f64::from(self.side.sign()))
    }

    pub fn total_costs(&self) -> Decimal {
        self.fills
            .iter()
            .map(|fill| fill.costs)
            .fold(Decimal::ZERO, |a, b| a + b)
    }

    /// Whether every fill came from a simulated venue.
    pub fn is_paper(&self) -> bool {
        !self.fills.is_empty() && self.fills.iter().all(|fill| fill.simulated)
    }

    /// Move to a new state, refusing anything illegal.
    ///
    /// The refusals matter more than the permissions. A cancelled order that
    /// can be filled by a late report, or a rejected order that can start
    /// working, is a reconciliation break waiting to happen.
    pub fn transition(&mut self, next: OrderState, at: Timestamp) -> Result<()> {
        // Written as one `matches!` so the legal moves read as a table. The
        // refusals matter more than the permissions: a cancelled order that a
        // late report can fill, or a rejected order that can start working, is
        // a reconciliation break waiting to happen.
        let legal = matches!(
            (&self.state, &next),
            (OrderState::Created, OrderState::RiskApproved { .. })
                | (OrderState::Created, OrderState::Rejected { .. })
                | (OrderState::RiskApproved { .. }, OrderState::Working { .. })
                | (
                    OrderState::RiskApproved { .. },
                    OrderState::Cancelled { .. }
                )
                | (OrderState::RiskApproved { .. }, OrderState::Rejected { .. })
                | (
                    OrderState::Working { .. },
                    OrderState::PartiallyFilled { .. }
                )
                | (OrderState::Working { .. }, OrderState::Filled { .. })
                | (OrderState::Working { .. }, OrderState::Cancelled { .. })
                | (OrderState::Working { .. }, OrderState::Rejected { .. })
                | (OrderState::Working { .. }, OrderState::Expired { .. })
                | (
                    OrderState::PartiallyFilled { .. },
                    OrderState::PartiallyFilled { .. }
                )
                | (
                    OrderState::PartiallyFilled { .. },
                    OrderState::Filled { .. }
                )
                | (
                    OrderState::PartiallyFilled { .. },
                    OrderState::Cancelled { .. }
                )
                | (
                    OrderState::PartiallyFilled { .. },
                    OrderState::Expired { .. }
                )
        );
        if !legal {
            return Err(Error::invalid(format!(
                "order {} cannot move from {} to {}",
                self.order_id.as_str(),
                self.state.as_str(),
                next.as_str()
            )));
        }
        self.transitions.push((at, next.as_str().to_string()));
        self.state = next;
        Ok(())
    }

    /// Record a fill and advance the state.
    pub fn apply_fill(&mut self, fill: Fill) -> Result<()> {
        if !self.state.is_open() {
            return Err(Error::invalid(format!(
                "order {} is {} and cannot be filled",
                self.order_id.as_str(),
                self.state.as_str()
            )));
        }
        if fill.quantity <= Decimal::ZERO {
            return Err(Error::invalid("a fill for zero quantity is not a fill"));
        }
        // A redelivered venue report — the same fill_id arriving twice over a
        // retried connection — would otherwise double-book: nothing above
        // checks identity, only total quantity, and a second copy of a fill
        // that still fits inside what remains passes the over-fill guard
        // below and quietly doubles the position. The fill_id exists
        // precisely so a duplicate can be told apart from a second, distinct
        // print at the same price and size.
        if self.fills.iter().any(|f| f.fill_id == fill.fill_id) {
            return Err(Error::invalid(format!(
                "fill {} was already applied to order {}; a redelivered report is not a new fill",
                fill.fill_id.as_str(),
                self.order_id.as_str()
            )));
        }
        // Over-filling is a venue error or a bug, and accepting it would put
        // the book out of step with reality in a way nothing else would catch.
        if fill.quantity > self.remaining_quantity() {
            return Err(Error::invalid(format!(
                "fill of {} exceeds the {} remaining on order {}",
                fill.quantity,
                self.remaining_quantity(),
                self.order_id.as_str()
            )));
        }
        let at = fill.at;
        self.fills.push(fill);
        if self.is_complete() {
            self.transition(OrderState::Filled { at }, at)
        } else {
            self.transition(
                OrderState::PartiallyFilled {
                    filled: self.filled_quantity(),
                },
                at,
            )
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.quantity <= Decimal::ZERO {
            return Err(Error::invalid(
                "an order quantity must be positive; the side carries the direction",
            ));
        }
        if self.arrival_price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "order {} has no usable arrival price, so slippage could not be measured",
                self.order_id.as_str()
            )));
        }
        if self.hypotheses.is_empty() {
            return Err(Error::invalid(format!(
                "order {} traces to no hypothesis; an untraceable order is one nobody can explain",
                self.order_id.as_str()
            )));
        }
        if self.proposal_id.trim().is_empty() {
            return Err(Error::invalid(format!(
                "order {} traces to no proposal",
                self.order_id.as_str()
            )));
        }
        if self.scope.trim().is_empty() {
            return Err(Error::invalid(format!(
                "order {} names no scope, so a scoped kill switch could not stop it",
                self.order_id.as_str()
            )));
        }
        Ok(())
    }
}
