//! AGENCY-034 / 045 / 055: the agency loop in its fixed order, the gate that
//! abstains when a lever's effect is not identified, and the one function
//! that may call an adapter.
//!
//! The failure prevented is an action that ran before anybody simulated it
//! or checked it. [`StageLog`] accepts a stage only when it is the next one
//! and nothing before it failed, and [`execute`], the only caller of an
//! [`Adapter`] in this crate, asks the log for `Act` before it does anything.
//! So an adapter call with simulation or the gate missing or failed is not a
//! path that exists, and neither is an attribution recorded before its
//! action.
//!
//! No adapter is implemented here or anywhere in the workspace. With no
//! policy, or a policy at shadow, [`execute`] returns the steps that would
//! have run and calls nothing.
//!
//! In front of every adapter call [`execute`] makes stands an
//! [`ActionGuard`]: the conduct gate, the one tool registry, the approval
//! chain and the justification check (see [`crate::guard`]). With authority
//! to act and no guard, `execute` refuses; it never sends unguarded. After
//! an action, [`learn`] runs observe-effect, attribute and update in order,
//! keeping the outcome in [`ActionOutcomeMemory`] and holding the policy to
//! what it predicted. [`widen`] is the one path by which a policy gains
//! authority, and it returns a record of who widened it on what evidence.
//!
//! A shadowed pass learns too, and says so. [`execute`] marks `Act` as
//! *shadowed* rather than passed, so [`learn`] admits an outcome after it
//! only when the outcome declares itself [`Observed::Simulated`], and admits
//! a [`Observed::Measured`] outcome only after an `Act` that passed. Either
//! way round is refused: a simulated effect filed as measured is a
//! prediction wearing an observation's clothes, and a measured effect after a
//! shadowed act is a measurement of nothing. [`learn_shadowed`] is the seam a
//! composition root calls after a shadowed pass, with the evidence
//! [`crate::simulated`] computes from the pass's own branches.

use crate::affordance::{AffordanceGraph, ToolEdgeDraft};
use crate::attribution::EffectAttribution;
use crate::autonomy::{ActionPolicy, Authority, WideningEvidence};
use crate::causal::Objective;
use crate::comparison::ComparisonDraft;
use crate::generate::{ScenarioSource, Simulated, Structural, StructuralDecl};
use crate::goal::GoalSpec;
use crate::guard::{ActionGuard, ApprovalChain, GuardDecl, PassDecl, Voice};
use crate::memory::{
    ActionOutcome, ActionOutcomeDraft, ActionOutcomeMemory, FailureEpisode, Observed,
};
use crate::plan::{
    ActingIdentity, Candidate, InterventionPlan, PlanNode, Proposal, Selection, Step, select,
};
use qip_compliance::eligibility::{EligibilityRegister, Subject};
use qip_contracts::action::{Method, ToolAudit};
use qip_contracts::action_intent::ActionIntent;
use qip_core::{Decimal, Error, Timestamp};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// The eleven stages of §24's loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Observe,
    Predict,
    Specify,
    IdentifyLevers,
    Generate,
    Simulate,
    Gate,
    Act,
    ObserveEffect,
    Attribute,
    Update,
}

/// The declared order. A stage is admitted only at its own position.
pub const ORDER: [Stage; 11] = [
    Stage::Observe,
    Stage::Predict,
    Stage::Specify,
    Stage::IdentifyLevers,
    Stage::Generate,
    Stage::Simulate,
    Stage::Gate,
    Stage::Act,
    Stage::ObserveEffect,
    Stage::Attribute,
    Stage::Update,
];

/// The stages one pass of the loop has been through.
///
/// `Act` has a third state beside passed and failed: *shadowed*, which
/// [`execute`] records when it called nothing. It is kept off `passed`
/// because a report listing `Act` among the stages that passed would say an
/// action happened, and it is kept off `failed` because nothing refused the
/// pass. The stages after it may still run, but only on an outcome that
/// declares itself simulated ([`learn`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageLog {
    passed: Vec<Stage>,
    failed: Option<Stage>,
    shadowed_act: bool,
}

impl StageLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The position the next stage must hold: every stage that passed, plus
    /// the shadowed `Act` that holds its slot without having passed.
    fn position(&self) -> usize {
        self.passed.len() + usize::from(self.shadowed_act)
    }

    /// Refuse `stage` unless it is the next in [`ORDER`] and nothing failed.
    fn expect(&self, stage: Stage) -> Result<(), Error> {
        if let Some(failed) = self.failed {
            return Err(Error::guard(format!(
                "{failed:?} failed, so {stage:?} may not run; start a new pass"
            )));
        }
        match ORDER.get(self.position()) {
            Some(next) if *next == stage => Ok(()),
            Some(next) => Err(Error::guard(format!(
                "{stage:?} is out of order; {next:?} must pass first"
            ))),
            None => Err(Error::guard("the pass is complete; start a new one")),
        }
    }

    pub fn pass(&mut self, stage: Stage) -> Result<(), Error> {
        self.expect(stage)?;
        self.passed.push(stage);
        Ok(())
    }

    /// Record that `Act` ran in shadow: every check cleared and nothing was
    /// sent. Admitted only where `Act` itself would be.
    fn shadow_act(&mut self) -> Result<(), Error> {
        self.expect(Stage::Act)?;
        self.shadowed_act = true;
        Ok(())
    }

    /// Whether `Act` was shadowed rather than passed.
    pub fn act_shadowed(&self) -> bool {
        self.shadowed_act
    }

    /// Record that `stage` ran and failed. Every later stage is then refused.
    pub fn fail(&mut self, stage: Stage) -> Result<(), Error> {
        self.expect(stage)?;
        self.failed = Some(stage);
        Ok(())
    }

    pub fn passed(&self) -> &[Stage] {
        &self.passed
    }

    pub fn failed(&self) -> Option<Stage> {
        self.failed
    }
}

/// What is known about how well a lever's effect on the target is identified.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LeverEvidence {
    /// In `[0, 1]`: 1 is an effect established by experiment, 0 a bare
    /// correlation.
    pub identifiability: Decimal,
    /// Whether treatment may be assigned on this lever at all.
    pub experimentable: bool,
}

/// The gate's answer for a plan that held every hard constraint.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Execute(InterventionPlan),
    /// These levers are weakly identified and may be experimented on.
    ProposeExperiment(Vec<String>),
    /// Nothing has been measured about these levers.
    GatherEvidence(Vec<String>),
    /// These levers are weakly identified and no experiment is permitted.
    Abstain(Vec<String>),
}

/// AGENCY-045: act only where every lever the plan pulls is identified at
/// least as well as the goal demands; otherwise say what would have to
/// happen first, and never return the plan.
///
/// The floor is the goal's own declaration: a goal that tolerates
/// uncertainty `u` needs identifiability of at least `1 - u`. A lever with no
/// evidence is not "identified at zero"; it is unmeasured, and no tolerance
/// admits it.
pub fn decide(
    goal: &GoalSpec,
    plan: InterventionPlan,
    evidence: &BTreeMap<String, LeverEvidence>,
) -> Result<Decision, Error> {
    let floor = Decimal::ONE
        .checked_sub(goal.acceptable_uncertainty)
        .ok_or_else(|| Error::numeric("identifiability floor overflowed"))?;
    let mut steps = Vec::new();
    plan.root.all_steps(&mut steps);
    let levers: BTreeSet<&str> = steps.iter().map(|s| s.variable.as_str()).collect();
    let (mut unmeasured, mut experiment, mut dead_end) = (Vec::new(), Vec::new(), Vec::new());
    for lever in levers {
        match evidence.get(lever) {
            None => unmeasured.push(lever.to_string()),
            Some(known) => {
                if known.identifiability < Decimal::ZERO || known.identifiability > Decimal::ONE {
                    return Err(Error::invalid(format!(
                        "identifiability of `{lever}` must lie in [0, 1]; it is refused, not clamped"
                    )));
                }
                if known.identifiability >= floor {
                    continue;
                }
                if known.experimentable {
                    experiment.push(lever.to_string());
                } else {
                    dead_end.push(lever.to_string());
                }
            }
        }
    }
    // The most final answer first: evidence on one lever does not rescue a
    // plan that also needs a lever nobody may experiment on.
    Ok(if !dead_end.is_empty() {
        Decision::Abstain(dead_end)
    } else if !unmeasured.is_empty() {
        Decision::GatherEvidence(unmeasured)
    } else if !experiment.is_empty() {
        Decision::ProposeExperiment(experiment)
    } else {
        Decision::Execute(plan)
    })
}

/// Something that performs one step in the world. Nothing in this workspace
/// implements it outside a test; a composition root that wants one has the
/// paper-trading boundary and an ADR to answer to first.
pub trait Adapter {
    fn call(&mut self, step: &Step) -> Result<(), Error>;
}

/// What [`execute`] did with a gated plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Execution {
    /// The steps that would have run. No adapter was called.
    Shadow(Vec<Step>),
    Executed(Vec<Step>),
    /// The guard refused before any call: the conduct gate, the registry,
    /// the approvals or the justification. The reason.
    Withheld(String),
    /// Every check cleared, the execution record was written and the
    /// registry charged, and then the registry or the adapter refused the
    /// call. Kept as a value rather than an error so the approvals and the
    /// execution record of an attempt survive to be journaled: under ADR
    /// 0114 every adapter in a shipped binary refuses, and an attempt that
    /// left no record would be an attempt nobody could audit.
    Refused {
        reason: String,
        attempted: Vec<Step>,
    },
}

/// What a withheld plan leaves behind (R12-RES-7110): the plan itself, the
/// guard's reasons, the steps that could not clear on their own, and the
/// option the platform would rather offer. A refusal that kept none of this
/// would leave the next pass to propose the same plan again, and the desk
/// nothing to read but a sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockedPlan {
    /// Every step of the plan as the pass chose it.
    pub steps: Vec<Step>,
    /// The guard's refusal, verbatim: it names each check that failed.
    pub reason: String,
    /// The steps the guard refuses even when each is asked alone.
    pub offending: Vec<Step>,
    pub alternative: Alternative,
}

/// The truthful option sought after a plan was withheld. It is a proposal:
/// it carries no approval and is never sent by the pass that found it.
#[derive(Debug, Clone, PartialEq)]
pub enum Alternative {
    /// The plan without its offending steps, which the guard cleared when
    /// asked read-only.
    Reduced(Vec<Step>),
    /// Nothing that remains clears, and the reason; the option left is the
    /// no-action baseline every candidate set carries.
    NoAction(String),
}

/// Ask the guard, read-only, what of a withheld plan can be said truthfully.
fn blocked_plan(
    guard: &ActionGuard,
    goal: &GoalSpec,
    priced: &[(Step, Decimal, Method)],
    reason: &str,
) -> BlockedPlan {
    let steps: Vec<Step> = priced.iter().map(|(step, _, _)| step.clone()).collect();
    let (clear, refused): (Vec<_>, Vec<_>) = priced
        .iter()
        .cloned()
        .partition(|one| guard.preflight(goal, std::slice::from_ref(one)).is_ok());
    let offending: Vec<Step> = refused.into_iter().map(|(step, _, _)| step).collect();
    let alternative = if offending.is_empty() {
        Alternative::NoAction(
            "every step clears alone, so the refusal is of the plan as a whole; no subset is \
             proposed"
                .to_string(),
        )
    } else if clear.is_empty() {
        Alternative::NoAction("no step of the plan clears alone".to_string())
    } else if guard.preflight(goal, &clear).is_ok() {
        Alternative::Reduced(crate::plan::without_steps(&steps, &offending))
    } else {
        Alternative::NoAction("the steps that clear alone do not clear together".to_string())
    };
    BlockedPlan {
        steps,
        reason: reason.to_string(),
        offending,
        alternative,
    }
}

/// What [`execute`] returns: what it did, and the approval records it made.
#[derive(Debug, Clone, PartialEq)]
pub struct Acted {
    pub execution: Execution,
    /// The plan and its alternative, when the guard withheld it.
    pub blocked: Option<BlockedPlan>,
    /// The goal and plan approvals, and the execution record once anything
    /// was sent. `None` when no guard was supplied, or it refused.
    pub approvals: Option<ApprovalChain>,
    /// The voices the guard admitted this pass: one per step sent, so the
    /// journal can carry what was said under which name (R12-AGENCY-1064).
    pub released: Vec<Voice>,
    /// One §27 [`ActionIntent`] per step the guard cleared, in shadow as
    /// when acting (R12-CONTRACT-7082); empty when no guard ran or it
    /// refused.
    pub intents: Vec<ActionIntent>,
}

/// A plan that has cleared the gate, and the context it is executed in.
#[derive(Debug, Clone, Copy)]
pub struct Gated<'a> {
    pub goal: &'a GoalSpec,
    pub graph: &'a AffordanceGraph,
    pub plan: &'a InterventionPlan,
    pub facts: &'a BTreeSet<String>,
    /// The chosen plan's simulation, one result per (model, scenario)
    /// branch, when a structural model was declared. Authority to act is
    /// used only over a plan every branch agrees helps (R12-MODEL-4014).
    pub simulated: Option<&'a Simulated>,
}

/// The only caller of an [`Adapter`].
///
/// Refuses unless `log` shows simulation and the gate passed. With no policy,
/// or one at shadow, it calls nothing and records `Act` as shadowed rather
/// than passed, so nothing can later be attributed to an action that never
/// happened: [`learn`] admits only an outcome that says it is simulated. At
/// narrow-reversible every step is checked before the first is sent: one of
/// another class, or one whose tool is not declared reversible, refuses the
/// whole plan with no call made.
///
/// Then the guard. With authority to act and no guard, refused: an adapter
/// call nobody conduct-checked is not a path that exists. With a guard, every
/// check in [`crate::guard`] runs over the whole plan before the first call,
/// and each call is admitted, audited, by the registry immediately before it
/// is made. In shadow with a guard the same checks run read-only, so a shadow
/// report never says a step "cleared every gate" that the guard would stop.
///
/// A call the registry or the adapter refuses ends the act stage as
/// [`Execution::Refused`], carrying the approvals and the execution record
/// rather than discarding them in an `Err`: under ADR 0114 every shipped
/// adapter refuses, and the attempt is exactly what must be journaled.
pub fn execute(
    log: &mut StageLog,
    policy: Option<&ActionPolicy>,
    gated: Gated<'_>,
    guard: Option<&mut ActionGuard>,
    adapter: &mut dyn Adapter,
) -> Result<Acted, Error> {
    log.expect(Stage::Act)?;
    let steps: Vec<Step> = gated
        .plan
        .root
        .simulate(gated.facts)
        .into_iter()
        .cloned()
        .collect();
    let acting = match policy {
        Some(policy) if policy.authority() == Authority::NarrowReversible => Some(policy),
        _ => None,
    };
    if acting.is_some()
        && let Err(refusal) = counterfactually_supported(gated.simulated)
    {
        log.fail(Stage::Act)?;
        return Ok(Acted {
            execution: Execution::Withheld(refusal.message().to_string()),
            blocked: None,
            approvals: None,
            released: Vec::new(),
            intents: Vec::new(),
        });
    }
    let mut priced = Vec::with_capacity(steps.len());
    for step in &steps {
        let edge = gated
            .graph
            .edges(&step.variable)
            .iter()
            .find(|e| e.tool == step.tool)
            .ok_or_else(|| {
                Error::denied(format!(
                    "tool `{}` has no edge on `{}` in this graph",
                    step.tool, step.variable
                ))
            })?;
        if let Some(policy) = acting {
            if edge.method != policy.class() {
                return Err(Error::denied(format!(
                    "tool `{}` is a {:?} action; this policy holds authority for {:?} only",
                    step.tool,
                    edge.method,
                    policy.class()
                )));
            }
            if !edge.reversible {
                return Err(Error::denied(format!(
                    "tool `{}` is irreversible; narrow-reversible authority does not cover it",
                    step.tool
                )));
            }
        }
        priced.push((step.clone(), edge.cost, edge.method));
    }

    let Some(guard) = guard else {
        if acting.is_some() {
            return Err(Error::denied(
                "no action guard is configured: the conduct gate, tool registry and approvals \
                 stand in front of every adapter call, and nothing is sent without them",
            ));
        }
        log.shadow_act()?;
        return Ok(Acted {
            execution: Execution::Shadow(steps),
            blocked: None,
            approvals: None,
            released: Vec::new(),
            intents: Vec::new(),
        });
    };
    let (mut chain, cleared) = match guard.preflight(gated.goal, &priced) {
        Ok(passed) => passed,
        Err(refusal) => {
            log.fail(Stage::Act)?;
            let blocked = blocked_plan(guard, gated.goal, &priced, refusal.message());
            return Ok(Acted {
                execution: Execution::Withheld(refusal.message().to_string()),
                blocked: Some(blocked),
                approvals: None,
                released: Vec::new(),
                intents: Vec::new(),
            });
        }
    };
    let intents: Vec<ActionIntent> = cleared.iter().map(|c| c.intent.clone()).collect();
    if acting.is_none() {
        log.shadow_act()?;
        return Ok(Acted {
            execution: Execution::Shadow(steps),
            blocked: None,
            approvals: Some(chain),
            released: Vec::new(),
            intents,
        });
    }

    guard.execution(&mut chain, &gated.goal.acting_identity, &steps)?;
    let voices_from = guard.voices().count();
    for step in &cleared {
        let sent = guard
            .authorise(step)
            .and_then(|()| adapter.call(&step.step));
        if let Err(refusal) = sent {
            log.fail(Stage::Act)?;
            return Ok(Acted {
                blocked: None,
                execution: Execution::Refused {
                    reason: refusal.message().to_string(),
                    attempted: steps,
                },
                approvals: Some(chain),
                released: guard.voices().skip(voices_from).cloned().collect(),
                intents,
            });
        }
    }
    log.pass(Stage::Act)?;
    Ok(Acted {
        blocked: None,
        execution: Execution::Executed(steps),
        approvals: Some(chain),
        released: guard.voices().skip(voices_from).cloned().collect(),
        intents,
    })
}

/// R12-MODEL-4014 / R12-COVERAGE-7186: authority to act is used only over a
/// plan simulated against counterfactual branches — every (model,
/// scenario) the structural models declare, each comparing the world with
/// the action to the same world without it — and only when every branch
/// says the action moves the objective toward the goal. A plan nobody
/// simulated is acted on by nothing; a plan the competing models disagree
/// on is weakly identified, and a weakly identified effect is a reason to
/// experiment, not to act. Shadow is not gated here: it sends nothing.
fn counterfactually_supported(simulated: Option<&Simulated>) -> Result<(), Error> {
    let results = simulated.map_or(&[][..], |s| s.results.as_slice());
    if results.is_empty() {
        return Err(Error::denied(
            "authority to act is used only over a plan simulated against counterfactual \
             branches, and this plan was simulated against none; declare a structural model",
        ));
    }
    let against: Vec<String> = results
        .iter()
        .filter(|r| !(r.effect.is_finite() && r.effect > 0.0))
        .map(|r| format!("{}/{} ({})", r.model, r.scenario, r.effect))
        .collect();
    if !against.is_empty() {
        return Err(Error::denied(format!(
            "the plan is weakly identified: {} of {} counterfactual branches do not move the \
             objective toward the goal ({}); competing models that disagree are a reason to \
             experiment, not to act",
            against.len(),
            results.len(),
            against.join(", ")
        )));
    }
    Ok(())
}

/// R12-AGENCY-6103: a policy gains authority by this path only, and the
/// widening leaves a record of the evidence and the approver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widening {
    pub class: Method,
    pub goal_id: String,
    pub intervention_id: String,
    pub action_id: String,
    pub approver: String,
}

/// Widen `policy` on `evidence` and `approver` together, through
/// [`ActionPolicy::widen`], which validates the attribution, refuses a
/// correlation however strong, and refuses an approver without the authority.
pub fn widen(
    policy: &mut ActionPolicy,
    evidence: &EffectAttribution,
    approver: &ActingIdentity,
) -> Result<Widening, Error> {
    widen_to(
        policy,
        Authority::NarrowReversible,
        &WideningEvidence::attribution(evidence),
        &[approver],
    )
}

/// Widen `policy` to `target` through [`ActionPolicy::widen_to`], which reads
/// the level's [`crate::autonomy::SCHEDULE`] row (R12-EXPAND-6071, ADR 0141).
/// A caller that may name a level is a caller whose lesser evidence is
/// refused by that level's row, rather than one only ever asking for the
/// first step. The record names the first attribution offered and the first
/// approver.
pub fn widen_to(
    policy: &mut ActionPolicy,
    target: Authority,
    evidence: &WideningEvidence<'_>,
    approvers: &[&ActingIdentity],
) -> Result<Widening, Error> {
    let (Some(first), Some(approver)) = (evidence.attributions.first(), approvers.first()) else {
        return Err(Error::denied(
            "a widening names at least one identified attribution and one approver",
        ));
    };
    policy.widen_to(target, evidence, approvers)?;
    Ok(Widening {
        class: policy.class(),
        goal_id: first.goal_id.clone(),
        intervention_id: first.intervention_id.clone(),
        action_id: first.action_id.clone(),
        approver: approver.name.clone(),
    })
}

/// R12-AGENCY-6107 / 6108 / 054: after an action, observe its effect,
/// attribute it and update, in that order, and only after `Act` passed or
/// was shadowed.
///
/// Refused, with the stage that refused it recorded as failed:
/// * an outcome whose action sequence is not the tools that ran (or, after a
///   shadowed act, would have run) — an effect credited to something that
///   never happened (observe-effect);
/// * an outcome whose [`Observed`] does not match how `Act` ended: a
///   measured effect after a shadowed act measured nothing, and a simulated
///   effect after an executed act is a prediction standing in for the
///   observation that exists (observe-effect);
/// * an attribution that does not validate, measures another class of action,
///   or is less sure of itself than the outcome claims to be: an outcome
///   recorded at confidence 0.9 on an attribution identified at 0.2 is a
///   correlation written up as a cause (attribute, R12-AGENCY-6103).
///
/// Then the outcome is remembered and the policy is held to its prediction
/// (`observed - counterfactual` against the predicted interval, and the
/// side effects against those predicted), which may drop it to shadow.
/// Returns the authority the policy now holds.
pub fn learn(
    log: &mut StageLog,
    steps: &[Step],
    policy: &mut ActionPolicy,
    memory: &mut ActionOutcomeMemory,
    outcome: ActionOutcomeDraft,
    attribution: &EffectAttribution,
) -> Result<Authority, Error> {
    learn_outcome(log, steps, policy, memory, outcome, attribution)
        .map(|(authority, _, _)| authority)
}

/// [`learn`], also returning the outcome as kept, so a caller can journal
/// the record the memory holds rather than the draft it was given, and the
/// failure episode the memory filed when the outcome drifted.
fn learn_outcome(
    log: &mut StageLog,
    steps: &[Step],
    policy: &mut ActionPolicy,
    memory: &mut ActionOutcomeMemory,
    outcome: ActionOutcomeDraft,
    attribution: &EffectAttribution,
) -> Result<(Authority, ActionOutcome, Option<FailureEpisode>), Error> {
    log.expect(Stage::ObserveEffect)?;
    let outcome = match outcome.build() {
        Ok(outcome) => outcome,
        Err(refusal) => {
            log.fail(Stage::ObserveEffect)?;
            return Err(refusal);
        }
    };
    let ran: Vec<&str> = steps.iter().map(|s| s.tool.as_str()).collect();
    let claimed: Vec<&str> = outcome.action_sequence.iter().map(String::as_str).collect();
    if ran != claimed {
        log.fail(Stage::ObserveEffect)?;
        return Err(Error::denied(format!(
            "the outcome credits {claimed:?}, but {ran:?} ran; an effect is recorded against \
             the action that happened"
        )));
    }
    match (log.act_shadowed(), outcome.observed) {
        (true, Observed::Measured) => {
            log.fail(Stage::ObserveEffect)?;
            return Err(Error::denied(
                "the act was shadowed and nothing ran, so no effect was measured in the world; \
                 learn from the simulated outcome and declare it simulated",
            ));
        }
        (false, Observed::Simulated) => {
            log.fail(Stage::ObserveEffect)?;
            return Err(Error::denied(
                "the act ran, so its effect is the one observed; a simulated outcome is a \
                 prediction, and learning from it would overwrite the measurement that exists",
            ));
        }
        (true, Observed::Simulated) | (false, Observed::Measured) => {}
    }
    log.pass(Stage::ObserveEffect)?;

    if let Err(refusal) = attribute(policy, &outcome.attribution_confidence, attribution) {
        log.fail(Stage::Attribute)?;
        return Err(refusal);
    }
    log.pass(Stage::Attribute)?;

    let effect = outcome
        .actual_observations
        .checked_sub(outcome.no_action_counterfactual)
        .ok_or_else(|| Error::numeric("attributed effect overflowed"))?;
    let side_effects = outcome.side_effects.clone();
    memory.keep(outcome.clone());
    // R12-RES-7111: a drift does not only narrow the policy; it files the
    // plan that failed, how, and the stop and roll-back it calls for.
    let before = policy.authority();
    let drift = policy.drift(effect, &side_effects);
    let authority = policy.observe(effect, &side_effects);
    let failure = FailureEpisode::of(policy.class(), &outcome, effect, drift, (before, authority));
    if let Some(episode) = &failure {
        memory.keep_failure(episode.clone());
    }
    log.pass(Stage::Update)?;
    Ok((authority, outcome, failure))
}

/// What one learning left: the record the memory kept, the attribution it
/// was learned on, the effect (`observed - counterfactual`) the policy was
/// held to, and the authority it held before and after. The policy's
/// predicted interval and side effects are carried so a reader of the log
/// can recompute `after` from `effect` alone (R12-AGENCY-7133).
#[derive(Debug, Clone, PartialEq)]
pub struct Learned {
    pub outcome: ActionOutcome,
    pub attribution: EffectAttribution,
    pub effect: Decimal,
    pub predicted_uplift: (Decimal, Decimal),
    pub predicted_side_effects: BTreeSet<String>,
    pub before: Authority,
    pub after: Authority,
    /// The failure episode, when the effect drifted (R12-RES-7111).
    pub failure: Option<FailureEpisode>,
}

/// How a learning ended.
#[derive(Debug, Clone, PartialEq)]
pub enum LearningOutcome {
    Learned(Box<Learned>),
    /// Nothing was learned, and why: the outcome needed was absent, or a
    /// stage refused the one offered.
    Withheld(String),
}

/// One learning, with the stages the pass had been through once it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Learning {
    pub passed: Vec<Stage>,
    pub failed: Option<Stage>,
    pub outcome: LearningOutcome,
}

impl Learning {
    /// A learning that never reached the engine: the pass's own stages, and
    /// the reason nothing was offered. A composition root records this
    /// rather than inventing an outcome.
    pub fn withheld(report: &Report, reason: impl Into<String>) -> Self {
        Self {
            passed: report.passed.clone(),
            failed: report.failed,
            outcome: LearningOutcome::Withheld(reason.into()),
        }
    }
}

/// [`learn`] after a shadowed pass, from its report.
///
/// The pass must have ended [`Outcome::Shadowed`] with `Act` shadowed; any
/// other report is refused as an `Err`, because a caller offering a
/// simulated outcome for a pass that did not shadow a plan has confused two
/// passes. A stage that refuses the outcome is not an `Err`: it ends as
/// [`LearningOutcome::Withheld`] with the stage recorded failed, so the
/// refusal is journaled beside the pass like the act stage's own refusals.
pub fn learn_shadowed(
    report: &Report,
    policy: &mut ActionPolicy,
    memory: &mut ActionOutcomeMemory,
    outcome: ActionOutcomeDraft,
    attribution: &EffectAttribution,
) -> Result<Learning, Error> {
    let Outcome::Shadowed(steps) = &report.outcome else {
        return Err(Error::guard(
            "the pass was not shadowed, so it simulated no action to learn from",
        ));
    };
    if !report.act_shadowed || report.failed.is_some() {
        return Err(Error::guard(
            "the report says shadowed but its stage log does not; the two describe different \
             passes",
        ));
    }
    let mut log = StageLog {
        passed: report.passed.clone(),
        failed: report.failed,
        shadowed_act: report.act_shadowed,
    };
    let before = policy.authority();
    let outcome = match learn_outcome(&mut log, steps, policy, memory, outcome, attribution) {
        Ok((after, outcome, failure)) => LearningOutcome::Learned(Box::new(Learned {
            effect: outcome
                .actual_observations
                .checked_sub(outcome.no_action_counterfactual)
                .ok_or_else(|| Error::numeric("attributed effect overflowed"))?,
            outcome,
            attribution: attribution.clone(),
            predicted_uplift: policy.predicted_uplift(),
            predicted_side_effects: policy.predicted_side_effects().clone(),
            before,
            after,
            failure,
        })),
        Err(refusal) => LearningOutcome::Withheld(refusal.message().to_string()),
    };
    Ok(Learning {
        passed: log.passed,
        failed: log.failed,
        outcome,
    })
}

fn attribute(
    policy: &ActionPolicy,
    claimed: &Decimal,
    attribution: &EffectAttribution,
) -> Result<(), Error> {
    attribution.validate()?;
    if attribution.action_class != policy.class() {
        return Err(Error::denied(format!(
            "the attribution measures {:?}, not this policy's {:?}",
            attribution.action_class,
            policy.class()
        )));
    }
    // The statistics/money crossing: identifiability is an `f64` statistic,
    // the outcome's confidence a `Decimal`. Compared as a `Decimal` so the
    // comparison is exact on the record's side; a non-finite figure has
    // already been refused by `validate`.
    let supported = Decimal::from_f64(attribution.identifiability)
        .ok_or_else(|| Error::numeric("identifiability could not be read as a decimal"))?;
    if *claimed > supported {
        return Err(Error::denied(format!(
            "the outcome claims attribution confidence {claimed}, above the {supported} the \
             attribution identifies; correlation is not upgraded to cause by restating it"
        )));
    }
    Ok(())
}
#[derive(Debug, Clone, Deserialize)]
pub struct VariableDecl {
    pub name: String,
    pub observable: bool,
    pub controllable: bool,
    /// In the owned-system registry. Absent means not owned.
    #[serde(default)]
    pub owned: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDecl {
    pub variable: String,
    pub edge: ToolEdgeDraft,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProposalDraft {
    pub root: PlanNode,
    pub comparison: ComparisonDraft,
}

/// Everything one pass reads. A document, so a composition root can take it
/// from an operator; every part still goes through the constructor that
/// refuses it.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub goal: GoalSpec,
    /// The variables the goal's target state is stated over.
    pub targets: Vec<String>,
    pub variables: Vec<VariableDecl>,
    pub causes: Vec<(String, String)>,
    pub tool_edges: Vec<EdgeDecl>,
    pub identity: ActingIdentity,
    pub evidence: BTreeMap<String, LeverEvidence>,
    pub observed_facts: BTreeSet<String>,
    /// The operator's own candidates. May be empty when `structural`
    /// declares generation: the planner then proposes its own.
    #[serde(default)]
    pub proposals: Vec<ProposalDraft>,
    /// What stands in front of every adapter call. Optional in the document
    /// so a shadow request can omit it; a pass with authority to act and no
    /// guard is refused by [`execute`], never sent unguarded.
    #[serde(default)]
    pub guards: Option<GuardDecl>,
    /// This pass's declarations for a guard the caller keeps across passes
    /// ([`run_governed`]). Refused beside `guards`: one guard per pass.
    #[serde(default)]
    pub pass: Option<PassDecl>,
    /// Competing structural models, scenario branches and generation
    /// (R12-AGENCY-6084 / 6087 / 6088).
    #[serde(default)]
    pub structural: Option<StructuralDecl>,
    /// The venues, products, strategies and entities this goal touches, by
    /// kind, so legal eligibility can be asked of each (R12-GOV-6116).
    #[serde(default)]
    pub touches: Vec<Subject>,
}

/// The candidate the gate let through, as the pass knew it: where it came
/// from, what it predicted, and how well its levers are identified. Carried
/// so a learning after the pass reads the plan's own prediction rather than
/// re-deriving one.
#[derive(Debug, Clone, PartialEq)]
pub struct ChosenCandidate {
    /// Index into [`Report::candidates`].
    pub index: usize,
    pub label: String,
    /// The comparison's expected effect: the prediction the outcome is
    /// scored against.
    pub expected_causal_effect: Decimal,
    /// The horizon the plan's effect is expected at.
    pub time_to_effect_secs: u64,
    /// The weakest lever's identifiability under the request's evidence —
    /// the figure the gate admitted the plan on, and the ceiling on how sure
    /// any attribution of its effect may be.
    pub identifiability: Decimal,
    /// The action classes of the tools the plan's steps use.
    pub methods: BTreeSet<Method>,
}

/// How a pass ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A stage failed, or the act stage's guard withheld the plan; the
    /// reason.
    Stopped(String),
    /// No proposal beat doing nothing, or none survived.
    NoAction,
    Abstain(Vec<String>),
    GatherEvidence(Vec<String>),
    ProposeExperiment(Vec<String>),
    /// Cleared every gate; these steps would have run. None did.
    Shadowed(Vec<Step>),
    Executed(Vec<Step>),
    /// Cleared every gate and was attempted; the registry or the adapter
    /// refused the call. The attempt's records are in `approvals` and
    /// `audit`.
    Refused {
        reason: String,
        attempted: Vec<Step>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub passed: Vec<Stage>,
    pub failed: Option<Stage>,
    pub levers: Vec<String>,
    /// Candidate index and why it was refused. Indices count the operator's
    /// proposals first, then generated candidates; [`Report::candidates`]
    /// names each.
    pub refused: Vec<(usize, Error)>,
    pub outcome: Outcome,
    /// GOV-6124's records, when a guard was supplied and admitted the plan.
    pub approvals: Option<ApprovalChain>,
    /// Every registry decision the guard has audited, this pass included.
    pub audit: Vec<ToolAudit>,
    /// The voices the guard admitted this pass, one per step sent
    /// (R12-AGENCY-1064). Empty in shadow and when nothing was sent.
    pub released: Vec<Voice>,
    /// The §27 intent of every step the guard cleared, in shadow as when
    /// acting (R12-CONTRACT-7082).
    pub intents: Vec<ActionIntent>,
    /// The plan the guard withheld and the alternative sought
    /// (R12-RES-7110); `None` when nothing was withheld.
    pub blocked: Option<BlockedPlan>,
    /// A label per candidate, in index order: `proposal N` for the
    /// operator's, the steps for a generated one.
    pub candidates: Vec<String>,
    /// Per simulated candidate, one result per (model, scenario).
    pub simulations: Vec<Simulated>,
    /// Why the goal is not legally eligible, when an eligibility register
    /// was consulted and refused; empty otherwise.
    pub eligibility: Vec<String>,
    /// `Act` was shadowed: every check cleared and nothing was sent. Off
    /// `passed` on purpose; see [`StageLog`].
    pub act_shadowed: bool,
    /// The candidate the gate admitted, when one was.
    pub chosen: Option<ChosenCandidate>,
    /// The objective the simulations were scored on, when a structural model
    /// was declared; a simulated effect is signed toward it.
    pub objective: Option<Objective>,
}

fn report(
    log: StageLog,
    levers: Vec<String>,
    refused: Vec<(usize, Error)>,
    outcome: Outcome,
) -> Report {
    Report {
        passed: log.passed,
        failed: log.failed,
        levers,
        refused,
        outcome,
        approvals: None,
        audit: Vec::new(),
        released: Vec::new(),
        intents: Vec::new(),
        blocked: None,
        candidates: Vec::new(),
        simulations: Vec::new(),
        eligibility: Vec::new(),
        act_shadowed: log.shadowed_act,
        chosen: None,
        objective: None,
    }
}

/// One pass of the loop, observe through act, in the declared order, with
/// the guard the request carries (if any) built fresh for this pass.
///
/// `Err` means the request itself was malformed. A request that was
/// well-formed and went nowhere is an `Ok` report that says which stage
/// stopped it.
pub fn run(
    request: Request,
    policy: Option<&ActionPolicy>,
    adapter: &mut dyn Adapter,
) -> Result<Report, Error> {
    run_with(request, policy, None, adapter)
}

/// [`run`] with a guard the caller keeps, so the registry's rate, budget,
/// revocations and audit persist across passes. A request that also carries
/// its own guard is refused: two guards are two sources of truth about what
/// may be sent.
pub fn run_with(
    request: Request,
    policy: Option<&ActionPolicy>,
    guard: Option<&mut ActionGuard>,
    adapter: &mut dyn Adapter,
) -> Result<Report, Error> {
    run_governed(
        request,
        Governance {
            policy,
            guard,
            eligibility: None,
        },
        adapter,
    )
}

/// What a composition root holds across passes and lends to each one.
#[derive(Debug)]
pub struct Governance<'a> {
    pub policy: Option<&'a ActionPolicy>,
    pub guard: Option<&'a mut ActionGuard>,
    /// The legal-eligibility register and the instant to ask it at. With
    /// one, every subject the request `touches` — and every entity the goal
    /// affects, which must be among them — must be eligible in every
    /// jurisdiction the goal names, or no candidate is legally eligible.
    pub eligibility: Option<(&'a EligibilityRegister, Timestamp)>,
}

/// [`run_with`], with the eligibility register a composition root holds.
///
/// When the caller supplies the guard and the request declares a `pass`,
/// the pass is begun on that guard before the act stage and ended after it
/// on every path, so its approvals never outlive it.
pub fn run_governed(
    request: Request,
    governance: Governance<'_>,
    adapter: &mut dyn Adapter,
) -> Result<Report, Error> {
    run_governed_with(request, governance, None, adapter)
}

/// [`run_governed`] with the scenarios a composition root derives from a
/// model this crate may not depend on (R12-AGENCY-6088): they are simulated
/// beside the request's declared scenarios under every structural model.
pub fn run_governed_with(
    mut request: Request,
    governance: Governance<'_>,
    scenarios: Option<&dyn ScenarioSource>,
    adapter: &mut dyn Adapter,
) -> Result<Report, Error> {
    let Governance {
        policy,
        guard,
        eligibility,
    } = governance;
    let declared = request.guards.take();
    let pass = request.pass.take();
    if declared.is_some() && (guard.is_some() || pass.is_some()) {
        return Err(Error::invalid(
            "the request carries a guard and also a separate guard or pass; supply exactly one",
        ));
    }
    let mut owned = declared.map(ActionGuard::from_decl).transpose()?;
    let guard = match owned.as_mut() {
        Some(built) => Some(built),
        None => guard,
    };
    match (guard, pass) {
        (Some(guard), Some(pass)) => {
            guard.begin_pass(pass)?;
            let result = stages(
                request,
                policy,
                Some(&mut *guard),
                eligibility,
                scenarios,
                adapter,
            );
            guard.end_pass();
            result
        }
        (None, Some(_)) => Err(Error::invalid(
            "the request declares a pass but no guard was supplied to begin it on",
        )),
        (guard, None) => stages(request, policy, guard, eligibility, scenarios, adapter),
    }
}

/// The legal-eligibility verdict for `goal` over `touches`, as refusals.
fn eligibility_refusals(
    goal: &GoalSpec,
    touches: &[Subject],
    register: &EligibilityRegister,
    at: Timestamp,
) -> Vec<String> {
    let mut refusals: Vec<String> = goal
        .entities_affected
        .iter()
        .filter(|entity| !touches.iter().any(|s| &s.id == *entity))
        .map(|entity| {
            format!(
                "`{entity}`, which the goal affects, is not declared in `touches`; name its kind \
                 so its eligibility can be asked"
            )
        })
        .collect();
    for jurisdiction in &goal.jurisdictions {
        refusals.extend(
            register
                .refusals(touches, jurisdiction, at)
                .iter()
                .map(ToString::to_string),
        );
    }
    refusals
}

fn stages(
    request: Request,
    policy: Option<&ActionPolicy>,
    guard: Option<&mut ActionGuard>,
    eligibility: Option<(&EligibilityRegister, Timestamp)>,
    scenarios: Option<&dyn ScenarioSource>,
    adapter: &mut dyn Adapter,
) -> Result<Report, Error> {
    let mut graph = AffordanceGraph::new();
    for variable in &request.variables {
        graph.add_variable(&variable.name, variable.observable, variable.controllable)?;
        if variable.owned {
            graph.declare_owned(&variable.name)?;
        }
    }
    for (from, to) in &request.causes {
        graph.add_cause(from, to)?;
    }
    for declared in request.tool_edges {
        graph.add_tool_edge(&declared.variable, declared.edge)?;
    }
    if request.targets.is_empty() {
        return Err(Error::invalid(
            "`targets` is empty; name the variables the goal's target state is stated over",
        ));
    }
    let structural = request
        .structural
        .map(|decl| Structural::build_with(decl, scenarios))
        .transpose()?;
    let mut log = StageLog::new();

    // Observe: a desired state over something nobody can see is not checkable.
    if let Some(blind) = request
        .targets
        .iter()
        .find(|t| graph.is_observable(t) != Some(true))
    {
        log.fail(Stage::Observe)?;
        let why = format!("target `{blind}` is not an observable variable");
        return Ok(report(log, vec![], vec![], Outcome::Stopped(why)));
    }
    log.pass(Stage::Observe)?;

    // Predict: each proposal's nine-axis comparison is its prediction, and
    // a structural model that will generate candidates is the prediction
    // for those.
    let mut proposals = Vec::with_capacity(request.proposals.len());
    for draft in request.proposals {
        match draft.comparison.build() {
            Ok(comparison) => proposals.push(Proposal {
                root: draft.root,
                comparison,
            }),
            Err(missing) => {
                log.fail(Stage::Predict)?;
                let why = format!("a proposal carries no complete prediction: {missing}");
                return Ok(report(log, vec![], vec![], Outcome::Stopped(why)));
            }
        }
    }
    let generates = structural.as_ref().is_some_and(Structural::generates);
    if proposals.is_empty() && !generates {
        log.fail(Stage::Predict)?;
        let why = "no proposal was made, so nothing was predicted".to_string();
        return Ok(report(log, vec![], vec![], Outcome::Stopped(why)));
    }
    log.pass(Stage::Predict)?;

    // Specify: `request.goal` is a `GoalSpec`, which exists only validated.
    // Its legal eligibility is decided here, once, for every candidate.
    let refusals = match eligibility {
        Some((register, at)) => eligibility_refusals(&request.goal, &request.touches, register, at),
        None => Vec::new(),
    };
    let legally_eligible = refusals.is_empty();
    log.pass(Stage::Specify)?;

    let targets: Vec<&str> = request.targets.iter().map(String::as_str).collect();
    let levers = graph.levers(&targets);
    if levers.is_empty() {
        log.fail(Stage::IdentifyLevers)?;
        let why = "no controllable variable with a tool reaches a target".to_string();
        let mut done = report(log, levers, vec![], Outcome::Stopped(why));
        done.eligibility = refusals;
        return Ok(done);
    }
    log.pass(Stage::IdentifyLevers)?;

    // Generate: the operator's proposals, each held to the simulation when
    // there is one, and the planner's own candidates.
    let mut labels: Vec<String> = (1..=proposals.len())
        .map(|i| format!("proposal {i}"))
        .collect();
    let mut simulations: Vec<Simulated> = Vec::new();
    let mut pre_refused: Vec<(usize, Error)> = Vec::new();
    let mut candidates: Vec<(usize, Proposal)> = Vec::new();
    for (index, mut proposal) in proposals.into_iter().enumerate() {
        if !legally_eligible {
            proposal.comparison.legally_eligible = false;
        }
        if let Some(structural) = &structural {
            let results = structural.simulate(&proposal.root, &request.observed_facts)?;
            let claim = structural.check_claim(&proposal, &results);
            simulations.push(Simulated {
                label: labels[index].clone(),
                results,
            });
            if let Err(refusal) = claim {
                pre_refused.push((index, refusal));
                continue;
            }
        }
        candidates.push((index, proposal));
    }
    if let Some(structural) = &structural {
        for (proposal, simulated) in structural.generate(
            &graph,
            &levers,
            &request.evidence,
            &request.observed_facts,
            legally_eligible,
        )? {
            let index = labels.len();
            labels.push(simulated.label.clone());
            simulations.push(simulated);
            candidates.push((index, proposal));
        }
    }
    let origin: Vec<usize> = candidates.iter().map(|(i, _)| *i).collect();
    let Selection {
        chosen,
        origin: chosen_from,
        refused,
    } = select(
        &request.goal,
        &graph,
        &request.identity,
        &levers,
        candidates.into_iter().map(|(_, p)| p).collect(),
    )?;
    let mut refused: Vec<(usize, Error)> = refused
        .into_iter()
        .map(|(i, e)| (origin.get(i).copied().unwrap_or(i), e))
        .chain(pre_refused)
        .collect();
    refused.sort_by_key(|(i, _)| *i);
    log.pass(Stage::Generate)?;
    let finish =
        |log: StageLog, levers: Vec<String>, refused: Vec<(usize, Error)>, outcome: Outcome| {
            let mut done = report(log, levers, refused, outcome);
            done.candidates = labels.clone();
            done.simulations = simulations.clone();
            done.eligibility = refusals.clone();
            done.objective = structural.as_ref().map(|s| s.objective().clone());
            done
        };
    let Candidate::Plan(plan) = chosen else {
        return Ok(finish(log, levers, refused, Outcome::NoAction));
    };
    // The plan's own prediction and the evidence it was admitted on, kept
    // for whatever learns from this pass. The index `select` returned counts
    // the candidates it was given; `origin` maps it back to the label list.
    let chosen = chosen_from
        .and_then(|c| origin.get(c.index).map(|index| (*index, c.comparison)))
        .map(|(index, comparison)| -> Result<ChosenCandidate, Error> {
            let mut steps = Vec::new();
            plan.root.all_steps(&mut steps);
            let mut identifiability = Decimal::ONE;
            let mut methods = BTreeSet::new();
            for step in &steps {
                identifiability = identifiability.min(
                    request
                        .evidence
                        .get(&step.variable)
                        .map_or(Decimal::ZERO, |e| e.identifiability),
                );
                if let Some(edge) = graph
                    .edges(&step.variable)
                    .iter()
                    .find(|e| e.tool == step.tool)
                {
                    methods.insert(edge.method);
                }
            }
            Ok(ChosenCandidate {
                index,
                label: labels.get(index).cloned().ok_or_else(|| {
                    Error::numeric(format!("chosen candidate {index} has no label"))
                })?,
                expected_causal_effect: comparison.expected_causal_effect,
                time_to_effect_secs: comparison.time_to_effect_secs,
                identifiability,
                methods,
            })
        })
        .transpose()?;

    if plan.root.simulate(&request.observed_facts).is_empty() {
        log.fail(Stage::Simulate)?;
        let why = "under the observed facts no step of the chosen plan fires".to_string();
        return Ok(finish(log, levers, refused, Outcome::Stopped(why)));
    }
    log.pass(Stage::Simulate)?;

    let gated = match decide(&request.goal, plan, &request.evidence)? {
        Decision::Execute(plan) => Ok(plan),
        Decision::Abstain(weak) => Err(Outcome::Abstain(weak)),
        Decision::GatherEvidence(weak) => Err(Outcome::GatherEvidence(weak)),
        Decision::ProposeExperiment(weak) => Err(Outcome::ProposeExperiment(weak)),
    };
    let plan = match gated {
        Ok(plan) => plan,
        Err(outcome) => {
            log.fail(Stage::Gate)?;
            return Ok(finish(log, levers, refused, outcome));
        }
    };
    log.pass(Stage::Gate)?;

    let gated = Gated {
        goal: &request.goal,
        graph: &graph,
        plan: &plan,
        facts: &request.observed_facts,
        simulated: chosen
            .as_ref()
            .and_then(|c| simulations.iter().find(|s| s.label == c.label)),
    };
    let mut guard = guard;
    let acted = execute(&mut log, policy, gated, guard.as_deref_mut(), adapter)?;
    let outcome = match acted.execution {
        Execution::Shadow(steps) => Outcome::Shadowed(steps),
        Execution::Executed(steps) => Outcome::Executed(steps),
        Execution::Withheld(why) => Outcome::Stopped(format!("the act stage withheld: {why}")),
        Execution::Refused { reason, attempted } => Outcome::Refused { reason, attempted },
    };
    let mut done = finish(log, levers, refused, outcome);
    done.approvals = acted.approvals;
    done.released = acted.released;
    done.intents = acted.intents;
    done.blocked = acted.blocked;
    done.audit = guard.map(|g| g.audit().to_vec()).unwrap_or_default();
    done.chosen = chosen;
    Ok(done)
}
