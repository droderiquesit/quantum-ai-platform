//! Monte Carlo simulation.
//!
//! Three generators, because the right one depends on what is being asked:
//!
//! * [`Generator::Bootstrap`] resamples history in blocks. It preserves the
//!   volatility clustering and fat tails the data actually has, and cannot
//!   produce a regime the history does not contain.
//! * [`Generator::Parametric`] draws from a Student-t. It can produce moves the
//!   history never saw, at the cost of assuming a shape.
//! * [`Generator::Antithetic`] pairs each parametric draw with its negation,
//!   which halves the variance of a symmetric estimate at no extra cost.
//!
//! Every result carries a standard error. A Monte Carlo estimate reported
//! without one invites the reader to treat simulation noise as signal, and the
//! difference between a 5th percentile of -8.1% and -8.4% is usually noise.

use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_numerics::stats;
use serde::{Deserialize, Serialize};

/// How paths are generated.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Generator {
    /// Stationary block bootstrap with geometric block lengths.
    ///
    /// Geometric rather than fixed: a fixed block length introduces a
    /// periodicity the original series does not have.
    Bootstrap { mean_block: f64 },
    /// Student-t innovations with the given degrees of freedom.
    ///
    /// Five degrees of freedom gives an excess kurtosis of six, which is in
    /// the range daily equity returns actually show. A normal draw would
    /// understate the tail by roughly the amount the tail matters.
    Parametric { degrees_of_freedom: f64 },
    /// Parametric with antithetic pairing.
    Antithetic { degrees_of_freedom: f64 },
}

impl Generator {
    pub fn describe(&self) -> String {
        match self {
            Self::Bootstrap { mean_block } => format!(
                "stationary block bootstrap, mean block {mean_block:.0}: preserves the history's clustering but cannot produce a regime it does not contain"
            ),
            Self::Parametric { degrees_of_freedom } => format!(
                "Student-t({degrees_of_freedom:.0}) innovations: can produce unseen moves, at the cost of assuming a shape"
            ),
            Self::Antithetic { degrees_of_freedom } => format!(
                "Student-t({degrees_of_freedom:.0}) with antithetic pairing: same assumption, lower simulation noise"
            ),
        }
    }
}

/// The distribution of an outcome, with the noise in its own estimates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    pub paths: usize,
    pub mean: f64,
    /// Standard error of the mean. The precision of the estimate, not the
    /// spread of the outcome.
    pub mean_standard_error: f64,
    pub stddev: f64,
    pub median: f64,
    pub p05: f64,
    pub p95: f64,
    /// Standard error of the 5th percentile, by the order-statistic
    /// approximation. Reported because the tail is always the least precisely
    /// estimated number in a Monte Carlo, and it is usually the one quoted.
    pub p05_standard_error: f64,
    /// Mean of the worst five percent of paths.
    pub expected_shortfall_95: f64,
    pub probability_of_loss: f64,
    pub skewness: f64,
    pub excess_kurtosis: f64,
    pub generator: Generator,
}

impl Distribution {
    /// Whether the mean is distinguishable from zero at roughly two standard
    /// errors.
    ///
    /// Deliberately not a p-value: a Monte Carlo over resampled history has no
    /// clean null hypothesis, and dressing the comparison up as a test would
    /// suggest more than it can support.
    pub fn mean_is_distinguishable_from_zero(&self) -> bool {
        self.mean.abs() > 2.0 * self.mean_standard_error
    }

    /// Paths needed for the mean's standard error to reach `target`.
    ///
    /// Answers "is it worth running more" with a number instead of a guess.
    pub fn paths_for_precision(&self, target: f64) -> usize {
        if target <= 0.0 || self.stddev <= 0.0 {
            return self.paths;
        }
        ((self.stddev / target).powi(2)).ceil() as usize
    }

    pub fn summarise(&self) -> String {
        format!(
            "{:.2}% ± {:.2}% mean over {} paths, 5-95 range [{:.2}%, {:.2}%], {:.0}% chance of loss, {:.2}% expected shortfall",
            self.mean * 100.0,
            self.mean_standard_error * 100.0,
            self.paths,
            self.p05 * 100.0,
            self.p95 * 100.0,
            self.probability_of_loss * 100.0,
            self.expected_shortfall_95 * 100.0
        )
    }
}

/// Runs Monte Carlo over a return series.
#[derive(Debug)]
pub struct MonteCarlo {
    generator: Generator,
    paths: usize,
}

impl MonteCarlo {
    pub fn new(generator: Generator, paths: usize) -> Result<Self> {
        if paths < 100 {
            return Err(Error::invalid(
                "fewer than a hundred paths gives a standard error wider than anything it would measure",
            ));
        }
        if let Generator::Bootstrap { mean_block } = generator
            && mean_block < 1.0
        {
            return Err(Error::invalid("mean block length must be at least one"));
        }
        if let Generator::Parametric {
            degrees_of_freedom, ..
        }
        | Generator::Antithetic {
            degrees_of_freedom, ..
        } = generator
            && degrees_of_freedom <= 2.0
        {
            return Err(Error::invalid(
                "Student-t with two or fewer degrees of freedom has infinite variance",
            ));
        }
        Ok(Self { generator, paths })
    }

    pub fn generator(&self) -> Generator {
        self.generator
    }

    /// Simulate cumulative returns over `steps` periods.
    pub fn run(&self, history: &[f64], steps: usize, rng: &mut Xoshiro256) -> Result<Distribution> {
        if history.len() < 20 {
            return Err(Error::invalid(format!(
                "{} observations is too few to simulate from",
                history.len()
            )));
        }
        if steps == 0 {
            return Err(Error::invalid("a simulation needs at least one step"));
        }

        let mut outcomes = Vec::with_capacity(self.paths);
        match self.generator {
            Generator::Bootstrap { mean_block } => {
                for _ in 0..self.paths {
                    outcomes.push(bootstrap_path(history, steps, mean_block, rng));
                }
            }
            Generator::Parametric { degrees_of_freedom } => {
                let mean = stats::mean(history);
                let sigma = stats::stddev(history);
                for _ in 0..self.paths {
                    outcomes.push(parametric_path(
                        mean,
                        sigma,
                        degrees_of_freedom,
                        steps,
                        rng,
                        1.0,
                    ));
                }
            }
            Generator::Antithetic { degrees_of_freedom } => {
                let mean = stats::mean(history);
                let sigma = stats::stddev(history);
                // The pairing only works if both paths use the *same* draws.
                // Drawing twice — even from a forked stream — produces two
                // independent paths and no variance reduction at all, which is
                // the mistake this comment exists to prevent repeating.
                let mut innovations = vec![0.0; steps];
                for _ in 0..self.paths.div_ceil(2) {
                    for innovation in innovations.iter_mut() {
                        *innovation = rng.student_t(degrees_of_freedom);
                    }
                    outcomes.push(path_from(
                        mean,
                        sigma,
                        degrees_of_freedom,
                        &innovations,
                        1.0,
                    ));
                    if outcomes.len() < self.paths {
                        outcomes.push(path_from(
                            mean,
                            sigma,
                            degrees_of_freedom,
                            &innovations,
                            -1.0,
                        ));
                    }
                }
            }
        }

        Ok(summarise(outcomes, self.generator))
    }
}

fn bootstrap_path(history: &[f64], steps: usize, mean_block: f64, rng: &mut Xoshiro256) -> f64 {
    let n = history.len();
    let continue_probability = 1.0 - 1.0 / mean_block;
    let mut total = 0.0;
    let mut index = rng.below(n as u64) as usize;
    for _ in 0..steps {
        total += history[index];
        if rng.next_f64() < continue_probability {
            // Wrap rather than stop, so every observation is equally likely at
            // every position in the path.
            index = (index + 1) % n;
        } else {
            index = rng.below(n as u64) as usize;
        }
    }
    total.exp() - 1.0
}

fn parametric_path(
    mean: f64,
    sigma: f64,
    degrees_of_freedom: f64,
    steps: usize,
    rng: &mut Xoshiro256,
    sign: f64,
) -> f64 {
    let mut total = 0.0;
    let scale = student_t_scale(degrees_of_freedom);
    for _ in 0..steps {
        let innovation = rng.student_t(degrees_of_freedom) * scale;
        total += mean + sign * sigma * innovation;
    }
    total.exp() - 1.0
}

/// A path from innovations already drawn, so an antithetic pair can share them.
fn path_from(
    mean: f64,
    sigma: f64,
    degrees_of_freedom: f64,
    innovations: &[f64],
    sign: f64,
) -> f64 {
    let scale = student_t_scale(degrees_of_freedom);
    let total: f64 = innovations
        .iter()
        .map(|innovation| mean + sign * sigma * innovation * scale)
        .sum();
    total.exp() - 1.0
}

/// A Student-t has variance `nu/(nu-2)`, so draws must be rescaled or a path
/// would carry more volatility than the history it was fitted to.
fn student_t_scale(degrees_of_freedom: f64) -> f64 {
    ((degrees_of_freedom - 2.0) / degrees_of_freedom).sqrt()
}

fn summarise(mut outcomes: Vec<f64>, generator: Generator) -> Distribution {
    outcomes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let paths = outcomes.len();
    let mean = stats::mean(&outcomes);
    let stddev = stats::stddev(&outcomes);
    let p05 = stats::quantile_sorted(&outcomes, 0.05);
    let p95 = stats::quantile_sorted(&outcomes, 0.95);
    let tail = (paths / 20).max(1);
    let expected_shortfall_95 = stats::mean(&outcomes[..tail]);

    // Standard error of a sample quantile: sqrt(p(1-p)/n) / f(q), with the
    // density estimated from the spacing of the order statistics around it.
    let p05_standard_error = {
        let index = ((0.05 * paths as f64) as usize).clamp(1, paths.saturating_sub(2));
        let window = (paths / 50).max(1);
        let low = index.saturating_sub(window);
        let high = (index + window).min(paths - 1);
        let spacing = outcomes[high] - outcomes[low];
        let density = if spacing.abs() < 1e-12 {
            f64::INFINITY
        } else {
            (high - low) as f64 / (paths as f64 * spacing)
        };
        if density.is_finite() && density > 0.0 {
            (0.05 * 0.95 / paths as f64).sqrt() / density
        } else {
            0.0
        }
    };

    Distribution {
        paths,
        mean,
        mean_standard_error: stddev / (paths as f64).sqrt(),
        stddev,
        median: stats::quantile_sorted(&outcomes, 0.5),
        p05,
        p95,
        p05_standard_error,
        expected_shortfall_95,
        probability_of_loss: outcomes.iter().filter(|o| **o < 0.0).count() as f64 / paths as f64,
        skewness: stats::skewness(&outcomes),
        excess_kurtosis: stats::excess_kurtosis(&outcomes),
        generator,
    }
}
