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

use crate::affordance::{AffordanceGraph, ToolEdgeDraft};
use crate::autonomy::{ActionPolicy, Authority};
use crate::comparison::ComparisonDraft;
use crate::goal::GoalSpec;
use crate::plan::{
    ActingIdentity, Candidate, InterventionPlan, PlanNode, Proposal, Selection, Step, select,
};
use qip_core::{Decimal, Error};
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageLog {
    passed: Vec<Stage>,
    failed: Option<Stage>,
}

impl StageLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuse `stage` unless it is the next in [`ORDER`] and nothing failed.
    fn expect(&self, stage: Stage) -> Result<(), Error> {
        if let Some(failed) = self.failed {
            return Err(Error::guard(format!(
                "{failed:?} failed, so {stage:?} may not run; start a new pass"
            )));
        }
        match ORDER.get(self.passed.len()) {
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
}

/// The only caller of an [`Adapter`].
///
/// Refuses unless `log` shows simulation and the gate passed. With no policy,
/// or one at shadow, it calls nothing and leaves `Act` unrecorded, so nothing
/// can later be attributed to an action that never happened. At
/// narrow-reversible every step is checked before the first is sent: one of
/// another class, or one whose tool is not declared reversible, refuses the
/// whole plan with no call made.
pub fn execute(
    log: &mut StageLog,
    policy: Option<&ActionPolicy>,
    graph: &AffordanceGraph,
    plan: &InterventionPlan,
    facts: &BTreeSet<String>,
    adapter: &mut dyn Adapter,
) -> Result<Execution, Error> {
    log.expect(Stage::Act)?;
    let steps: Vec<Step> = plan.root.simulate(facts).into_iter().cloned().collect();
    let policy = match policy {
        Some(policy) if policy.authority() == Authority::NarrowReversible => policy,
        _ => return Ok(Execution::Shadow(steps)),
    };
    for step in &steps {
        let edge = graph
            .edges(&step.variable)
            .iter()
            .find(|e| e.tool == step.tool)
            .ok_or_else(|| {
                Error::denied(format!(
                    "tool `{}` has no edge on `{}` in this graph",
                    step.tool, step.variable
                ))
            })?;
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
    for step in &steps {
        if let Err(refusal) = adapter.call(step) {
            log.fail(Stage::Act)?;
            return Err(refusal);
        }
    }
    log.pass(Stage::Act)?;
    Ok(Execution::Executed(steps))
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
    pub proposals: Vec<ProposalDraft>,
}

/// How a pass ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A stage failed before any plan was chosen; the reason.
    Stopped(String),
    /// No proposal beat doing nothing, or none survived.
    NoAction,
    Abstain(Vec<String>),
    GatherEvidence(Vec<String>),
    ProposeExperiment(Vec<String>),
    /// Cleared every gate; these steps would have run. None did.
    Shadowed(Vec<Step>),
    Executed(Vec<Step>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub passed: Vec<Stage>,
    pub failed: Option<Stage>,
    pub levers: Vec<String>,
    /// Proposal index and why it was refused.
    pub refused: Vec<(usize, Error)>,
    pub outcome: Outcome,
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
    }
}

/// One pass of the loop, observe through act, in the declared order.
///
/// `Err` means the request itself was malformed. A request that was
/// well-formed and went nowhere is an `Ok` report that says which stage
/// stopped it.
pub fn run(
    request: Request,
    policy: Option<&ActionPolicy>,
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

    // Predict: each proposal's nine-axis comparison is its prediction.
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
    if proposals.is_empty() {
        log.fail(Stage::Predict)?;
        let why = "no proposal was made, so nothing was predicted".to_string();
        return Ok(report(log, vec![], vec![], Outcome::Stopped(why)));
    }
    log.pass(Stage::Predict)?;

    // Specify: `request.goal` is a `GoalSpec`, which exists only validated.
    log.pass(Stage::Specify)?;

    let targets: Vec<&str> = request.targets.iter().map(String::as_str).collect();
    let levers = graph.levers(&targets);
    if levers.is_empty() {
        log.fail(Stage::IdentifyLevers)?;
        let why = "no controllable variable with a tool reaches a target".to_string();
        return Ok(report(log, levers, vec![], Outcome::Stopped(why)));
    }
    log.pass(Stage::IdentifyLevers)?;

    let Selection { chosen, refused } =
        select(&request.goal, &graph, &request.identity, &levers, proposals)?;
    log.pass(Stage::Generate)?;
    let Candidate::Plan(plan) = chosen else {
        return Ok(report(log, levers, refused, Outcome::NoAction));
    };

    if plan.root.simulate(&request.observed_facts).is_empty() {
        log.fail(Stage::Simulate)?;
        let why = "under the observed facts no step of the chosen plan fires".to_string();
        return Ok(report(log, levers, refused, Outcome::Stopped(why)));
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
            return Ok(report(log, levers, refused, outcome));
        }
    };
    log.pass(Stage::Gate)?;

    let outcome = match execute(
        &mut log,
        policy,
        &graph,
        &plan,
        &request.observed_facts,
        adapter,
    )? {
        Execution::Shadow(steps) => Outcome::Shadowed(steps),
        Execution::Executed(steps) => Outcome::Executed(steps),
    };
    Ok(report(log, levers, refused, outcome))
}
