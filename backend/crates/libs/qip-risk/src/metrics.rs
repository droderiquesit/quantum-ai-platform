//! Return-series risk and performance metrics.
//!
//! Conventions are stated on each function because the same name means
//! different things at different firms. Where a choice materially changes the
//! number — the denominator of a Sharpe ratio, the definition of a drawdown
//! period — it is documented rather than assumed.

use qip_numerics::distributions::normal_inv_cdf;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};

/// Trading days per year, the annualisation convention used throughout.
pub const TRADING_DAYS: f64 = 252.0;

/// Value at risk from the empirical distribution of returns.
///
/// The `confidence` is the probability the loss is *not* exceeded, so 0.99
/// returns the 1st percentile. The result is a positive number representing a
/// loss.
///
/// Historical VaR makes no distributional assumption, which is its virtue, but
/// it cannot see a loss larger than the worst in the sample — so it understates
/// tail risk in a quiet sample by construction.
pub fn historical_var(returns: &[f64], confidence: f64) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let quantile = stats::quantile(returns, 1.0 - confidence.clamp(0.0, 1.0));
    (-quantile).max(0.0)
}

/// Value at risk under a normal assumption.
///
/// Cheap and stable, and wrong in the direction that matters: real return
/// distributions have fatter tails, so this understates the loss at high
/// confidence. Compare against [`historical_var`] rather than choosing one.
pub fn parametric_var(returns: &[f64], confidence: f64) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let mean = stats::mean(returns);
    let sd = stats::stddev(returns);
    let z = normal_inv_cdf(1.0 - confidence.clamp(0.001, 0.999));
    (-(mean + z * sd)).max(0.0)
}

/// Expected shortfall: the mean loss conditional on exceeding VaR.
///
/// The number that actually matters for sizing. VaR says how far the loss
/// reaches on a bad day; expected shortfall says how bad the days beyond it
/// are, which is what determines whether a position can be survived.
pub fn expected_shortfall(returns: &[f64], confidence: f64) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let threshold = stats::quantile(returns, 1.0 - confidence.clamp(0.0, 1.0));
    let tail: Vec<f64> = returns
        .iter()
        .copied()
        .filter(|r| *r <= threshold)
        .collect();
    if tail.is_empty() {
        return historical_var(returns, confidence);
    }
    (-stats::mean(&tail)).max(0.0)
}

/// The shape of a drawdown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawdownProfile {
    /// Worst peak-to-trough decline, as a positive fraction.
    pub max_drawdown: f64,
    /// Index at which the peak preceding the worst drawdown occurred.
    pub peak_index: usize,
    /// Index of the trough.
    pub trough_index: usize,
    /// Periods from peak to trough.
    pub decline_periods: usize,
    /// Periods from trough back to the prior peak. `None` if never recovered.
    pub recovery_periods: Option<usize>,
    /// Longest run of periods spent below a previous peak.
    ///
    /// Often more informative than the depth: a 15% drawdown lasting two years
    /// is harder to hold than a 25% one lasting a month.
    pub longest_underwater: usize,
    /// Current decline from the running peak.
    pub current_drawdown: f64,
}

/// Compute the drawdown profile of an equity curve.
pub fn drawdown_profile(equity: &[f64]) -> DrawdownProfile {
    let mut profile = DrawdownProfile::default();
    if equity.len() < 2 {
        return profile;
    }

    let mut peak = equity[0];
    let mut peak_index = 0usize;
    let mut worst = 0.0f64;
    let mut underwater = 0usize;
    let mut longest_underwater = 0usize;

    for (index, value) in equity.iter().enumerate() {
        if *value >= peak {
            peak = *value;
            peak_index = index;
            underwater = 0;
        } else {
            underwater += 1;
            longest_underwater = longest_underwater.max(underwater);
        }
        if peak > 0.0 {
            let decline = (peak - value) / peak;
            if decline > worst {
                worst = decline;
                profile.peak_index = peak_index;
                profile.trough_index = index;
            }
        }
    }

    profile.max_drawdown = worst;
    profile.decline_periods = profile.trough_index.saturating_sub(profile.peak_index);
    profile.longest_underwater = longest_underwater;

    // Recovery: the first index after the trough that regains the prior peak.
    if worst > 0.0 {
        let peak_value = equity[profile.peak_index];
        profile.recovery_periods = equity
            .iter()
            .enumerate()
            .skip(profile.trough_index)
            .find(|(_, v)| **v >= peak_value)
            .map(|(index, _)| index - profile.trough_index);
    }

    let last = equity[equity.len() - 1];
    let running_peak = equity.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    profile.current_drawdown = if running_peak > 0.0 {
        ((running_peak - last) / running_peak).max(0.0)
    } else {
        0.0
    };
    profile
}

/// Tail characteristics of a return series.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TailRisk {
    pub var_95: f64,
    pub var_99: f64,
    pub expected_shortfall_95: f64,
    pub expected_shortfall_99: f64,
    /// Parametric VaR at 99%, for comparison against the historical figure.
    pub parametric_var_99: f64,
    /// How much fatter the observed tail is than a normal one would be.
    ///
    /// Above 1.0 means a normal model understates the tail, which is the usual
    /// case and the reason both are reported.
    pub tail_fatness: f64,
    pub worst_return: f64,
    pub skewness: f64,
    pub excess_kurtosis: f64,
}

impl TailRisk {
    pub fn compute(returns: &[f64]) -> Self {
        if returns.len() < 2 {
            return Self::default();
        }
        let historical = historical_var(returns, 0.99);
        let parametric = parametric_var(returns, 0.99);
        Self {
            var_95: historical_var(returns, 0.95),
            var_99: historical,
            expected_shortfall_95: expected_shortfall(returns, 0.95),
            expected_shortfall_99: expected_shortfall(returns, 0.99),
            parametric_var_99: parametric,
            tail_fatness: if parametric > 1e-12 {
                historical / parametric
            } else {
                1.0
            },
            worst_return: returns.iter().copied().fold(f64::INFINITY, f64::min),
            skewness: stats::skewness(returns),
            excess_kurtosis: stats::excess_kurtosis(returns),
        }
    }

    /// Whether the tail is materially fatter than a normal model assumes.
    pub fn understated_by_normal_model(&self) -> bool {
        self.tail_fatness > 1.2 || self.excess_kurtosis > 1.0
    }
}

/// The full risk and performance picture for a return series.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiskMetrics {
    pub observations: usize,
    /// Periods per year used to annualise. Needed to state the uncertainty of
    /// the annualised ratios correctly.
    pub periods_per_year: f64,
    /// Compound annual growth rate.
    pub annualised_return: f64,
    pub annualised_volatility: f64,
    /// Excess return over the risk-free rate per unit of volatility.
    pub sharpe_ratio: f64,
    /// Sharpe using only downside deviation.
    pub sortino_ratio: f64,
    /// Annualised return divided by maximum drawdown.
    pub calmar_ratio: f64,
    pub drawdown: DrawdownProfile,
    pub tail: TailRisk,
    /// Fraction of periods with a positive return.
    pub hit_rate: f64,
    /// Mean gain divided by mean loss, both as magnitudes.
    pub gain_to_pain: f64,
    /// Best single-period return.
    pub best_return: f64,
    /// Total compounded return over the period.
    pub total_return: f64,
}

impl RiskMetrics {
    /// Compute from a series of periodic returns.
    ///
    /// `periods_per_year` sets the annualisation; `risk_free_rate` is annual.
    pub fn compute(returns: &[f64], periods_per_year: f64, risk_free_rate: f64) -> Self {
        let mut metrics = Self {
            observations: returns.len(),
            periods_per_year: periods_per_year.max(1.0),
            ..Self::default()
        };
        if returns.len() < 2 {
            return metrics;
        }

        let equity = stats::cumulative_growth(returns);
        metrics.total_return = equity[equity.len() - 1] - 1.0;
        metrics.drawdown = drawdown_profile(&equity);
        metrics.tail = TailRisk::compute(returns);

        let years = returns.len() as f64 / periods_per_year.max(1e-9);
        metrics.annualised_return = if years > 0.0 && equity[equity.len() - 1] > 0.0 {
            equity[equity.len() - 1].powf(1.0 / years) - 1.0
        } else {
            0.0
        };
        metrics.annualised_volatility = stats::stddev(returns) * periods_per_year.sqrt();

        // Sharpe on the arithmetic mean of periodic excess returns, which is
        // the standard construction; using the geometric mean would understate
        // it for a volatile series.
        let periodic_rf = risk_free_rate / periods_per_year;
        let excess: Vec<f64> = returns.iter().map(|r| r - periodic_rf).collect();
        let excess_sd = stats::stddev(&excess);
        metrics.sharpe_ratio = if excess_sd > 1e-12 {
            stats::mean(&excess) / excess_sd * periods_per_year.sqrt()
        } else {
            0.0
        };

        // Sortino penalises only downside deviation, measured against zero.
        let downside: Vec<f64> = returns.iter().map(|r| r.min(0.0)).collect();
        let downside_deviation =
            (downside.iter().map(|d| d * d).sum::<f64>() / downside.len() as f64).sqrt();
        metrics.sortino_ratio = if downside_deviation > 1e-12 {
            stats::mean(&excess) / downside_deviation * periods_per_year.sqrt()
        } else {
            0.0
        };

        metrics.calmar_ratio = if metrics.drawdown.max_drawdown > 1e-9 {
            metrics.annualised_return / metrics.drawdown.max_drawdown
        } else {
            0.0
        };

        let wins: Vec<f64> = returns.iter().copied().filter(|r| *r > 0.0).collect();
        let losses: Vec<f64> = returns.iter().copied().filter(|r| *r < 0.0).collect();
        metrics.hit_rate = wins.len() as f64 / returns.len() as f64;
        metrics.gain_to_pain = if losses.is_empty() {
            f64::INFINITY
        } else if wins.is_empty() {
            0.0
        } else {
            stats::mean(&wins) / stats::mean(&losses).abs()
        };
        metrics.best_return = returns.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        metrics
    }

    /// Compute from daily returns.
    pub fn daily(returns: &[f64], risk_free_rate: f64) -> Self {
        Self::compute(returns, TRADING_DAYS, risk_free_rate)
    }

    /// Whether the record is long enough for the ratios to mean anything.
    ///
    /// A Sharpe ratio from thirty observations has a standard error of roughly
    /// 0.2; quoting one to two decimal places from such a sample is false
    /// precision, and sizing on it is worse.
    pub fn is_statistically_meaningful(&self) -> bool {
        self.observations >= 60
    }

    /// Standard error of the *annualised* Sharpe ratio.
    ///
    /// The usual asymptotic result, `sqrt((1 + S^2/2) / n)`, is the error of
    /// the *periodic* ratio. Annualising multiplies the ratio by the square
    /// root of the period count, so the error scales too. Comparing an
    /// annualised ratio against a periodic error understates the uncertainty by
    /// roughly sixteen times at daily frequency, which would make pure noise
    /// look like a significant edge.
    pub fn sharpe_standard_error(&self) -> f64 {
        if self.observations < 2 {
            return f64::INFINITY;
        }
        let n = self.observations as f64;
        let periods = self.periods_per_year.max(1.0);
        ((periods + 0.5 * self.sharpe_ratio * self.sharpe_ratio) / n).sqrt()
    }

    /// Whether the Sharpe ratio is distinguishable from zero at roughly 95%.
    pub fn sharpe_is_significant(&self) -> bool {
        let error = self.sharpe_standard_error();
        error.is_finite() && self.sharpe_ratio.abs() > 1.96 * error
    }
}

/// Annualised volatility of a return series, standard deviation scaled by the
/// square root of time.
///
/// The same construction [`RiskMetrics::compute`] uses for
/// `annualised_volatility`, pulled out so [`crate::limits::RiskState::with_tail_risk`]
/// can fill [`crate::limits::LimitKind::MaxVolatility`]'s input from the same
/// return series it already carries for value at risk and expected shortfall,
/// rather than leaving the field at whatever a caller happened to set — which,
/// until this existed, was never, so the shipped volatility limit took the
/// same always-empty path the tail limits once did.
///
/// A series shorter than two returns nothing meaningful to annualise, so this
/// returns zero rather than a standard deviation of one sample.
pub fn annualised_volatility(returns: &[f64]) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    stats::stddev(returns) * TRADING_DAYS.sqrt()
}

/// Beta of a return series against a benchmark.
pub fn beta(returns: &[f64], benchmark: &[f64]) -> f64 {
    let variance = stats::variance(benchmark);
    if variance <= 1e-15 {
        return 0.0;
    }
    stats::covariance(returns, benchmark) / variance
}

/// Annualised alpha against a benchmark, given beta.
pub fn alpha(returns: &[f64], benchmark: &[f64], periods_per_year: f64) -> f64 {
    let b = beta(returns, benchmark);
    (stats::mean(returns) - b * stats::mean(benchmark)) * periods_per_year
}

/// Tracking error: annualised volatility of the return difference.
pub fn tracking_error(returns: &[f64], benchmark: &[f64], periods_per_year: f64) -> f64 {
    let n = returns.len().min(benchmark.len());
    if n < 2 {
        return 0.0;
    }
    let differences: Vec<f64> = (0..n).map(|i| returns[i] - benchmark[i]).collect();
    stats::stddev(&differences) * periods_per_year.sqrt()
}

/// Information ratio: active return per unit of tracking error.
pub fn information_ratio(returns: &[f64], benchmark: &[f64], periods_per_year: f64) -> f64 {
    let error = tracking_error(returns, benchmark, periods_per_year);
    if error <= 1e-12 {
        return 0.0;
    }
    let n = returns.len().min(benchmark.len());
    let active: Vec<f64> = (0..n).map(|i| returns[i] - benchmark[i]).collect();
    stats::mean(&active) * periods_per_year / error
}
