//! Orders, fills and what a simulated execution has to admit about itself.
//!
//! The types here are values; the engine that produces them is
//! [`crate::market::MarketSimulator`]. Keeping them apart is deliberate — a
//! report is what a test asserts against, and it should be readable without
//! the machinery that built it.
//!
//! An [`ExecutionReport`] is built so the uncomfortable facts cannot be
//! dropped on the floor:
//!
//! * `filled` and `residual` are both present and both [`Decimal`]. A partial
//!   fill's leftover is exact — repeatedly filling and re-sending closes to the
//!   last unit rather than to within a rounding error that grows.
//! * `status` distinguishes *nothing was there* from *the venue stopped
//!   answering*. A strategy can retry the first; the second means an order may
//!   or may not exist somewhere and the residual is the only thing known.
//! * `crossed_by` is on the report, so a strategy that traded through a crossed
//!   market can see that it was crossed after the fact and not only during it.
//! * `mark` carries the observation the decision was priced off, with its age.
//!
//! [`ExecutionReport::adversity_bps`] is the single scalar in which a worse
//! execution is always a larger number. It exists so the direction of an
//! injected condition can be asserted: adding a condition may not lower it.

use crate::venue::{BookCondition, Mark};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_market::book::Side;
use serde::{Deserialize, Serialize};

/// An order handed to the simulator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimOrder {
    pub object_id: String,
    pub venue: String,
    pub side: Side,
    pub quantity: Decimal,
    /// A price the order will not trade through. `None` is a market order.
    pub limit_price: Option<Decimal>,
    /// Which leg of a multi-leg plan this is; zero for a standalone order.
    ///
    /// Carried on the order rather than inferred from position in a vector so
    /// a condition scoped to "leg two" means the same thing whether the plan
    /// is executed whole or a leg is replayed on its own.
    pub leg: usize,
    /// How many working slices the order is broken into.
    ///
    /// More than one is what lets a venue outage happen *mid-order*: the
    /// slices before the outage fill, the ones after do not, and the residual
    /// is exact.
    pub slices: usize,
    /// Interval between slices.
    pub slice_interval: Duration,
}

impl SimOrder {
    /// A market order, executed in one slice.
    pub fn market(
        object_id: impl Into<String>,
        venue: impl Into<String>,
        side: Side,
        quantity: Decimal,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            venue: venue.into(),
            side,
            quantity,
            limit_price: None,
            leg: 0,
            slices: 1,
            slice_interval: Duration::ZERO,
        }
    }

    /// The same order with a price it will not trade through.
    pub fn with_limit(mut self, limit_price: Decimal) -> Self {
        self.limit_price = Some(limit_price);
        self
    }

    /// The same order tagged as one leg of a plan. Legs are zero-indexed.
    pub fn on_leg(mut self, leg: usize) -> Self {
        self.leg = leg;
        self
    }

    /// Work the order in `slices` pieces, `interval` apart.
    pub fn worked_in(mut self, slices: usize, interval: Duration) -> Self {
        self.slices = slices.max(1);
        self.slice_interval = interval;
        self
    }

    pub fn validate(&self) -> Result<()> {
        if !self.quantity.is_positive() {
            return Err(Error::invalid(
                "an order must be for a positive quantity; direction is the side",
            ));
        }
        if self.object_id.trim().is_empty() || self.venue.trim().is_empty() {
            return Err(Error::invalid("an order needs an instrument and a venue"));
        }
        if let Some(limit) = self.limit_price
            && !limit.is_positive()
        {
            return Err(Error::invalid("a limit price must be positive"));
        }
        if self.slices == 0 {
            return Err(Error::invalid("an order must have at least one slice"));
        }
        if self.slice_interval.as_nanos() < 0 {
            return Err(Error::invalid("a slice interval cannot run backwards"));
        }
        Ok(())
    }
}

/// How an order ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillStatus {
    /// The whole quantity filled.
    Complete,
    /// Some filled. The residual is exact and is the caller's again.
    Partial,
    /// The book had nothing to fill against.
    NoLiquidity,
    /// The limit price was not marketable against the touch.
    NotMarketable,
    /// The venue stopped responding.
    ///
    /// Distinct from every other status because it is the one where the state
    /// of the order is *unknown* on the venue's side. The simulator fills
    /// nothing further and reports what is left, which is the only honest
    /// answer: an order that silently completed during an outage is the
    /// backtest telling a story about a venue that was not talking.
    VenueUnreachable,
    /// The feed could not be trusted at the instant the order would have
    /// priced, so nothing was sent.
    FeedUnusable,
}

impl FillStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::NoLiquidity => "no_liquidity",
            Self::NotMarketable => "not_marketable",
            Self::VenueUnreachable => "venue_unreachable",
            Self::FeedUnusable => "feed_unusable",
        }
    }

    /// Whether anything at all traded.
    pub const fn traded(&self) -> bool {
        matches!(self, Self::Complete | Self::Partial)
    }

    /// Whether the residual is known to still be the caller's.
    ///
    /// True for everything except an outage, where the residual is known but
    /// whether the venue also holds it is not.
    pub const fn residual_is_certain(&self) -> bool {
        !matches!(self, Self::VenueUnreachable)
    }
}

/// One working slice of an order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FillSlice {
    pub at: Timestamp,
    pub filled: Decimal,
    pub notional: Decimal,
    /// The last and worst price that printed in this slice.
    pub worst_price: Option<Decimal>,
    pub levels_consumed: usize,
    /// Depth the book was showing on the side being taken.
    pub depth_available: Decimal,
    /// Whether the venue answered this slice.
    pub venue_responding: bool,
}

impl FillSlice {
    pub fn average_price(&self) -> Option<Decimal> {
        self.notional.checked_div(self.filled)
    }
}

/// What one order actually did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub object_id: String,
    pub venue: String,
    pub side: Side,
    pub leg: usize,
    pub requested: Decimal,
    pub submitted_at: Timestamp,
    /// When the order reached the venue: `submitted_at` plus the latency the
    /// regime imposed. Never earlier than `submitted_at`.
    pub arrived_at: Timestamp,
    pub latency: Duration,
    pub filled: Decimal,
    /// Exactly what is left. `requested - filled`, in `Decimal`, so a chain of
    /// partial fills does not drift.
    pub residual: Decimal,
    /// Cash paid (a buy) or received (a sell) for `filled`.
    pub notional: Decimal,
    /// Fees charged on the filled notional.
    pub commission: Decimal,
    pub status: FillStatus,
    /// The price the execution is measured against: the book's own mid at
    /// arrival, or the taker touch when the book is crossed and has no mid.
    pub reference: Option<Decimal>,
    pub slices: Vec<FillSlice>,
    /// The observation the order priced off, with its age.
    pub mark: Mark,
    pub book_condition: BookCondition,
    /// How far the bid was through the ask, when it was.
    pub crossed_by: Option<Decimal>,
    /// Depth the book showed on the side being taken, at arrival.
    pub depth_available: Decimal,
    /// Conditions active on this order, in schedule order.
    pub conditions: Vec<String>,
}

impl ExecutionReport {
    /// Volume-weighted fill price, or `None` when nothing traded.
    pub fn average_price(&self) -> Option<Decimal> {
        self.notional.checked_div(self.filled)
    }

    /// Fraction of the request that filled, in `[0, 1]`.
    pub fn fill_fraction(&self) -> f64 {
        if !self.requested.is_positive() {
            return 0.0;
        }
        (self.filled.to_f64() / self.requested.to_f64()).clamp(0.0, 1.0)
    }

    /// Fraction of the request that did not fill.
    pub fn unfilled_fraction(&self) -> f64 {
        1.0 - self.fill_fraction()
    }

    /// Cost against the reference in basis points, positive being worse for
    /// the taker.
    pub fn slippage_bps(&self) -> Option<f64> {
        let reference = self.reference?;
        if !reference.is_positive() {
            return None;
        }
        let average = self.average_price()?;
        let signed = match self.side {
            Side::Buy => average - reference,
            Side::Sell => reference - average,
        };
        Some(signed.to_f64() / reference.to_f64() * 10_000.0)
    }

    /// One scalar in which a worse execution is always a larger number.
    ///
    /// The filled part contributes its slippage; the unfilled part contributes
    /// a full percentage point of adversity per percent unfilled, which
    /// dominates any slippage a thin book could have flattered the filled part
    /// with. This is what lets a test assert the *direction* of an injected
    /// condition without having to compare a dozen fields pairwise, and
    /// without asserting something false: a condition may legitimately move
    /// the price level — a flash event does — but it may never make the
    /// execution itself better.
    pub fn adversity_bps(&self) -> f64 {
        let filled = self.fill_fraction();
        let slippage = self.slippage_bps().unwrap_or(0.0);
        filled * slippage + self.unfilled_fraction() * 10_000.0
    }

    /// Signed cash effect of the order: negative for a buy, positive for a
    /// sell, fees always against the caller.
    pub fn cash_flow(&self) -> Decimal {
        match self.side {
            Side::Buy => -(self.notional + self.commission),
            Side::Sell => self.notional - self.commission,
        }
    }

    /// Whether the touch was inverted when this order priced.
    pub fn was_crossed(&self) -> bool {
        self.book_condition == BookCondition::Crossed
    }

    /// Whether the mark this priced off was a last-known value.
    pub fn priced_off_stale_data(&self) -> bool {
        self.mark.is_stale()
    }

    pub fn summarise(&self) -> String {
        let price = match self.average_price() {
            Some(price) => format!("{price}"),
            None => "no fill".to_string(),
        };
        format!(
            "{} {} {}/{} of {} at {price} ({}), residual {}, {:.1}bp adversity{}{}",
            self.venue,
            self.side.as_str(),
            self.filled,
            self.requested,
            self.object_id,
            self.status.as_str(),
            self.residual,
            self.adversity_bps(),
            if self.was_crossed() {
                ", CROSSED market"
            } else {
                ""
            },
            if self.priced_off_stale_data() {
                ", priced off a stale mark"
            } else {
                ""
            }
        )
    }
}

/// Several orders executed as one thing.
///
/// The unit a scenario like "a venue outage on leg two" is written against:
/// the legs carry their own indices, so the condition finds the leg whether or
/// not the plan is executed in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPlan {
    legs: Vec<SimOrder>,
}

impl ExecutionPlan {
    pub fn new() -> Self {
        Self { legs: Vec::new() }
    }

    /// Append an order, stamping it with its leg index.
    pub fn leg(mut self, order: SimOrder) -> Self {
        let index = self.legs.len();
        self.legs.push(order.on_leg(index));
        self
    }

    pub fn legs(&self) -> &[SimOrder] {
        &self.legs
    }

    pub fn is_empty(&self) -> bool {
        self.legs.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        if self.legs.is_empty() {
            return Err(Error::invalid("an execution plan needs at least one leg"));
        }
        for leg in &self.legs {
            leg.validate()?;
        }
        Ok(())
    }
}

/// What a plan actually did, leg by leg.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanReport {
    pub legs: Vec<ExecutionReport>,
}

impl PlanReport {
    /// Whether every leg filled in full.
    ///
    /// The question that matters for a spread trade: a plan that filled two of
    /// three legs has left the book with a position nobody chose.
    pub fn is_complete(&self) -> bool {
        self.legs
            .iter()
            .all(|leg| leg.status == FillStatus::Complete)
    }

    /// Legs that did not complete, with what is left on each.
    pub fn residuals(&self) -> Vec<(usize, Decimal, FillStatus)> {
        self.legs
            .iter()
            .filter(|leg| leg.status != FillStatus::Complete)
            .map(|leg| (leg.leg, leg.residual, leg.status))
            .collect()
    }

    /// Legs the venue stopped answering on.
    pub fn unreachable_legs(&self) -> Vec<usize> {
        self.legs
            .iter()
            .filter(|leg| leg.status == FillStatus::VenueUnreachable)
            .map(|leg| leg.leg)
            .collect()
    }

    /// The worst adversity across the legs.
    pub fn worst_adversity_bps(&self) -> f64 {
        self.legs
            .iter()
            .map(ExecutionReport::adversity_bps)
            .fold(0.0_f64, f64::max)
    }

    pub fn summarise(&self) -> String {
        let mut lines: Vec<String> = self
            .legs
            .iter()
            .map(|leg| format!("  leg {}: {}", leg.leg, leg.summarise()))
            .collect();
        lines.insert(
            0,
            if self.is_complete() {
                "plan complete".to_string()
            } else {
                format!(
                    "plan INCOMPLETE: {} leg(s) left a residual",
                    self.residuals().len()
                )
            },
        );
        lines.join("\n")
    }
}
