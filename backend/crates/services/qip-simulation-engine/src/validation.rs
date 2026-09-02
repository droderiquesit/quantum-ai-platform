//! Guarding against a backtest that is really a search result.
//!
//! Run enough strategies over one history and the best of them will look good
//! whether or not any of them is. This module holds the corrections that make
//! that visible.
//!
//! * [`deflated_sharpe`] discounts a Sharpe ratio for how many strategies were
//!   tried, and for the non-normality of the returns that produced it.
//! * [`sharpe_standard_error`] is the error that deflation divides by,
//!   exposed so a caller that needs the same figure takes it from here
//!   rather than restating it.
//! * [`PurgedSplit`] builds cross-validation folds that remove the observations
//!   overlapping a test set and embargo the ones immediately after it. Without
//!   purging, a strategy holding positions for ten days leaks ten days of the
//!   test set into the training set at every boundary.
//! * [`WalkForward`] runs the only validation that matches how a strategy is
//!   actually deployed: fit on the past, trade the future, never the reverse.
//!
//! None of this makes an overfitted strategy work. It makes an overfitted
//! strategy look like one.

use qip_core::error::{Error, Result};
use qip_numerics::distributions;
use qip_numerics::stats;
use serde::{Deserialize, Serialize};

/// The correction for having tried many strategies.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeflatedSharpe {
    /// The Sharpe ratio as observed.
    pub observed: f64,
    /// The Sharpe ratio the best of `trials` random strategies would have
    /// shown by chance alone.
    pub expected_maximum: f64,
    /// Probability the observed Sharpe exceeds what selection alone explains.
    pub probability: f64,
    pub trials: usize,
    pub observations: usize,
    pub skewness: f64,
    pub excess_kurtosis: f64,
}

impl DeflatedSharpe {
    /// Whether the result survives the correction at a conventional bar.
    pub fn is_credible(&self) -> bool {
        self.probability >= 0.95
    }

    /// Whether the observed Sharpe is above what the search alone would give.
    ///
    /// Worth checking before reading anything into [`Self::probability`],
    /// because the probability behaves differently on either side of this
    /// line. Above it, more uncertainty in the estimate — from fat tails or
    /// negative skew — lowers confidence, which is the intuitive direction.
    /// Below it, more uncertainty *raises* the probability, because a wider
    /// distribution puts more mass above a threshold the point estimate does
    /// not reach. That is correct and still not what anyone means by
    /// "confidence", so a result below the threshold should be read as failing
    /// the test rather than as scoring somewhere on it.
    pub fn clears_selection_threshold(&self) -> bool {
        self.observed > self.expected_maximum
    }

    pub fn summarise(&self) -> String {
        format!(
            "Sharpe {:.2} over {} observations; {} trial(s) would produce {:.2} by chance, leaving {:.1}% confidence the result is not selection",
            self.observed,
            self.observations,
            self.trials,
            self.expected_maximum,
            self.probability * 100.0
        )
    }
}

/// Euler-Mascheroni constant, in the expected-maximum formula below.
const EULER_MASCHERONI: f64 = 0.577_215_664_901_532_9;

/// Deflate a Sharpe ratio for multiple testing and non-normality.
///
/// The expected maximum of `n` independent standard normals is approximated by
/// `(1-γ)·Φ⁻¹(1 - 1/n) + γ·Φ⁻¹(1 - 1/(n·e))`, and the observed Sharpe is
/// compared against it using the standard error that accounts for skew and
/// kurtosis. Both come from Bailey and López de Prado's treatment of the
/// deflated Sharpe ratio.
///
/// The non-normality correction matters in the direction people find
/// inconvenient: negative skew and fat tails *increase* the standard error, so
/// a strategy that sells tail risk needs a higher Sharpe to clear the same bar.
///
/// That direction holds for a result above the selection threshold, which is
/// the only case worth deflating. Below the threshold the extra uncertainty
/// raises the probability instead — see
/// [`DeflatedSharpe::clears_selection_threshold`], which is the flag to check
/// before quoting the number.
pub fn deflated_sharpe(
    returns: &[f64],
    trials: usize,
    periods_per_year: f64,
) -> Result<DeflatedSharpe> {
    if returns.len() < 20 {
        return Err(Error::invalid(format!(
            "{} observations is too few to deflate a Sharpe ratio",
            returns.len()
        )));
    }
    if trials == 0 {
        return Err(Error::invalid("at least one trial must have been run"));
    }

    let n = returns.len() as f64;
    let mean = stats::mean(returns);
    let sigma = stats::stddev(returns);
    if sigma < 1e-12 {
        return Err(Error::numeric("returns do not vary"));
    }
    let periodic_sharpe = mean / sigma;
    let observed = periodic_sharpe * periods_per_year.max(1.0).sqrt();
    let skewness = stats::skewness(returns);
    let excess_kurtosis = stats::excess_kurtosis(returns);

    // Expected maximum Sharpe from `trials` independent random strategies.
    let trials_f = trials as f64;
    let expected_maximum_periodic = if trials == 1 {
        0.0
    } else {
        let a = distributions::normal_inv_cdf(1.0 - 1.0 / trials_f);
        let b = distributions::normal_inv_cdf(1.0 - 1.0 / (trials_f * std::f64::consts::E));
        ((1.0 - EULER_MASCHERONI) * a + EULER_MASCHERONI * b) / n.sqrt()
    };

    let standard_error =
        sharpe_standard_error(periodic_sharpe, skewness, excess_kurtosis, returns.len())?;

    let z = (periodic_sharpe - expected_maximum_periodic) / standard_error;
    let probability = distributions::normal_cdf(z);

    Ok(DeflatedSharpe {
        observed,
        expected_maximum: expected_maximum_periodic * periods_per_year.max(1.0).sqrt(),
        probability,
        trials,
        observations: returns.len(),
        skewness,
        excess_kurtosis,
    })
}

/// Standard error of a per-period Sharpe ratio under non-normal returns.
///
/// `sqrt((1 − γ₃·SR + ¼·γ₄·SR²) / (n − 1))` (Bailey and López de Prado),
/// floored where skew and kurtosis would drive the variance below zero.
/// This is the one place the formula is written: [`deflated_sharpe`] divides
/// by it, and the holdout band in `qip-lifecycle` widens by it. It was once
/// restated there, and nothing would have noticed the two drifting apart —
/// a band judged on a different error than the deflation that admitted it
/// would demote, or keep, a strategy for a reason nobody could reproduce.
///
/// Refuses fewer than two observations, which give the ratio no error, and a
/// result that is not a number.
pub fn sharpe_standard_error(
    periodic_sharpe: f64,
    skewness: f64,
    excess_kurtosis: f64,
    observations: usize,
) -> Result<f64> {
    if observations < 2 {
        return Err(Error::invalid(format!(
            "{observations} observation(s) give a Sharpe ratio no standard error"
        )));
    }
    let n = observations as f64;
    let variance = (1.0 - skewness * periodic_sharpe
        + 0.25 * excess_kurtosis * periodic_sharpe * periodic_sharpe)
        / (n - 1.0);
    let standard_error = variance.max(1e-18).sqrt();
    if !standard_error.is_finite() {
        return Err(Error::numeric(
            "the Sharpe standard error is not finite; the inputs are not numbers",
        ));
    }
    Ok(standard_error)
}

/// One training and test split.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub train: Vec<usize>,
    pub test: Vec<usize>,
    /// Observations removed for overlapping the test set.
    pub purged: usize,
    /// Observations removed immediately after the test set.
    pub embargoed: usize,
}

impl Split {
    pub fn is_valid(&self) -> bool {
        !self.train.is_empty() && !self.test.is_empty()
    }
}

/// Cross-validation folds with purging and an embargo.
///
/// Standard k-fold on a time series leaks in two ways. A label computed over a
/// holding period straddles the boundary, so observations near a test fold
/// contain information from inside it — purging removes those. And serial
/// correlation means the observations just after a test fold are nearly the
/// same data — an embargo removes those too.
#[derive(Clone, Copy, Debug)]
pub struct PurgedSplit {
    folds: usize,
    /// Observations a label spans, which sets the purge width.
    pub label_horizon: usize,
    /// Observations embargoed after each test fold.
    pub embargo: usize,
}

impl PurgedSplit {
    pub fn new(folds: usize, label_horizon: usize, embargo: usize) -> Result<Self> {
        if folds < 2 {
            return Err(Error::invalid("cross-validation needs at least two folds"));
        }
        Ok(Self {
            folds,
            label_horizon,
            embargo,
        })
    }

    pub fn folds(&self) -> usize {
        self.folds
    }

    /// Build the splits over `n` ordered observations.
    pub fn split(&self, n: usize) -> Result<Vec<Split>> {
        if n < self.folds * 2 {
            return Err(Error::invalid(format!(
                "{n} observations cannot be split into {} usable folds",
                self.folds
            )));
        }
        let fold_size = n / self.folds;
        let mut splits = Vec::with_capacity(self.folds);

        for fold in 0..self.folds {
            let test_start = fold * fold_size;
            let test_end = if fold == self.folds - 1 {
                n
            } else {
                test_start + fold_size
            };

            // Purge anything whose label would overlap the test window, and
            // embargo the window immediately following it.
            let purge_start = test_start.saturating_sub(self.label_horizon);
            let embargo_end = (test_end + self.embargo).min(n);

            let mut train = Vec::new();
            let mut purged = 0usize;
            let mut embargoed = 0usize;
            for i in 0..n {
                if i >= test_start && i < test_end {
                    continue;
                }
                if i >= purge_start && i < test_start {
                    purged += 1;
                    continue;
                }
                if i >= test_end && i < embargo_end {
                    embargoed += 1;
                    continue;
                }
                train.push(i);
            }

            splits.push(Split {
                train,
                test: (test_start..test_end).collect(),
                purged,
                embargoed,
            });
        }
        Ok(splits)
    }
}

/// Anchored or rolling walk-forward windows.
///
/// The only validation that matches deployment: every test window is strictly
/// after the training window that produced the parameters used on it.
#[derive(Clone, Copy, Debug)]
pub struct WalkForward {
    pub train_size: usize,
    pub test_size: usize,
    /// Anchored keeps the training window growing from the start; rolling
    /// keeps it fixed-length. Anchored uses more data; rolling adapts faster
    /// to a regime change. Neither is right in general, so it is a choice the
    /// caller makes explicitly.
    pub anchored: bool,
    /// Observations skipped between train and test, for label overlap.
    pub embargo: usize,
}

impl WalkForward {
    pub fn new(
        train_size: usize,
        test_size: usize,
        anchored: bool,
        embargo: usize,
    ) -> Result<Self> {
        if train_size == 0 || test_size == 0 {
            return Err(Error::invalid(
                "walk-forward needs a non-empty training and test window",
            ));
        }
        Ok(Self {
            train_size,
            test_size,
            anchored,
            embargo,
        })
    }

    pub fn split(&self, n: usize) -> Result<Vec<Split>> {
        let first_test = self.train_size + self.embargo;
        if n <= first_test {
            return Err(Error::invalid(format!(
                "{n} observations cannot support a {}-observation training window plus a {}-observation embargo",
                self.train_size, self.embargo
            )));
        }

        let mut splits = Vec::new();
        let mut test_start = first_test;
        while test_start < n {
            let test_end = (test_start + self.test_size).min(n);
            let train_end = test_start - self.embargo;
            let train_start = if self.anchored {
                0
            } else {
                train_end.saturating_sub(self.train_size)
            };
            splits.push(Split {
                train: (train_start..train_end).collect(),
                test: (test_start..test_end).collect(),
                purged: 0,
                embargoed: self.embargo,
            });
            test_start = test_end;
        }
        Ok(splits)
    }
}

/// How out-of-sample performance compared to in-sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverfittingReport {
    pub in_sample_sharpe: f64,
    pub out_of_sample_sharpe: f64,
    /// Fraction of the in-sample Sharpe that survived out of sample.
    pub degradation: f64,
    /// Folds where out-of-sample performance was negative.
    pub negative_folds: usize,
    pub total_folds: usize,
}

impl OverfittingReport {
    /// Whether the pattern looks like overfitting rather than a real edge.
    ///
    /// Two conditions, either of which is damning: most of the in-sample
    /// performance vanished, or more than half the out-of-sample folds lost
    /// money. A strategy can survive one bad fold; a strategy whose edge
    /// exists only in the folds it was fitted on cannot.
    pub fn looks_overfitted(&self) -> bool {
        self.degradation < 0.3
            || (self.total_folds > 0 && self.negative_folds * 2 > self.total_folds)
    }

    pub fn summarise(&self) -> String {
        format!(
            "in-sample Sharpe {:.2} became {:.2} out of sample ({:.0}% retained) across {} fold(s), {} negative",
            self.in_sample_sharpe,
            self.out_of_sample_sharpe,
            self.degradation * 100.0,
            self.total_folds,
            self.negative_folds
        )
    }
}

/// Compare in-sample against out-of-sample results across folds.
pub fn assess_overfitting(
    in_sample: &[Vec<f64>],
    out_of_sample: &[Vec<f64>],
) -> Result<OverfittingReport> {
    if in_sample.len() != out_of_sample.len() {
        return Err(Error::invalid(
            "in-sample and out-of-sample fold counts differ",
        ));
    }
    if in_sample.is_empty() {
        return Err(Error::invalid("no folds to assess"));
    }

    let sharpe = |returns: &[f64]| -> f64 {
        let sigma = stats::stddev(returns);
        if sigma < 1e-12 {
            0.0
        } else {
            stats::mean(returns) / sigma
        }
    };

    let in_sharpes: Vec<f64> = in_sample.iter().map(|r| sharpe(r)).collect();
    let out_sharpes: Vec<f64> = out_of_sample.iter().map(|r| sharpe(r)).collect();
    let in_mean = stats::mean(&in_sharpes);
    let out_mean = stats::mean(&out_sharpes);

    // Degradation is undefined when the in-sample result is not positive; a
    // strategy that lost money in sample has nothing to degrade from, and
    // reporting a ratio would make it look like it held up.
    let degradation = if in_mean <= 1e-12 {
        0.0
    } else {
        (out_mean / in_mean).clamp(-1.0, 2.0)
    };

    Ok(OverfittingReport {
        in_sample_sharpe: in_mean,
        out_of_sample_sharpe: out_mean,
        degradation,
        negative_folds: out_sharpes.iter().filter(|s| **s < 0.0).count(),
        total_folds: out_sharpes.len(),
    })
}
