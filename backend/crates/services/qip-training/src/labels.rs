//! Outcome labels: what happened after a decision instant.
//!
//! A label is computed from data *after* the instant it describes, which makes
//! it the one thing that must never be an input at or before that instant. The
//! builder therefore takes the instant and the horizon explicitly and refuses a
//! window the series does not cover, rather than shortening it: a label built
//! from half a horizon is a different label wearing the same name.
//!
//! Prices are statistics here and are `f64`; the crossing from `Decimal` is the
//! caller's, at the moment it builds a [`MidSeries`].
//!
//! Sign conventions, so a trained model and its reader agree:
//!
//! * `future_return` is `mid(t+h) / mid(t) - 1`.
//! * `adverse_selection` is positive when the price moved *against* the side
//!   that filled, measured from the fill price.
//! * `market_impact` is positive when the price moved in the direction the
//!   order traded, measured from the mid at the decision instant.
//! * `fill_outcome` is `1.0` when a resting limit order would have been traded
//!   through within the horizon, else `0.0`; it is the realised outcome a
//!   fill-probability model is trained on, not a probability.
//! * `opportunity_decay` is the share of the initial edge to a stated target
//!   price that is still available at the horizon.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use std::collections::BTreeMap;

/// The side of an order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    fn sign(self) -> f64 {
        match self {
            Self::Buy => 1.0,
            Self::Sell => -1.0,
        }
    }
}

/// A trade print: when, and at what price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Print {
    pub at: Timestamp,
    pub price: f64,
}

/// Mid prices in time order.
#[derive(Clone, Debug, PartialEq)]
pub struct MidSeries {
    ticks: Vec<(Timestamp, f64)>,
}

impl MidSeries {
    /// Refuses unordered times and non-positive or non-finite prices.
    pub fn new(ticks: Vec<(Timestamp, f64)>) -> Result<Self> {
        for pair in ticks.windows(2) {
            if pair[1].0 <= pair[0].0 {
                return Err(Error::invalid(
                    "mid ticks must be in strictly increasing time order; sort and deduplicate first",
                ));
            }
        }
        if ticks.iter().any(|(_, p)| !p.is_finite() || *p <= 0.0) {
            return Err(Error::invalid("a mid price must be finite and positive"));
        }
        Ok(Self { ticks })
    }

    /// The last mid at or before `at`.
    fn at_or_before(&self, at: Timestamp) -> Option<f64> {
        let end = self.ticks.partition_point(|(t, _)| *t <= at);
        end.checked_sub(1).map(|i| self.ticks[i].1)
    }

    /// The mid at `at`, refusing an instant outside the observed span.
    fn require(&self, at: Timestamp) -> Result<f64> {
        let last = self.ticks.last().map(|(t, _)| *t);
        if last.is_none_or(|last| at > last) {
            return Err(Error::unavailable(format!(
                "no mid observed through {at}; the label's horizon is not fully covered"
            )));
        }
        self.at_or_before(at)
            .ok_or_else(|| Error::unavailable(format!("no mid observed at or before {at}")))
    }
}

/// The builder: one series, one decision instant, any number of horizons.
#[derive(Debug)]
pub struct LabelBuilder<'a> {
    mids: &'a MidSeries,
}

impl<'a> LabelBuilder<'a> {
    pub fn new(mids: &'a MidSeries) -> Self {
        Self { mids }
    }

    fn end(at: Timestamp, horizon: Duration) -> Result<Timestamp> {
        if horizon.as_nanos() <= 0 {
            return Err(Error::invalid("a label horizon must be positive"));
        }
        Ok(at.saturating_add(horizon))
    }

    /// `mid(t+h) / mid(t) - 1`.
    pub fn future_return(&self, at: Timestamp, horizon: Duration) -> Result<f64> {
        let start = self.mids.require(at)?;
        let end = self.mids.require(Self::end(at, horizon)?)?;
        Ok(end / start - 1.0)
    }

    /// Square root of the summed squared log returns of the ticks in
    /// `(t, t+h]`, taken from the mid at `t`.
    pub fn realized_volatility(&self, at: Timestamp, horizon: Duration) -> Result<f64> {
        let end = Self::end(at, horizon)?;
        let mut previous = self.mids.require(at)?;
        self.mids.require(end)?;
        let mut sum = 0.0;
        for (t, p) in &self.mids.ticks {
            if *t > at && *t <= end {
                sum += (p / previous).ln().powi(2);
                previous = *p;
            }
        }
        Ok(sum.sqrt())
    }

    /// Price movement against a fill, from the fill price to `mid(t+h)`.
    pub fn adverse_selection(
        &self,
        side: Side,
        fill_price: f64,
        filled_at: Timestamp,
        horizon: Duration,
    ) -> Result<f64> {
        if !fill_price.is_finite() || fill_price <= 0.0 {
            return Err(Error::invalid("a fill price must be finite and positive"));
        }
        let after = self.mids.require(Self::end(filled_at, horizon)?)?;
        Ok(-side.sign() * (after - fill_price) / fill_price)
    }

    /// Price movement in the traded direction, from the mid at `t` to `mid(t+h)`.
    pub fn market_impact(&self, side: Side, at: Timestamp, horizon: Duration) -> Result<f64> {
        let before = self.mids.require(at)?;
        let after = self.mids.require(Self::end(at, horizon)?)?;
        Ok(side.sign() * (after - before) / before)
    }

    /// Whether a resting limit order at `limit` would have been traded
    /// through by a print in `(t, t+h]`.
    pub fn fill_outcome(
        &self,
        side: Side,
        limit: f64,
        at: Timestamp,
        horizon: Duration,
        prints: &[Print],
    ) -> Result<f64> {
        let end = Self::end(at, horizon)?;
        self.mids.require(end)?;
        let filled = prints.iter().any(|p| {
            p.at > at
                && p.at <= end
                && match side {
                    Side::Buy => p.price <= limit,
                    Side::Sell => p.price >= limit,
                }
        });
        Ok(if filled { 1.0 } else { 0.0 })
    }

    /// The fraction of the initial edge to `target` still available at `t+h`.
    ///
    /// Refuses a zero initial edge: the ratio is undefined and returning one
    /// would read as an opportunity that never decays.
    pub fn opportunity_decay(
        &self,
        side: Side,
        target: f64,
        at: Timestamp,
        horizon: Duration,
    ) -> Result<f64> {
        let start = self.mids.require(at)?;
        let after = self.mids.require(Self::end(at, horizon)?)?;
        let initial = side.sign() * (target - start);
        if initial == 0.0 {
            return Err(Error::invalid(
                "the opportunity had no edge at the decision instant, so it has no decay",
            ));
        }
        Ok(side.sign() * (target - after) / initial)
    }

    /// One label at every horizon, keyed by the horizon in nanoseconds so the
    /// output order is a function of the horizons alone.
    pub fn at_horizons(
        &self,
        horizons: &[Duration],
        label: impl Fn(&Self, Duration) -> Result<f64>,
    ) -> Result<BTreeMap<i64, f64>> {
        horizons
            .iter()
            .map(|h| label(self, *h).map(|v| (h.as_nanos(), v)))
            .collect()
    }
}
