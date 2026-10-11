//! AGENCY-050 / AGENCY-015: Action Outcome Memory, and what is learned from it.
//!
//! A memory that keeps "we did X and the KPI rose" teaches the platform to
//! repeat whatever it happened to be doing when the KPI rose anyway. Each
//! record therefore holds the prediction, the observation and the no-action
//! counterfactual side by side, with the context they were made in, and a
//! record missing any of the nine is refused: the effect learned from is
//! `observed - counterfactual`, never the observation alone.
//!
//! R12-AGENCY-6108 adds two declarations to the nine: the audience the
//! action reached and the delay before its effect was observed. Without them
//! the memory could say *what* worked in a regime but not *for whom* or *how
//! soon*, and a recommendation blind to delay credits a slow action's effect
//! to whatever happened to run just before it was measured.
//!
//! [`crate::engine::learn`] is the production writer: it keeps an outcome
//! only after the action ran and its attribution validated.
//!
//! The memory is a bounded working set. When it is full the oldest record
//! leaves it; the durable record of an intervention is the event log, not
//! this.

use crate::{non_empty, required, text};
use qip_core::{Decimal, Error};
use std::collections::{BTreeMap, VecDeque};

/// The nine fields §24 requires, and the audience and delay R12-AGENCY-6108
/// learns over, each optional so absence is a refusal that names the field.
#[derive(Debug, Clone, Default)]
pub struct ActionOutcomeDraft {
    pub intervention_context: Option<String>,
    /// The audience or environment state the action met: the regime.
    pub environment_state: Option<String>,
    /// Who the action reached: the segment, desk or venue population.
    pub audience: Option<String>,
    /// Seconds from the action to the observation of its effect.
    pub delay_secs: Option<u64>,
    /// The tools used, in order.
    pub action_sequence: Option<Vec<String>>,
    pub causal_hypothesis: Option<String>,
    /// Predicted change in the success metric.
    pub predicted_effect: Option<Decimal>,
    /// The success metric as observed after the action.
    pub actual_observations: Option<Decimal>,
    /// Confidence in the attribution, in `[0, 1]`.
    pub attribution_confidence: Option<Decimal>,
    /// May be declared empty ("none observed"), but must be declared.
    pub side_effects: Option<Vec<String>>,
    /// The success metric as estimated had nothing been done.
    pub no_action_counterfactual: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ActionOutcome {
    pub intervention_context: String,
    pub environment_state: String,
    pub audience: String,
    pub delay_secs: u64,
    pub action_sequence: Vec<String>,
    pub causal_hypothesis: String,
    pub predicted_effect: Decimal,
    pub actual_observations: Decimal,
    pub attribution_confidence: Decimal,
    pub side_effects: Vec<String>,
    pub no_action_counterfactual: Decimal,
}

impl ActionOutcomeDraft {
    pub fn build(self) -> Result<ActionOutcome, Error> {
        let confidence = required("attribution_confidence", self.attribution_confidence)?;
        if confidence < Decimal::ZERO || confidence > Decimal::ONE {
            return Err(Error::invalid(
                "`attribution_confidence` must lie in [0, 1]",
            ));
        }
        Ok(ActionOutcome {
            intervention_context: text("intervention_context", self.intervention_context)?,
            environment_state: text("environment_state", self.environment_state)?,
            audience: text("audience", self.audience)?,
            delay_secs: required("delay_secs", self.delay_secs)?,
            action_sequence: non_empty("action_sequence", self.action_sequence)?,
            causal_hypothesis: text("causal_hypothesis", self.causal_hypothesis)?,
            predicted_effect: required("predicted_effect", self.predicted_effect)?,
            actual_observations: required("actual_observations", self.actual_observations)?,
            attribution_confidence: confidence,
            side_effects: required("side_effects", self.side_effects)?,
            no_action_counterfactual: required(
                "no_action_counterfactual",
                self.no_action_counterfactual,
            )?,
        })
    }
}

/// The most recent `capacity` outcomes.
#[derive(Debug)]
pub struct ActionOutcomeMemory {
    capacity: usize,
    records: VecDeque<ActionOutcome>,
}

impl ActionOutcomeMemory {
    pub fn with_capacity(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a memory of zero records remembers nothing; give it a positive capacity",
            ));
        }
        Ok(Self {
            capacity,
            records: VecDeque::new(),
        })
    }

    /// Validate and keep one outcome. A refused draft is not kept.
    pub fn record(&mut self, draft: ActionOutcomeDraft) -> Result<(), Error> {
        let outcome = draft.build()?;
        self.keep(outcome);
        Ok(())
    }

    /// Keep an outcome already built, evicting the oldest when full.
    pub(crate) fn keep(&mut self, outcome: ActionOutcome) {
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(outcome);
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// AGENCY-015: the action sequence with the largest mean attributed
    /// effect in this environment state, whatever audience it reached, or
    /// `None` when not acting is the better intervention there.
    pub fn recommend(
        &self,
        environment_state: &str,
        min_support: usize,
    ) -> Result<Option<Vec<String>>, Error> {
        Ok(self
            .recommend_for(environment_state, None, min_support)?
            .map(|r| r.action_sequence))
    }

    /// R12-AGENCY-6108: what works, for whom, in which regime, with what
    /// delay — or `None` when not acting is better.
    ///
    /// Only records in `environment_state` (and, when given, reaching
    /// `audience`) are read; what worked for another audience is not
    /// evidence about this one. A sequence seen fewer than `min_support`
    /// times there has not earned an opinion, and a mean effect
    /// (`observed - counterfactual`) that is not strictly positive loses to
    /// doing nothing. The winner carries its mean delay, so a caller knows
    /// how long to wait before judging it.
    // ponytail: a plain mean over a support floor. Weight by attribution
    // confidence once an estimator exists (AGENCY-028).
    pub fn recommend_for(
        &self,
        environment_state: &str,
        audience: Option<&str>,
        min_support: usize,
    ) -> Result<Option<Recommendation>, Error> {
        if min_support == 0 {
            return Err(Error::invalid(
                "`min_support` of zero would learn from no evidence; require at least one",
            ));
        }
        // Per sequence: summed effect, count, summed delay.
        let mut seen: BTreeMap<&[String], (Decimal, i64, u128)> = BTreeMap::new();
        for outcome in &self.records {
            if outcome.environment_state != environment_state
                || audience.is_some_and(|a| a != outcome.audience)
            {
                continue;
            }
            let effect = outcome
                .actual_observations
                .checked_sub(outcome.no_action_counterfactual)
                .ok_or_else(|| Error::numeric("attributed effect overflowed"))?;
            let entry =
                seen.entry(outcome.action_sequence.as_slice())
                    .or_insert((Decimal::ZERO, 0, 0));
            entry.0 = entry
                .0
                .checked_add(effect)
                .ok_or_else(|| Error::numeric("summed effect overflowed"))?;
            entry.1 += 1;
            entry.2 += u128::from(outcome.delay_secs);
        }
        let mut best: Option<Recommendation> = None;
        for (sequence, (sum, count, delay)) in seen {
            if usize::try_from(count).is_ok_and(|n| n < min_support) {
                continue;
            }
            let mean = sum
                .checked_div(Decimal::from_int(count))
                .ok_or_else(|| Error::numeric("mean effect could not be computed"))?;
            if mean > best.as_ref().map_or(Decimal::ZERO, |b| b.mean_effect) {
                let support = u128::try_from(count)
                    .map_err(|_| Error::numeric("support count is negative"))?;
                let mean_delay = u64::try_from(delay / support)
                    .map_err(|_| Error::numeric("mean delay overflowed"))?;
                best = Some(Recommendation {
                    action_sequence: sequence.to_vec(),
                    mean_effect: mean,
                    mean_delay_secs: mean_delay,
                    support: count,
                });
            }
        }
        Ok(best)
    }
}

/// What the memory recommends, and on what.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub action_sequence: Vec<String>,
    /// Mean of `observed - counterfactual`, strictly positive.
    pub mean_effect: Decimal,
    /// Mean seconds from action to observed effect.
    pub mean_delay_secs: u64,
    /// How many records it rests on.
    pub support: i64,
}
