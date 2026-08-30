//! Tax-lot accounting.
//!
//! A position is a stack of lots, each with its own acquisition price and date.
//! When a sale closes part of a position, which lot it closes determines the
//! realised gain and its holding period — so the choice is explicit
//! ([`LotMethod`]) rather than implied by an average.

use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// Which lot a closing trade consumes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LotMethod {
    /// Oldest first. The default in most jurisdictions.
    #[default]
    FirstInFirstOut,
    /// Newest first.
    LastInFirstOut,
    /// Highest cost first, which minimises realised gains.
    HighestCost,
    /// Lowest cost first, which maximises them.
    LowestCost,
}

impl LotMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FirstInFirstOut => "fifo",
            Self::LastInFirstOut => "lifo",
            Self::HighestCost => "highest_cost",
            Self::LowestCost => "lowest_cost",
        }
    }
}

/// One acquisition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lot {
    /// Signed: positive for a long lot, negative for a short.
    pub quantity: Decimal,
    /// Price paid or received per unit, excluding costs.
    pub price: Decimal,
    /// Transaction costs attributed to the lot.
    pub costs: Decimal,
    pub acquired_at: Timestamp,
    /// Order that created the lot, for lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
}

impl Lot {
    pub fn new(quantity: Decimal, price: Decimal, acquired_at: Timestamp) -> Self {
        Self {
            quantity,
            price,
            costs: Decimal::ZERO,
            acquired_at,
            order_id: None,
        }
    }

    pub fn with_costs(mut self, costs: Decimal) -> Self {
        self.costs = costs;
        self
    }

    pub fn with_order(mut self, order_id: impl Into<String>) -> Self {
        self.order_id = Some(order_id.into());
        self
    }

    /// Total outlay including costs. Negative for a short lot's proceeds.
    pub fn cost_basis(&self) -> Decimal {
        self.quantity * self.price + self.costs
    }

    /// Cost per unit including attributed costs.
    pub fn unit_cost(&self) -> Decimal {
        if self.quantity.is_zero() {
            return self.price;
        }
        self.cost_basis()
            .checked_div(self.quantity)
            .unwrap_or(self.price)
    }

    pub fn is_long(&self) -> bool {
        self.quantity.is_positive()
    }
}

/// A closed round trip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealisedTrade {
    /// Quantity closed, always positive.
    pub quantity: Decimal,
    pub open_price: Decimal,
    pub close_price: Decimal,
    /// Costs from both the opening and closing sides.
    pub costs: Decimal,
    pub opened_at: Timestamp,
    pub closed_at: Timestamp,
    /// True when the closed lot was long.
    pub was_long: bool,
}

impl RealisedTrade {
    /// Profit net of costs.
    pub fn realised_pnl(&self) -> Decimal {
        let gross = if self.was_long {
            (self.close_price - self.open_price) * self.quantity
        } else {
            (self.open_price - self.close_price) * self.quantity
        };
        gross - self.costs
    }

    /// Return on the opening notional.
    pub fn return_pct(&self) -> f64 {
        let notional = (self.open_price * self.quantity).abs();
        if !notional.is_positive() {
            return 0.0;
        }
        self.realised_pnl().to_f64() / notional.to_f64()
    }

    pub fn holding_period(&self) -> qip_core::Duration {
        self.closed_at.since(self.opened_at)
    }

    pub fn is_win(&self) -> bool {
        self.realised_pnl().is_positive()
    }
}

/// Consume `quantity` from `lots` under `method`, returning the closed trades.
///
/// `quantity` is the magnitude to close. Lots are consumed in the method's
/// order, partially where necessary, and exhausted lots are removed.
pub fn close_lots(
    lots: &mut Vec<Lot>,
    quantity: Decimal,
    close_price: Decimal,
    close_costs: Decimal,
    closed_at: Timestamp,
) -> Vec<RealisedTrade> {
    close_lots_with(
        lots,
        quantity,
        close_price,
        close_costs,
        closed_at,
        LotMethod::FirstInFirstOut,
    )
}

/// Close lots under an explicit method.
pub fn close_lots_with(
    lots: &mut Vec<Lot>,
    quantity: Decimal,
    close_price: Decimal,
    close_costs: Decimal,
    closed_at: Timestamp,
    method: LotMethod,
) -> Vec<RealisedTrade> {
    let mut remaining = quantity.abs();
    if !remaining.is_positive() || lots.is_empty() {
        return Vec::new();
    }
    let total_to_close = remaining;

    // Order the lots for consumption. Indices are used so the originals can be
    // mutated in place and the surviving order preserved.
    let mut order: Vec<usize> = (0..lots.len()).collect();
    match method {
        LotMethod::FirstInFirstOut => {
            order.sort_by_key(|i| lots[*i].acquired_at.as_nanos());
        }
        LotMethod::LastInFirstOut => {
            order.sort_by_key(|i| std::cmp::Reverse(lots[*i].acquired_at.as_nanos()));
        }
        LotMethod::HighestCost => {
            order.sort_by(|a, b| lots[*b].unit_cost().cmp(&lots[*a].unit_cost()));
        }
        LotMethod::LowestCost => {
            order.sort_by(|a, b| lots[*a].unit_cost().cmp(&lots[*b].unit_cost()));
        }
    }

    let mut trades = Vec::new();
    let mut consumed = vec![Decimal::ZERO; lots.len()];

    for index in order {
        if !remaining.is_positive() {
            break;
        }
        let available = lots[index].quantity.abs();
        if !available.is_positive() {
            continue;
        }
        let take = available.min(remaining);
        // Costs are apportioned across the closed quantity pro rata, so a
        // partial close carries its share and no more.
        let opening_costs = lots[index]
            .costs
            .checked_mul(take)
            .and_then(|scaled| scaled.checked_div(available))
            .unwrap_or(Decimal::ZERO);
        let closing_costs = close_costs
            .checked_mul(take)
            .and_then(|scaled| scaled.checked_div(total_to_close))
            .unwrap_or(Decimal::ZERO);

        trades.push(RealisedTrade {
            quantity: take,
            open_price: lots[index].price,
            close_price,
            costs: opening_costs + closing_costs,
            opened_at: lots[index].acquired_at,
            closed_at,
            was_long: lots[index].is_long(),
        });

        consumed[index] = take;
        remaining -= take;
    }

    // Reduce or remove consumed lots, keeping the surviving order.
    let mut surviving = Vec::with_capacity(lots.len());
    for (index, lot) in lots.iter().enumerate() {
        let taken = consumed[index];
        if !taken.is_positive() {
            surviving.push(lot.clone());
            continue;
        }
        let available = lot.quantity.abs();
        if taken >= available {
            continue;
        }
        let remaining_fraction = (available - taken)
            .checked_div(available)
            .unwrap_or(Decimal::ZERO);
        let sign = if lot.is_long() {
            Decimal::ONE
        } else {
            Decimal::NEG_ONE
        };
        surviving.push(Lot {
            quantity: (available - taken) * sign,
            price: lot.price,
            costs: lot.costs * remaining_fraction,
            acquired_at: lot.acquired_at,
            order_id: lot.order_id.clone(),
        });
    }
    *lots = surviving;

    trades
}
