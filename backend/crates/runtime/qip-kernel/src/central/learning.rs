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
//! authority, and so does retiring: the monitor retires a strategy that was
//! pushed off capital and is still decaying past its
//! [`qip_lifecycle::demotion::RetirementThreshold`], through the same review
//! call, with no one asked (blueprint §20.3). Scaling up does not:
//! [`LearningVerdict::Scale`] is a recommendation that still has to walk
//! through the scaled gate and collect two more names. [`LearningVerdict::Retire`]
//! is therefore two things by turns — the record of a retirement the ledger
//! has already made, or a recommendation for one it has not yet — and
//! [`StrategyReview::stage_after`] says which.
//!
//! **A retirement dispositions the strategy's open positions** (blueprint
//! §35.2: "never orphaned silently"). The review that retired a strategy is
//! followed, in the same call, by a [`RetirementDisposition`] naming every
//! lot the attribution holds for it — cell, instrument, signed quantity,
//! average price — and the instruction for each: unwind, as a flatten intent
//! for the owning cell's own DECIDE/ACT path. No order is created here, and
//! nothing reaches a venue from this module; the record is the schedule, and
//! [`CentralPlane::scheduled_unwinds`] is the same schedule read back from the
//! ledger and the books, so a retired strategy still holding a lot is a
//! visible state rather than an orphan nobody can list. The composition sits
//! here rather than in `qip-lifecycle` or `qip-capital` because it crosses
//! them: the ledger knows the retirement and the books know the lots.
//!
//! The disposition is refused rather than guessed when the centre holds two
//! claims about the strategy's positions and they disagree. The attribution's
//! books are what the centre moves positions from; a cell's reported book is
//! a second claim where a cell has made one. A retirement whose lots the two
//! claims cannot agree on is a [`DispositionRefused`] record — the
//! reconciliation break §35.2 says an ownerless position is — and not a
//! flatten instruction for a quantity one of the two claims says is wrong.
//!
//! Handover — reassigning a position to a funded strategy that shares its
//! thesis — is not produced. The centre records no thesis shared between two
//! strategies, and a handover chosen on anything else would be an owner
//! picked to make the record look complete. The instruction has one arm for
//! that reason, and grows a second when the shared-thesis fact exists.

use super::factory::StrategyReview;
use super::plane::CentralPlane;
use qip_ai::registry::ModelRegistry;
use qip_contracts::gate::{GateStage, Promotion};
use qip_contracts::signal::StrategyId;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use qip_events::{EventBody, Topic};
use qip_lifecycle::demotion::LiveObservation;
use qip_simulation_engine::validation::assess_overfitting;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
/// [`Self::Adapt`] and the demotion behind it happen on their own, and so
/// does [`Self::Retire`] when the review's `stage_after` is
/// [`GateStage::Retired`]. [`Self::Scale`] is a reading: the one verdict that
/// would commit more capital needs two people.
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
    /// The edge is gone rather than smaller. Either the ledger has already
    /// retired the strategy — sustained decay at the floor, no one asked —
    /// or realised performance is not positive and the strategy has just
    /// been demoted out of capital, in which case this is the recommendation
    /// the automatic retirement will act on if the decay holds.
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

    /// The verdict one review argues for.
    ///
    /// A review that ends at [`GateStage::Retired`] is read as retirement
    /// first, whatever else fired: the monitor retires from the floor, so
    /// such a review never started at a capital rung, and the
    /// demoted-off-capital reading below would otherwise call a retirement
    /// [`Self::Adapt`] — a terminal move reported as one to be adapted from.
    fn for_review(review: &StrategyReview, realised: f64, retained: f64) -> Self {
        if review.stage_after == GateStage::Retired {
            return Self::Retire;
        }
        let demoted_off_capital =
            review.stage_before.holds_capital() && !review.stage_after.holds_capital();
        if demoted_off_capital && realised <= 0.0 {
            Self::Retire
        } else if review.triggers.is_empty() {
            if retained >= 1.0 {
                Self::Scale
            } else {
                Self::Hold
            }
        } else {
            Self::Adapt
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

/// What is to be done with one of a retired strategy's positions.
///
/// One arm, deliberately: see the module documentation for why handover is
/// absent until the centre records which strategies share a thesis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispositionInstruction {
    /// Flatten the lot through the owning cell's own DECIDE/ACT path: the
    /// signed quantity to trade that brings the lot to zero. An intent for
    /// the cell to size and route against its ladder, never an order the
    /// centre submits.
    Unwind { flatten_by: Decimal },
}

/// One open lot a retired strategy held, and what is to be done with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DispositionedPosition {
    pub cell: String,
    pub instrument: String,
    /// Signed, as the attribution holds it; negative is short.
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub instruction: DispositionInstruction,
}

/// Blueprint §35.2: the record that a retired strategy's positions were not
/// left ownerless.
///
/// Written to the event log at the retirement, so the instruction is
/// reproducible from the log alone and a lot still open afterwards can be
/// read against it. An empty `positions` is recorded too: "held nothing" is
/// a finding about the book, and the absence of a record would be
/// indistinguishable from a retirement nobody dispositioned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetirementDisposition {
    pub strategy: StrategyId,
    pub retired_at: Timestamp,
    /// The retirement's rationale as the ledger recorded it.
    pub rationale: String,
    /// Keyed `cell/instrument`, the same order the strategy books iterate in,
    /// so a replay lists the lots as this record did.
    pub positions: BTreeMap<String, DispositionedPosition>,
}

impl EventBody for RetirementDisposition {
    // A claim about positions and what happens to them, which is what the
    // Act group's position topic is for; retained permanently with it.
    const TOPIC: Topic = Topic::PositionUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // Retirement is terminal, so one strategy is dispositioned once.
        Some(format!("retirement-disposition:{}", self.strategy))
    }
}

/// The two claims the centre holds about one of a retired strategy's lots,
/// which do not agree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionDiscrepancy {
    pub cell: String,
    pub instrument: String,
    /// What the attribution's books hold for the strategy there.
    pub attributed: Decimal,
    /// What the cell's last reported book says the strategy holds there.
    pub reported: Decimal,
}

/// Blueprint §35.2's other case: a retirement whose positions the centre
/// cannot name, recorded as the reconciliation break it is rather than as a
/// flatten instruction for a quantity one of two claims says is wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DispositionRefused {
    pub strategy: StrategyId,
    pub retired_at: Timestamp,
    /// Keyed `cell/instrument`, every lot on which the claims disagree.
    pub discrepancies: BTreeMap<String, PositionDiscrepancy>,
}

impl DispositionRefused {
    pub fn describe(&self) -> String {
        let lots: Vec<String> = self
            .discrepancies
            .values()
            .map(|d| {
                format!(
                    "{}/{}: attributed {}, cell reports {}",
                    d.cell, d.instrument, d.attributed, d.reported
                )
            })
            .collect();
        format!(
            "{} retired at {} with {} lot(s) the attribution and the cell's book disagree on; \
             no unwind was scheduled — reconcile the book, then disposition by hand: {}",
            self.strategy,
            self.retired_at,
            self.discrepancies.len(),
            lots.join("; ")
        )
    }
}

impl EventBody for DispositionRefused {
    // A reconciliation of the cell's book against the attribution that
    // finished in disagreement. Same group and retention as the disposition
    // it stands in for, so a replay finds the two side by side.
    const TOPIC: Topic = Topic::ReconciliationCompleted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("retirement-disposition-refused:{}", self.strategy))
    }
}

/// What one retirement did about the strategy's positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DispositionOutcome {
    Dispositioned(RetirementDisposition),
    Refused(DispositionRefused),
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
    /// One per strategy this tick retired: what became of its positions.
    pub dispositions: Vec<DispositionOutcome>,
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

        // Dispositioned before the learnings are assembled, at the seam where
        // the retirement is known: the review says the ledger retired the
        // strategy this tick, and the books are as the last settlement left
        // them. A review the monitor returned for an already-retired strategy
        // has no move and is not dispositioned again.
        let mut dispositions = Vec::new();
        for review in &reviews {
            if review.stage_after != GateStage::Retired || review.stage_before == GateStage::Retired
            {
                continue;
            }
            let Some(retirement) = &review.demotion else {
                continue;
            };
            dispositions.push(self.disposition_for(&review.strategy, retirement));
            self.forget_realised(&review.strategy);
        }

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
            let verdict = LearningVerdict::for_review(&review, realised, retained);

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
            dispositions,
        })
    }

    /// What becomes of one retired strategy's positions, from the two claims
    /// the centre holds about them.
    ///
    /// The attribution's books name the lots. Where the cell has reported a
    /// book of its own — the delta a cell ships carries none, so in a
    /// deployment this is the exception — that book must agree with the
    /// attribution on every lot of the strategy's, or the disposition is
    /// refused with each disagreement named. A cell that reported no book
    /// has made no second claim, and the attribution stands alone.
    fn disposition_for(&self, strategy: &StrategyId, retirement: &Promotion) -> DispositionOutcome {
        let mut attributed: BTreeMap<(String, String), (Decimal, Decimal)> = BTreeMap::new();
        for ((cell, owner, instrument), lot) in self.strategy_books() {
            if owner == strategy && !lot.quantity.is_zero() {
                attributed.insert(
                    (cell.clone(), instrument.clone()),
                    (lot.quantity, lot.average_price),
                );
            }
        }

        // The cells that have made a position claim at all, and what each
        // says the strategy holds. Two lines for one instrument at one cell
        // (two venues, say) are one holding.
        let mut claiming_cells: BTreeSet<String> = BTreeSet::new();
        let mut reported: BTreeMap<(String, String), Decimal> = BTreeMap::new();
        for position in self.reported_positions() {
            claiming_cells.insert(position.cell.clone());
            if &position.strategy == strategy && !position.quantity.is_zero() {
                *reported
                    .entry((position.cell.clone(), position.instrument.clone()))
                    .or_insert(Decimal::ZERO) += position.quantity;
            }
        }

        let mut discrepancies = BTreeMap::new();
        let lots: BTreeSet<&(String, String)> = attributed.keys().chain(reported.keys()).collect();
        for key in lots {
            let (cell, instrument) = key;
            if !claiming_cells.contains(cell) {
                continue;
            }
            let held = attributed.get(key).map_or(Decimal::ZERO, |(q, _)| *q);
            let claimed = reported.get(key).copied().unwrap_or(Decimal::ZERO);
            if held != claimed {
                discrepancies.insert(
                    format!("{cell}/{instrument}"),
                    PositionDiscrepancy {
                        cell: cell.clone(),
                        instrument: instrument.clone(),
                        attributed: held,
                        reported: claimed,
                    },
                );
            }
        }
        if !discrepancies.is_empty() {
            return DispositionOutcome::Refused(DispositionRefused {
                strategy: strategy.clone(),
                retired_at: retirement.at,
                discrepancies,
            });
        }

        let positions = attributed
            .into_iter()
            .map(|((cell, instrument), (quantity, average_price))| {
                (
                    format!("{cell}/{instrument}"),
                    DispositionedPosition {
                        cell,
                        instrument,
                        quantity,
                        average_price,
                        instruction: DispositionInstruction::Unwind {
                            flatten_by: -quantity,
                        },
                    },
                )
            })
            .collect();
        DispositionOutcome::Dispositioned(RetirementDisposition {
            strategy: strategy.clone(),
            retired_at: retirement.at,
            rationale: retirement.rationale.clone(),
            positions,
        })
    }

    /// Every lot a retired strategy still holds, by strategy then
    /// `cell/instrument`, with the flatten quantity for each.
    ///
    /// Derived from the ledger and the books on every call rather than kept
    /// as a schedule of its own, so there is no second record to drift from
    /// the settlements that move the lots: a lot the next fill closes leaves
    /// this list by the same arithmetic that closed it. Non-empty is the
    /// state §35.2 calls a reconciliation break, and this is how it is
    /// listed rather than discovered.
    pub fn scheduled_unwinds(&self) -> BTreeMap<StrategyId, BTreeMap<String, Decimal>> {
        let mut scheduled: BTreeMap<StrategyId, BTreeMap<String, Decimal>> = BTreeMap::new();
        for ((cell, strategy, instrument), lot) in self.strategy_books() {
            if lot.quantity.is_zero() || self.factory().stage_of(strategy) != GateStage::Retired {
                continue;
            }
            scheduled
                .entry(strategy.clone())
                .or_default()
                .insert(format!("{cell}/{instrument}"), -lot.quantity);
        }
        scheduled
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

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::gate::Promotion;
    use qip_core::Duration;
    use qip_lifecycle::demotion::DemotionTrigger;

    fn review(
        before: GateStage,
        after: GateStage,
        triggers: Vec<DemotionTrigger>,
    ) -> StrategyReview {
        let at = Timestamp::from_secs(1_700_000_000);
        let demotion = (before != after).then(|| Promotion {
            from: before,
            to: after,
            at,
            approver: None,
            rationale: "test".to_string(),
            evidence: Vec::new(),
        });
        StrategyReview {
            strategy: StrategyId::new("momentum-v3"),
            stage_before: before,
            stage_after: after,
            triggers,
            demotion,
        }
    }

    fn decay() -> DemotionTrigger {
        DemotionTrigger::PerformanceDecay {
            baseline_sharpe: 1.2,
            live_sharpe: 0.1,
            retained: 0.08,
        }
    }

    /// The monitor retires from the floor, so a retiring review starts at
    /// shadow, and the demoted-off-capital reading does not see it. Before
    /// this was ordered first, that review was reported as `adapt` — a
    /// terminal move the report told the desk to adapt from.
    #[test]
    fn a_review_the_ledger_retired_is_reported_as_retire_and_not_adapt() {
        let at = Timestamp::from_secs(1_700_000_000);
        let retiring = review(
            GateStage::Shadow,
            GateStage::Retired,
            vec![
                decay(),
                DemotionTrigger::SustainedUnderperformance {
                    at_floor_since: at,
                    decaying_for: Duration::from_days(90),
                    threshold: Duration::from_days(90),
                },
            ],
        );
        assert!(!retiring.triggers.is_empty(), "premise: triggers fired");
        assert!(
            !retiring.stage_before.holds_capital(),
            "premise: the retirement started at the floor, not at capital"
        );
        // Realised positive, so the older reading would never have said retire.
        assert_eq!(
            LearningVerdict::for_review(&retiring, 0.4, 0.3),
            LearningVerdict::Retire
        );

        // The same triggers on a review that only demoted still read as adapt.
        let demoted = review(GateStage::Pilot, GateStage::Shadow, vec![decay()]);
        assert_eq!(
            LearningVerdict::for_review(&demoted, 0.4, 0.3),
            LearningVerdict::Adapt
        );
    }
}
