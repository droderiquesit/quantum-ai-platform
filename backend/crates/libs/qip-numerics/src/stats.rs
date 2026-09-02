//! Descriptive statistics, estimators and regression.
//!
//! Sample conventions follow the ones used in institutional risk reporting and
//! are stated explicitly on each function, because the difference between a
//! population and a sample denominator is the difference between two risk
//! numbers that both look plausible.

use crate::matrix::Matrix;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub fn mean(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().sum::<f64>() / x.len() as f64
}

/// Sample variance (denominator `n - 1`). Zero for fewer than two points.
pub fn variance(x: &[f64]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (x.len() - 1) as f64
}

/// Population variance (denominator `n`).
pub fn variance_population(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / x.len() as f64
}

pub fn stddev(x: &[f64]) -> f64 {
    variance(x).sqrt()
}

/// Fisher (excess-free) skewness using the sample standard deviation.
pub fn skewness(x: &[f64]) -> f64 {
    let n = x.len();
    if n < 3 {
        return 0.0;
    }
    let m = mean(x);
    let sd = stddev(x);
    if sd <= 0.0 {
        return 0.0;
    }
    let sum: f64 = x.iter().map(|v| ((v - m) / sd).powi(3)).sum();
    let n = n as f64;
    n / ((n - 1.0) * (n - 2.0)) * sum
}

/// Excess kurtosis: 0.0 for a normal distribution.
pub fn excess_kurtosis(x: &[f64]) -> f64 {
    let n = x.len();
    if n < 4 {
        return 0.0;
    }
    let m = mean(x);
    let sd = stddev(x);
    if sd <= 0.0 {
        return 0.0;
    }
    let sum: f64 = x.iter().map(|v| ((v - m) / sd).powi(4)).sum();
    let n = n as f64;
    let numerator = n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * sum;
    let correction = 3.0 * (n - 1.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0));
    numerator - correction
}

/// Empirical quantile with linear interpolation between order statistics
/// (the "type 7" definition, matching most analytics packages).
pub fn quantile(sorted_or_not: &[f64], q: f64) -> f64 {
    if sorted_or_not.is_empty() {
        return f64::NAN;
    }
    let mut v: Vec<f64> = sorted_or_not
        .iter()
        .copied()
        .filter(|x| x.is_finite())
        .collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    quantile_sorted(&v, q)
}

/// [`quantile`] for input already sorted ascending — the hot path in risk code.
pub fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let q = q.clamp(0.0, 1.0);
    let pos = q * (sorted.len() - 1) as f64;
    let lower = pos.floor() as usize;
    let upper = pos.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let weight = pos - lower as f64;
    sorted[lower] * (1.0 - weight) + sorted[upper] * weight
}

pub fn median(x: &[f64]) -> f64 {
    quantile(x, 0.5)
}

/// Median absolute deviation, scaled to be a consistent estimator of sigma
/// under normality. Robust to the outliers that dominate financial data.
pub fn median_absolute_deviation(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let med = median(x);
    let deviations: Vec<f64> = x.iter().map(|v| (v - med).abs()).collect();
    median(&deviations) * 1.4826
}

/// Sample covariance (denominator `n - 1`).
pub fn covariance(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n < 2 {
        return 0.0;
    }
    let mx = mean(&x[..n]);
    let my = mean(&y[..n]);
    (0..n).map(|i| (x[i] - mx) * (y[i] - my)).sum::<f64>() / (n - 1) as f64
}

/// Pearson correlation. Zero when either series is constant.
pub fn correlation(x: &[f64], y: &[f64]) -> f64 {
    let sx = stddev(x);
    let sy = stddev(y);
    if sx <= 0.0 || sy <= 0.0 {
        return 0.0;
    }
    (covariance(x, y) / (sx * sy)).clamp(-1.0, 1.0)
}

/// Spearman rank correlation — robust to the non-linear relationships common
/// between macro series and asset returns.
pub fn rank_correlation(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n < 2 {
        return 0.0;
    }
    correlation(&ranks(&x[..n]), &ranks(&y[..n]))
}

/// Average ranks, with ties sharing the mean of the positions they occupy.
pub fn ranks(x: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| {
        x[*a]
            .partial_cmp(&x[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && (x[order[j + 1]] - x[order[i]]).abs() < 1e-15 {
            j += 1;
        }
        let average = ((i + j) as f64) / 2.0 + 1.0;
        for &idx in &order[i..=j] {
            out[idx] = average;
        }
        i = j + 1;
    }
    out
}

/// Columns are variables, rows observations.
pub fn covariance_matrix(observations: &Matrix) -> Result<Matrix> {
    let n = observations.rows();
    let k = observations.cols();
    if n < 2 {
        return Err(Error::invalid("covariance needs at least two observations"));
    }
    let means: Vec<f64> = (0..k).map(|c| mean(&observations.column(c))).collect();
    let mut cov = Matrix::zeros(k, k);
    for i in 0..k {
        for j in i..k {
            let mut acc = 0.0;
            for r in 0..n {
                acc += (observations.get(r, i) - means[i]) * (observations.get(r, j) - means[j]);
            }
            let value = acc / (n - 1) as f64;
            cov.set(i, j, value);
            cov.set(j, i, value);
        }
    }
    Ok(cov)
}

/// Correlation matrix derived from a covariance matrix.
pub fn correlation_from_covariance(cov: &Matrix) -> Result<Matrix> {
    if !cov.is_square() {
        return Err(Error::invalid("covariance matrix must be square"));
    }
    let n = cov.rows();
    let sd: Vec<f64> = (0..n).map(|i| cov.get(i, i).max(0.0).sqrt()).collect();
    let mut corr = Matrix::identity(n);
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let denom = sd[i] * sd[j];
            let value = if denom > 0.0 {
                (cov.get(i, j) / denom).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            corr.set(i, j, value);
        }
    }
    Ok(corr)
}

/// Ledoit-Wolf style shrinkage of a sample covariance toward a scaled identity.
///
/// Short return histories produce covariance estimates whose smallest
/// eigenvalues are pure noise; an optimiser will happily lever into exactly
/// those directions. Shrinkage is the cheapest defence and is applied by
/// default in portfolio construction.
pub fn shrink_covariance(cov: &Matrix, intensity: f64) -> Result<Matrix> {
    if !cov.is_square() {
        return Err(Error::invalid("covariance matrix must be square"));
    }
    let intensity = intensity.clamp(0.0, 1.0);
    let n = cov.rows();
    let average_variance = cov.trace() / n as f64;
    let target = Matrix::identity(n).scale(average_variance);
    cov.scale(1.0 - intensity).add(&target.scale(intensity))
}

/// Exponentially weighted mean with decay `lambda` (RiskMetrics uses 0.94).
pub fn ewma(x: &[f64], lambda: f64) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let lambda = lambda.clamp(0.0, 0.999_999);
    let mut weighted = 0.0;
    let mut total = 0.0;
    // Most recent observation carries the largest weight.
    for (age, value) in x.iter().rev().enumerate() {
        let w = (1.0 - lambda) * lambda.powi(age as i32);
        weighted += w * value;
        total += w;
    }
    if total <= 0.0 { 0.0 } else { weighted / total }
}

/// Autocorrelation at `lag`.
pub fn autocorrelation(x: &[f64], lag: usize) -> f64 {
    if lag == 0 {
        return 1.0;
    }
    if x.len() <= lag + 1 {
        return 0.0;
    }
    let m = mean(x);
    let denominator: f64 = x.iter().map(|v| (v - m) * (v - m)).sum();
    if denominator <= 0.0 {
        return 0.0;
    }
    let numerator: f64 = (lag..x.len()).map(|i| (x[i] - m) * (x[i - lag] - m)).sum();
    (numerator / denominator).clamp(-1.0, 1.0)
}

/// Robust z-scores built on the median and MAD.
pub fn robust_z_scores(x: &[f64]) -> Vec<f64> {
    let med = median(x);
    let mad = median_absolute_deviation(x);
    if mad <= 0.0 {
        return vec![0.0; x.len()];
    }
    x.iter().map(|v| (v - med) / mad).collect()
}

/// Trailing window statistics, emitted once the window is full.
pub fn rolling<F: Fn(&[f64]) -> f64>(x: &[f64], window: usize, f: F) -> Vec<f64> {
    if window == 0 || x.len() < window {
        return Vec::new();
    }
    (window..=x.len())
        .map(|end| f(&x[end - window..end]))
        .collect()
}

/// Ordinary least squares with an intercept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Regression {
    /// Index 0 is the intercept; the rest align with the design columns.
    pub coefficients: Vec<f64>,
    pub standard_errors: Vec<f64>,
    pub t_statistics: Vec<f64>,
    pub r_squared: f64,
    pub adjusted_r_squared: f64,
    pub residual_stddev: f64,
    pub observations: usize,
}

impl Regression {
    /// Two-sided p-value for each coefficient under a t distribution.
    pub fn p_values(&self) -> Vec<f64> {
        let df = (self.observations as f64 - self.coefficients.len() as f64).max(1.0);
        self.t_statistics
            .iter()
            .map(|t| 2.0 * (1.0 - crate::distributions::student_t_cdf(t.abs(), df)))
            .collect()
    }

    /// Whether a coefficient clears a significance level, ignoring the intercept.
    pub fn is_significant(&self, index: usize, alpha: f64) -> bool {
        self.p_values().get(index).is_some_and(|p| *p < alpha)
    }
}

/// Fit `y = b0 + b1 x1 + ... + bk xk` by ordinary least squares.
pub fn ols(design: &Matrix, y: &[f64]) -> Result<Regression> {
    let n = design.rows();
    let k = design.cols();
    if y.len() != n {
        return Err(Error::invalid("design rows and response length differ"));
    }
    if n <= k + 1 {
        return Err(Error::invalid(format!(
            "need more than {} observations for {k} regressors",
            k + 1
        )));
    }

    // Prepend the intercept column.
    let mut x = Matrix::zeros(n, k + 1);
    for r in 0..n {
        x.set(r, 0, 1.0);
        for c in 0..k {
            x.set(r, c + 1, design.get(r, c));
        }
    }

    let xt = x.transpose();
    let xtx = xt.matmul(&x)?;
    let xty = xt.mul_vec(y)?;
    let beta = xtx.solve(&xty)?;

    let fitted = x.mul_vec(&beta)?;
    let residuals: Vec<f64> = y.iter().zip(&fitted).map(|(a, b)| a - b).collect();
    let rss: f64 = residuals.iter().map(|r| r * r).sum();
    let y_mean = mean(y);
    let tss: f64 = y.iter().map(|v| (v - y_mean) * (v - y_mean)).sum();

    let df = (n - k - 1) as f64;
    let sigma2 = rss / df;
    let xtx_inv = xtx.inverse()?;
    let standard_errors: Vec<f64> = (0..=k)
        .map(|i| (sigma2 * xtx_inv.get(i, i)).max(0.0).sqrt())
        .collect();
    let t_statistics: Vec<f64> = beta
        .iter()
        .zip(&standard_errors)
        .map(|(b, se)| if *se > 0.0 { b / se } else { 0.0 })
        .collect();

    let r_squared = if tss > 0.0 { 1.0 - rss / tss } else { 0.0 };
    let adjusted_r_squared = if tss > 0.0 && df > 0.0 {
        1.0 - (1.0 - r_squared) * (n as f64 - 1.0) / df
    } else {
        0.0
    };

    Ok(Regression {
        coefficients: beta,
        standard_errors,
        t_statistics,
        r_squared,
        adjusted_r_squared,
        residual_stddev: sigma2.max(0.0).sqrt(),
        observations: n,
    })
}

/// Simple linear regression of `y` on a single `x`.
pub fn linear_fit(x: &[f64], y: &[f64]) -> Result<Regression> {
    let n = x.len().min(y.len());
    let design = Matrix::from_vec(n, 1, x[..n].to_vec())?;
    ols(&design, &y[..n])
}

/// Convert prices to simple returns.
pub fn simple_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| if w[0] != 0.0 { w[1] / w[0] - 1.0 } else { 0.0 })
        .collect()
}

/// Convert prices to log returns, which are additive across time.
pub fn log_returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .map(|w| {
            if w[0] > 0.0 && w[1] > 0.0 {
                (w[1] / w[0]).ln()
            } else {
                0.0
            }
        })
        .collect()
}

/// Compound a return series into a cumulative growth path starting at 1.0.
pub fn cumulative_growth(returns: &[f64]) -> Vec<f64> {
    let mut level = 1.0;
    let mut out = Vec::with_capacity(returns.len() + 1);
    out.push(level);
    for r in returns {
        level *= 1.0 + r;
        out.push(level);
    }
    out
}

/// Welford accumulator for streaming mean and variance.
///
/// Used on the real-time path, where storing the full history to recompute a
/// standard deviation is not an option.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunningStats {
    count: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

impl RunningStats {
    pub fn new() -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    pub fn push(&mut self, x: f64) {
        if !x.is_finite() {
            return;
        }
        self.count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (x - self.mean);
        self.min = self.min.min(x);
        self.max = self.max.max(x);
    }

    pub fn count(&self) -> u64 {
        self.count
    }
    pub fn mean(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.mean }
    }
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }
    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }
    pub fn min(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.min }
    }
    pub fn max(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.max }
    }

    /// Standard score of `x` against what has been seen so far.
    pub fn z_score(&self, x: f64) -> f64 {
        let sd = self.stddev();
        if sd <= 0.0 {
            0.0
        } else {
            (x - self.mean()) / sd
        }
    }
}
