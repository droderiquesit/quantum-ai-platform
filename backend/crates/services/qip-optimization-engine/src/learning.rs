//! What the router measured, and what it has learned from it (§11.1–11.2,
//! R12-QUANT-4047 and 4053).
//!
//! Before this module the router's only memory was a per-size win tally held
//! in the quantum arm's mutex. It learned from one thing (win or not), about
//! one substrate (the QPU), and it forgot everything at every restart: a
//! deep brain that had watched the arm lose nineteen times running would,
//! after a rollout, read it as a coin flip again and pay for it again. The
//! requirement is that routing is learned from measured quality, calibration,
//! latency, repeatability and cost, so the router now writes one
//! [`SolverMeasurement`] per run onto the [`crate::ComputeDecision`] it
//! journals, and folds the same measurements into a [`RoutingLearning`].
//!
//! **One fold, two callers.** [`RoutingLearning::absorb`] is what the live
//! solve calls and what a restarted process calls over the journal's
//! decisions, so the learning a replay rebuilds is the learning the live
//! router held by construction, not by two implementations agreeing. That is
//! the property `qip-kernel`'s `compute_routing` module relies on to resume
//! the router from the event log.

use crate::compute::Substrate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What became of one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementOutcome {
    /// An answer came back and was scored on the real problem.
    Answered,
    /// An answer came back and failed classical verification.
    Refused,
    /// No answer came back within the patience; a trial without a win.
    Abandoned,
    /// An answer about a different universe or encoding; dropped unread and
    /// not a trial — the substrate was not asked this question.
    Stale,
    /// The gateway no longer held the job; not a trial, for the same reason.
    Lost,
}

impl MeasurementOutcome {
    /// Whether the outcome counts as a trial of the substrate.
    pub const fn is_trial(&self) -> bool {
        matches!(self, Self::Answered | Self::Refused | Self::Abandoned)
    }
}

/// One run's measured outcome, journaled on the decision (R12-QUANT-4053).
///
/// Statistics, `f64`. `cost_micros` is the provider's declared price in USD
/// micro-units, billed when the job was sent — a job is paid for whether or
/// not its answer is used. Latency is in cycles because the router reads no
/// clock: a cycle is the unit an answer's lateness costs anything in, and a
/// wall-clock figure here would make the record a function of the machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverMeasurement {
    /// The name the solver registry knows the run by.
    pub solver: String,
    pub substrate: Substrate,
    pub problem_size: usize,
    pub outcome: MeasurementOutcome,
    /// Whether the answer passed classical verification and was feasible on
    /// the real problem.
    pub verified: bool,
    /// Whether it beat the classical baseline by more than the margin — the
    /// predicate the selection adopts on, and the one the learning counts.
    pub beat_baseline: bool,
    /// Whether its answer is the one the cycle used.
    pub used: bool,
    /// Improvement over the classical baseline, as a fraction; zero for the
    /// baseline and for a run with no answer.
    pub quality: f64,
    pub cost_micros: u64,
    pub latency_cycles: u32,
    /// The win probability the router estimated when it asked, where it
    /// asked through an estimate; what calibration is scored against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_win_probability: Option<f64>,
}

/// One substrate's measured record at one problem size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SubstrateRecord {
    pub trials: u32,
    pub wins: u32,
    /// Trials whose answer verified.
    pub verified: u32,
    pub quality_sum: f64,
    pub latency_cycles: u64,
    pub cost_micros: u64,
    /// Sum of squared errors between the predicted win probability and the
    /// outcome, over the trials that carried a prediction.
    pub brier_sum: f64,
    pub predicted: u32,
    /// Sum of the predicted win probabilities, and the wins among the same
    /// trials: together they say which way a poor calibration leans.
    pub predicted_sum: f64,
    pub predicted_wins: u32,
}

impl SubstrateRecord {
    /// `(wins + 1) / (trials + 2)`: one win and one loss of prior, so an
    /// untried substrate reads as a coin flip and one that always loses
    /// decays towards zero instead of being retried for ever.
    pub fn win_probability(&self) -> f64 {
        (f64::from(self.wins) + 1.0) / (f64::from(self.trials) + 2.0)
    }

    /// The share of trials whose answer verified; `None` before the first.
    pub fn repeatability(&self) -> Option<f64> {
        (self.trials > 0).then(|| f64::from(self.verified) / f64::from(self.trials))
    }

    /// Mean cycles from asking to reading; `None` before the first trial.
    pub fn mean_latency_cycles(&self) -> Option<f64> {
        (self.trials > 0).then(|| self.latency_cycles as f64 / f64::from(self.trials))
    }

    /// Mean improvement over the baseline across verified trials.
    pub fn mean_quality(&self) -> Option<f64> {
        (self.verified > 0).then(|| self.quality_sum / f64::from(self.verified))
    }

    /// The Brier score of the router's own win estimates: how well it knew
    /// in advance whether the substrate would pay. `None` until a trial
    /// carried a prediction.
    pub fn calibration_brier(&self) -> Option<f64> {
        (self.predicted > 0).then(|| self.brier_sum / f64::from(self.predicted))
    }

    /// The mean predicted win probability less the realised win share, over
    /// the trials that carried a prediction: positive when the router
    /// expected more than the substrate delivered. `None` until a trial
    /// carried a prediction.
    pub fn overconfidence(&self) -> Option<f64> {
        (self.predicted > 0).then(|| {
            let trials = f64::from(self.predicted);
            self.predicted_sum / trials - f64::from(self.predicted_wins) / trials
        })
    }
}

/// The router's learned routing state: a [`SubstrateRecord`] per substrate
/// and problem size. A `BTreeMap` because it is read into records whose
/// order must not move between replays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoutingLearning {
    records: BTreeMap<(Substrate, usize), SubstrateRecord>,
}

impl RoutingLearning {
    /// Fold one decision's measurements in. The live router and a replay over
    /// the journal both call this and nothing else.
    pub fn absorb(&mut self, measurements: &[SolverMeasurement]) {
        for measurement in measurements {
            if !measurement.outcome.is_trial() {
                continue;
            }
            let record = self
                .records
                .entry((measurement.substrate, measurement.problem_size))
                .or_default();
            record.trials = record.trials.saturating_add(1);
            if measurement.beat_baseline {
                record.wins = record.wins.saturating_add(1);
            }
            if measurement.verified {
                record.verified = record.verified.saturating_add(1);
                if measurement.quality.is_finite() {
                    record.quality_sum += measurement.quality;
                }
            }
            record.latency_cycles = record
                .latency_cycles
                .saturating_add(u64::from(measurement.latency_cycles));
            record.cost_micros = record.cost_micros.saturating_add(measurement.cost_micros);
            if let Some(predicted) = measurement.predicted_win_probability
                && predicted.is_finite()
            {
                let realised = if measurement.beat_baseline { 1.0 } else { 0.0 };
                record.brier_sum += (predicted - realised).powi(2);
                record.predicted = record.predicted.saturating_add(1);
                record.predicted_sum += predicted;
                if measurement.beat_baseline {
                    record.predicted_wins = record.predicted_wins.saturating_add(1);
                }
            }
        }
    }

    /// The record at a size, or the empty record — an untried substrate.
    pub fn record(&self, substrate: Substrate, problem_size: usize) -> SubstrateRecord {
        self.records
            .get(&(substrate, problem_size))
            .copied()
            .unwrap_or_default()
    }

    /// Every record, in substrate-then-size order.
    pub fn records(&self) -> impl Iterator<Item = (&(Substrate, usize), &SubstrateRecord)> {
        self.records.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
