//! AGENCY-001 / 005 / 016 / 042: intervention plans that stay inside what
//! their goal declared, in four structures, and candidate sets that always
//! carry the option of doing nothing.
//!
//! The failure prevented is a planner that treats a goal's utility as
//! licence: every step's cost comes from the affordance graph (a plan cannot
//! understate it), every step's tool must be one the acting identity holds
//! the authority for (wanting an outcome grants no permission), and a goal
//! only reachable by exceeding budget or risk envelope yields an error, not a
//! smaller plan quietly trimmed to fit.
//!
//! AGENCY-043 / AGENCY-047: [`select`] is the only ranker, and it ranks
//! [`InterventionPlan`]s, which exist only once every hard constraint held.
//! The learned effect estimate arrives beside the raw plan and is read after
//! the refusal, so no value of it, however large, is ever weighed against a
//! constraint. A penalty term would lose to a big enough estimate; a value
//! that cannot be constructed does not.

use crate::affordance::AffordanceGraph;
use crate::comparison::Comparison;
use qip_core::{Decimal, Error};
use serde::Deserialize;
use std::collections::BTreeSet;

use crate::goal::GoalSpec;

/// One use of one tool on one variable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Step {
    pub tool: String,
    pub variable: String,
    /// Exposure this step adds, in the goal's risk-envelope unit.
    pub exposure: Decimal,
}

/// A plan's structure (AGENCY-016).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanNode {
    Step(Step),
    Sequence(Vec<PlanNode>),
    Parallel(Vec<PlanNode>),
    /// Runs `then` only when `condition` is a recorded fact.
    Conditional {
        condition: String,
        then: Box<PlanNode>,
    },
    /// Runs `primary`, unless `trigger` has been observed, in which case it
    /// switches to `fallback`.
    Adaptive {
        primary: Box<PlanNode>,
        trigger: String,
        fallback: Box<PlanNode>,
    },
}

impl PlanNode {
    /// Every step on every branch: the worst case a bound must hold against.
    pub(crate) fn all_steps<'a>(&'a self, out: &mut Vec<&'a Step>) {
        match self {
            Self::Step(s) => out.push(s),
            Self::Sequence(n) | Self::Parallel(n) => n.iter().for_each(|c| c.all_steps(out)),
            Self::Conditional { then, .. } => then.all_steps(out),
            Self::Adaptive {
                primary, fallback, ..
            } => {
                primary.all_steps(out);
                fallback.all_steps(out);
            }
        }
    }

    /// The steps that would run given the facts recorded so far, in order.
    pub fn simulate<'a>(&'a self, facts: &BTreeSet<String>) -> Vec<&'a Step> {
        let mut out = Vec::new();
        self.run(facts, &mut out);
        out
    }

    fn run<'a>(&'a self, facts: &BTreeSet<String>, out: &mut Vec<&'a Step>) {
        match self {
            Self::Step(s) => out.push(s),
            Self::Sequence(n) | Self::Parallel(n) => n.iter().for_each(|c| c.run(facts, out)),
            Self::Conditional { condition, then } => {
                if facts.contains(condition) {
                    then.run(facts, out);
                }
            }
            Self::Adaptive {
                primary,
                trigger,
                fallback,
            } => {
                if facts.contains(trigger) {
                    fallback.run(facts, out);
                } else {
                    primary.run(facts, out);
                }
            }
        }
    }
}

/// The identity a plan acts as, and the authorities it independently holds.
#[derive(Debug, Clone, Deserialize)]
pub struct ActingIdentity {
    pub name: String,
    pub authorities: BTreeSet<String>,
}

/// A plan proven, at construction, to sit inside its goal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct InterventionPlan {
    pub root: PlanNode,
    pub total_cost: Decimal,
    pub total_exposure: Decimal,
}

impl InterventionPlan {
    /// Refuse a plan that exceeds the goal's budget or envelope, acts as the
    /// wrong identity, uses a lever whose authority that identity lacks, or
    /// uses a method or side effect the goal prohibits. Bounds are checked
    /// against the sum over **every** branch, since a conditional may fire.
    pub fn plan(
        goal: &GoalSpec,
        graph: &AffordanceGraph,
        who: &ActingIdentity,
        root: PlanNode,
    ) -> Result<Self, Error> {
        if who.name != goal.acting_identity {
            return Err(Error::denied(format!(
                "goal is bound to identity `{}`, not `{}`",
                goal.acting_identity, who.name
            )));
        }
        let mut steps = Vec::new();
        root.all_steps(&mut steps);
        let mut cost = Decimal::ZERO;
        let mut exposure = Decimal::ZERO;
        for step in steps {
            let edge = graph
                .edges(&step.variable)
                .iter()
                .find(|e| e.tool == step.tool)
                .ok_or_else(|| {
                    Error::denied(format!(
                        "tool `{}` has no edge on variable `{}` in the affordance graph",
                        step.tool, step.variable
                    ))
                })?;
            if !who.authorities.contains(&edge.authority) {
                return Err(Error::denied(format!(
                    "identity `{}` does not hold authority `{}` that `{}` requires",
                    who.name, edge.authority, step.tool
                )));
            }
            // AGENCY-047: infeasible by policy. Nothing about the plan's
            // expected effect is in scope here, so nothing can outweigh it.
            if edge.method.is_deceptive() {
                return Err(Error::denied(format!(
                    "tool `{}` works by {:?}, a deceptive or manipulative method; a plan that \
                     depends on it is infeasible, remove the step",
                    step.tool, edge.method
                )));
            }
            if goal.prohibited_methods.contains(&step.tool) {
                return Err(Error::denied(format!(
                    "method `{}` is prohibited by the goal",
                    step.tool
                )));
            }
            if let Some(bad) = edge
                .side_effects
                .iter()
                .find(|e| goal.prohibited_side_effects.contains(e))
            {
                return Err(Error::denied(format!(
                    "tool `{}` carries prohibited side effect `{bad}`",
                    step.tool
                )));
            }
            if step.exposure.is_negative() {
                return Err(Error::invalid("step exposure must not be negative"));
            }
            cost = cost
                .checked_add(edge.cost)
                .ok_or_else(|| Error::numeric("plan cost overflowed"))?;
            exposure = exposure
                .checked_add(step.exposure)
                .ok_or_else(|| Error::numeric("plan exposure overflowed"))?;
        }
        if cost > goal.budget {
            return Err(Error::guard(format!(
                "plan cost {cost} exceeds budget {}; no executable plan, raise the budget or narrow the goal",
                goal.budget
            )));
        }
        if exposure > goal.risk_envelope {
            return Err(Error::guard(format!(
                "plan exposure {exposure} exceeds risk envelope {}; no executable plan",
                goal.risk_envelope
            )));
        }
        Ok(Self {
            root,
            total_cost: cost,
            total_exposure: exposure,
        })
    }
}

/// One option under consideration.
#[derive(Debug, Clone, PartialEq)]
pub enum Candidate {
    /// Do nothing: the counterfactual every other option is judged against.
    NoAction,
    Plan(InterventionPlan),
}

/// A candidate set, valid only with exactly one no-action baseline and at
/// least one alternative to it (AGENCY-005).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CandidateSet {
    pub candidates: Vec<Candidate>,
}

impl CandidateSet {
    pub fn new(candidates: Vec<Candidate>) -> Result<Self, Error> {
        let baselines = candidates
            .iter()
            .filter(|c| matches!(c, Candidate::NoAction))
            .count();
        if baselines != 1 {
            return Err(Error::invalid(format!(
                "a candidate set needs exactly one no-action baseline, found {baselines}"
            )));
        }
        if candidates.len() < 2 {
            return Err(Error::invalid(
                "a candidate set needs at least one action to compare against the baseline",
            ));
        }
        Ok(Self { candidates })
    }
}

/// A plan as proposed, beside the learned estimate of what it would achieve.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub root: PlanNode,
    pub comparison: Comparison,
}

/// What [`select`] chose, and every proposal it refused with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub chosen: Candidate,
    /// Index into the proposals, and the refusal.
    pub refused: Vec<(usize, Error)>,
}

/// Rank the proposals that survive every hard constraint against the
/// no-action baseline, and choose the largest expected causal effect.
///
/// A proposal is refused, whatever its estimate, when its plan breaks a bound
/// [`InterventionPlan::plan`] holds, uses a variable that is not one of the
/// goal's `levers`, or is not legally eligible. Doing nothing is worth zero
/// by definition, so an effect that is not strictly positive loses to it.
pub fn select(
    goal: &GoalSpec,
    graph: &AffordanceGraph,
    who: &ActingIdentity,
    levers: &[String],
    proposals: Vec<Proposal>,
) -> Result<Selection, Error> {
    let mut refused = Vec::new();
    let mut admitted: Vec<(Decimal, InterventionPlan)> = Vec::new();
    for (index, proposal) in proposals.into_iter().enumerate() {
        let mut steps = Vec::new();
        proposal.root.all_steps(&mut steps);
        let stray = steps.iter().find(|s| !levers.contains(&s.variable));
        let verdict = if let Some(step) = stray {
            Err(Error::denied(format!(
                "`{}` is not a lever for this goal's targets",
                step.variable
            )))
        } else if !proposal.comparison.legally_eligible {
            Err(Error::denied("the proposal is not legally eligible"))
        } else {
            InterventionPlan::plan(goal, graph, who, proposal.root)
        };
        match verdict {
            Ok(plan) => admitted.push((proposal.comparison.expected_causal_effect, plan)),
            Err(refusal) => refused.push((index, refusal)),
        }
    }
    let mut best: Option<&(Decimal, InterventionPlan)> = None;
    for entry in &admitted {
        if entry.0 > best.map_or(Decimal::ZERO, |(effect, _)| *effect) {
            best = Some(entry);
        }
    }
    let chosen = best.map_or(Candidate::NoAction, |(_, plan)| {
        Candidate::Plan(plan.clone())
    });
    // Routed through `CandidateSet` so the baseline is present by the same
    // rule everywhere, not by this function remembering to add it.
    if !admitted.is_empty() {
        let mut all = vec![Candidate::NoAction];
        all.extend(admitted.into_iter().map(|(_, plan)| Candidate::Plan(plan)));
        CandidateSet::new(all)?;
    }
    Ok(Selection { chosen, refused })
}
