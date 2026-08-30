//! Order book and trade generation around a synthetic mid.
//!
//! Depth thins geometrically away from the touch, the spread widens with
//! volatility and with the regime's liquidity penalty, and trade arrivals
//! follow the intraday intensity profile. That combination is what makes the
//! execution engine's slippage and impact numbers mean something.

use qip_core::rng::Rng;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook, Side};
use qip_market::quote::{Trade, TradeCondition};

use super::price::{PriceState, Regime};

/// Book shape parameters for one instrument.
#[derive(Clone, Debug)]
pub struct BookParameters {
    /// Half-spread in basis points at unit volatility in a normal regime.
    pub base_half_spread_bps: f64,
    /// Typical size resting at the touch.
    pub base_touch_size: f64,
    /// Fraction of size remaining at each successive level.
    pub depth_decay: f64,
    /// Number of levels published per side.
    pub levels: usize,
    /// Tick size for the instrument.
    pub tick_size: Decimal,
    /// Expected trades per day at average intensity.
    pub daily_trade_count: f64,
    /// Median trade size.
    pub median_trade_size: f64,
}

impl Default for BookParameters {
    fn default() -> Self {
        Self {
            base_half_spread_bps: 1.5,
            base_touch_size: 800.0,
            depth_decay: 0.72,
            levels: 5,
            tick_size: Decimal::from_raw(10_000_000), // 0.01
            daily_trade_count: 4_000.0,
            median_trade_size: 180.0,
        }
    }
}

impl BookParameters {
    /// A thinly traded instrument: wide, shallow, infrequent.
    pub fn illiquid() -> Self {
        Self {
            base_half_spread_bps: 25.0,
            base_touch_size: 50.0,
            depth_decay: 0.55,
            levels: 3,
            daily_trade_count: 60.0,
            median_trade_size: 40.0,
            ..Self::default()
        }
    }

    /// A deeply liquid instrument: tight, deep, continuous.
    pub fn deep() -> Self {
        Self {
            base_half_spread_bps: 0.4,
            base_touch_size: 5_000.0,
            depth_decay: 0.85,
            levels: 8,
            daily_trade_count: 30_000.0,
            median_trade_size: 300.0,
            ..Self::default()
        }
    }

    /// Half-spread in basis points given current conditions.
    pub fn half_spread_bps(&self, state: &PriceState, regime: Regime) -> f64 {
        // Spread scales with volatility relative to a 20% annualised baseline,
        // then with the regime's liquidity penalty and any active shock.
        let volatility_ratio = (state.volatility() / 0.20).clamp(0.25, 12.0);
        self.base_half_spread_bps
            * volatility_ratio.sqrt()
            * regime.liquidity_penalty()
            * state.liquidity_shock
    }
}

/// Build a book around `mid`.
pub fn generate_book<R: Rng>(
    object_id: ObjectId,
    venue: &str,
    at: Timestamp,
    state: &PriceState,
    parameters: &BookParameters,
    regime: Regime,
    sequence: u64,
    rng: &mut R,
) -> OrderBook {
    let mid = state.price;
    let half_spread = mid * parameters.half_spread_bps(state, regime) / 10_000.0;
    let tick = parameters.tick_size.to_f64().max(1e-9);

    // Depth thins in stress, and unevenly: one side often empties first.
    let depth_scale = (1.0 / (regime.liquidity_penalty() * state.liquidity_shock)).clamp(0.05, 2.0);
    let skew = rng.uniform(-0.35, 0.35);

    let mut bids = Vec::with_capacity(parameters.levels);
    let mut asks = Vec::with_capacity(parameters.levels);

    // Offsets accumulate rather than being computed from the level index:
    // `half_spread + level * tick * random()` is not monotone in `level`, and a
    // book whose levels are not strictly ordered is malformed.
    let mut offset = half_spread;
    for level in 0..parameters.levels {
        if level > 0 {
            offset += tick * rng.uniform(1.0, 2.2);
        }
        let decay = parameters.depth_decay.powi(level as i32);

        let bid_price = ((mid - offset) / tick).floor() * tick;
        let ask_price = ((mid + offset) / tick).ceil() * tick;
        if bid_price <= 0.0 {
            continue;
        }

        let bid_size =
            parameters.base_touch_size * decay * depth_scale * (1.0 + skew) * rng.uniform(0.6, 1.4);
        let ask_size =
            parameters.base_touch_size * decay * depth_scale * (1.0 - skew) * rng.uniform(0.6, 1.4);

        bids.push(BookLevel {
            price: Decimal::from_f64(bid_price).unwrap_or(Decimal::ZERO),
            size: Decimal::from_f64(bid_size.max(1.0).round()).unwrap_or(Decimal::ONE),
            order_count: 1 + rng.below(8) as u32,
        });
        asks.push(BookLevel {
            price: Decimal::from_f64(ask_price).unwrap_or(Decimal::ZERO),
            size: Decimal::from_f64(ask_size.max(1.0).round()).unwrap_or(Decimal::ONE),
            order_count: 1 + rng.below(8) as u32,
        });
    }

    // Deduplicate levels that collapsed onto the same tick, keeping book order.
    bids.dedup_by(|a, b| a.price == b.price);
    asks.dedup_by(|a, b| a.price == b.price);

    let mut book = OrderBook::from_levels(object_id, venue, at, bids, asks);
    book.sequence = sequence;
    book
}

/// Generate the trades that printed over an interval.
///
/// Aggressor side leans with the step's realised return: a rising price is
/// being bought. That is what makes the order-flow imbalance metric mean
/// something rather than being noise around zero.
pub fn generate_trades<R: Rng>(
    object_id: &ObjectId,
    venue: &str,
    at: Timestamp,
    state: &PriceState,
    parameters: &BookParameters,
    regime: Regime,
    intensity: f64,
    fraction_of_day: f64,
    sequence_base: u64,
    rng: &mut R,
) -> Vec<Trade> {
    let expected = parameters.daily_trade_count * fraction_of_day * intensity.max(0.0);
    let count = rng.poisson(expected.clamp(0.0, 5_000.0));
    if count == 0 {
        return Vec::new();
    }

    // Buy pressure follows recent price action, saturating so an extreme move
    // skews the flow without forcing every print one way.
    let buy_probability = (0.5 + state.momentum * 6.0).clamp(0.05, 0.95);
    let half_spread = state.price * parameters.half_spread_bps(state, regime) / 10_000.0;

    (0..count)
        .map(|i| {
            let is_buy = rng.bernoulli(buy_probability);
            let side = if is_buy { Side::Buy } else { Side::Sell };
            // Most prints are at or inside the touch; some cross further.
            let aggression = rng.uniform(0.6, 1.6);
            let price = if is_buy {
                state.price + half_spread * aggression
            } else {
                state.price - half_spread * aggression
            };
            // Trade sizes are heavy-tailed: many small, occasionally very large.
            let size =
                (parameters.median_trade_size * (rng.normal_with(0.0, 0.9)).exp()).clamp(1.0, 1e7);

            Trade {
                object_id: object_id.clone(),
                venue: venue.to_string(),
                at,
                price: Decimal::from_f64(price.max(1e-6)).unwrap_or(Decimal::ONE),
                size: Decimal::from_f64(size.round()).unwrap_or(Decimal::ONE),
                aggressor: Some(side),
                condition: TradeCondition::Regular,
                trade_id: Some(format!("{}-{}", sequence_base, i)),
                quality: Default::default(),
            }
        })
        .collect()
}
