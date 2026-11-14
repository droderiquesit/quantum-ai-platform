//! A predictive interval with a coverage guarantee, calibrated on rows the fit
//! never saw (R12-MODEL-4006).
//!
//! A point prediction says nothing about how much to trust it; a standard
//! error derived from the model's own training residuals says how well it
//! memorised. Split conformal prediction takes the model as a black box and
//! the absolute errors it made on a *calibration* set the fit did not touch,
//! and widens every prediction by the `⌈(n+1)(1−α)⌉`-th smallest of them. If
//! the calibration rows and a new row are exchangeable the interval then
//! covers the realised value with probability at least `1−α`, whatever the
//! model is. The platform's holdout tail is that calibration set.
//!
//! Exchangeability is the assumption, and a time series only approximately
//! has it: under a regime change the realised coverage falls below `1−α`.
//! The interval is therefore recorded with the rows it was calibrated on, so
//! a reader sees how much evidence stands behind it.

use qip_core::error::{Error, Result};

use crate::dataset::TrainingDataset;
use crate::local::TrainedTeacher;

/// A symmetric interval around a prediction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConformalInterval {
    alpha: f64,
    half_width: f64,
    calibration_rows: usize,
}

impl ConformalInterval {
    /// Calibrate `teacher`'s interval on `calibration`, for miscoverage
    /// `alpha` (0.1 asks for 90% coverage).
    ///
    /// Refuses an `alpha` outside (0, 1), and a calibration set too small to
    /// support it: with `n` rows the finite-sample quantile index
    /// `⌈(n+1)(1−α)⌉` must not exceed `n`, and when it would the honest
    /// interval is unbounded — a number would be a guess.
    pub fn calibrate(
        teacher: &TrainedTeacher,
        calibration: &TrainingDataset,
        alpha: f64,
    ) -> Result<Self> {
        Self::of_predictions(
            &teacher.predict_all(calibration)?,
            calibration.targets(),
            alpha,
        )
    }

    /// Calibrate an interval from `predictions` made on calibration rows and
    /// the `targets` those rows realised, whatever produced the predictions:
    /// one teacher, or a blend of several ([`crate::Ensemble`]).
    ///
    /// The guarantee is the same and so is its condition: the predictor must
    /// not have been fitted, or its weights chosen, on these rows. Refuses a
    /// prediction count that differs from the target count, and everything
    /// [`Self::calibrate`] refuses.
    pub fn of_predictions(predictions: &[f64], targets: &[f64], alpha: f64) -> Result<Self> {
        if !alpha.is_finite() || alpha <= 0.0 || alpha >= 1.0 {
            return Err(Error::invalid(format!(
                "a conformal miscoverage must lie strictly between 0 and 1, not {alpha}"
            )));
        }
        if predictions.len() != targets.len() {
            return Err(Error::invalid(format!(
                "{} prediction(s) against {} target(s); an interval is calibrated on whole rows, \
                 not on whatever lines up",
                predictions.len(),
                targets.len()
            )));
        }
        let mut errors: Vec<f64> = predictions
            .iter()
            .zip(targets)
            .map(|(prediction, target)| (prediction - target).abs())
            .collect();
        let n = errors.len();
        // The 1-based rank of the quantile: ceil((n + 1)(1 - alpha)).
        let rank = (((n + 1) as f64) * (1.0 - alpha)).ceil() as usize;
        if rank > n {
            return Err(Error::invalid(format!(
                "{n} calibration row(s) cannot support a {:.0}% interval: the quantile rank \
                 {rank} is past the last row, so the honest interval is unbounded; calibrate \
                 on at least {} row(s) or ask for a wider miscoverage",
                (1.0 - alpha) * 100.0,
                ((1.0 / alpha).ceil() as usize).saturating_sub(1)
            )));
        }
        if errors.iter().any(|e| !e.is_finite()) {
            return Err(Error::numeric(
                "a calibration residual is not finite; the interval would be meaningless",
            ));
        }
        errors.sort_by(f64::total_cmp);
        Ok(Self {
            alpha,
            half_width: errors[rank - 1],
            calibration_rows: n,
        })
    }

    /// The miscoverage this interval was calibrated for.
    pub const fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Distance from a prediction to either end of its interval.
    pub const fn half_width(&self) -> f64 {
        self.half_width
    }

    /// How many rows stand behind it.
    pub const fn calibration_rows(&self) -> usize {
        self.calibration_rows
    }

    /// The interval around `prediction`.
    pub fn around(&self, prediction: f64) -> (f64, f64) {
        (prediction - self.half_width, prediction + self.half_width)
    }
}
