//! A capital plan, scored against what the strategies it funded then
//! realised (R12-MODEL-10123, owner decision 36).
//!
//! The allocator's plan, narrowed by the capital society (ADR 0229, ADR
//! 0230), says which strategy gets how much capital in which cell. Until this
//! module the plan was recorded and never judged: LEARN scored forecasts,
//! agents and routings, and the one decision that moves money stayed
//! unmarked, so an allocator that funded losers every day read the same as
//! one that funded winners.
//!
//! A plan is retained when it is made and scored once its window has closed,
//! against the returns each funded strategy realised in that cell *after* the
//! plan and not before it: a strategy's realised series is cumulative from
//! its baseline, and a plan scored on the whole series would be credited with
//! history it had no hand in.
//!
//! A score is journaled and nothing reads it back into an allocation. It
//! narrows no line and widens none.

use qip_contracts::allocation::AllocationPlan;
use qip_contracts::signal::StrategyId;
use qip_core::{Decimal, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::central::learning::CellOutcome;

/// The producer every [`CapitalPlanScored`] record is journaled under.
pub const CAPITAL_SCORE_ORIGIN: &str = "kernel/capital-score";

/// A plan held until its window closes, with how many realised returns each
/// funded line already had, so the score reads only what came after.
#[derive(Clone, Debug, PartialEq)]
pub struct RetainedCapitalPlan {
    pub plan: AllocationPlan,
    baselines: BTreeMap<(String, StrategyId), usize>,
}

impl RetainedCapitalPlan {
    /// Hold `plan`, noting the length of every realised series at this moment.
    pub fn retain(plan: AllocationPlan, outcomes: &[CellOutcome]) -> Self {
        let baselines = outcomes
            .iter()
            .map(|outcome| {
                (
                    (outcome.cell.clone(), outcome.strategy.clone()),
                    outcome.realised_returns.len(),
                )
            })
            .collect();
        Self { plan, baselines }
    }
}

/// One funded line judged against what its strategy realised afterwards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalLineScore {
    pub strategy: StrategyId,
    pub cell: String,
    pub notional: Decimal,
    /// Sum of the returns realised after the plan. A statistic, so `f64`.
    pub realised_return: f64,
    /// Whether capital put here earned something: the sum is above zero.
    pub correct: bool,
}

/// One capital plan resolved against the returns that followed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalPlanScored {
    pub cycle: u64,
    pub planned_at: Timestamp,
    pub scored_at: Timestamp,
    pub lines: Vec<CapitalLineScore>,
    /// Funded lines with no return realised after the plan. They are left
    /// out of the count rather than charged as misses: a strategy that did
    /// not trade in the window was not shown to be wrong.
    pub unscored: usize,
}

impl CapitalPlanScored {
    /// How many scored lines earned a positive return.
    pub fn correct(&self) -> usize {
        self.lines.iter().filter(|line| line.correct).count()
    }
}

impl EventBody for CapitalPlanScored {
    const TOPIC: Topic = Topic::ModelEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("capital-plan-scored:{}", self.planned_at.as_nanos()))
    }
}

/// Score a retained plan against the outcomes as they stand now.
///
/// `None` when no funded line had a return to be judged on: a record that
/// scored nothing would read as a plan that was assessed.
pub fn score_plan(
    retained: &RetainedCapitalPlan,
    outcomes: &[CellOutcome],
    cycle: u64,
    now: Timestamp,
) -> Option<CapitalPlanScored> {
    let mut lines = Vec::new();
    let mut unscored = 0usize;
    for allocation in &retained.plan.allocations {
        if !allocation.notional.is_positive() {
            continue;
        }
        let key = (allocation.cell.clone(), allocation.strategy.clone());
        let baseline = retained.baselines.get(&key).copied().unwrap_or(0);
        let after: Option<f64> = outcomes
            .iter()
            .find(|outcome| outcome.cell == key.0 && outcome.strategy == key.1)
            .and_then(|outcome| outcome.realised_returns.get(baseline..))
            .filter(|returns| !returns.is_empty())
            .map(|returns| returns.iter().sum::<f64>())
            .filter(|sum| sum.is_finite());
        match after {
            Some(realised_return) => lines.push(CapitalLineScore {
                strategy: allocation.strategy.clone(),
                cell: allocation.cell.clone(),
                notional: allocation.notional,
                realised_return,
                correct: realised_return > 0.0,
            }),
            None => unscored += 1,
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(CapitalPlanScored {
        cycle,
        planned_at: retained.plan.at,
        scored_at: now,
        lines,
        unscored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::allocation::Allocation;
    use qip_contracts::venue::VenueId;
    use qip_core::dec;

    fn at(hours: i64) -> Timestamp {
        Timestamp::from_nanos(hours * 3_600_000_000_000)
    }

    fn line(strategy: &str, notional: Decimal) -> Allocation {
        Allocation {
            strategy: StrategyId::new(strategy),
            cell: "home".to_string(),
            venue: VenueId::new("XNYS"),
            notional,
            indicated: notional,
            risk_adjusted_edge: 0.1,
            binding_constraints: Vec::new(),
        }
    }

    fn plan(allocations: Vec<Allocation>) -> AllocationPlan {
        AllocationPlan {
            at: at(1),
            total_budget: dec!("1000"),
            drawdown: 0.0,
            drawdown_multiplier: dec!("1"),
            budget: dec!("1000"),
            allocations,
            refusals: Vec::new(),
        }
    }

    fn outcome(strategy: &str, returns: Vec<f64>) -> CellOutcome {
        CellOutcome::new(StrategyId::new(strategy), "home", at(30), returns)
    }

    #[test]
    fn a_funded_strategy_that_earned_after_the_plan_scores_right_and_one_that_lost_scores_wrong() {
        let retained = RetainedCapitalPlan::retain(
            plan(vec![line("winner", dec!("100")), line("loser", dec!("100"))]),
            &[],
        );
        assert_eq!(retained.plan.allocations.len(), 2, "premise: two funded lines");
        let scored = score_plan(
            &retained,
            &[
                outcome("winner", vec![0.01, 0.02]),
                outcome("loser", vec![-0.03, 0.01]),
            ],
            4,
            at(30),
        )
        .expect("both lines had returns");
        assert_eq!(scored.lines.len(), 2);
        let verdicts: BTreeMap<&str, bool> = scored
            .lines
            .iter()
            .map(|l| (l.strategy.as_str(), l.correct))
            .collect();
        assert_eq!(verdicts.get("winner"), Some(&true));
        assert_eq!(verdicts.get("loser"), Some(&false));
        assert_eq!(scored.correct(), 1);
    }

    #[test]
    fn returns_realised_before_the_plan_do_not_credit_it() {
        let history = outcome("s", vec![0.5, 0.5]);
        let retained = RetainedCapitalPlan::retain(
            plan(vec![line("s", dec!("100"))]),
            std::slice::from_ref(&history),
        );
        let later = outcome("s", vec![0.5, 0.5, -0.02]);
        let scored = score_plan(&retained, &[later], 1, at(30)).expect("one return after");
        assert_eq!(scored.lines.len(), 1, "premise: the line was scored");
        assert!(
            !scored.lines[0].correct,
            "the plan was credited with returns that came before it: {scored:?}"
        );
    }

    #[test]
    fn a_funded_line_with_no_return_after_the_plan_is_left_unscored_not_charged() {
        let retained = RetainedCapitalPlan::retain(
            plan(vec![line("quiet", dec!("100")), line("busy", dec!("100"))]),
            &[outcome("quiet", vec![0.1])],
        );
        let scored = score_plan(
            &retained,
            &[outcome("quiet", vec![0.1]), outcome("busy", vec![0.01])],
            1,
            at(30),
        )
        .expect("the busy line had a return");
        assert_eq!(scored.unscored, 1);
        assert_eq!(scored.lines.len(), 1);
        assert_eq!(scored.lines[0].strategy.as_str(), "busy");
    }

    #[test]
    fn a_plan_with_nothing_to_judge_scores_nothing() {
        let retained = RetainedCapitalPlan::retain(plan(vec![line("s", dec!("100"))]), &[]);
        assert!(score_plan(&retained, &[], 1, at(30)).is_none());
    }
}
