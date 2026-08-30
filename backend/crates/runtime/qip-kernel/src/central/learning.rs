//! The learn edge: what the cells actually did, fed back into what they are
//! allowed to do next.
//!
//! One comparison decides everything here — what the strategy was promoted on
//! against what it realised. The comparison itself is not written in this
//! module. [`assess_overfitting`] already answers "how much of what we measured
//! survived", with the pilot baseline as the in-sample side and live returns as
//! the out-of-sample side, and
//! [`qip_lifecycle::DemotionMonitor`] already turns a bad answer into a
//! demotion without a human in the call. What this module adds is the wiring
//! and one policy decision:
//!
//! **Uncertainty ratchets up, never down.** A strategy's standard error in the
//! allocator is raised to at least the gap between what was expected and what
//! arrived. [`qip_capital::CapitalAllocator`] sizes on `sharpe - k · se`, so
//! being wrong by more than the stated error bar means the error bar was
//! understated, and the next allocation is made on the wider one. Letting it
//! fall again on a good quarter would mean a strategy could talk its way back
//! to full size by being right once.
//!
//! The verdict is advisory in one direction only. Scaling down happens here,
//! automatically, because [`qip_lifecycle::LifecycleLedger::demote`] needs no
//! authority. Scaling up does not: [`LearningVerdict::Scale`] is a
//! recommendation that still has to walk through the scaled gate and collect
//! two more names, and [`LearningVerdict::Retire`] is a recommendation because
//! retirement is terminal and a human's call.

use super::factory::StrategyReview;
use super::plane::CentralPlane;
use qip_ai::registry::ModelRegistry;
use qip_contracts::signal::StrategyId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use qip_lifecycle::demotion::LiveObservation;
use qip_simulation_engine::validation::assess_overfitting;
use serde::{Deserialize, Serialize};

/// What one cell realised for one strategy since the baseline was established.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellOutcome {
    pub strategy: StrategyId,
    pub cell: String,
    pub at: Timestamp,
    /// Returns realised live, in time order.
    pub realised_returns: Vec<f64>,
    /// Cumulative realised loss, positive when money has been lost.
    pub realised_loss: Decimal,
    /// Peak-to-trough drawdown as a fraction of the high-water mark.
    pub peak_to_trough_drawdown: f64,
    pub consecutive_losing_days: u32,
    /// Mean realised cost per fill, in basis points.
    pub realised_cost_bps: f64,
}

impl CellOutcome {
    /// A quiet outcome: nothing lost, nothing drawn down, no losing streak.
    ///
    /// The starting point for a caller filling in one or two fields, so a test
    /// or a cell adapter does not have to write six zeroes to say "the only
    /// thing that happened is these returns".
    pub fn new(
        strategy: StrategyId,
        cell: impl Into<String>,
        at: Timestamp,
        realised_returns: Vec<f64>,
    ) -> Self {
        Self {
            strategy,
            cell: cell.into(),
            at,
            realised_returns,
            realised_loss: Decimal::ZERO,
            peak_to_trough_drawdown: 0.0,
            consecutive_losing_days: 0,
            realised_cost_bps: 0.0,
        }
    }

    pub fn with_realised_loss(mut self, loss: Decimal) -> Self {
        self.realised_loss = loss;
        self
    }

    pub fn with_drawdown(mut self, drawdown: f64) -> Self {
        self.peak_to_trough_drawdown = drawdown;
        self
    }

    pub fn with_losing_days(mut self, days: u32) -> Self {
        self.consecutive_losing_days = days;
        self
    }

    pub fn with_realised_cost_bps(mut self, bps: f64) -> Self {
        self.realised_cost_bps = bps;
        self
    }
}

/// What expected-versus-actual argues for.
///
/// Only [`Self::Adapt`] and the demotion behind it happen on their own. The
/// other three are readings, and the two that would commit more capital or end
/// a strategy for good need a person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LearningVerdict {
    /// Live matched or beat the baseline and nothing tripped. A candidate for
    /// the scaled gate, which is where the argument actually gets made.
    Scale,
    /// Live is below the baseline but nothing tripped. Leave it where it is.
    Hold,
    /// A trigger fired. The strategy has already been pushed down; what is
    /// left is deciding whether to re-earn the rung or change the strategy.
    Adapt,
    /// The edge is gone rather than smaller — realised performance is not
    /// positive and the strategy has been demoted out of capital.
    /// A recommendation: retirement is terminal, and terminal is a human's
    /// call.
    Retire,
}

impl LearningVerdict {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Scale => "scale",
            Self::Hold => "hold",
            Self::Adapt => "adapt",
            Self::Retire => "retire",
        }
    }
}

/// Expected against actual for one strategy, and what follows from it.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyLearning {
    pub strategy: StrategyId,
    pub cell: String,
    /// The triggers that fired and the demotion, if any.
    pub review: StrategyReview,
    /// The baseline's Sharpe — what the promotion was granted on.
    pub expected_sharpe: f64,
    /// The Sharpe live actually produced.
    pub realised_sharpe: f64,
    /// Fraction of the baseline that survived.
    pub retained: f64,
    pub verdict: LearningVerdict,
    /// The `(expected_sharpe, standard_error)` the allocator will size on
    /// next, where the strategy had a proposal to update.
    pub resized: Option<(f64, f64)>,
}

impl StrategyLearning {
    pub fn summarise(&self) -> String {
        format!(
            "{} at {}: expected {:.2}, realised {:.2} ({:.0}% retained), {} trigger(s) — {}",
            self.strategy,
            self.cell,
            self.expected_sharpe,
            self.realised_sharpe,
            self.retained * 100.0,
            self.review.triggers.len(),
            self.verdict.as_str()
        )
    }
}

/// What one learn edge produced.
#[derive(Clone, Debug, PartialEq)]
pub struct LearningReport {
    pub at: Timestamp,
    pub learnings: Vec<StrategyLearning>,
    /// Outcomes that could not be judged, with why. An outcome for a strategy
    /// that never reached pilot has no baseline to be judged against, and
    /// treating that as a failure would make the tick fail whenever a cell
    /// reported on something it also runs in shadow.
    pub skipped: Vec<(StrategyId, String)>,
}

impl LearningReport {
    /// Strategies this tick pushed down.
    pub fn demoted(&self) -> Vec<&StrategyLearning> {
        self.learnings
            .iter()
            .filter(|learning| learning.review.moved())
            .collect()
    }

    /// Strategies whose live performance argues for a scaling decision.
    pub fn scaling_candidates(&self) -> Vec<&StrategyLearning> {
        self.learnings
            .iter()
            .filter(|learning| learning.verdict == LearningVerdict::Scale)
            .collect()
    }
}

impl CentralPlane {
    /// Feed realised cell outcomes back into the ladder and the allocator.
    ///
    /// Two things happen per outcome and they are deliberately in this order.
    /// The demotion triggers run first, so a strategy that breached a kill
    /// condition is off capital before anything reasons about resizing it.
    /// Then the allocator's evidence is updated, so the next plan is built on
    /// what happened rather than on what was proposed.
    pub fn learn(
        &mut self,
        outcomes: &[CellOutcome],
        models: Option<&ModelRegistry>,
        now: Timestamp,
    ) -> Result<LearningReport> {
        let mut skipped = Vec::new();
        let mut observations = Vec::new();
        for outcome in outcomes {
            if self.factory().baseline(&outcome.strategy).is_none() {
                skipped.push((
                    outcome.strategy.clone(),
                    format!(
                        "{} has no pilot baseline, so there is nothing for live performance to \
                         have decayed from",
                        outcome.strategy
                    ),
                ));
                continue;
            }
            observations.push(LiveObservation {
                strategy: outcome.strategy.clone(),
                at: outcome.at,
                returns: outcome.realised_returns.clone(),
                realised_loss: outcome.realised_loss,
                peak_to_trough_drawdown: outcome.peak_to_trough_drawdown,
                consecutive_losing_days: outcome.consecutive_losing_days,
                realised_cost_bps: outcome.realised_cost_bps,
                envelope: self.envelope(&outcome.cell, &outcome.strategy).cloned(),
            });
        }

        let reviews = self.factory_mut().review(&observations, models, now)?;

        let mut learnings = Vec::new();
        for review in reviews {
            let Some(outcome) = outcomes
                .iter()
                .find(|outcome| outcome.strategy == review.strategy)
            else {
                continue;
            };
            let Some(baseline) = self.factory().baseline(&review.strategy) else {
                continue;
            };

            // The same comparison the demotion monitor makes, read for its
            // numbers rather than for its verdict: the baseline is the
            // in-sample side and live is the out-of-sample side.
            let comparison = assess_overfitting(
                std::slice::from_ref(&baseline.returns),
                std::slice::from_ref(&outcome.realised_returns),
            );
            let (expected, realised, retained) = match comparison {
                Ok(report) => (
                    report.in_sample_sharpe,
                    report.out_of_sample_sharpe,
                    report.degradation,
                ),
                Err(_) => (0.0, 0.0, 0.0),
            };

            let resized = self.resize(&review.strategy, expected, realised);
            let demoted_off_capital =
                review.stage_before.holds_capital() && !review.stage_after.holds_capital();
            let verdict = if demoted_off_capital && realised <= 0.0 {
                LearningVerdict::Retire
            } else if review.triggers.is_empty() {
                if retained >= 1.0 {
                    LearningVerdict::Scale
                } else {
                    LearningVerdict::Hold
                }
            } else {
                LearningVerdict::Adapt
            };

            learnings.push(StrategyLearning {
                strategy: review.strategy.clone(),
                cell: outcome.cell.clone(),
                review,
                expected_sharpe: expected,
                realised_sharpe: realised,
                retained,
                verdict,
                resized,
            });
        }

        Ok(LearningReport {
            at: now,
            learnings,
            skipped,
        })
    }

    /// Update the allocator's evidence for one strategy.
    ///
    /// The point estimate becomes what was realised. The standard error is
    /// raised to at least how wrong the last estimate was and never lowered —
    /// see this module's documentation for why the ratchet only turns one way.
    fn resize(
        &mut self,
        strategy: &StrategyId,
        expected: f64,
        realised: f64,
    ) -> Option<(f64, f64)> {
        let proposal = self.proposal(strategy)?.clone();
        let error = (expected - realised).abs();
        let widened = if error.is_finite() && error > proposal.sharpe_standard_error {
            error
        } else {
            proposal.sharpe_standard_error
        };
        let mut updated = proposal;
        updated.expected_sharpe = realised;
        updated.sharpe_standard_error = widened;
        let resized = (updated.expected_sharpe, updated.sharpe_standard_error);
        self.set_proposal(updated);
        Some(resized)
    }
}
