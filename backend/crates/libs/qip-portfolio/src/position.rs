//! A position in one instrument.

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::lot::{Lot, LotMethod, RealisedTrade, close_lots_with};

/// Which way a position points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    Long,
    Short,
    Flat,
}

impl PositionSide {
    pub fn of(quantity: Decimal) -> Self {
        match quantity.signum() {
            1 => Self::Long,
            -1 => Self::Short,
            _ => Self::Flat,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Flat => "flat",
        }
    }
}

/// A holding, tracked by lot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub object_id: ObjectId,
    pub symbol: String,
    /// Open lots. Empty when flat.
    pub lots: Vec<Lot>,
    /// Cumulative realised profit, net of costs.
    pub realised_pnl: Decimal,
    /// Cumulative transaction costs paid.
    pub total_costs: Decimal,
    /// Multiplier from one unit to the underlying notional.
    pub contract_multiplier: Decimal,
    /// Closed round trips, for attribution.
    pub closed_trades: Vec<RealisedTrade>,
    pub lot_method: LotMethod,
    pub opened_at: Option<Timestamp>,
    pub updated_at: Timestamp,
}

impl Position {
    pub fn new(object_id: ObjectId, symbol: impl Into<String>, at: Timestamp) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            lots: Vec::new(),
            realised_pnl: Decimal::ZERO,
            total_costs: Decimal::ZERO,
            contract_multiplier: Decimal::ONE,
            closed_trades: Vec::new(),
            lot_method: LotMethod::default(),
            opened_at: None,
            updated_at: at,
        }
    }

    pub fn with_multiplier(mut self, multiplier: Decimal) -> Self {
        self.contract_multiplier = multiplier;
        self
    }

    pub fn with_lot_method(mut self, method: LotMethod) -> Self {
        self.lot_method = method;
        self
    }

    /// Net signed quantity across all lots.
    pub fn quantity(&self) -> Decimal {
        self.lots.iter().map(|l| l.quantity).sum()
    }

    pub fn side(&self) -> PositionSide {
        PositionSide::of(self.quantity())
    }

    pub fn is_flat(&self) -> bool {
        self.quantity().is_zero()
    }

    /// Total outlay across open lots, including costs.
    pub fn cost_basis(&self) -> Decimal {
        self.lots.iter().map(Lot::cost_basis).sum()
    }

    /// Weighted average entry price of the open lots, excluding costs.
    pub fn average_price(&self) -> Decimal {
        let quantity = self.quantity();
        if quantity.is_zero() {
            return Decimal::ZERO;
        }
        let notional: Decimal = self.lots.iter().map(|l| l.quantity * l.price).sum();
        notional.checked_div(quantity).unwrap_or(Decimal::ZERO)
    }

    /// Market value at `price`, signed by direction.
    pub fn market_value(&self, price: Decimal) -> Decimal {
        self.quantity() * price * self.contract_multiplier
    }

    /// Gross notional exposure, unsigned.
    pub fn notional_exposure(&self, price: Decimal) -> Decimal {
        self.market_value(price).abs()
    }

    /// Profit not yet realised, at `price`, net of the costs still attributed
    /// to the open lots.
    ///
    /// Netting the costs is what makes realised plus unrealised equal the
    /// change in equity. Measuring unrealised gross while realised is net
    /// leaves the difference — the entry costs of positions still open —
    /// unaccounted for anywhere, and the books stop reconciling.
    pub fn unrealised_pnl(&self, price: Decimal) -> Decimal {
        let gross: Decimal = self
            .lots
            .iter()
            .map(|lot| (price - lot.price) * lot.quantity * self.contract_multiplier)
            .sum();
        let open_costs: Decimal = self.lots.iter().map(|lot| lot.costs).sum();
        gross - open_costs
    }

    /// Realised plus unrealised.
    pub fn total_pnl(&self, price: Decimal) -> Decimal {
        self.realised_pnl + self.unrealised_pnl(price)
    }

    /// Apply a fill.
    ///
    /// A fill in the same direction opens a lot; one in the opposite direction
    /// closes lots and realises profit, and if it exceeds the position it flips
    /// it, opening a new lot for the excess.
    ///
    /// Returns the cash flow: negative when buying, positive when selling.
    pub fn apply_fill(
        &mut self,
        quantity: Decimal,
        price: Decimal,
        costs: Decimal,
        at: Timestamp,
        order_id: Option<String>,
    ) -> Decimal {
        if quantity.is_zero() {
            return -costs;
        }
        self.updated_at = at;
        self.total_costs += costs;
        if self.opened_at.is_none() {
            self.opened_at = Some(at);
        }

        let current = self.quantity();
        let cash_flow = -(quantity * price * self.contract_multiplier) - costs;

        // Same direction, or opening from flat: a new lot.
        if current.is_zero() || current.signum() == quantity.signum() {
            let mut lot = Lot::new(quantity, price, at).with_costs(costs);
            lot.order_id = order_id;
            self.lots.push(lot);
            return cash_flow;
        }

        // Opposite direction: close against existing lots.
        let closing = quantity.abs().min(current.abs());
        let trades = close_lots_with(
            &mut self.lots,
            closing,
            price,
            costs
                .checked_mul(closing)
                .and_then(|scaled| scaled.checked_div(quantity.abs()))
                .unwrap_or(Decimal::ZERO),
            at,
            self.lot_method,
        );
        for trade in &trades {
            // Realised profit is measured in instrument units and scaled to
            // currency by the contract multiplier.
            self.realised_pnl += trade.realised_pnl() * self.contract_multiplier;
        }
        self.closed_trades.extend(trades);

        // Any excess flips the position.
        let excess = quantity.abs() - closing;
        if excess.is_positive() {
            let sign = if quantity.is_positive() {
                Decimal::ONE
            } else {
                Decimal::NEG_ONE
            };
            let remaining_costs = costs
                .checked_mul(excess)
                .and_then(|scaled| scaled.checked_div(quantity.abs()))
                .unwrap_or(Decimal::ZERO);
            let mut lot = Lot::new(excess * sign, price, at).with_costs(remaining_costs);
            lot.order_id = order_id;
            self.lots.push(lot);
        }

        if self.is_flat() {
            self.opened_at = None;
        }
        cash_flow
    }

    /// Apply a corporate action's quantity and price adjustments.
    pub fn apply_adjustment(&mut self, quantity_factor: Decimal, price_factor: Decimal) {
        if quantity_factor == Decimal::ONE && price_factor == Decimal::ONE {
            return;
        }
        for lot in &mut self.lots {
            lot.quantity = lot.quantity * quantity_factor;
            lot.price = lot.price * price_factor;
        }
    }

    /// Number of round trips closed.
    pub fn trade_count(&self) -> usize {
        self.closed_trades.len()
    }

    /// Fraction of closed trades that were profitable.
    pub fn hit_rate(&self) -> f64 {
        if self.closed_trades.is_empty() {
            return 0.0;
        }
        self.closed_trades.iter().filter(|t| t.is_win()).count() as f64
            / self.closed_trades.len() as f64
    }
}

/// Published whenever a position changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionUpdated {
    pub portfolio_id: String,
    pub object_id: ObjectId,
    pub symbol: String,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub market_value: Decimal,
    pub realised_pnl: Decimal,
    pub unrealised_pnl: Decimal,
    pub at: Timestamp,
}

impl EventBody for PositionUpdated {
    const TOPIC: Topic = Topic::PositionUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.portfolio_id,
            self.object_id,
            self.at.as_nanos()
        ))
    }
}
