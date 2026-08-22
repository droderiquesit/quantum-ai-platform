//! Demotions that fire without a human in the loop.
//!
//! Promotion is slow on purpose: evidence, a gate, two names. Demotion has to
//! be fast, and anything fast enough to matter cannot wait for a person. The
//! triggers here run on live observations and push a strategy down themselves.
//!
//! Five things end a strategy's run at its current rung, and none of them is a
//! judgement call:
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
//!
//! Every trigger demotes to a rung that holds no capital, except performance
//! decay, which steps down one rung. The reasoning is in
//! [`DemotionTrigger::demote_to`].

use crate::evidence::KillCondition;
use crate::ledger::LifecycleLedger;
use qip_ai::registry::ModelRegistry;
use qip_contracts::gate::{GateStage, Promotion};
use qip_contracts::signal::StrategyId;
use qip_contracts::CapitalEnvelope;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
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
            Self::Drawdown(fraction) => (observation.peak_to_trough_drawdown >= *fraction).then(
                || {
                    format!(
                        "drawdown {:.2}% reached the {:.2}% kill condition",
                        observation.peak_to_trough_drawdown * 100.0,
                        fraction * 100.0
                    )
                },
            ),
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
        }
    }

    /// Where this trigger puts the strategy.
    ///
    /// Decay steps down one rung, because a strategy that has retained some of
    /// its edge at pilot size may well be fine at pilot size — what has been
    /// falsified is the case for the larger allocation, not the strategy.
    ///
    /// Everything else drops to shadow, the highest rung that holds no
    /// capital. Drift, a breached kill condition, an unreviewable model and an
    /// expired grant all say the same thing: the reason this strategy was
    /// trusted with money no longer holds. Shadow rather than retired because
    /// these are recoverable — the strategy keeps computing decisions, and if
    /// it re-earns the shadow and pilot gates it can come back. Retirement is
    /// a human's call.
    pub const fn demote_to(&self, from: GateStage) -> GateStage {
        match self {
            Self::PerformanceDecay { .. } => match from {
                GateStage::Scaled => GateStage::Pilot,
                _ => GateStage::Shadow,
            },
            _ => GateStage::Shadow,
        }
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
}

impl Default for DemotionPolicy {
    fn default() -> Self {
        Self {
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
            if let Ok(report) =
                assess_overfitting(
                    std::slice::from_ref(&baseline.returns),
                    std::slice::from_ref(&observation.returns),
                )
                && report.degradation < self.policy.minimum_retained_performance
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
    /// Demotes to the lowest rung any trigger asks for, in one move, so the
    /// ledger records one demotion with every reason rather than a cascade
    /// that is harder to read back. Returns the triggers and the demotion, or
    /// the triggers and `None` when the strategy is already at or below where
    /// they would put it.
    pub fn enforce(
        &self,
        ledger: &mut LifecycleLedger,
        baseline: &PilotBaseline,
        observation: &LiveObservation,
        models: Option<&ModelRegistry>,
        now: Timestamp,
    ) -> Result<(Vec<DemotionTrigger>, Option<Promotion>)> {
        let triggers = self.triggers(baseline, observation, models, now);
        if triggers.is_empty() {
            return Ok((triggers, None));
        }

        let current = ledger.stage_of(&baseline.strategy);
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

        let reason = triggers
            .iter()
            .map(DemotionTrigger::describe)
            .collect::<Vec<_>>()
            .join("; ");
        let raised_by = triggers
            .iter()
            .map(|trigger| trigger.name())
            .collect::<Vec<_>>()
            .join(",");
        let demotion = ledger.demote(&baseline.strategy, target, raised_by, reason, now)?;
        Ok((triggers, Some(demotion)))
    }
}
