//! Transaction costs and liquidity.
//!
//! Every instrument carries its own cost model. Execution and simulation share
//! it, which is the point: a backtest that assumes a tighter spread than the
//! execution engine will actually pay produces a strategy that only works on
//! paper.

use qip_core::Decimal;
use serde::{Deserialize, Serialize};

/// How readily a position can be turned into cash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidityProfile {
    /// Typical traded volume per day, in instrument units.
    pub average_daily_volume: Decimal,
    /// Quoted bid-ask spread in basis points of mid.
    pub typical_spread_bps: f64,
    /// Depth available at the touch, in units.
    pub top_of_book_depth: Decimal,
    /// Days to exit a position of average size without undue impact.
    pub days_to_liquidate: f64,
    /// Fraction of daily volume the platform is willing to be.
    pub max_participation_rate: f64,
    /// True where the instrument only trades by appointment or negotiation.
    pub is_negotiated: bool,
}

impl Default for LiquidityProfile {
    fn default() -> Self {
        Self {
            average_daily_volume: Decimal::ZERO,
            typical_spread_bps: 10.0,
            top_of_book_depth: Decimal::ZERO,
            days_to_liquidate: 1.0,
            max_participation_rate: 0.1,
            is_negotiated: false,
        }
    }
}

impl LiquidityProfile {
    /// Liquid, tight-spread listed instrument.
    pub fn listed(average_daily_volume: Decimal, spread_bps: f64) -> Self {
        Self {
            average_daily_volume,
            typical_spread_bps: spread_bps,
            top_of_book_depth: average_daily_volume
                .checked_div(Decimal::from_int(2000))
                .unwrap_or(Decimal::ZERO),
            days_to_liquidate: 1.0,
            max_participation_rate: 0.1,
            is_negotiated: false,
        }
    }

    /// Instrument that trades only by negotiation: private credit, real assets.
    pub fn illiquid(days_to_liquidate: f64) -> Self {
        Self {
            average_daily_volume: Decimal::ZERO,
            typical_spread_bps: 250.0,
            top_of_book_depth: Decimal::ZERO,
            days_to_liquidate,
            max_participation_rate: 0.0,
            is_negotiated: true,
        }
    }

    /// Days to exit `quantity` at the permitted participation rate.
    ///
    /// Returns `None` for a negotiated instrument, where no volume-based
    /// estimate is meaningful and `days_to_liquidate` is the only guide.
    pub fn days_to_exit(&self, quantity: Decimal) -> Option<f64> {
        if self.is_negotiated || self.max_participation_rate <= 0.0 {
            return None;
        }
        let daily_capacity = self.average_daily_volume.to_f64() * self.max_participation_rate;
        if daily_capacity <= 0.0 {
            return None;
        }
        Some((quantity.abs().to_f64() / daily_capacity).max(0.0))
    }

    /// Fraction of average daily volume represented by `quantity`.
    pub fn participation_for(&self, quantity: Decimal) -> f64 {
        let adv = self.average_daily_volume.to_f64();
        if adv <= 0.0 {
            return f64::INFINITY;
        }
        quantity.abs().to_f64() / adv
    }
}

/// Cost model for trading one instrument.
///
/// Total cost is the sum of an explicit fee, half the spread, and a market
/// impact term following the square-root law — the empirical regularity that
/// impact scales with the square root of participation rather than linearly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionCostModel {
    /// Commission and exchange fees, in basis points of notional.
    pub commission_bps: f64,
    /// Fixed cost per order, in the instrument currency.
    pub fixed_fee: Decimal,
    /// Half-spread paid on a marketable order, in basis points.
    pub half_spread_bps: f64,
    /// Coefficient of the square-root impact law, in basis points.
    ///
    /// Roughly the impact of trading 100% of a day's volume; empirical
    /// estimates for liquid equities cluster around 30-60bp.
    pub impact_coefficient_bps: f64,
    /// Taxes and levies, in basis points (stamp duty, financial transaction tax).
    pub tax_bps: f64,
    /// Borrow cost for a short position, in basis points per year.
    pub short_borrow_bps_annual: f64,
}

impl Default for TransactionCostModel {
    fn default() -> Self {
        Self {
            commission_bps: 1.0,
            fixed_fee: Decimal::ZERO,
            half_spread_bps: 2.5,
            impact_coefficient_bps: 40.0,
            tax_bps: 0.0,
            short_borrow_bps_annual: 50.0,
        }
    }
}

impl TransactionCostModel {
    /// A cost model for a liquid listed instrument.
    pub fn listed(spread_bps: f64) -> Self {
        Self {
            half_spread_bps: spread_bps / 2.0,
            ..Self::default()
        }
    }

    /// A cost model for an instrument that trades by negotiation.
    pub fn negotiated() -> Self {
        Self {
            commission_bps: 25.0,
            fixed_fee: Decimal::ZERO,
            half_spread_bps: 125.0,
            impact_coefficient_bps: 0.0,
            tax_bps: 0.0,
            short_borrow_bps_annual: 0.0,
        }
    }

    /// Estimated all-in cost of trading `notional` at `participation` of daily
    /// volume, in the instrument's currency.
    pub fn estimate(&self, notional: Decimal, participation: f64) -> Decimal {
        let magnitude = notional.abs();
        let explicit = magnitude.apply_bps(self.commission_bps + self.tax_bps);
        let spread = magnitude.apply_bps(self.half_spread_bps);
        let impact = magnitude.apply_bps(self.impact_bps(participation));
        explicit + spread + impact + self.fixed_fee
    }

    /// Market impact in basis points at a given participation rate.
    pub fn impact_bps(&self, participation: f64) -> f64 {
        if !participation.is_finite() || participation <= 0.0 {
            return 0.0;
        }
        // Square-root law, capped so a pathological participation figure cannot
        // produce a nonsensical cost that silently blocks all trading.
        self.impact_coefficient_bps * participation.min(4.0).sqrt()
    }

    /// Total cost in basis points, for comparing instruments.
    pub fn total_bps(&self, participation: f64) -> f64 {
        self.commission_bps + self.tax_bps + self.half_spread_bps + self.impact_bps(participation)
    }

    /// Cost of holding a short position for `days`, in basis points.
    pub fn short_carry_bps(&self, days: f64) -> f64 {
        self.short_borrow_bps_annual * days / 365.0
    }

    /// The participation rate at which expected impact eats `alpha_bps` of
    /// expected alpha — the point past which trading faster destroys the trade.
    pub fn breakeven_participation(&self, alpha_bps: f64) -> f64 {
        let budget = alpha_bps - self.commission_bps - self.tax_bps - self.half_spread_bps;
        if budget <= 0.0 || self.impact_coefficient_bps <= 0.0 {
            return 0.0;
        }
        (budget / self.impact_coefficient_bps).powi(2)
    }
}
