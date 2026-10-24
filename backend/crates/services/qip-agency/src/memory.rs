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
//! only after the action ran — or, since the shadow-learning lane, after a
//! pass shadowed it — and its attribution validated. Which of the two it was
//! is the record's twelfth field, [`Observed`], and it is not optional: a
//! simulated effect filed as a measured one is the exact lie this memory
//! exists to refuse, and [`ActionOutcomeMemory::recommend_for`] reads only
//! what was measured, so the platform cannot learn to repeat what its own
//! model predicted would work.
//!
//! The memory is a bounded working set. When it is full the oldest record
//! leaves it; the durable record of an intervention is the event log, not
//! this.

use crate::affordance::Method;
use crate::autonomy::{Authority, Drift};
use crate::{non_empty, required, text};
use qip_core::{Decimal, Error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Where an outcome's observation came from. Required on every record, so a
/// reader never has to infer from a figure whether anything happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observed {
    /// The success metric was measured in the world after an executed action.
    Measured,
    /// The figures are the pass's own simulation of the action it shadowed:
    /// nothing ran, nothing was reached, and the no-action counterfactual is
    /// the structural model's, not the venue's.
    Simulated,
}

/// The nine fields §24 requires, and the audience and delay R12-AGENCY-6108
/// learns over, each optional so absence is a refusal that names the field.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Measured in the world, or simulated by the pass that shadowed the
    /// action.
    pub observed: Option<Observed>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub observed: Observed,
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
            observed: required("observed", self.observed)?,
        })
    }
}

/// What a plan's failure calls for, specific to the plan that failed
/// (R12-RES-7111).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Mitigation {
    /// Stop: send no further step of these tools for this class. The
    /// policy is already back at shadow, so the engine will not; the
    /// record says which tools a person restarting the class must look at.
    Halt { tools: Vec<String> },
    /// Roll back one executed step, newest first. Only after an executed
    /// act, and every step narrow-reversible authority could send was
    /// declared reversible, so a reversal exists. A shadowed act sent
    /// nothing and has nothing to reverse.
    Reverse { tool: String },
}

/// A plan that did not do what its policy predicted: the plan, how it
/// departed, the authority it cost, and the stop and roll-back it calls for.
/// Kept beside the outcomes so a failure is a record the platform can read
/// back, not only a policy silently narrowed (R12-RES-7111).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureEpisode {
    pub class: Method,
    pub action_sequence: Vec<String>,
    pub environment_state: String,
    pub observed: Observed,
    /// `observed - counterfactual`.
    pub effect: Decimal,
    /// Every departure; never empty.
    pub drift: Vec<Drift>,
    pub authority_before: Authority,
    pub authority_after: Authority,
    /// The halt first, then each executed step's reversal, newest first.
    pub mitigations: Vec<Mitigation>,
}

impl FailureEpisode {
    /// The episode for `outcome`, or `None` when nothing drifted: a plan
    /// that held to its prediction has not failed.
    pub fn of(
        class: Method,
        outcome: &ActionOutcome,
        effect: Decimal,
        drift: Vec<Drift>,
        authority: (Authority, Authority),
    ) -> Option<Self> {
        if drift.is_empty() {
            return None;
        }
        let mut mitigations = vec![Mitigation::Halt {
            tools: outcome.action_sequence.clone(),
        }];
        if outcome.observed == Observed::Measured {
            mitigations.extend(
                outcome
                    .action_sequence
                    .iter()
                    .rev()
                    .map(|tool| Mitigation::Reverse { tool: tool.clone() }),
            );
        }
        Some(Self {
            class,
            action_sequence: outcome.action_sequence.clone(),
            environment_state: outcome.environment_state.clone(),
            observed: outcome.observed,
            effect,
            drift,
            authority_before: authority.0,
            authority_after: authority.1,
            mitigations,
        })
    }
}

/// The most recent `capacity` outcomes, and as many failure episodes.
#[derive(Debug)]
pub struct ActionOutcomeMemory {
    capacity: usize,
    records: VecDeque<ActionOutcome>,
    failures: VecDeque<FailureEpisode>,
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
            failures: VecDeque::new(),
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

    /// Keep a failure episode, evicting the oldest when full.
    pub(crate) fn keep_failure(&mut self, episode: FailureEpisode) {
        if self.failures.len() == self.capacity {
            self.failures.pop_front();
        }
        self.failures.push_back(episode);
    }

    /// Every failure episode held, oldest first.
    pub fn failures(&self) -> impl Iterator<Item = &FailureEpisode> {
        self.failures.iter()
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
    /// evidence about this one. Only [`Observed::Measured`] records are
    /// read: a simulated outcome is the planner's own prediction filed under
    /// another name, and recommending on it would teach the platform to
    /// repeat what its model said would work rather than what did. A
    /// sequence seen fewer than `min_support`
    /// times there has not earned an opinion, and a mean effect
    /// (`observed - counterfactual`) that is not strictly positive loses to
    /// doing nothing. The winner carries its mean delay, so a caller knows
    /// how long to wait before judging it. A record whose
    /// `attribution_confidence` is zero is remembered and never read: it rests
    /// on no identified attribution (ADR 0145).
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
            // A record at zero confidence rests on no identified attribution
            // (`engine::attribute` refuses a confidence above the
            // attribution's identifiability, and an unidentified effect is
            // filed at zero), so a mean over it is a correlation presenting
            // as a recommendation. It stays remembered; it is not read
            // (ADR 0145 decision 2).
            if outcome.observed != Observed::Measured
                || outcome.attribution_confidence <= Decimal::ZERO
                || outcome.environment_state != environment_state
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
