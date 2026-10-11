//! AGENCY-036: candidates are compared on nine declared axes, and all nine
//! are weighed.
//!
//! A comparison that silently omits `downside` or `conduct_risk` ranks the
//! dangerous option first. Records are only buildable with all nine, so
//! nothing partial can reach the ranker: [`crate::plan::select`] takes a
//! built `Comparison` on every proposal.
//!
//! Until R12-AGENCY-6087's round this module said the ranker read
//! `expected_causal_effect` and `legally_eligible` and that "the other seven
//! axes are recorded for the reader and weighed by nothing yet". A recorded
//! axis nothing reads is a computed value left unused, which looks like a
//! control and is not. Each axis now has one stated job, and
//! [`Comparison::refusal`] and [`Comparison::rank`] are where it is done:
//!
//! | Axis | How it is weighed |
//! |---|---|
//! | `legally_eligible` | refusal: an ineligible plan is not a candidate |
//! | `downside` | refusal above the goal's risk envelope; else first tie-break, lower wins |
//! | `time_to_effect_secs` | refusal beyond the goal's time horizon; else last tie-break, sooner wins |
//! | `expected_causal_effect` | the score's magnitude |
//! | `confidence` | multiplies the score: an effect nobody is sure of is worth less |
//! | `conduct_risk` | multiplies the score by `1 - conduct_risk` |
//! | `reversibility` | second tie-break, more reversible wins |
//! | `cost` | third tie-break, cheaper wins |
//! | `capital_usage` | fourth tie-break, less capital wins |
//!
//! The score is `effect × confidence × (1 − conduct_risk)`, and doing nothing
//! scores zero, so a plan must have a strictly positive score to be chosen
//! over the baseline. The bounds are refusals rather than penalties for the
//! reason [`crate::plan`] gives: a penalty loses to a large enough estimate.

use crate::goal::GoalSpec;
use crate::required;
use qip_core::{Decimal, Error};
use std::cmp::Ordering;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ComparisonDraft {
    pub expected_causal_effect: Option<Decimal>,
    pub confidence: Option<Decimal>,
    pub cost: Option<Decimal>,
    pub capital_usage: Option<Decimal>,
    pub time_to_effect_secs: Option<u64>,
    pub reversibility: Option<Decimal>,
    pub legally_eligible: Option<bool>,
    pub conduct_risk: Option<Decimal>,
    pub downside: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Comparison {
    pub expected_causal_effect: Decimal,
    pub confidence: Decimal,
    pub cost: Decimal,
    pub capital_usage: Decimal,
    pub time_to_effect_secs: u64,
    pub reversibility: Decimal,
    pub legally_eligible: bool,
    pub conduct_risk: Decimal,
    /// The size of the worst loss the plan risks, as a non-negative figure.
    pub downside: Decimal,
}

fn unit(field: &str, value: Option<Decimal>) -> Result<Decimal, Error> {
    let value = required(field, value)?;
    if value < Decimal::ZERO || value > Decimal::ONE {
        return Err(Error::invalid(format!(
            "`{field}` must lie in [0, 1]; it is refused, not clamped"
        )));
    }
    Ok(value)
}

fn non_negative(field: &str, value: Option<Decimal>) -> Result<Decimal, Error> {
    let value = required(field, value)?;
    if value.is_negative() {
        return Err(Error::invalid(format!(
            "`{field}` must not be negative; state its magnitude"
        )));
    }
    Ok(value)
}

impl ComparisonDraft {
    /// Refuse a missing axis, and an axis outside the range its weighing
    /// reads it in: a confidence of 1.5 would multiply an effect up, and a
    /// negative downside would read as a gain at the tie-break.
    pub fn build(self) -> Result<Comparison, Error> {
        Ok(Comparison {
            expected_causal_effect: required(
                "expected_causal_effect",
                self.expected_causal_effect,
            )?,
            confidence: unit("confidence", self.confidence)?,
            cost: non_negative("cost", self.cost)?,
            capital_usage: non_negative("capital_usage", self.capital_usage)?,
            time_to_effect_secs: required("time_to_effect_secs", self.time_to_effect_secs)?,
            reversibility: unit("reversibility", self.reversibility)?,
            legally_eligible: required("legally_eligible", self.legally_eligible)?,
            conduct_risk: unit("conduct_risk", self.conduct_risk)?,
            downside: non_negative("downside", self.downside)?,
        })
    }
}

impl Comparison {
    /// Assemble a comparison the engine computed itself (a generated
    /// candidate), through the same validation an operator's goes through.
    #[allow(clippy::too_many_arguments)]
    pub fn computed(
        expected_causal_effect: Decimal,
        confidence: Decimal,
        cost: Decimal,
        capital_usage: Decimal,
        time_to_effect_secs: u64,
        reversibility: Decimal,
        legally_eligible: bool,
        conduct_risk: Decimal,
        downside: Decimal,
    ) -> Result<Self, Error> {
        ComparisonDraft {
            expected_causal_effect: Some(expected_causal_effect),
            confidence: Some(confidence),
            cost: Some(cost),
            capital_usage: Some(capital_usage),
            time_to_effect_secs: Some(time_to_effect_secs),
            reversibility: Some(reversibility),
            legally_eligible: Some(legally_eligible),
            conduct_risk: Some(conduct_risk),
            downside: Some(downside),
        }
        .build()
    }

    /// The hard axes: why this candidate may not be chosen at all for
    /// `goal`, or `None`.
    pub fn refusal(&self, goal: &GoalSpec) -> Option<Error> {
        if !self.legally_eligible {
            return Some(Error::denied("the proposal is not legally eligible"));
        }
        if self.downside > goal.risk_envelope {
            return Some(Error::guard(format!(
                "the proposal's downside {} exceeds the goal's risk envelope {}; a loss the goal \
                 cannot absorb is not traded against its upside",
                self.downside, goal.risk_envelope
            )));
        }
        if self.time_to_effect_secs > goal.time_horizon_secs {
            return Some(Error::guard(format!(
                "the proposal takes effect after {}s, beyond the goal's {}s horizon; an effect \
                 after the horizon is no effect on this goal",
                self.time_to_effect_secs, goal.time_horizon_secs
            )));
        }
        None
    }

    /// `effect × confidence × (1 − conduct_risk)`. `Err` when the product
    /// does not fit, which the ranker treats as a refusal of that one
    /// candidate rather than of the pass: an estimate too large to weigh is
    /// not thereby the best.
    pub fn score(&self) -> Result<Decimal, Error> {
        let clean = Decimal::ONE
            .checked_sub(self.conduct_risk)
            .ok_or_else(|| Error::numeric("conduct weight overflowed"))?;
        self.expected_causal_effect
            .checked_mul(self.confidence)
            .and_then(|v| v.checked_mul(clean))
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the expected effect {} could not be weighed by its confidence and conduct \
                     risk without overflowing; state it in a smaller unit",
                    self.expected_causal_effect
                ))
            })
    }

    /// Order two candidates already scored: higher score first, then the
    /// tie-breaks in the table above. `Ordering::Greater` means `self` is
    /// preferred.
    pub fn rank(&self, own_score: Decimal, other: &Self, other_score: Decimal) -> Ordering {
        own_score
            .cmp(&other_score)
            .then_with(|| other.downside.cmp(&self.downside))
            .then_with(|| self.reversibility.cmp(&other.reversibility))
            .then_with(|| other.cost.cmp(&self.cost))
            .then_with(|| other.capital_usage.cmp(&self.capital_usage))
            .then_with(|| other.time_to_effect_secs.cmp(&self.time_to_effect_secs))
    }
}
