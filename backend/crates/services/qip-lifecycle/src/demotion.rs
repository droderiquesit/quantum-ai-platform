//! Demotions that fire without a human in the loop.
//!
//! Promotion is slow on purpose: evidence, a gate, two names. Demotion has to
//! be fast, and anything fast enough to matter cannot wait for a person. The
//! triggers here run on live observations and push a strategy down themselves.
//!
//! Seven things end a strategy's run at its current rung, and none of them is
//! a judgement call:
//!
//! * **Performance decay** against the pilot baseline. Measured with
//!   [`qip_simulation_engine::validation::assess_overfitting`], which is
//!   already the comparison "how much of what we measured survived" — the
//!   pilot is the in-sample side and live is the out-of-sample side.
//! * **Regime drift**: realised volatility has moved far enough from the
//!   pilot's that the strategy is trading a different market from the one it
//!   was sized in.
//! * **A breached kill condition**, stated at the pilot gate and evaluated
//!   here.
//! * **An overdue model review**, taken straight from
//!   [`qip_ai::registry::ModelCard::decision_eligibility`] rather than
//!   re-derived, so a model that may not drive a decision cannot drive one
//!   through a strategy either.
//! * **An expired capital envelope**. The cell will already have stopped —
//!   [`CapitalEnvelope::admit`] refuses once expiry passes — and the central
//!   plane's record has to agree, or an operator reads a stage that no longer
//!   describes anything.
//! * **Live performance outside the holdout band** — the Phase 3 gate's own
//!   criterion, judged by [`LifecycleLedger::band_verdict`] against the band
//!   the holdout admission carries. Distinct from decay: decay compares live
//!   to the pilot, the band compares live to the validation the whole walk
//!   rests on, and it is two-sided.
//! * **Sustained underperformance** at the floor. A strategy that was pushed
//!   off capital and is still decaying against its pilot once the
//!   [`RetirementThreshold`] has elapsed is retired, not demoted — the
//!   blueprint's §20.3, "retirement is as automated as promotion". A platform
//!   that only ever adds strategies accumulates dead weight consuming
//!   evaluation budget while contributing nothing.
//!
//! Every trigger demotes to a rung that holds no capital, except performance
//! decay, which steps down one rung, and sustained underperformance, which
//! retires. The reasoning is in [`DemotionTrigger::demote_to`].
//!
//! The retirement is judged on time at the floor rather than on a count of
//! reviews, and the reason is where the count would have to live. The
//! monitor is `Copy` and stateless — every review copies it out of the
//! factory — and the ledger records moves and nothing else, so a review that
//! found decay and moved nothing leaves no record a later review could count.
//! A counter kept anywhere else would be a fact the ledger cannot replay,
//! and a retirement decided on it would not be reproducible from the record.
//! The instant the strategy was pushed off capital *is* in the ledger, so
//! "still decaying this long after being demoted" is a decision the ledger
//! and one observation reproduce exactly, and one that does not depend on
//! how often the LEARN stage happened to run.

use crate::evidence::KillCondition;
use crate::ledger::LifecycleLedger;
use qip_ai::registry::ModelRegistry;
use qip_contracts::CapitalEnvelope;
use qip_contracts::gate::{GateStage, Promotion};
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_numerics::stats;
use qip_simulation_engine::validation::assess_overfitting;
use serde::{Deserialize, Serialize};

/// What the pilot established, and therefore what live performance is judged
/// against.
///
/// Recorded once, when the strategy entered pilot. Refreshing it from live
/// data would make decay undetectable: a baseline that follows the strategy
/// down is not a baseline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PilotBaseline {
    pub strategy: StrategyId,
    pub established_at: Timestamp,
    /// Returns realised during the pilot, in time order.
    pub returns: Vec<f64>,
    /// The conditions stated at the pilot gate.
    pub kill_conditions: Vec<KillCondition>,
    /// The model driving the strategy, as `name@version`.
    pub model_reference: Option<String>,
}

/// What the strategy has actually done lately.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiveObservation {
    pub strategy: StrategyId,
    pub at: Timestamp,
    /// Returns since the baseline was established, in time order.
    pub returns: Vec<f64>,
    /// Cumulative realised loss, positive when money has been lost.
    pub realised_loss: Decimal,
    /// Peak-to-trough drawdown as a fraction of the high-water mark.
    pub peak_to_trough_drawdown: f64,
    pub consecutive_losing_days: u32,
    /// Mean realised cost per fill, in basis points.
    pub realised_cost_bps: f64,
    /// The envelope the strategy is trading under, if it holds one.
    pub envelope: Option<CapitalEnvelope>,
}

impl KillCondition {
    /// Whether this condition has been breached, and by how much.
    ///
    /// Returns the detail rather than a bare bool so the demotion record says
    /// what happened, not merely that something did.
    pub fn breach(&self, observation: &LiveObservation) -> Option<String> {
        match self {
            Self::RealisedLoss(limit) => (observation.realised_loss >= *limit).then(|| {
                format!(
                    "realised loss {} reached the {limit} kill condition",
                    observation.realised_loss
                )
            }),
            Self::Drawdown(fraction) => {
                (observation.peak_to_trough_drawdown >= *fraction).then(|| {
                    format!(
                        "drawdown {:.2}% reached the {:.2}% kill condition",
                        observation.peak_to_trough_drawdown * 100.0,
                        fraction * 100.0
                    )
                })
            }
            Self::ConsecutiveLosingDays(days) => (observation.consecutive_losing_days >= *days)
                .then(|| {
                    format!(
                        "{} consecutive losing session(s) reached the {days}-session kill \
                         condition",
                        observation.consecutive_losing_days
                    )
                }),
            Self::CostOverrun {
                modelled_bps,
                tolerance_bps,
            } => (observation.realised_cost_bps > modelled_bps + tolerance_bps).then(|| {
                format!(
                    "realised cost {:.2}bp exceeds the modelled {modelled_bps:.2}bp by more \
                     than the {tolerance_bps:.2}bp tolerance",
                    observation.realised_cost_bps
                )
            }),
        }
    }
}

/// Why a strategy is being pushed down.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DemotionTrigger {
    /// Live performance has not held up against the pilot.
    PerformanceDecay {
        baseline_sharpe: f64,
        live_sharpe: f64,
        /// Fraction of the pilot's performance that survived.
        retained: f64,
    },
    /// The market the strategy is trading is not the one it was sized in.
    RegimeDrift {
        baseline_volatility: f64,
        live_volatility: f64,
        /// Absolute log ratio of the two volatilities.
        shift: f64,
    },
    /// A condition the strategy stated before it started has been met.
    KillConditionBreached { condition: String, detail: String },
    /// The model behind the strategy may no longer drive a decision.
    ModelReviewOverdue { model: String, reason: String },
    /// The grant the cell is trading under has run out.
    CapitalEnvelopeExpired { expired_at: Timestamp },
    /// Live performance has left the band the holdout validation defined,
    /// in either direction.
    OutsideHoldoutBand {
        live_sharpe: f64,
        lower: f64,
        upper: f64,
        observations: usize,
    },
    /// The strategy was pushed off capital, has stayed in decay against its
    /// pilot for at least the retirement threshold, and is now retired.
    SustainedUnderperformance {
        /// When the demotion that put it at the floor was recorded.
        at_floor_since: Timestamp,
        /// How long it has been there, still decaying.
        decaying_for: Duration,
        /// The threshold it passed.
        threshold: Duration,
    },
}

impl DemotionTrigger {
    pub fn describe(&self) -> String {
        match self {
            Self::PerformanceDecay {
                baseline_sharpe,
                live_sharpe,
                retained,
            } => format!(
                "pilot Sharpe {baseline_sharpe:.2} became {live_sharpe:.2} live, retaining \
                 {:.0}%",
                retained * 100.0
            ),
            Self::RegimeDrift {
                baseline_volatility,
                live_volatility,
                shift,
            } => format!(
                "realised volatility moved from {baseline_volatility:.4} to \
                 {live_volatility:.4}, a log shift of {shift:.2}"
            ),
            Self::KillConditionBreached { condition, detail } => {
                format!("kill condition ({condition}) breached: {detail}")
            }
            Self::ModelReviewOverdue { model, reason } => {
                format!("{model} may not drive a decision: {reason}")
            }
            Self::CapitalEnvelopeExpired { expired_at } => format!(
                "the capital envelope expired at {}",
                expired_at.to_rfc3339()
            ),
            Self::OutsideHoldoutBand {
                live_sharpe,
                lower,
                upper,
                observations,
            } => format!(
                "live Sharpe {live_sharpe:.2} over {observations} observation(s) is outside \
                 the holdout band [{lower:.2}, {upper:.2}]"
            ),
            Self::SustainedUnderperformance {
                at_floor_since,
                decaying_for,
                threshold,
            } => format!(
                "pushed off capital at {} and still decaying {:.1} day(s) later, past the \
                 {:.1}-day retirement threshold",
                at_floor_since.to_rfc3339(),
                decaying_for.as_days_f64(),
                threshold.as_days_f64()
            ),
        }
    }

    /// A short name for the trigger, for metrics and log lines.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::PerformanceDecay { .. } => "performance_decay",
            Self::RegimeDrift { .. } => "regime_drift",
            Self::KillConditionBreached { .. } => "kill_condition_breached",
            Self::ModelReviewOverdue { .. } => "model_review_overdue",
            Self::CapitalEnvelopeExpired { .. } => "capital_envelope_expired",
            Self::OutsideHoldoutBand { .. } => "outside_holdout_band",
            Self::SustainedUnderperformance { .. } => "sustained_underperformance",
        }
    }

    /// Where this trigger puts the strategy.
    ///
    /// Decay steps down one rung, because a strategy that has retained some of
    /// its edge at pilot size may well be fine at pilot size — what has been
    /// falsified is the case for the larger allocation, not the strategy.
    ///
    /// Sustained underperformance retires. It fires only for a strategy that
    /// is already at the floor and has stayed in decay there for the whole
    /// [`RetirementThreshold`], so by the time it fires the recoverable case
    /// has had its chance and not taken it.
    ///
    /// Everything else drops to shadow, the highest rung that holds no
    /// capital. Drift, a breached kill condition, an unreviewable model and an
    /// expired grant all say the same thing: the reason this strategy was
    /// trusted with money no longer holds. Shadow rather than retired because
    /// these are recoverable — the strategy keeps computing decisions, and if
    /// it re-earns the shadow and pilot gates it can come back.
    pub const fn demote_to(&self, from: GateStage) -> GateStage {
        match self {
            Self::PerformanceDecay { .. } => match from {
                GateStage::Scaled => GateStage::Pilot,
                _ => GateStage::Shadow,
            },
            Self::SustainedUnderperformance { .. } => GateStage::Retired,
            _ => GateStage::Shadow,
        }
    }
}

/// How long a strategy may sit at the floor, still decaying, before it is
/// retired without anyone being asked.
///
/// Built only through [`Self::after_decaying_for`], which refuses a zero or
/// negative span: a zero threshold would retire a strategy on the very review
/// that demoted it, which is a demotion wearing retirement's terminal
/// consequences, and a negative one would fire on every review after the
/// demotion regardless of what the clock said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementThreshold {
    sustained_for: Duration,
}

impl RetirementThreshold {
    /// Retire a strategy that is still in decay this long after being pushed
    /// off capital.
    pub fn after_decaying_for(sustained_for: Duration) -> Result<Self> {
        if sustained_for <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "a retirement threshold of {:.1} day(s) is refused: a strategy retired the \
                 instant it is demoted has had no time at the floor to recover, so nothing \
                 about its retirement was sustained. Give the threshold a positive span, such \
                 as `Duration::from_days(90)`",
                sustained_for.as_days_f64()
            )));
        }
        Ok(Self { sustained_for })
    }

    /// The span a strategy must have been decaying at the floor.
    pub const fn sustained_for(&self) -> Duration {
        self.sustained_for
    }
}

/// The bars the automatic triggers apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemotionPolicy {
    /// Fraction of pilot performance that must survive live.
    pub minimum_retained_performance: f64,
    /// Absolute log ratio of live to baseline volatility that counts as a
    /// regime change.
    pub maximum_regime_shift: f64,
    /// Live observations below which decay and drift are not yet judged.
    pub minimum_live_observations: usize,
    /// How long a strategy pushed off capital may keep decaying before it is
    /// retired.
    pub retirement: RetirementThreshold,
}

impl Default for DemotionPolicy {
    fn default() -> Self {
        Self {
            // A quarter at the floor, still decaying. The decay judgement
            // itself needs twenty live sessions, so a strategy has had at
            // least that long to re-earn its rung before the clock matters;
            // ninety days is long enough that a regime the strategy was not
            // sized for has had time to pass, and short enough that a dead
            // strategy does not hold its evaluation slot for a year. Built
            // from the literal rather than the refusing constructor because a
            // default cannot fail, and the literal is positive — the unit
            // test beside this module holds it to that.
            retirement: RetirementThreshold {
                sustained_for: Duration::from_days(90),
            },
            // The same 30% bar the overfitting assessment uses for a backtest.
            // Live performance losing 70% of the pilot's is the same finding
            // as out-of-sample losing 70% of in-sample, arriving later and
            // costing money.
            minimum_retained_performance: 0.3,
            // ln(2): realised volatility has doubled or halved. Position
            // sizing, stop distances and the impact estimate were all set in
            // the old regime, and every one of them is wrong by a factor of
            // two in the new one.
            maximum_regime_shift: std::f64::consts::LN_2,
            // Twenty sessions. Below that the standard error on a Sharpe
            // ratio is wide enough that a demotion would be noise, and the
            // point of an automatic trigger is that it does not cry wolf.
            minimum_live_observations: 20,
        }
    }
}

/// Evaluates the automatic triggers and applies them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DemotionMonitor {
    pub policy: DemotionPolicy,
}

impl DemotionMonitor {
    pub fn new(policy: DemotionPolicy) -> Self {
        Self { policy }
    }

    /// Everything wrong right now, in a stable order.
    ///
    /// Returns every trigger rather than the first, because an incident review
    /// needs to know that the model was also overdue, not only that the loss
    /// limit was hit first.
    pub fn triggers(
        &self,
        baseline: &PilotBaseline,
        observation: &LiveObservation,
        models: Option<&ModelRegistry>,
        now: Timestamp,
    ) -> Vec<DemotionTrigger> {
        let mut triggers = Vec::new();

        if observation.returns.len() >= self.policy.minimum_live_observations {
            if let Ok(report) = assess_overfitting(
                std::slice::from_ref(&baseline.returns),
                std::slice::from_ref(&observation.returns),
            ) && report.degradation < self.policy.minimum_retained_performance
            {
                triggers.push(DemotionTrigger::PerformanceDecay {
                    baseline_sharpe: report.in_sample_sharpe,
                    live_sharpe: report.out_of_sample_sharpe,
                    retained: report.degradation,
                });
            }

            let baseline_volatility = stats::stddev(&baseline.returns);
            let live_volatility = stats::stddev(&observation.returns);
            if baseline_volatility > 0.0 && live_volatility > 0.0 {
                let shift = (live_volatility / baseline_volatility).ln().abs();
                if shift > self.policy.maximum_regime_shift {
                    triggers.push(DemotionTrigger::RegimeDrift {
                        baseline_volatility,
                        live_volatility,
                        shift,
                    });
                }
            }
        }

        for condition in &baseline.kill_conditions {
            if let Some(detail) = condition.breach(observation) {
                triggers.push(DemotionTrigger::KillConditionBreached {
                    condition: condition.describe(),
                    detail,
                });
            }
        }

        if let (Some(reference), Some(registry)) = (baseline.model_reference.as_ref(), models)
            && let Err(error) = registry.require_for_decision(reference, now)
        {
            triggers.push(DemotionTrigger::ModelReviewOverdue {
                model: reference.clone(),
                reason: error.message().to_string(),
            });
        }

        if let Some(envelope) = observation.envelope.as_ref()
            && !envelope.is_live(now)
        {
            triggers.push(DemotionTrigger::CapitalEnvelopeExpired {
                expired_at: envelope.expires_at(),
            });
        }

        triggers
    }

    /// Evaluate the triggers and act on them.
    ///
    /// Adds the holdout-band trigger to [`Self::triggers`], which cannot see
    /// the ledger the band lives in, and applies the same observation floor
    /// to it. A capital-holding strategy with no band on record stops the
    /// monitor with the ledger's refusal rather than being judged against
    /// nothing; below capital there is no live performance to judge, and the
    /// absence is ignored.
    ///
    /// Demotes to the lowest rung any trigger asks for, in one move, so the
    /// ledger records one demotion with every reason rather than a cascade
    /// that is harder to read back. Returns the triggers and the demotion, or
    /// the triggers and `None` when the strategy is already at or below where
    /// they would put it.
    ///
    /// Retires instead when the strategy is already at the floor and
    /// [`Self::retirement_due`] finds it has been decaying there past the
    /// policy's [`RetirementThreshold`] — through the ledger's own `retire`,
    /// with the same attribution a demotion carries and no approver, because
    /// a demotion needs none and this is the demotion that was always going
    /// to follow.
    ///
    /// A strategy that is already retired is reported and left alone: its
    /// triggers are returned so the review can say what it found, and nothing
    /// is moved, because a cell keeps reporting on a strategy for a while
    /// after the centre withdraws it and a review that failed on every such
    /// report would stop the whole learn tick for every other strategy in it.
    ///
    /// What retirement does *not* do yet: disposition the strategy's open
    /// positions. Blueprint §35.2 says a retired strategy's positions are
    /// reassigned or scheduled for unwinding, never left ownerless, and
    /// nothing here or anywhere in the tree does that — the ledger records
    /// the withdrawal and the books are untouched. That composition crosses
    /// the lifecycle, portfolio and capital services and belongs in the
    /// kernel; §35 owes it, and this crate does not pretend to have paid it.
    pub fn enforce(
        &self,
        ledger: &mut LifecycleLedger,
        baseline: &PilotBaseline,
        observation: &LiveObservation,
        models: Option<&ModelRegistry>,
        now: Timestamp,
    ) -> Result<(Vec<DemotionTrigger>, Option<Promotion>)> {
        let mut triggers = self.triggers(baseline, observation, models, now);
        let current = ledger.stage_of(&baseline.strategy);

        if observation.returns.len() >= self.policy.minimum_live_observations {
            match ledger.band_verdict(&baseline.strategy, &observation.returns) {
                Ok(verdict) if !verdict.inside => {
                    triggers.push(DemotionTrigger::OutsideHoldoutBand {
                        live_sharpe: verdict.live,
                        lower: verdict.lower,
                        upper: verdict.upper,
                        observations: verdict.observations,
                    });
                }
                Ok(_) => {}
                Err(error) if current.holds_capital() => return Err(error),
                Err(_) => {}
            }
        }

        if triggers.is_empty() || current == GateStage::Retired {
            return Ok((triggers, None));
        }

        if let Some(retirement) =
            self.retirement_due(ledger, &baseline.strategy, current, &triggers, now)
        {
            triggers.push(retirement);
            let (raised_by, reason) = Self::attribution(&triggers);
            let retired = ledger.retire(&baseline.strategy, raised_by, reason, now)?;
            return Ok((triggers, Some(retired)));
        }

        let Some(target) = triggers
            .iter()
            .map(|trigger| trigger.demote_to(current))
            .min()
        else {
            return Ok((triggers, None));
        };
        if target >= current {
            return Ok((triggers, None));
        }

        let (raised_by, reason) = Self::attribution(&triggers);
        let demotion = ledger.demote(&baseline.strategy, target, raised_by, reason, now)?;
        Ok((triggers, Some(demotion)))
    }

    /// Whether this review is the one that retires the strategy.
    ///
    /// Three things have to be true at once, and each is read from the record
    /// rather than remembered: the strategy holds no capital; its last move
    /// was downward, so it was *pushed* to where it stands rather than still
    /// climbing; and this review found performance decay, at least the
    /// policy's threshold after that push. Only decay counts as the sustained
    /// signal — a breached kill condition or an expired envelope at the floor
    /// says nothing about whether the edge is gone.
    ///
    /// Returns the trigger to record rather than a bool, so the ledger's
    /// rationale carries the two instants and the span an incident review
    /// will want to check.
    fn retirement_due(
        &self,
        ledger: &LifecycleLedger,
        strategy: &StrategyId,
        current: GateStage,
        triggers: &[DemotionTrigger],
        now: Timestamp,
    ) -> Option<DemotionTrigger> {
        if current.holds_capital() {
            return None;
        }
        if !triggers
            .iter()
            .any(|trigger| matches!(trigger, DemotionTrigger::PerformanceDecay { .. }))
        {
            return None;
        }
        let last = ledger.history(strategy).last()?;
        if last.promotion.to >= last.promotion.from {
            return None;
        }
        let at_floor_since = last.promotion.at;
        let decaying_for = now.since(at_floor_since);
        let threshold = self.policy.retirement.sustained_for();
        if decaying_for < threshold {
            return None;
        }
        Some(DemotionTrigger::SustainedUnderperformance {
            at_floor_since,
            decaying_for,
            threshold,
        })
    }

    /// The `raised_by` and `reason` a move records: every trigger's name, and
    /// every trigger's account of itself.
    fn attribution(triggers: &[DemotionTrigger]) -> (String, String) {
        let raised_by = triggers
            .iter()
            .map(|trigger| trigger.name())
            .collect::<Vec<_>>()
            .join(",");
        let reason = triggers
            .iter()
            .map(DemotionTrigger::describe)
            .collect::<Vec<_>>()
            .join("; ");
        (raised_by, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default policy is built from a literal rather than the refusing
    /// constructor, so nothing checks it at runtime. A zero here would retire
    /// every strategy on the review that demoted it, in every composition
    /// root that takes the default.
    #[test]
    fn the_default_retirement_threshold_is_a_positive_span() {
        let threshold = DemotionPolicy::default().retirement.sustained_for();
        assert!(
            threshold > Duration::ZERO,
            "the default threshold is {threshold:?}"
        );
        let admitted = RetirementThreshold::after_decaying_for(threshold)
            .map(|admitted| admitted.sustained_for())
            .ok();
        assert_eq!(
            admitted,
            Some(threshold),
            "the default must be a value the constructor would have admitted"
        );
    }
}
