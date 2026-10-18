//! Learning from a shadowed pass: the outcome the pass simulated for the
//! plan it would have run, and the attribution of that effect, each
//! labelled simulated so no reader mistakes either for a measurement.
//!
//! # Why a shadow pass learns at all
//!
//! Every shipped binary's adapter refuses (ADR 0114), so no pass executes and
//! no effect is ever measured in the world. Until this module
//! [`crate::engine::learn`] therefore had no caller outside tests, and the
//! observe-effect, attribute and update stages were a library the platform
//! could run and never did. What a shadow pass *does* have is the one thing
//! the learn stages need and the operator cannot supply by hand: a
//! counterfactual. Its structural model evaluated every (model, scenario)
//! branch twice, once with the plan's shifts applied and once without, and
//! `with_action - no_action` in each is a no-action counterfactual by
//! construction.
//!
//! # What is honest about it, and what is not
//!
//! The effect is the model's, not the venue's. So the outcome declares
//! itself [`Observed::Simulated`], its regime key is the branches it was
//! simulated under rather than a market state, its audience is nobody, and
//! the attribution's identification names the model and says nothing was
//! observed. [`crate::memory::ActionOutcomeMemory::recommend_for`] reads none
//! of it; what it feeds is [`crate::autonomy::ActionPolicy::observe`], the
//! drift rule — a policy whose own model puts the effect outside the uplift
//! it was widened on is returned to shadow before any adapter exists — and
//! the journal, where the record of a pass now carries the update it caused.
//!
//! # What is refused rather than guessed
//!
//! A pass that was not shadowed; a pass with no structural model, which
//! simulated nothing; a plan whose steps span two action classes, which
//! names no single policy to update; a plan predicting its effect at a
//! horizon of zero, which no observation could sit at. Each is an error the
//! caller journals as a withheld learning, not an outcome somebody made up.

use crate::attribution::{Effect, EffectAttribution, HorizonObservation, Identification};
use crate::causal::Direction;
use crate::engine::{Outcome, Report};
use crate::memory::{ActionOutcomeDraft, Observed};
use qip_contracts::action::Method;
use qip_core::{Decimal, Error};

/// What a shadowed pass offers the learn stages.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedEvidence {
    /// The one action class the plan's steps belong to: the policy that
    /// learns.
    pub class: Method,
    pub outcome: ActionOutcomeDraft,
    pub attribution: EffectAttribution,
}

/// The statistics/money crossing, stated once: every simulated figure is an
/// `f64`, and the outcome's are `Decimal`. A non-finite figure is refused,
/// not zeroed.
fn decimal(what: &str, value: f64) -> Result<Decimal, Error> {
    Decimal::from_f64(value).ok_or_else(|| {
        Error::numeric(format!(
            "the simulated {what} ({value}) is not a finite decimal"
        ))
    })
}

/// Build the simulated outcome and its attribution from `report`, for pass
/// `pass` pursuing `goal_target`.
pub fn evidence(report: &Report, pass: u64, goal_target: &str) -> Result<SimulatedEvidence, Error> {
    let Outcome::Shadowed(steps) = &report.outcome else {
        return Err(Error::denied(
            "the pass was not shadowed: no plan cleared every gate, so there is no simulated \
             action to learn from",
        ));
    };
    let chosen = report.chosen.as_ref().ok_or_else(|| {
        Error::denied("the pass shadowed a plan but recorded no chosen candidate to learn from")
    })?;
    let simulated = report
        .simulations
        .iter()
        .find(|s| s.label == chosen.label)
        .filter(|s| !s.results.is_empty())
        .ok_or_else(|| {
            Error::denied(format!(
                "no structural model simulated `{}`; a shadow pass with no simulated outcome has \
                 nothing to learn from, and nothing is invented for it",
                chosen.label
            ))
        })?;
    let objective = report.objective.as_ref().ok_or_else(|| {
        Error::denied("the pass simulated branches but recorded no objective to sign them by")
    })?;
    let class = match chosen.methods.iter().collect::<Vec<_>>()[..] {
        [one] => *one,
        [] => {
            return Err(Error::denied(
                "the plan's steps name no registered action class; no policy learns from it",
            ));
        }
        ref many => {
            return Err(Error::denied(format!(
                "the plan's steps span the action classes {many:?}; one policy learns per pass, \
                 and which it would be is not guessed"
            )));
        }
    };
    if chosen.time_to_effect_secs == 0 {
        return Err(Error::denied(
            "the plan predicts its effect at a horizon of zero seconds; no observation sits \
             there, so the simulated one is not filed at it",
        ));
    }
    let sign = match objective.direction {
        Direction::Increase => 1.0,
        Direction::Decrease => -1.0,
    };
    let n = simulated.results.len() as f64;
    let with_action = simulated.results.iter().map(|r| r.with_action).sum::<f64>() / n;
    let no_action = simulated.results.iter().map(|r| r.no_action).sum::<f64>() / n;
    let mean = simulated.results.iter().map(|r| r.effect).sum::<f64>() / n;
    // Sample standard deviation across branches, as the estimators use; one
    // branch disagrees with nothing.
    let std_dev = if simulated.results.len() > 1 {
        (simulated
            .results
            .iter()
            .map(|r| (r.effect - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0))
            .sqrt()
    } else {
        0.0
    };
    let branches: Vec<String> = simulated
        .results
        .iter()
        .map(|r| format!("{}/{}", r.model, r.scenario))
        .collect();
    let mut models: Vec<&str> = simulated.results.iter().map(|r| r.model.as_str()).collect();
    models.dedup();
    let mut scenarios: Vec<&str> = simulated
        .results
        .iter()
        .map(|r| r.scenario.as_str())
        .collect();
    scenarios.sort_unstable();
    scenarios.dedup();
    let mut chain: Vec<String> = Vec::new();
    for step in steps {
        if !chain.contains(&step.variable) {
            chain.push(step.variable.clone());
        }
    }
    chain.push(objective.variable.clone());
    let direction = match objective.direction {
        Direction::Increase => "increase",
        Direction::Decrease => "decrease",
    };
    let pulled = steps
        .iter()
        .map(|s| format!("{}@{}", s.tool, s.variable))
        .collect::<Vec<_>>()
        .join("+");

    let attribution = EffectAttribution {
        goal_id: format!("agency-pass-{pass}:goal"),
        intervention_id: format!("agency-pass-{pass}:plan"),
        action_id: format!("agency-pass-{pass}:shadow"),
        action_class: class,
        identification: Identification::Observational {
            strategy: format!(
                "simulated: structural model(s) {} under scenario(s) {}; the no-action branch \
                 of each is the counterfactual, and nothing was observed",
                models.join(", "),
                scenarios.join(", ")
            ),
        },
        observations: vec![HorizonObservation {
            horizon_ms: chosen.time_to_effect_secs.saturating_mul(1000),
            value: mean,
        }],
        counterfactual_method: "the no-action branch of each simulated (model, scenario), not a \
                                measured control"
            .to_string(),
        effect: Effect { mean, std_dev },
        causal_chain: chain,
        confounders: vec![
            "none: the effect is computed inside the declared structural model, which admits no \
             confounder it does not declare"
                .to_string(),
        ],
        competing_explanations: vec![
            "the structural model misdescribes the venue; a simulated effect is evidence about \
             the model and not about the world"
                .to_string(),
        ],
        side_effects: vec!["none: a simulation observes no side effect".to_string()],
        // Capped by the weakest lever's evidence: the model is identified by
        // its declared graph, and the graph is believed exactly as well as
        // the levers on it are.
        identifiability: chosen.identifiability.to_f64(),
        resulting_updates: vec![format!(
            "the {class:?} policy is held to its predicted uplift, and the outcome is kept in \
             the action outcome memory as simulated"
        )],
    };
    let outcome = ActionOutcomeDraft {
        intervention_context: Some(format!("shadow pass {pass}: {goal_target}")),
        environment_state: Some(format!("simulated: {}", branches.join(", "))),
        audience: Some("simulated: nobody reached".to_string()),
        delay_secs: Some(chosen.time_to_effect_secs),
        action_sequence: Some(steps.iter().map(|s| s.tool.clone()).collect()),
        causal_hypothesis: Some(format!(
            "{pulled} moves `{}` ({direction}); figures are signed so that positive is toward \
             the goal, and are the model's, not the venue's",
            objective.variable
        )),
        predicted_effect: Some(chosen.expected_causal_effect),
        actual_observations: Some(decimal("objective with the action", sign * with_action)?),
        attribution_confidence: Some(chosen.identifiability),
        side_effects: Some(Vec::new()),
        no_action_counterfactual: Some(decimal("objective without the action", sign * no_action)?),
        observed: Some(Observed::Simulated),
    };
    Ok(SimulatedEvidence {
        class,
        outcome,
        attribution,
    })
}
