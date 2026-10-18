//! What a raised gap turns into before anything runs: the acquisition it
//! asks for (EXPAND-049), the derivation it was concluded by (REASON-3062),
//! and the research plan synthesised from its goal and constraints
//! (REASON-3061).
//!
//! The failure each prevents. A gap that names no acquisition is a ticket
//! nobody can start. A queued item that cannot say which rule raised it, on
//! which evidence, against which assumption, cannot be audited when it turns
//! out to have been noise. And a research plan written by hand per item is a
//! plan that skips the shadow stage the one time it matters; synthesised
//! from the same function every time, the order is a property and is tested.

use crate::gap::{GapTrigger, RaisedGap};
use qip_agents::tools::ToolKind;
use qip_contracts::expansion::GapClass;
use serde::{Deserialize, Serialize};

/// The acquisition a gap of each class asks for (§23.1 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionAction {
    /// Find a lawful source; licensing is evaluated before anything is fetched.
    DiscoverLawfulData,
    /// Extend the ontology with the type that was missing.
    ExtendOntology,
    /// Construct a simulation that can test the missing causal link.
    ConstructSimulation,
    /// Gather the past episodes an estimate is too thin without.
    GatherEpisodes,
    /// Select or fit a model family for the decayed or missing model.
    SelectModel,
    /// Build the tool itself.
    BuildTool,
    /// Instantiate a specialist for the seat nobody competent holds.
    InstantiateSpecialist,
    /// Replay ticks to study the execution shortfall.
    ReplayTicks,
    /// Formulate the problem for a solver the platform does not have.
    FormulateSolver,
}

impl AcquisitionAction {
    /// The one acquisition each class asks for. Total over [`GapClass`], so a
    /// tenth class is a compile error here rather than a gap with no action.
    pub const fn for_class(class: GapClass) -> Self {
        match class {
            GapClass::Data => Self::DiscoverLawfulData,
            GapClass::Ontology => Self::ExtendOntology,
            GapClass::Causal => Self::ConstructSimulation,
            GapClass::Memory => Self::GatherEpisodes,
            GapClass::Model => Self::SelectModel,
            GapClass::Tool => Self::BuildTool,
            GapClass::Specialist => Self::InstantiateSpecialist,
            GapClass::Execution => Self::ReplayTicks,
            GapClass::ComputeQuantumResearch => Self::FormulateSolver,
        }
    }

    /// The tool the acquisition runs on, by the name a tool registry
    /// resolves it by, and its kind. Whether that tool exists is the
    /// registry's answer, not this table's: a missing one becomes a build
    /// task of its own at admission (EXPAND-068).
    ///
    /// This is the primary tool; [`Self::tools`] is every tool the action
    /// needs, and the primary one is always its first.
    pub const fn tool(self) -> (&'static str, ToolKind) {
        self.tools()[0]
    }

    /// Every tool the acquisition needs, primary first.
    ///
    /// Two actions need a second tool, and each is the one the platform
    /// would otherwise never notice it lacks. Formulating for a quantum
    /// backend needs a classical solver beside it, because a quantum result
    /// is scored against the classical baseline computed every time
    /// (ADR 0006) and a formulation with no solver to compute it cannot be
    /// evaluated at all. Replaying a venue's ticks needs the adapter that
    /// speaks that venue's feed; a replay simulator with no adapter replays
    /// nothing. Before this table had a second column, no gap could propose
    /// a `Solver` or a `VenueAdapter` (R12-EXPAND-6037).
    pub const fn tools(self) -> &'static [(&'static str, ToolKind)] {
        match self {
            Self::DiscoverLawfulData => &[("source-discovery", ToolKind::Connector)],
            Self::ExtendOntology => &[("ontology-editor", ToolKind::Parser)],
            Self::ConstructSimulation => &[("stress-simulator", ToolKind::Simulator)],
            Self::GatherEpisodes => &[("episode-archive", ToolKind::Connector)],
            Self::SelectModel => &[("model-fit", ToolKind::ModelEndpoint)],
            Self::BuildTool => &[("tool-builder", ToolKind::CodeRuntime)],
            Self::InstantiateSpecialist => &[("specialist-factory", ToolKind::CodeRuntime)],
            Self::ReplayTicks => &[
                ("tick-replay", ToolKind::Simulator),
                ("venue-adapter", ToolKind::VenueAdapter),
            ],
            Self::FormulateSolver => &[
                ("quantum-backend", ToolKind::QuantumBackend),
                ("classical-solver", ToolKind::Solver),
            ],
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DiscoverLawfulData => "discover_lawful_data",
            Self::ExtendOntology => "extend_ontology",
            Self::ConstructSimulation => "construct_simulation",
            Self::GatherEpisodes => "gather_episodes",
            Self::SelectModel => "select_model",
            Self::BuildTool => "build_tool",
            Self::InstantiateSpecialist => "instantiate_specialist",
            Self::ReplayTicks => "replay_ticks",
            Self::FormulateSolver => "formulate_solver",
        }
    }
}

/// The machine-readable derivation a queued item carries (REASON-3062):
/// the rule that fired, the evidence it fired on, the assumption the
/// evidence violates, and how uncertain the conclusion is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Derivation {
    /// The conclusion: the research question.
    pub conclusion: String,
    /// The detector rule that fired, the first evidence id by convention of
    /// `detectors.rs`, or the trigger's name where none was given.
    pub rule: String,
    pub evidence: Vec<String>,
    /// The assumption the platform was operating under that this evidence
    /// says no longer holds.
    pub violated_assumptions: Vec<String>,
    /// One minus the detector's severity: a weak detection is an uncertain
    /// conclusion, and saying so is what stops a weak one reading as fact.
    pub uncertainty: f64,
}

/// The assumption each trigger's evidence contradicts.
pub const fn violated_assumption(trigger: GapTrigger) -> &'static str {
    match trigger {
        GapTrigger::Surprise => "the world model's expectation for this subject was adequate",
        GapTrigger::LowConfidence => "beliefs in this class are calibrated well enough to act on",
        GapTrigger::Contradiction => "the sources reading this subject agree on its direction",
        GapTrigger::MissedOpportunity => {
            "the gate or desk that passed on this was right to pass on it"
        }
        GapTrigger::UnexplainedPnl => "the platform's models explain its own results",
        GapTrigger::ModelDrift => "the model still describes the data it is serving",
        GapTrigger::UnsupportedRequest => "some brain or tool can answer what was asked",
    }
}

impl Derivation {
    pub fn of(question: &str, raised: &RaisedGap) -> Self {
        let rule = raised
            .signal
            .evidence
            .first()
            .filter(|e| e.starts_with("detector:"))
            .cloned()
            .unwrap_or_else(|| format!("trigger:{:?}", raised.trigger));
        Self {
            conclusion: question.to_string(),
            rule,
            evidence: raised.signal.evidence.clone(),
            violated_assumptions: vec![violated_assumption(raised.trigger).to_string()],
            uncertainty: 1.0 - raised.signal.severity,
        }
    }
}

/// The four places research may run, in the order it must run in
/// (EXPAND-050). There is deliberately no live environment: a research
/// campaign that could name one is a path to capital that skipped every
/// promotion gate, and the absence is the guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Historical,
    Simulated,
    Paper,
    Shadow,
}

impl Environment {
    pub const ORDER: [Self; 4] = [Self::Historical, Self::Simulated, Self::Paper, Self::Shadow];
}

/// One step of a synthesised research plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Build a tool the acquisition needs and the registry does not hold.
    BuildTool {
        tool: String,
    },
    Acquire {
        action: AcquisitionAction,
    },
    Run {
        environment: Environment,
    },
    /// Evaluate on the nine criteria (`promotion::Criterion`).
    Evaluate,
    /// Promote to a capability on a passing evaluation, or record the null.
    Decide,
}

/// A research plan synthesised from an explicit goal and constraints
/// (REASON-3061). The goal is the question; the constraints are whether the
/// tool exists and the compute the item was costed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchPlan {
    pub goal: String,
    pub steps: Vec<Step>,
}

impl ResearchPlan {
    /// Every plan acquires, runs all four environments in order, evaluates
    /// and decides; one whose tool is not registered builds it first.
    pub fn synthesise(goal: &str, class: GapClass, tool_registered: bool) -> Self {
        Self::synthesise_with(goal, class, |_| tool_registered)
    }

    /// As [`Self::synthesise`], asking `registered` of each tool the action
    /// needs, so a plan builds exactly the tools that are missing, in the
    /// order [`AcquisitionAction::tools`] lists them.
    pub fn synthesise_with(goal: &str, class: GapClass, registered: impl Fn(&str) -> bool) -> Self {
        let action = AcquisitionAction::for_class(class);
        let mut steps = Vec::with_capacity(9);
        for (tool, _) in action.tools() {
            if !registered(tool) {
                steps.push(Step::BuildTool {
                    tool: (*tool).to_string(),
                });
            }
        }
        steps.push(Step::Acquire { action });
        steps.extend(
            Environment::ORDER
                .iter()
                .map(|&environment| Step::Run { environment }),
        );
        steps.push(Step::Evaluate);
        steps.push(Step::Decide);
        Self {
            goal: goal.to_string(),
            steps,
        }
    }
}
