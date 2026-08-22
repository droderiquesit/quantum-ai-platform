//! The features the platform ships with.
//!
//! Every one of them is built on the primitives already in `qip-quant` and
//! `qip-numerics` where those exist, and computed here only where they do not
//! — an order-flow imbalance and a microprice deviation are microstructure
//! quantities that have no meaning in a daily research library.
//!
//! The rule they all obey: when the history is short, an input is stale, or
//! the denominator of a ratio is zero, the answer is
//! [`FeatureValue::Undefined`]. Not zero, not the last good value, not a
//! configured default. A default that looks like data is how a strategy comes
//! to trade on nothing at all, and it does so with full confidence because
//! every downstream check sees a number.

use crate::definition::{FeatureContext, FeatureDefinition, ValueKind};
use crate::state::MarketReads;
use qip_contracts::{FeatureKey, FeatureValue};
use qip_core::error::Result;
use qip_core::{Decimal, ObjectId};
use qip_market::book::{OrderBook, Side};
use qip_numerics::stats;

/// A two-sided book we are willing to price off.
///
/// One-sided is not a book, and a crossed one is a book we know to be wrong;
/// deriving a mid from either produces a number with no market behind it.
fn priceable(book: &OrderBook) -> Option<&OrderBook> {
    if book.best_bid().is_none() || book.best_ask().is_none() || book.is_crossed() {
        return None;
    }
    Some(book)
}

/// The mid price: the midpoint of the touch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mid {
    subject: ObjectId,
}

impl Mid {
    pub const NAME: &'static str = "mid";

    pub fn new(subject: ObjectId) -> Self {
        Self { subject }
    }

    pub fn key(subject: &ObjectId) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
    }
}

impl FeatureDefinition for Mid {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Exact
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        Ok(priceable(state.book())
            .and_then(OrderBook::mid)
            .map_or(FeatureValue::Undefined, FeatureValue::Exact))
    }
}

/// The touch spread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spread {
    subject: ObjectId,
}

impl Spread {
    pub const NAME: &'static str = "spread";

    pub fn new(subject: ObjectId) -> Self {
        Self { subject }
    }

    pub fn key(subject: &ObjectId) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
    }
}

impl FeatureDefinition for Spread {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Exact
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        Ok(priceable(state.book())
            .and_then(OrderBook::spread)
            .map_or(FeatureValue::Undefined, FeatureValue::Exact))
    }
}

/// The size-weighted mid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Microprice {
    subject: ObjectId,
}

impl Microprice {
    pub const NAME: &'static str = "microprice";

    pub fn new(subject: ObjectId) -> Self {
        Self { subject }
    }

    pub fn key(subject: &ObjectId) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
    }
}

impl FeatureDefinition for Microprice {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Exact
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        Ok(priceable(state.book())
            .and_then(OrderBook::microprice)
            .map_or(FeatureValue::Undefined, FeatureValue::Exact))
    }
}

/// How far the microprice sits from the mid, in half-spreads.
///
/// The scale matters: an absolute deviation is not comparable between a penny
/// spread and a dollar one, and a strategy calibrated on one instrument would
/// read the other as permanently extreme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicropriceDeviation {
    subject: ObjectId,
}

impl MicropriceDeviation {
    pub const NAME: &'static str = "microprice_deviation";

    pub fn new(subject: ObjectId) -> Self {
        Self { subject }
    }

    pub fn key(subject: &ObjectId) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
    }
}

impl FeatureDefinition for MicropriceDeviation {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject)
    }

    fn dependencies(&self) -> Vec<FeatureKey> {
        vec![
            Microprice::key(&self.subject),
            Mid::key(&self.subject),
            Spread::key(&self.subject),
        ]
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let (Some(microprice), Some(mid), Some(spread)) = (
            ctx.dependency(0).as_exact(),
            ctx.dependency(1).as_exact(),
            ctx.dependency(2).as_exact(),
        ) else {
            return Ok(FeatureValue::Undefined);
        };
        if !spread.is_positive() {
            return Ok(FeatureValue::Undefined);
        }
        let half = spread.to_f64() / 2.0;
        Ok(FeatureValue::Statistic(
            (microprice - mid).to_f64() / half,
        ))
    }
}

/// Depth imbalance over the first `levels` of each side, in `[-1, 1]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookPressure {
    subject: ObjectId,
    levels: usize,
}

impl BookPressure {
    pub const NAME: &'static str = "book_pressure";

    pub fn new(subject: ObjectId, levels: usize) -> Self {
        Self {
            subject,
            levels: levels.max(1),
        }
    }

    pub fn key(subject: &ObjectId, levels: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone()).with("levels", levels.max(1))
    }
}

impl FeatureDefinition for BookPressure {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.levels)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::DEPTH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let book = state.book();
        let bid = book.depth(Side::Buy, self.levels);
        let ask = book.depth(Side::Sell, self.levels);
        // A side with nothing on it is an absent market, not an infinitely
        // pressured one.
        if !bid.is_positive() || !ask.is_positive() {
            return Ok(FeatureValue::Undefined);
        }
        Ok(FeatureValue::Statistic(book.imbalance(self.levels)))
    }
}

/// Annualised realised volatility of the sampled mid over `window` samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealisedVolatility {
    subject: ObjectId,
    window: usize,
}

impl RealisedVolatility {
    pub const NAME: &'static str = "realised_volatility";

    pub fn new(subject: ObjectId, window: usize) -> Self {
        Self {
            subject,
            window: window.max(2),
        }
    }

    pub fn key(subject: &ObjectId, window: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone()).with("window", window.max(2))
    }
}

impl FeatureDefinition for RealisedVolatility {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.window)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let mids = state.recent_mids(self.window + 1);
        if mids.is_empty() {
            return Ok(FeatureValue::Undefined);
        }
        Ok(
            qip_quant::signal::realised_volatility(&mids, self.window, ctx.samples_per_year())
                .map_or(FeatureValue::Undefined, FeatureValue::Statistic),
        )
    }
}

/// Exponential moving average of the sampled mid over `window` samples.
///
/// Computed in exact decimal and seeded from the oldest sample in the window.
/// An EMA with no stated seed is a different number depending on how long the
/// process has been running, and two cells that started at different times
/// would disagree about the same market.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExponentialMovingAverage {
    subject: ObjectId,
    window: usize,
}

impl ExponentialMovingAverage {
    pub const NAME: &'static str = "ema";

    pub fn new(subject: ObjectId, window: usize) -> Self {
        Self {
            subject,
            window: window.max(2),
        }
    }

    pub fn key(subject: &ObjectId, window: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone()).with("window", window.max(2))
    }
}

impl FeatureDefinition for ExponentialMovingAverage {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.window)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Exact
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let samples = state.samples();
        if samples.len() < self.window {
            return Ok(FeatureValue::Undefined);
        }
        let window = &samples[samples.len() - self.window..];
        let periods = i64::try_from(self.window).unwrap_or(i64::MAX);
        let Some(alpha) =
            Decimal::from_int(2).checked_div(Decimal::from_int(periods.saturating_add(1)))
        else {
            return Ok(FeatureValue::Undefined);
        };
        let mut average = window[0].1;
        for (_, mid) in &window[1..] {
            let Some(step) = mid.checked_sub(average).and_then(|gap| gap.checked_mul(alpha)) else {
                return Ok(FeatureValue::Undefined);
            };
            let Some(next) = average.checked_add(step) else {
                return Ok(FeatureValue::Undefined);
            };
            average = next;
        }
        Ok(FeatureValue::Exact(average))
    }
}

/// Order-flow imbalance over the last `window` touch changes, in `[-1, 1]`.
///
/// The signed flow divided by the gross flow, so it reads as a share of
/// activity rather than a quantity. A raw sum is not comparable between a
/// thousand-lot instrument and a one-lot one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderFlowImbalance {
    subject: ObjectId,
    window: usize,
}

impl OrderFlowImbalance {
    pub const NAME: &'static str = "order_flow_imbalance";

    pub fn new(subject: ObjectId, window: usize) -> Self {
        Self {
            subject,
            window: window.max(1),
        }
    }

    pub fn key(subject: &ObjectId, window: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone()).with("window", window.max(1))
    }
}

impl FeatureDefinition for OrderFlowImbalance {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.window)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let flow = state.flow();
        if flow.len() < self.window {
            return Ok(FeatureValue::Undefined);
        }
        let recent = &flow[flow.len() - self.window..];
        let gross: f64 = recent.iter().map(|value| value.abs()).sum();
        if gross <= 0.0 {
            // No flow at all has no direction. Reporting balance would say the
            // two sides were equally busy, which is a different market.
            return Ok(FeatureValue::Undefined);
        }
        let net: f64 = recent.iter().sum();
        Ok(FeatureValue::Statistic((net / gross).clamp(-1.0, 1.0)))
    }
}

/// Autocorrelation of trade signs at `lag`, over the last `window` prints.
///
/// Positive values are the signature of a large order being worked in slices;
/// it is the most robust microstructure regularity there is, and the reason a
/// passive quote gets run over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeSignAutocorrelation {
    subject: ObjectId,
    window: usize,
    lag: usize,
}

impl TradeSignAutocorrelation {
    pub const NAME: &'static str = "trade_sign_autocorrelation";

    pub fn new(subject: ObjectId, window: usize, lag: usize) -> Self {
        Self {
            subject,
            window: window.max(4),
            lag: lag.max(1),
        }
    }

    pub fn key(subject: &ObjectId, window: usize, lag: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
            .with("window", window.max(4))
            .with("lag", lag.max(1))
    }
}

impl FeatureDefinition for TradeSignAutocorrelation {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.window, self.lag)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TRADES
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let trades = state.trades();
        if trades.len() < self.window || self.window <= self.lag + 1 {
            return Ok(FeatureValue::Undefined);
        }
        let signs: Vec<f64> = trades[trades.len() - self.window..]
            .iter()
            .map(|print| f64::from(print.sign))
            .collect();
        // An unvarying sign series has no autocorrelation to measure; the
        // library reports zero for it, which reads as "no persistence" when
        // the truth is "nothing to compare".
        if stats::variance(&signs) <= 0.0 {
            return Ok(FeatureValue::Undefined);
        }
        Ok(FeatureValue::Statistic(stats::autocorrelation(
            &signs, self.lag,
        )))
    }
}

/// Correlation of sampled log returns between two instruments.
///
/// Aligned on the sampling grid, not on update counts. Two instruments do not
/// quote at the same moments, and pairing their *n*th updates correlates two
/// different stretches of the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollingCorrelation {
    subject: ObjectId,
    other: ObjectId,
    window: usize,
}

impl RollingCorrelation {
    pub const NAME: &'static str = "rolling_correlation";

    pub fn new(subject: ObjectId, other: ObjectId, window: usize) -> Self {
        Self {
            subject,
            other,
            window: window.max(2),
        }
    }

    pub fn key(subject: &ObjectId, other: &ObjectId, window: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
            .with("with", other.as_str())
            .with("window", window.max(2))
    }
}

impl FeatureDefinition for RollingCorrelation {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, &self.other, self.window)
    }

    fn subjects(&self) -> Vec<ObjectId> {
        vec![self.subject.clone(), self.other.clone()]
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let (Some(left), Some(right)) = (ctx.fresh(&self.subject), ctx.fresh(&self.other)) else {
            return Ok(FeatureValue::Undefined);
        };
        let Some((left_mids, right_mids)) =
            aligned_tail(left.samples(), right.samples(), self.window + 1)
        else {
            return Ok(FeatureValue::Undefined);
        };
        let left_returns = stats::log_returns(&left_mids);
        let right_returns = stats::log_returns(&right_mids);
        if stats::stddev(&left_returns) <= 0.0 || stats::stddev(&right_returns) <= 0.0 {
            // A series that did not move has no correlation with anything.
            return Ok(FeatureValue::Undefined);
        }
        Ok(FeatureValue::Statistic(stats::correlation(
            &left_returns,
            &right_returns,
        )))
    }
}

/// The last `count` mids the two series share a sampling bucket for.
///
/// Both inputs are strictly ascending in bucket, so one pass suffices.
fn aligned_tail(
    left: &[(i64, Decimal)],
    right: &[(i64, Decimal)],
    count: usize,
) -> Option<(Vec<f64>, Vec<f64>)> {
    if count == 0 {
        return None;
    }
    let mut left_values = Vec::new();
    let mut right_values = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        match left[i].0.cmp(&right[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                left_values.push(left[i].1.to_f64());
                right_values.push(right[j].1.to_f64());
                i += 1;
                j += 1;
            }
        }
    }
    if left_values.len() < count {
        return None;
    }
    let start = left_values.len() - count;
    Some((left_values[start..].to_vec(), right_values[start..].to_vec()))
}

/// Nanoseconds since the last print that updated the last sale.
///
/// The one feature that moves with the clock rather than with the message
/// stream: an instrument that has not printed for a minute is a different
/// market from one that printed a moment ago, and no message says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeSinceLastTrade {
    subject: ObjectId,
}

impl TimeSinceLastTrade {
    pub const NAME: &'static str = "time_since_last_trade";

    pub fn new(subject: ObjectId) -> Self {
        Self { subject }
    }

    pub fn key(subject: &ObjectId) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone())
    }
}

impl FeatureDefinition for TimeSinceLastTrade {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject)
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TRADES
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Count
    }

    fn time_sensitive(&self) -> bool {
        true
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        // Deliberately not `fresh`: this feature measures staleness, so
        // refusing to answer once the instrument is stale would suppress the
        // very reading a consumer needs.
        let Some(state) = ctx.instrument(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let Some(last) = state.last_trade_at() else {
            // Never having traded is not the same as having traded now.
            return Ok(FeatureValue::Undefined);
        };
        let elapsed = ctx.as_of().since(last).as_nanos().max(0);
        Ok(FeatureValue::Count(elapsed as u64))
    }
}

/// Where the current spread sits in its own recent distribution, in `[0, 1]`.
///
/// A spread is only wide or narrow relative to the instrument's own history;
/// a tick in one name is a crisis in another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpreadPercentile {
    subject: ObjectId,
    window: usize,
}

impl SpreadPercentile {
    pub const NAME: &'static str = "spread_percentile";

    pub fn new(subject: ObjectId, window: usize) -> Self {
        Self {
            subject,
            window: window.max(2),
        }
    }

    pub fn key(subject: &ObjectId, window: usize) -> FeatureKey {
        FeatureKey::new(Self::NAME, subject.clone()).with("window", window.max(2))
    }
}

impl FeatureDefinition for SpreadPercentile {
    fn key(&self) -> FeatureKey {
        Self::key(&self.subject, self.window)
    }

    fn dependencies(&self) -> Vec<FeatureKey> {
        vec![Spread::key(&self.subject)]
    }

    fn reads(&self) -> MarketReads {
        MarketReads::TOUCH
    }

    fn value_kind(&self) -> ValueKind {
        ValueKind::Statistic
    }

    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue> {
        let Some(current) = ctx.dependency(0).as_exact() else {
            return Ok(FeatureValue::Undefined);
        };
        let Some(state) = ctx.fresh(&self.subject) else {
            return Ok(FeatureValue::Undefined);
        };
        let spreads = state.spreads();
        if spreads.len() < self.window {
            return Ok(FeatureValue::Undefined);
        }
        let recent = &spreads[spreads.len() - self.window..];
        let below = recent.iter().filter(|spread| **spread <= current).count();
        Ok(FeatureValue::Statistic(
            below as f64 / recent.len() as f64,
        ))
    }
}

/// A representative set of features for one instrument.
///
/// Registration order does not matter — the graph resolves dependencies in
/// either direction — but the suite is listed dependency-first so a reader can
/// follow it.
pub fn standard_suite(subject: &ObjectId) -> Vec<Box<dyn FeatureDefinition>> {
    vec![
        Box::new(Mid::new(subject.clone())),
        Box::new(Spread::new(subject.clone())),
        Box::new(Microprice::new(subject.clone())),
        Box::new(MicropriceDeviation::new(subject.clone())),
        Box::new(BookPressure::new(subject.clone(), 5)),
        Box::new(RealisedVolatility::new(subject.clone(), 20)),
        Box::new(ExponentialMovingAverage::new(subject.clone(), 20)),
        Box::new(OrderFlowImbalance::new(subject.clone(), 10)),
        Box::new(TradeSignAutocorrelation::new(subject.clone(), 20, 1)),
        Box::new(TimeSinceLastTrade::new(subject.clone())),
        Box::new(SpreadPercentile::new(subject.clone(), 20)),
    ]
}
