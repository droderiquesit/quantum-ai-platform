//! Forecasting where capital will be needed, with an interval around every
//! number.
//!
//! The rule this crate exists to implement is *do not wait until an opportunity
//! exists to move capital*. That rule is only safe if the forecast admits how
//! much it does not know. A point forecast used for pre-positioning is how
//! capital ends up in the wrong place with confidence: the number reads as a
//! fact, the wire goes out, and the fact turns out to have been the centre of a
//! distribution nobody looked at the width of.
//!
//! So there is no way to express a demand forecast in this crate without an
//! [`Interval`]. [`Interval`]'s bounds are private and its constructor refuses
//! an out-of-order triple, so a forecast cannot be widened, narrowed or
//! flattened into a point after it has been produced.
//!
//! The interval is a **prediction interval**, not a confidence interval on the
//! fitted mean. The distinction is the difference between "where the average
//! demand lies" and "where next week's demand lies", and pre-positioning cares
//! only about the second. The arithmetic is the textbook one — residual
//! dispersion, plus the uncertainty in the fit, plus a leverage term that
//! widens the further ahead the forecast reaches — computed off
//! [`qip_numerics::stats::linear_fit`] rather than restated here.
//!
//! Two consequences follow and both are deliberate:
//!
//! * **Extrapolating further widens the interval.** The leverage term grows
//!   with the square of the distance from the sample's centre, so a fortnight
//!   ahead is visibly less certain than a day ahead rather than equally
//!   certain with a later date on it.
//! * **A short history widens the interval.** Four observations produce a wide
//!   band and the planner will decline to act on it, which is the correct
//!   behaviour for a lane the fabric has barely seen.

use crate::location::CapitalLocation;
use qip_core::error::{Error, Result};
use qip_core::rng::Rng;
use qip_core::{Decimal, Duration, Timestamp};
use qip_numerics::distributions::normal_inv_cdf;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};

/// What kind of capital is being forecast.
///
/// The kinds are not interchangeable, and the difference that matters to this
/// crate is what happens when you are short of one. Being short of trading cash
/// costs an opportunity. Being short of margin costs a liquidation by somebody
/// else, at a price they choose, on a timetable you do not control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemandKind {
    /// Cash needed to fund trading at a venue.
    Cash,
    /// Eligible collateral needed to be posted.
    Collateral,
    /// Currency that has to be sourced through the FX market to exist at all.
    FxFunding,
    /// Inventory a market-making or facilitation book needs on hand.
    Inventory,
    /// Margin a clearing house or prime broker will call for.
    Margin,
}

impl DemandKind {
    /// A stable label for logs and refusal messages.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Cash => "cash",
            Self::Collateral => "collateral",
            Self::FxFunding => "fx_funding",
            Self::Inventory => "inventory",
            Self::Margin => "margin",
        }
    }

    /// Whether failing to meet this demand is a default rather than a missed
    /// trade.
    ///
    /// Margin and collateral are contractual: a shortfall is somebody else
    /// selling your book. Cash, FX funding and inventory shortfalls cost the
    /// trade that could not be done, which is expensive but survivable. The
    /// planner reads this to set how much harder a shortfall is penalised than
    /// an equivalent surplus.
    pub const fn is_contractual(&self) -> bool {
        matches!(self, Self::Collateral | Self::Margin)
    }
}

/// A forecast bound, with the coverage it claims.
///
/// The bounds are private on purpose. An interval whose ends can be reassigned
/// in place is not an interval; it is three numbers that happen to be sorted
/// today, and the whole safety argument of this crate rests on the planner
/// being unable to read an optimistic bound where a conservative one belongs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    lower: Decimal,
    point: Decimal,
    upper: Decimal,
    confidence: f64,
}

impl Interval {
    /// Build an interval, refusing an out-of-order or unbounded one.
    pub fn new(lower: Decimal, point: Decimal, upper: Decimal, confidence: f64) -> Result<Self> {
        if lower > point || point > upper {
            return Err(Error::invalid(format!(
                "a forecast interval must be ordered; got lower {lower}, point {point}, \
                 upper {upper}"
            )));
        }
        if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
            return Err(Error::invalid(
                "an interval's coverage must be a probability strictly between 0 and 1; \
                 a claimed coverage of 1 is not an interval, it is a promise",
            ));
        }
        Ok(Self {
            lower,
            point,
            upper,
            confidence,
        })
    }

    /// An interval that has collapsed to a single number.
    ///
    /// Available because a genuinely known quantity exists — a margin call
    /// already issued, a settlement already instructed — and forcing a fake
    /// band around it would be its own kind of dishonesty. The coverage is
    /// still recorded, so nothing downstream has to guess whether the collapse
    /// was meant.
    pub fn exact(value: Decimal, confidence: f64) -> Result<Self> {
        Self::new(value, value, value, confidence)
    }

    /// The bound the planner may take a benefit from.
    pub fn lower(&self) -> Decimal {
        self.lower
    }

    /// The central estimate. Never used on its own to justify a transfer.
    pub fn point(&self) -> Decimal {
        self.point
    }

    /// The bound the planner must charge a cost at.
    pub fn upper(&self) -> Decimal {
        self.upper
    }

    /// The coverage the interval claims.
    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Upper less lower, in exact currency units.
    pub fn width(&self) -> Decimal {
        self.upper - self.lower
    }

    /// Width as a fraction of the point estimate — a dimensionless statistic,
    /// so `f64`.
    ///
    /// Infinite where the point estimate is zero and the band is not, which is
    /// the honest answer: a forecast centred on nothing with a wide band around
    /// it carries no relative information at all.
    pub fn relative_width_stat(&self) -> f64 {
        if self.point.is_zero() {
            return if self.width().is_zero() {
                0.0
            } else {
                f64::INFINITY
            };
        }
        self.width().to_f64() / self.point.abs().to_f64()
    }

    /// Whether a realised value fell inside the band.
    pub fn contains(&self, value: Decimal) -> bool {
        value >= self.lower && value <= self.upper
    }

    /// Implied standard deviation of the demand distribution.
    ///
    /// Recovered from the band and its claimed coverage under a normal
    /// approximation, which is the same approximation the band was built with.
    /// A statistic, hence `f64` and named for it.
    pub fn dispersion_stat(&self) -> f64 {
        let z = normal_inv_cdf(1.0 - (1.0 - self.confidence) / 2.0);
        if !z.is_finite() || z <= 0.0 {
            return 0.0;
        }
        (self.width().to_f64() / (2.0 * z)).max(0.0)
    }

    /// The demand level a stated share of the distribution falls below.
    ///
    /// Used for the shortfall buffer, never for the decision to move at all:
    /// the gate is taken at [`Self::lower`] whatever this says.
    pub fn quantile(&self, probability: f64) -> Decimal {
        let clamped = probability.clamp(0.001, 0.999);
        let dispersion = self.dispersion_stat();
        if dispersion <= 0.0 {
            return self.point;
        }
        let offset = normal_inv_cdf(clamped) * dispersion;
        Decimal::from_f64(offset)
            .map(|shift| self.point + shift)
            .unwrap_or(self.point)
    }
}

/// One historical reading of demand at a location.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DemandObservation {
    /// When the demand was observed.
    pub at: Timestamp,
    /// How much was needed, in the location's currency.
    pub amount: Decimal,
}

impl DemandObservation {
    /// Record an observation.
    pub fn new(at: Timestamp, amount: Decimal) -> Self {
        Self { at, amount }
    }
}

/// Forecast demand for one kind of capital at one location.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DemandForecast {
    /// The region, currency and venue the demand will arise at.
    pub location: CapitalLocation,
    /// Which kind of capital.
    pub kind: DemandKind,
    /// When the forecast was produced.
    pub as_of: Timestamp,
    /// How far ahead it reaches.
    pub horizon: Duration,
    interval: Interval,
    observations: usize,
}

impl DemandForecast {
    /// Assemble a forecast from a band somebody else produced.
    ///
    /// Refuses a non-positive horizon: a forecast that reaches no distance is
    /// an observation, and pre-positioning against one is a transfer that
    /// arrives after the need it was meant to meet.
    pub fn new(
        location: CapitalLocation,
        kind: DemandKind,
        as_of: Timestamp,
        horizon: Duration,
        interval: Interval,
        observations: usize,
    ) -> Result<Self> {
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(
                "a demand forecast needs a positive horizon to reach into",
            ));
        }
        if interval.lower().is_negative() {
            return Err(Error::invalid(
                "demand cannot be negative; a negative lower bound is a surplus, which is \
                 recalled rather than pre-positioned",
            ));
        }
        Ok(Self {
            location,
            kind,
            as_of,
            horizon,
            interval,
            observations,
        })
    }

    /// The band around the forecast.
    pub fn interval(&self) -> &Interval {
        &self.interval
    }

    /// How many readings the forecast was fitted on. Zero means it was asserted.
    pub fn observations(&self) -> usize {
        self.observations
    }

    /// The instant the demand is expected to bite.
    ///
    /// The number the settlement calendar is checked against: capital that
    /// arrives after this is capital that was not there.
    pub fn needed_by(&self) -> Timestamp {
        self.as_of.saturating_add(self.horizon)
    }

    /// Draw one plausible realisation of this demand.
    ///
    /// For scenario generation and backtests, never for planning — the planner
    /// in this crate draws no random numbers at all. The draw is floored at
    /// zero because negative demand is not a thing that happens; a location
    /// that needs nothing needs nothing.
    pub fn sample(&self, rng: &mut impl Rng) -> Decimal {
        let dispersion = self.interval.dispersion_stat();
        if dispersion <= 0.0 {
            return self.interval.point();
        }
        let draw = rng.normal_with(self.interval.point().to_f64(), dispersion);
        Decimal::from_f64(draw.max(0.0)).unwrap_or(self.interval.point())
    }

    /// A sentence an operator can read next to a proposed transfer.
    pub fn describe(&self) -> String {
        format!(
            "{} demand at {} of {} in [{}, {}] at {:.0}% coverage over {:.1} day(s), \
             fitted on {} observation(s)",
            self.kind.as_str(),
            self.location,
            self.interval.point(),
            self.interval.lower(),
            self.interval.upper(),
            self.interval.confidence() * 100.0,
            self.horizon.as_days_f64(),
            self.observations,
        )
    }
}

/// Fits demand forecasts from observed history.
///
/// Deliberately thin. The estimator is
/// [`qip_numerics::stats::linear_fit`] — already in the tree, already tested,
/// already deterministic — and this type supplies the two things the estimator
/// does not know: that the quantity being predicted is a future observation
/// rather than a fitted mean, and that a lane with four readings deserves a
/// wider band than one with four hundred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemandForecaster {
    confidence: f64,
    floor_dispersion_fraction: f64,
}

impl DemandForecaster {
    /// The coverage the shipped forecaster claims.
    ///
    /// Eighty percent rather than ninety-five. A pre-positioning decision is
    /// taken repeatedly against a modest cost, not once against a catastrophic
    /// one, and a band wide enough to survive a tail event would refuse every
    /// transfer the fabric exists to make. The tail is handled by the shortfall
    /// asymmetry in [`crate::transfer`], which is where it belongs.
    pub const DEFAULT_CONFIDENCE: f64 = 0.80;

    /// The smallest dispersion the forecaster will claim, as a fraction of the
    /// point estimate.
    ///
    /// A history that happens to be identical four times running fits with zero
    /// residual and would otherwise produce a zero-width interval — a point
    /// forecast wearing an interval's clothes, and the exact failure this
    /// module was written to prevent. The floor keeps some humility in the band
    /// however tidy the sample looks.
    pub const DEFAULT_FLOOR_DISPERSION_FRACTION: f64 = 0.05;

    /// A forecaster at the shipped coverage and dispersion floor.
    pub fn new() -> Self {
        Self {
            confidence: Self::DEFAULT_CONFIDENCE,
            floor_dispersion_fraction: Self::DEFAULT_FLOOR_DISPERSION_FRACTION,
        }
    }

    /// Change the coverage the intervals claim.
    pub fn with_confidence(mut self, confidence: f64) -> Result<Self> {
        if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
            return Err(Error::invalid(
                "forecast coverage must be a probability strictly between 0 and 1",
            ));
        }
        self.confidence = confidence;
        Ok(self)
    }

    /// Change the floor on claimed dispersion.
    pub fn with_dispersion_floor(mut self, fraction: f64) -> Result<Self> {
        if !fraction.is_finite() || fraction < 0.0 {
            return Err(Error::invalid(
                "the dispersion floor must be a non-negative fraction",
            ));
        }
        self.floor_dispersion_fraction = fraction;
        Ok(self)
    }

    /// The coverage this forecaster claims.
    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Fit a forecast for one lane.
    ///
    /// Observations may arrive in any order and at irregular spacing; they are
    /// sorted and regressed on elapsed days, so a lane sampled twice on Monday
    /// and once on Friday is not treated as three evenly spaced readings.
    pub fn forecast(
        &self,
        location: CapitalLocation,
        kind: DemandKind,
        history: &[DemandObservation],
        as_of: Timestamp,
        horizon: Duration,
    ) -> Result<DemandForecast> {
        if history.is_empty() {
            return Err(Error::invalid(format!(
                "no history for {} at {location}; a lane the fabric has never observed \
                 gets no forecast rather than a default one",
                kind.as_str()
            )));
        }
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(
                "a demand forecast needs a positive horizon to reach into",
            ));
        }

        let mut sorted: Vec<DemandObservation> = history.to_vec();
        sorted.sort_by_key(|o| o.at.as_nanos());
        let origin = sorted[0].at;
        let days: Vec<f64> = sorted
            .iter()
            .map(|o| o.at.since(origin).as_days_f64())
            .collect();
        let amounts: Vec<f64> = sorted.iter().map(|o| o.amount.to_f64()).collect();
        let target_day = as_of.saturating_add(horizon).since(origin).as_days_f64();

        let z = normal_inv_cdf(1.0 - (1.0 - self.confidence) / 2.0);
        let (point_stat, half_width_stat) = self.fit(&days, &amounts, target_day, z);

        let point = Decimal::from_f64(point_stat.max(0.0))
            .ok_or_else(|| Error::numeric("the demand point estimate was not representable"))?;
        // The floor is applied to the half-width rather than to the residual so
        // it survives every branch below, including the exact-fit one.
        let floored = half_width_stat.max(z * self.floor_dispersion_fraction * point_stat.abs());
        let half = Decimal::from_f64(floored.max(0.0))
            .ok_or_else(|| Error::numeric("the demand interval half-width was not representable"))?;

        let interval = Interval::new(
            (point - half).max(Decimal::ZERO),
            point,
            point + half,
            self.confidence,
        )?;
        DemandForecast::new(location, kind, as_of, horizon, interval, sorted.len())
    }

    /// Point estimate and prediction half-width at `target_day`.
    ///
    /// Three regimes, and the boundaries between them are about how much the
    /// sample can support rather than about taste. One reading supports no
    /// dispersion estimate at all, so the band comes entirely from the floor.
    /// Two supports a spread but not a trend. Three or more supports a fitted
    /// line, and only then does the leverage term — the part that widens as the
    /// forecast reaches further from the sample's centre — mean anything.
    fn fit(&self, days: &[f64], amounts: &[f64], target_day: f64, z: f64) -> (f64, f64) {
        if amounts.len() < 2 {
            return (amounts.first().copied().unwrap_or(0.0), 0.0);
        }
        if amounts.len() < 3 {
            let mean = stats::mean(amounts);
            let sd = stats::stddev(amounts);
            return (mean, z * sd * (1.0 + 1.0 / amounts.len() as f64).sqrt());
        }

        let Ok(fit) = stats::linear_fit(days, amounts) else {
            let mean = stats::mean(amounts);
            let sd = stats::stddev(amounts);
            return (mean, z * sd * (1.0 + 1.0 / amounts.len() as f64).sqrt());
        };
        let intercept = fit.coefficients.first().copied().unwrap_or(0.0);
        let slope = fit.coefficients.get(1).copied().unwrap_or(0.0);
        let point = intercept + slope * target_day;

        let n = amounts.len() as f64;
        let mean_day = stats::mean(days);
        let spread: f64 = days.iter().map(|d| (d - mean_day) * (d - mean_day)).sum();
        // The leverage term. Predicting at the centre of the sample costs
        // nothing extra; predicting a fortnight past its end costs a widening
        // proportional to the square of the distance, which is what stops a
        // long-horizon forecast reading as confidently as a short one.
        let leverage = if spread > 0.0 {
            (target_day - mean_day) * (target_day - mean_day) / spread
        } else {
            0.0
        };
        let half = z * fit.residual_stddev * (1.0 + 1.0 / n + leverage).sqrt();
        (point, if half.is_finite() { half } else { 0.0 })
    }
}

impl Default for DemandForecaster {
    fn default() -> Self {
        Self::new()
    }
}
