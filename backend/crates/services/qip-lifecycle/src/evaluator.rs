//! An evaluator that judges a model from its predictions alone
//! (R12-MODEL-6001).
//!
//! # Why this exists
//!
//! The deep brain's class scoreboard decides which model class is registered
//! in a regime, and it was fed by `FitDiagnostics::claims_skill` — a verdict
//! the trainer computes about its own fit. The creator of an idea was its sole
//! judge: a trainer that overstated its holdout skill would be believed by the
//! only reader that decides what is registered. This type is the other party.
//! It takes the *predictions* a model made on a holdout and the *targets* that
//! happened, and computes the skill itself. Its inputs are two slices of
//! numbers, so there is nothing a generator's own score can be passed in as.
//!
//! Independence here is structural rather than promised: this file names no
//! trainer type and no fit diagnostic, and the architecture suite in
//! `qip-acceptance` fails if it ever does.

use qip_core::error::{Error, Result};

/// What a model has to show on a holdout before it is called skilled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvaluatorPolicy {
    /// Held-out observations needed to say anything.
    pub minimum_observations: usize,
    /// Out-of-sample R² the predictions must reach. Five per cent: a model
    /// with no signal averages slightly below zero out of sample.
    pub minimum_r2: f64,
}

impl Default for EvaluatorPolicy {
    fn default() -> Self {
        Self {
            minimum_observations: 30,
            minimum_r2: 0.05,
        }
    }
}

/// The evaluator's finding on one holdout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Judgement {
    pub observations: usize,
    /// Coefficient of determination of the predictions against the targets;
    /// negative when the model does worse than predicting the mean.
    pub r2: f64,
    pub skilled: bool,
}

/// Judges predictions against outcomes, knowing nothing of how the
/// predictions were produced.
#[derive(Clone, Copy, Debug, Default)]
pub struct Evaluator {
    policy: EvaluatorPolicy,
}

impl Evaluator {
    pub const fn new(policy: EvaluatorPolicy) -> Self {
        Self { policy }
    }

    pub const fn policy(&self) -> &EvaluatorPolicy {
        &self.policy
    }

    /// Judge `predictions` against `targets`.
    ///
    /// Refuses unequal lengths and non-finite values rather than scoring the
    /// part that lines up: a judgement over a truncated holdout is a verdict
    /// on a sample nobody chose. A holdout too short to say anything is a
    /// judgement of "not skilled", not an error — the same answer a gate
    /// gives for evidence not submitted.
    pub fn judge(&self, predictions: &[f64], targets: &[f64]) -> Result<Judgement> {
        if predictions.len() != targets.len() {
            return Err(Error::invalid(format!(
                "{} prediction(s) against {} target(s); an evaluator scores whole holdouts, not \
                 whatever lines up",
                predictions.len(),
                targets.len()
            )));
        }
        if predictions
            .iter()
            .chain(targets.iter())
            .any(|value| !value.is_finite())
        {
            return Err(Error::numeric(
                "a prediction or a target is not finite; one NaN would score every other as \
                 skilled or unskilled for no reason",
            ));
        }
        let observations = targets.len();
        if observations < self.policy.minimum_observations || observations == 0 {
            return Ok(Judgement {
                observations,
                r2: f64::NEG_INFINITY,
                skilled: false,
            });
        }
        let mean = targets.iter().sum::<f64>() / observations as f64;
        let total: f64 = targets.iter().map(|t| (t - mean).powi(2)).sum();
        let residual: f64 = predictions
            .iter()
            .zip(targets)
            .map(|(p, t)| (t - p).powi(2))
            .sum();
        // A holdout whose targets do not vary has no variance to explain; its
        // R² is undefined, and an undefined score is not skill.
        if total <= 0.0 {
            return Ok(Judgement {
                observations,
                r2: f64::NEG_INFINITY,
                skilled: false,
            });
        }
        let r2 = 1.0 - residual / total;
        Ok(Judgement {
            observations,
            r2,
            skilled: r2 >= self.policy.minimum_r2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(n: usize) -> Vec<f64> {
        (0..n).map(|i| ((i * 7) % 11) as f64 - 5.0).collect()
    }

    #[test]
    fn predictions_that_track_the_targets_are_skilled_and_the_mean_is_not() {
        let evaluator = Evaluator::default();
        let targets = series(60);
        let tracking = evaluator.judge(&targets, &targets).expect("judges");
        assert!(tracking.skilled && (tracking.r2 - 1.0).abs() < 1e-12);
        let mean = targets.iter().sum::<f64>() / 60.0;
        let flat = vec![mean; 60];
        let judged = evaluator.judge(&flat, &targets).expect("judges");
        assert!(
            !judged.skilled,
            "predicting the mean explains nothing: {judged:?}"
        );
        assert!(judged.r2.abs() < 1e-9);
    }

    #[test]
    fn a_holdout_too_short_to_say_anything_is_unskilled_and_a_mismatch_is_refused() {
        let evaluator = Evaluator::default();
        let short = series(10);
        assert!(!evaluator.judge(&short, &short).expect("judges").skilled);
        assert!(evaluator.judge(&short, &series(11)).is_err());
        let mut broken = series(40);
        broken[3] = f64::NAN;
        assert!(evaluator.judge(&broken, &series(40)).is_err());
    }
}
