//! Term structures: yield curves, forward curves, volatility term structures.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Timestamp};
use qip_numerics::interpolate::{Curve, Method};
use serde::{Deserialize, Serialize};

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
    pub fn new(
        name: impl Into<String>,
        currency: Currency,
        as_of: Timestamp,
        mut points: Vec<CurvePoint>,
    ) -> Result<Self> {
        if points.is_empty() {
            return Err(Error::invalid("a term structure needs at least one point"));
        }
        points.sort_by(|a, b| {
            a.tenor_years
                .partial_cmp(&b.tenor_years)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        points.dedup_by(|a, b| (a.tenor_years - b.tenor_years).abs() < 1e-12);

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
