//! Term structures: yield curves, forward curves, volatility term structures.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Timestamp};
use qip_numerics::interpolate::{Curve, Method};
use serde::{Deserialize, Serialize};

/// Two tenors closer together than this are the same maturity quoted twice.
///
/// A day is `1/365.25 ≈ 2.7e-3` years, so this is roughly four minutes of
/// tenor: far below any maturity a curve distinguishes, and far above the
/// rounding of a tenor computed from two timestamps.
const DISTINCT_TENOR_YEARS: f64 = 1e-9;

/// One observed point on a curve.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Tenor in years.
    pub tenor_years: f64,
    /// Rate or level as a decimal, e.g. 0.0435 for 4.35%.
    pub value: f64,
}

/// A curve observed at an instant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TermStructure {
    pub name: String,
    pub currency: Currency,
    pub as_of: Timestamp,
    points: Vec<CurvePoint>,
    #[serde(skip, default = "default_curve")]
    interpolator: Option<Curve>,
}

fn default_curve() -> Option<Curve> {
    None
}

impl TermStructure {
    /// Build a curve from observed points.
    ///
    /// Monotone cubic interpolation is used deliberately: a natural spline can
    /// overshoot between two quoted tenors and manufacture a negative forward
    /// rate that no market participant would quote.
    ///
    /// # Refusals
    ///
    /// Every one of these was previously a silent correction, and each is a
    /// caller bug that the correction let survive into a discount factor:
    ///
    /// * A **duplicated maturity** is refused naming the tenor. This method
    ///   used to `dedup_by` a tenor within `1e-12` of its neighbour, so a
    ///   vendor publishing the 10y point twice at two different yields had one
    ///   of them dropped — and which one depended on the sort, which was not
    ///   stable across the two orders the same file can arrive in. A curve
    ///   that interpolates differently on a replay than it did live is not a
    ///   replay.
    /// * A **negative tenor** is refused. A point before the curve's own
    ///   observation instant has no meaning, and the monotone interpolator
    ///   accepted it as the new front end, silently re-anchoring flat
    ///   extrapolation onto a rate nobody quoted.
    /// * A **non-finite tenor or value** is refused. `NaN` compared `Equal`
    ///   under the sort below, so it landed wherever the input happened to put
    ///   it and poisoned every interpolated rate downstream of it.
    ///
    /// Reordering the points is not a correction: the order a vendor lists
    /// tenors in carries no information, and the curve is defined by the set.
    pub fn new(
        name: impl Into<String>,
        currency: Currency,
        as_of: Timestamp,
        mut points: Vec<CurvePoint>,
    ) -> Result<Self> {
        if points.is_empty() {
            return Err(Error::invalid("a term structure needs at least one point"));
        }
        for point in &points {
            if !point.tenor_years.is_finite() {
                return Err(Error::invalid(format!(
                    "tenor {} is not a finite number of years; supply the tenor the point was \
                     quoted at rather than a placeholder",
                    point.tenor_years
                )));
            }
            if !point.value.is_finite() {
                return Err(Error::invalid(format!(
                    "the value quoted at tenor {} is {}, which is not a finite rate; drop the \
                     point rather than curving through it",
                    point.tenor_years, point.value
                )));
            }
            if point.tenor_years < 0.0 {
                return Err(Error::invalid(format!(
                    "tenor {} is negative; a term structure is quoted forward from its as-of \
                     instant, so re-anchor the curve rather than quoting a past tenor",
                    point.tenor_years
                )));
            }
        }
        // Every tenor is finite by the loop above, so this comparison is total
        // and the `unwrap_or` arm is unreachable rather than a swallowed NaN.
        points.sort_by(|a, b| {
            a.tenor_years
                .partial_cmp(&b.tenor_years)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for pair in points.windows(2) {
            if (pair[1].tenor_years - pair[0].tenor_years).abs() < DISTINCT_TENOR_YEARS {
                return Err(Error::invalid(format!(
                    "tenor {} is quoted twice, at {} and {}; reconcile the duplicate upstream \
                     rather than letting the curve pick one",
                    pair[0].tenor_years, pair[0].value, pair[1].value
                )));
            }
        }

        let xs: Vec<f64> = points.iter().map(|p| p.tenor_years).collect();
        let ys: Vec<f64> = points.iter().map(|p| p.value).collect();
        let interpolator = Curve::new(xs, ys, Method::MonotoneCubic)?;

        Ok(Self {
            name: name.into(),
            currency,
            as_of,
            points,
            interpolator: Some(interpolator),
        })
    }

    pub fn points(&self) -> &[CurvePoint] {
        &self.points
    }

    /// Rate at an arbitrary tenor, flat-extrapolated beyond the quoted range.
    pub fn rate_at(&self, tenor_years: f64) -> f64 {
        match &self.interpolator {
            Some(curve) => curve.value_at(tenor_years),
            None => self.nearest_value(tenor_years),
        }
    }

    /// Instantaneous forward rate implied between two tenors.
    ///
    /// From the zero rates `z1`, `z2` at `t1 < t2` under continuous
    /// compounding: `f = (z2*t2 - z1*t1) / (t2 - t1)`.
    pub fn forward_rate(&self, from_years: f64, to_years: f64) -> Option<f64> {
        if to_years <= from_years || from_years < 0.0 {
            return None;
        }
        let z1 = self.rate_at(from_years);
        let z2 = self.rate_at(to_years);
        Some((z2 * to_years - z1 * from_years) / (to_years - from_years))
    }

    /// Discount factor to `tenor_years` under continuous compounding.
    pub fn discount_factor(&self, tenor_years: f64) -> f64 {
        (-self.rate_at(tenor_years) * tenor_years).exp()
    }

    /// Present value of `amount` receivable at `tenor_years`.
    ///
    /// **This is the crossing point between the statistical half of this file
    /// and the money half.** A rate and a discount factor are statistics and
    /// stay `f64`; a present value is money and is [`Decimal`] from here on.
    /// The conversion happens once, on the factor, and the multiplication that
    /// produces the answer is exact decimal arithmetic — so two callers
    /// discounting the same amount at the same tenor get the same cents, which
    /// `f64` multiplication does not guarantee across a re-association.
    ///
    /// Refuses rather than returning a plausible number: a negative or
    /// non-finite tenor has no discount factor, and a factor that cannot be
    /// represented as a `Decimal` means the curve is quoting a rate no
    /// valuation should be built on.
    pub fn present_value(&self, amount: Decimal, tenor_years: f64) -> Result<Decimal> {
        if !tenor_years.is_finite() {
            return Err(Error::invalid(format!(
                "cannot discount to a tenor of {tenor_years} years; supply the instrument's own \
                 time to the cashflow"
            )));
        }
        if tenor_years < 0.0 {
            return Err(Error::invalid(format!(
                "cannot discount to a tenor of {tenor_years} years; a cashflow already received \
                 is booked, not discounted"
            )));
        }
        let factor = self.discount_factor(tenor_years);
        let factor = Decimal::from_f64(factor).ok_or_else(|| {
            Error::numeric(format!(
                "the discount factor {factor} at tenor {tenor_years} on curve {} is not \
                 representable; the curve is quoting a rate no valuation should use",
                self.name
            ))
        })?;
        amount.checked_mul(factor).ok_or_else(|| {
            Error::numeric(format!(
                "discounting {amount} at tenor {tenor_years} on curve {} overflowed",
                self.name
            ))
        })
    }

    /// Slope between two tenors, in basis points — the classic 2s10s measure.
    pub fn slope_bps(&self, short_years: f64, long_years: f64) -> f64 {
        (self.rate_at(long_years) - self.rate_at(short_years)) * 10_000.0
    }

    /// True when any segment of the curve slopes downward.
    ///
    /// An inverted curve is one of the regime signals the macro agent watches.
    pub fn is_inverted(&self) -> bool {
        self.points
            .windows(2)
            .any(|w| w[1].value < w[0].value - 1e-9)
    }

    /// Shift the whole curve in parallel, in basis points.
    pub fn shifted(&self, bps: f64) -> Result<Self> {
        let shifted: Vec<CurvePoint> = self
            .points
            .iter()
            .map(|p| CurvePoint {
                tenor_years: p.tenor_years,
                value: p.value + bps / 10_000.0,
            })
            .collect();
        Self::new(self.name.clone(), self.currency, self.as_of, shifted)
    }

    /// Steepen or flatten: rotate about `pivot_years` by `bps` at the long end.
    pub fn rotated(&self, pivot_years: f64, bps: f64) -> Result<Self> {
        let span = self
            .points
            .last()
            .map(|p| p.tenor_years - pivot_years)
            .filter(|s| s.abs() > 1e-9)
            .unwrap_or(1.0);
        let rotated: Vec<CurvePoint> = self
            .points
            .iter()
            .map(|p| CurvePoint {
                tenor_years: p.tenor_years,
                value: p.value + (p.tenor_years - pivot_years) / span * bps / 10_000.0,
            })
            .collect();
        Self::new(self.name.clone(), self.currency, self.as_of, rotated)
    }

    fn nearest_value(&self, tenor_years: f64) -> f64 {
        self.points
            .iter()
            .min_by(|a, b| {
                (a.tenor_years - tenor_years)
                    .abs()
                    .partial_cmp(&(b.tenor_years - tenor_years).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map_or(0.0, |p| p.value)
    }
}
