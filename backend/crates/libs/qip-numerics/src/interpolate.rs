//! Interpolation for curves.
//!
//! Yield curves, volatility surfaces and forward curves are all sampled at a
//! handful of tenors and read at arbitrary points. Monotone cubic interpolation
//! is the default because a natural spline will happily invent an arbitrage
//! between two quoted points by overshooting.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Piecewise linear. Never overshoots; kinked derivatives.
    Linear,
    /// Piecewise constant, taking the value at or before the query point.
    PreviousStep,
    /// Fritsch-Carlson monotone cubic. Smooth and overshoot-free.
    MonotoneCubic,
    /// Natural cubic spline. Smoothest, but may overshoot — use knowingly.
    NaturalCubic,
}

/// A 1-D interpolated curve over strictly increasing knots.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Curve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    method: Method,
    /// Precomputed slopes for the cubic methods.
    slopes: Vec<f64>,
}

impl Curve {
    /// Build a curve. Knots must be strictly increasing in `xs`.
    pub fn new(xs: Vec<f64>, ys: Vec<f64>, method: Method) -> Result<Self> {
        if xs.len() != ys.len() {
            return Err(Error::invalid("curve x and y lengths differ"));
        }
        if xs.is_empty() {
            return Err(Error::invalid("curve needs at least one point"));
        }
        if xs.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Error::invalid("curve x values must be strictly increasing"));
        }
        if !xs.iter().chain(&ys).all(|v| v.is_finite()) {
            return Err(Error::numeric("curve contains non-finite values"));
        }
        let slopes = match method {
            Method::MonotoneCubic => monotone_slopes(&xs, &ys),
            Method::NaturalCubic => natural_second_derivatives(&xs, &ys),
            _ => Vec::new(),
        };
        Ok(Self {
            xs,
            ys,
            method,
            slopes,
        })
    }

    pub fn linear(xs: Vec<f64>, ys: Vec<f64>) -> Result<Self> {
        Self::new(xs, ys, Method::Linear)
    }

    pub fn monotone(xs: Vec<f64>, ys: Vec<f64>) -> Result<Self> {
        Self::new(xs, ys, Method::MonotoneCubic)
    }

    pub fn knots(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.xs.iter().copied().zip(self.ys.iter().copied())
    }

    pub fn domain(&self) -> (f64, f64) {
        (self.xs[0], self.xs[self.xs.len() - 1])
    }

    /// Evaluate at `x`, clamping to the end values outside the knot range.
    ///
    /// Flat extrapolation is deliberate: extrapolating a curve fitted to
    /// 2y-30y out to 50y with a cubic produces numbers no desk would quote.
    pub fn value_at(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if n == 1 {
            return self.ys[0];
        }
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }

        let i = match self
            .xs
            .binary_search_by(|v| v.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(exact) => return self.ys[exact],
            Err(insert) => insert - 1,
        };

        let (x0, x1) = (self.xs[i], self.xs[i + 1]);
        let (y0, y1) = (self.ys[i], self.ys[i + 1]);
        let h = x1 - x0;
        let t = (x - x0) / h;

        match self.method {
            Method::PreviousStep => y0,
            Method::Linear => y0 + t * (y1 - y0),
            Method::MonotoneCubic => {
                let (m0, m1) = (self.slopes[i], self.slopes[i + 1]);
                // Hermite basis functions.
                let t2 = t * t;
                let t3 = t2 * t;
                (2.0 * t3 - 3.0 * t2 + 1.0) * y0
                    + (t3 - 2.0 * t2 + t) * h * m0
                    + (-2.0 * t3 + 3.0 * t2) * y1
                    + (t3 - t2) * h * m1
            }
            Method::NaturalCubic => {
                let (d0, d1) = (self.slopes[i], self.slopes[i + 1]);
                let a = (x1 - x) / h;
                let b = (x - x0) / h;
                a * y0 + b * y1 + ((a * a * a - a) * d0 + (b * b * b - b) * d1) * (h * h) / 6.0
            }
        }
    }

    /// First derivative by central differences — the forward rate of a
    /// discount curve, or a volatility skew's slope.
    pub fn derivative_at(&self, x: f64) -> f64 {
        let (lo, hi) = self.domain();
        let span = (hi - lo).max(1e-9);
        let h = (span * 1e-6).max(1e-9);
        let left = (x - h).max(lo);
        let right = (x + h).min(hi);
        if right <= left {
            return 0.0;
        }
        (self.value_at(right) - self.value_at(left)) / (right - left)
    }

    /// Integral over `[a, b]` by composite Simpson's rule.
    pub fn integral(&self, a: f64, b: f64) -> f64 {
        if b <= a {
            return 0.0;
        }
        let steps = 512;
        let h = (b - a) / steps as f64;
        let mut total = self.value_at(a) + self.value_at(b);
        for i in 1..steps {
            let x = a + i as f64 * h;
            total += self.value_at(x) * if i % 2 == 0 { 2.0 } else { 4.0 };
        }
        total * h / 3.0
    }
}

/// Fritsch-Carlson slope limiting: guarantees the interpolant is monotone
/// wherever the data is.
fn monotone_slopes(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let deltas: Vec<f64> = (0..n - 1)
        .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
        .collect();
    let mut m = vec![0.0; n];
    m[0] = deltas[0];
    m[n - 1] = deltas[n - 2];
    for i in 1..n - 1 {
        if deltas[i - 1] * deltas[i] <= 0.0 {
            // A local extremum: flatten so the curve cannot overshoot.
            m[i] = 0.0;
        } else {
            m[i] = 0.5 * (deltas[i - 1] + deltas[i]);
        }
    }
    for i in 0..n - 1 {
        if deltas[i].abs() < 1e-15 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let alpha = m[i] / deltas[i];
        let beta = m[i + 1] / deltas[i];
        let magnitude = (alpha * alpha + beta * beta).sqrt();
        if magnitude > 3.0 {
            let scale = 3.0 / magnitude;
            m[i] = scale * alpha * deltas[i];
            m[i + 1] = scale * beta * deltas[i];
        }
    }
    m
}

/// Second derivatives for a natural cubic spline (zero curvature at the ends).
fn natural_second_derivatives(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let mut second = vec![0.0; n];
    if n < 3 {
        return second;
    }
    let mut u = vec![0.0; n];
    for i in 1..n - 1 {
        let sig = (xs[i] - xs[i - 1]) / (xs[i + 1] - xs[i - 1]);
        let p = sig * second[i - 1] + 2.0;
        second[i] = (sig - 1.0) / p;
        u[i] =
            (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]) - (ys[i] - ys[i - 1]) / (xs[i] - xs[i - 1]);
        u[i] = (6.0 * u[i] / (xs[i + 1] - xs[i - 1]) - sig * u[i - 1]) / p;
    }
    for i in (0..n - 1).rev() {
        second[i] = second[i] * second[i + 1] + u[i];
    }
    second
}
