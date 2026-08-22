//! Prediction scoring, calibration and drift.
//!
//! The platform makes probabilistic claims — a thesis with a confidence, a
//! model with a predicted direction. What matters is not whether any one was
//! right but whether the stated confidences mean anything: of the claims made
//! at 70% confidence, roughly 70% should come true. A system that is
//! consistently overconfident will size positions too large no matter how good
//! its individual calls are.

use serde::{Deserialize, Serialize};

/// One prediction and what actually happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredictionOutcome {
    /// Stated probability that the claim was true, in `[0, 1]`.
    pub predicted_probability: f64,
    /// Whether it turned out true.
    pub occurred: bool,
    /// Optional label, so calibration can be broken down by agent or model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohort: Option<String>,
}

impl PredictionOutcome {
    pub fn new(predicted_probability: f64, occurred: bool) -> Self {
        Self {
            predicted_probability: predicted_probability.clamp(0.0, 1.0),
            occurred,
            cohort: None,
        }
    }

    pub fn in_cohort(mut self, cohort: impl Into<String>) -> Self {
        self.cohort = Some(cohort.into());
        self
    }

    /// Squared error of this single prediction.
    pub fn squared_error(&self) -> f64 {
        let actual = f64::from(u8::from(self.occurred));
        (self.predicted_probability - actual).powi(2)
    }
}

/// One bucket of the reliability diagram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibrationBin {
    pub lower: f64,
    pub upper: f64,
    pub count: usize,
    /// Mean stated probability in the bin.
    pub mean_predicted: f64,
    /// Fraction that actually occurred.
    pub observed_rate: f64,
}

impl CalibrationBin {
    /// How far the bin is from perfectly calibrated. Positive means
    /// overconfident.
    pub fn overconfidence(&self) -> f64 {
        self.mean_predicted - self.observed_rate
    }
}

/// Calibration assessment over a set of predictions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub count: usize,
    /// Mean squared error. Lower is better; 0.25 is what always saying 50%
    /// achieves on a balanced set.
    pub brier_score: f64,
    /// Mean negative log likelihood.
    pub log_loss: f64,
    /// Average stated probability minus average outcome rate. Positive means
    /// systematically overconfident.
    pub bias: f64,
    /// Mean absolute gap between stated and observed rates across bins.
    pub expected_calibration_error: f64,
    pub bins: Vec<CalibrationBin>,
    /// Base rate of the outcome.
    pub base_rate: f64,
    /// Brier skill against always predicting the base rate. Positive means the
    /// predictions add information; zero or negative means they do not.
    pub skill_score: f64,
}

impl Calibration {
    /// Assess a set of predictions using `bin_count` equal-width bins.
    pub fn assess(outcomes: &[PredictionOutcome], bin_count: usize) -> Self {
        let count = outcomes.len();
        if count == 0 {
            return Self {
                count: 0,
                brier_score: 0.0,
                log_loss: 0.0,
                bias: 0.0,
                expected_calibration_error: 0.0,
                bins: Vec::new(),
                base_rate: 0.0,
                skill_score: 0.0,
            };
        }

        let brier_score = outcomes
            .iter()
            .map(PredictionOutcome::squared_error)
            .sum::<f64>()
            / count as f64;

        // Clamp before taking a log so a confident miss is heavily but finitely
        // penalised rather than producing infinity.
        const EPSILON: f64 = 1e-9;
        let log_loss = -outcomes
            .iter()
            .map(|o| {
                let p = o.predicted_probability.clamp(EPSILON, 1.0 - EPSILON);
                if o.occurred { p.ln() } else { (1.0 - p).ln() }
            })
            .sum::<f64>()
            / count as f64;

        let mean_predicted = outcomes
            .iter()
            .map(|o| o.predicted_probability)
            .sum::<f64>()
            / count as f64;
        let base_rate = outcomes.iter().filter(|o| o.occurred).count() as f64 / count as f64;

        let bin_count = bin_count.max(1);
        let mut bins = Vec::with_capacity(bin_count);
        let mut weighted_error = 0.0;
        for index in 0..bin_count {
            let lower = index as f64 / bin_count as f64;
            let upper = (index + 1) as f64 / bin_count as f64;
            let members: Vec<&PredictionOutcome> = outcomes
                .iter()
                .filter(|o| {
                    o.predicted_probability >= lower
                        && (o.predicted_probability < upper
                            || (index == bin_count - 1 && o.predicted_probability <= upper))
                })
                .collect();
            if members.is_empty() {
                bins.push(CalibrationBin {
                    lower,
                    upper,
                    count: 0,
                    mean_predicted: 0.0,
                    observed_rate: 0.0,
                });
                continue;
            }
            let bin_predicted =
                members.iter().map(|o| o.predicted_probability).sum::<f64>() / members.len() as f64;
            let bin_observed =
                members.iter().filter(|o| o.occurred).count() as f64 / members.len() as f64;
            weighted_error +=
                members.len() as f64 / count as f64 * (bin_predicted - bin_observed).abs();
            bins.push(CalibrationBin {
                lower,
                upper,
                count: members.len(),
                mean_predicted: bin_predicted,
                observed_rate: bin_observed,
            });
        }

        // Skill against the trivial forecast of always predicting the base rate.
        let reference_brier = base_rate * (1.0 - base_rate);
        let skill_score = if reference_brier > 0.0 {
            1.0 - brier_score / reference_brier
        } else {
            0.0
        };

        Self {
            count,
            brier_score,
            log_loss,
            bias: mean_predicted - base_rate,
            expected_calibration_error: weighted_error,
            bins,
            base_rate,
            skill_score,
        }
    }

    /// Whether the predictions are systematically overconfident.
    pub fn is_overconfident(&self, tolerance: f64) -> bool {
        self.bias > tolerance
    }

    /// Multiplier a caller should apply to stated confidences to correct the
    /// observed bias.
    ///
    /// The learning stage feeds this back into the reasoning engine, so an
    /// agent whose confidences have been running hot is discounted rather than
    /// merely noted.
    pub fn confidence_adjustment(&self) -> f64 {
        if self.count < 20 {
            // Too few observations to justify adjusting anything.
            return 1.0;
        }
        let mean_predicted = self.base_rate + self.bias;
        if mean_predicted <= 1e-9 {
            return 1.0;
        }
        (self.base_rate / mean_predicted).clamp(0.5, 1.5)
    }
}

/// Distribution shift between a reference sample and a current one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    /// Population stability index. Under 0.1 is stable, 0.1-0.25 is a
    /// moderate shift, above 0.25 is a material one.
    pub population_stability_index: f64,
    /// Difference in means, in reference standard deviations.
    pub standardised_mean_shift: f64,
    /// Ratio of current to reference standard deviation.
    pub volatility_ratio: f64,
    pub reference_count: usize,
    pub current_count: usize,
}

impl DriftReport {
    /// Compare a current sample against a reference sample.
    pub fn compare(reference: &[f64], current: &[f64], bins: usize) -> Self {
        if reference.len() < 2 || current.len() < 2 {
            return Self {
                population_stability_index: 0.0,
                standardised_mean_shift: 0.0,
                volatility_ratio: 1.0,
                reference_count: reference.len(),
                current_count: current.len(),
            };
        }

        let reference_mean = qip_numerics::stats::mean(reference);
        let reference_sd = qip_numerics::stats::stddev(reference);
        let current_mean = qip_numerics::stats::mean(current);
        let current_sd = qip_numerics::stats::stddev(current);

        // Bin on the reference quantiles, so the comparison is against where
        // the reference data actually lived rather than an arbitrary grid.
        let bins = bins.max(2);
        let mut edges: Vec<f64> = (1..bins)
            .map(|i| qip_numerics::stats::quantile(reference, i as f64 / bins as f64))
            .collect();
        edges.dedup_by(|a, b| (*a - *b).abs() < 1e-12);

        let bucket_of =
            |value: f64| -> usize { edges.iter().take_while(|edge| value >= **edge).count() };
        let bucket_count = edges.len() + 1;

        let mut reference_shares = vec![0.0; bucket_count];
        for value in reference {
            reference_shares[bucket_of(*value)] += 1.0 / reference.len() as f64;
        }
        let mut current_shares = vec![0.0; bucket_count];
        for value in current {
            current_shares[bucket_of(*value)] += 1.0 / current.len() as f64;
        }

        // A share of zero would make the log infinite, so empty buckets are
        // floored at a share smaller than any real one.
        const FLOOR: f64 = 1e-6;
        let psi: f64 = reference_shares
            .iter()
            .zip(&current_shares)
            .map(|(r, c)| {
                let r = r.max(FLOOR);
                let c = c.max(FLOOR);
                (c - r) * (c / r).ln()
            })
            .sum();

        Self {
            population_stability_index: psi.max(0.0),
            standardised_mean_shift: if reference_sd > 0.0 {
                (current_mean - reference_mean) / reference_sd
            } else {
                0.0
            },
            volatility_ratio: if reference_sd > 0.0 {
                current_sd / reference_sd
            } else {
                1.0
            },
            reference_count: reference.len(),
            current_count: current.len(),
        }
    }

    /// Whether the shift is material enough to require re-evaluation.
    pub fn requires_attention(&self) -> bool {
        self.population_stability_index > 0.25
            || self.standardised_mean_shift.abs() > 1.0
            || !(0.5..2.0).contains(&self.volatility_ratio)
    }

    /// A short description for an operator.
    pub fn summary(&self) -> String {
        format!(
            "PSI {:.3}, mean shift {:.2} sd, volatility ratio {:.2} ({} reference vs {} current)",
            self.population_stability_index,
            self.standardised_mean_shift,
            self.volatility_ratio,
            self.reference_count,
            self.current_count
        )
    }
}
