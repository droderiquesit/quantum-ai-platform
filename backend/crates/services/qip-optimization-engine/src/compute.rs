//! The problem compiler and the compute decision record (§11.1–11.2).
//!
//! Before this module the router's choice of where a problem ran was a solver
//! name and a sentence, and the sentence only ever discussed one substrate —
//! the quantum one. A reader of the journal could not tell whether a GPU or a
//! TPU had been considered and declined, or simply never existed in the
//! router's vocabulary, and those are different facts: the first is a decision,
//! the second is a gap wearing one's clothes. R12-QUANT-4046 asks that a
//! routed problem record which substrate was chosen *among the declared set*
//! and why, so the declared set is [`Substrate::ALL`] and every
//! [`ComputeDecision`] carries a verdict for each member of it — including the
//! ones that are declined because nothing in this workspace can reach them.
//!
//! The compiler half is [`TaskSpec::compile`]. A task is admitted only with a
//! class, a variable count, its constraints, an objective, a tolerance, a
//! deadline and the properties a candidate must be verified against
//! (R12-QUANT-4048, 4049). Each is optional on the spec so that a missing one
//! is a refusal naming it, never a default: a tolerance nobody chose is a
//! feasibility test nobody can defend, and a task with no verification
//! property is one whose answer can only be believed.
//!
//! A compiled task always carries a classical formulation, and a quantum one
//! only where the task's structure makes it eligible (R12-QUANT-4050). That is
//! what lets the router say "the QPU was not considered because the problem is
//! continuous" as a property of the compiled task rather than as a branch
//! somebody has to find.
//!
//! None of this is on the hot path. The compiler runs inside the DECIDE
//! stage's construction, beside the classical solve it describes; no edge or
//! reflex crate depends on it.

use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};

/// What kind of work a task is (§11.2 step 1). Eight classes, the eight the
/// blueprint names, and no catch-all: a task that fits none of them is refused
/// by [`TaskSpec::compile`] rather than routed under a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    Forecasting,
    GraphSearch,
    /// Allocation under a cardinality limit is combinatorial; the same
    /// allocation without one is its convex relaxation, which is filed here
    /// too and marked continuous on the compiled task so that no quantum
    /// formulation is generated for it.
    CombinatorialOptimisation,
    Sampling,
    KernelMethod,
    Simulation,
    SymbolicSolving,
    ModelTraining,
}

impl TaskClass {
    pub const ALL: [Self; 8] = [
        Self::Forecasting,
        Self::GraphSearch,
        Self::CombinatorialOptimisation,
        Self::Sampling,
        Self::KernelMethod,
        Self::Simulation,
        Self::SymbolicSolving,
        Self::ModelTraining,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Forecasting => "forecasting",
            Self::GraphSearch => "graph_search",
            Self::CombinatorialOptimisation => "combinatorial_optimisation",
            Self::Sampling => "sampling",
            Self::KernelMethod => "kernel_method",
            Self::Simulation => "simulation",
            Self::SymbolicSolving => "symbolic_solving",
            Self::ModelTraining => "model_training",
        }
    }

    /// Whether a discrete instance of this class has a QUBO/Ising form the
    /// in-tree providers accept. Kernel and variational formulations would
    /// need machinery this workspace does not have, so they are not claimed.
    const fn has_qubo_form(&self) -> bool {
        matches!(self, Self::CombinatorialOptimisation | Self::GraphSearch)
    }
}

/// The substrates a task could run on (§11.1). The declared set: a
/// [`ComputeDecision`] carries one verdict for each, so a substrate that was
/// never reachable is recorded as declined for that reason instead of being
/// absent from the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Substrate {
    Cpu,
    Gpu,
    Tpu,
    QuantumInspired,
    Qpu,
    ParallelEnsemble,
}

impl Substrate {
    pub const ALL: [Self; 6] = [
        Self::Cpu,
        Self::Gpu,
        Self::Tpu,
        Self::QuantumInspired,
        Self::Qpu,
        Self::ParallelEnsemble,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Tpu => "tpu",
            Self::QuantumInspired => "quantum_inspired",
            Self::Qpu => "qpu",
            Self::ParallelEnsemble => "parallel_ensemble",
        }
    }
}

/// How a compiled task may be solved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Formulation {
    /// Convex quadratic program; a global optimum exists and is found.
    QuadraticProgram,
    /// Exhaustive search over the binary encoding; provably optimal.
    ExhaustiveEnumeration,
    /// Classical simulated annealing over the binary encoding; a heuristic.
    SimulatedAnnealing,
    /// QUBO/Ising, the form a QAOA provider accepts and the quantum-inspired
    /// rung searches. Never the classical baseline.
    Qubo,
}

impl Formulation {
    pub const fn is_quantum(&self) -> bool {
        matches!(self, Self::Qubo)
    }

    /// Whether the classical formulation certifies its answer optimal, in
    /// which case nothing dispatched elsewhere can do better than tie.
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::QuadraticProgram | Self::ExhaustiveEnumeration)
    }
}

/// What a caller declares about a task (§11.2 step 2). Every field is an
/// `Option` so that [`Self::compile`] can refuse the missing one by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskSpec {
    pub class: Option<TaskClass>,
    pub variables: Option<usize>,
    /// Constraint labels. `Some(vec![])` is an unconstrained task, which is a
    /// statement; `None` is a caller that did not say, which is refused.
    pub constraints: Option<Vec<String>>,
    pub objective: Option<String>,
    /// Feasibility tolerance a candidate is scored against.
    pub tolerance: Option<f64>,
    pub deadline: Option<Duration>,
    /// The properties a returned candidate is checked against before use.
    pub verification: Option<Vec<String>>,
    /// The classical formulation. Required: a task with no classical
    /// baseline has nothing to measure a quantum claim against (ADR 0006).
    pub classical: Option<Formulation>,
    /// Whether the variables are binary choices.
    pub discrete: bool,
}

/// A task the compiler admitted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledTask {
    pub class: TaskClass,
    pub variables: usize,
    pub constraints: Vec<String>,
    pub objective: String,
    pub tolerance: f64,
    pub deadline: Duration,
    pub verification: Vec<String>,
    pub classical: Formulation,
    /// Present only where the task is eligible for one (R12-QUANT-4050).
    pub quantum: Option<Formulation>,
    pub discrete: bool,
}

fn required<T>(value: Option<T>, name: &str) -> Result<T> {
    value.ok_or_else(|| {
        Error::invalid(format!(
            "a task is refused without {name}; declare it rather than letting the compiler \
             default it, because a defaulted {name} is a choice nobody made"
        ))
    })
}

impl TaskSpec {
    /// Admit the task or name what is missing.
    pub fn compile(self) -> Result<CompiledTask> {
        let class = required(self.class, "a task class")?;
        let variables = required(self.variables, "a variable count")?;
        if variables == 0 {
            return Err(Error::invalid(
                "a task with no variables has nothing to solve; refuse it at the caller",
            ));
        }
        let constraints = required(self.constraints, "its constraints")?;
        let objective = required(self.objective, "an objective")?;
        if objective.trim().is_empty() {
            return Err(Error::invalid(
                "a task's objective is blank; name the function both paths are scored on",
            ));
        }
        let tolerance = required(self.tolerance, "a tolerance")?;
        if !tolerance.is_finite() || tolerance <= 0.0 {
            return Err(Error::invalid(format!(
                "a tolerance of {tolerance} is not a positive finite number; a zero tolerance \
                 refuses every floating-point answer and an infinite one admits every answer"
            )));
        }
        let deadline = required(self.deadline, "a deadline")?;
        if deadline.is_zero() {
            return Err(Error::invalid(
                "a zero deadline expires before any substrate can start; set the time the task \
                 may take",
            ));
        }
        let verification = required(self.verification, "its verification properties")?;
        if verification.is_empty() {
            return Err(Error::invalid(
                "a task with no verification property is one whose answer can only be believed; \
                 name at least the classical re-scoring every candidate goes through",
            ));
        }
        let classical = required(self.classical, "a classical formulation")?;
        if classical.is_quantum() {
            return Err(Error::invalid(
                "the classical formulation named a quantum form; the baseline must be classical \
                 or there is nothing to measure a quantum claim against (ADR 0006)",
            ));
        }
        let quantum = (self.discrete && class.has_qubo_form()).then_some(Formulation::Qubo);
        Ok(CompiledTask {
            class,
            variables,
            constraints,
            objective,
            tolerance,
            deadline,
            verification,
            classical,
            quantum,
            discrete: self.discrete,
        })
    }
}

/// What a substrate did with the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubstrateRole {
    /// Its answer is the one the cycle used.
    Answered,
    /// It ran, and its answer is the baseline the used one was measured
    /// against.
    Baseline,
    /// A job was handed to it this cycle; its answer is judged in a later one.
    Dispatched,
    /// Not used, for the stated reason.
    Declined,
}

/// One substrate's part in a decision, with the reason.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubstrateVerdict {
    pub substrate: Substrate,
    pub role: SubstrateRole,
    pub reason: String,
}

/// The expected-information-value estimate behind a dispatch (R12-QUANT-4051),
/// and the measured record it was computed from (R12-QUANT-4047).
///
/// `win_probability` is the substrate's measured record at this problem size
/// with one win and one loss of prior — `(wins + 1) / (trials + 2)` — so an
/// untried substrate reads as a coin flip and one that has lost every time it
/// was tried decays towards zero instead of being retried forever. A
/// statistic, `f64`; `cost_micros` is the provider's declared price per job
/// and is not converted into it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InformationValue {
    pub trials: u32,
    pub wins: u32,
    pub win_probability: f64,
    /// `win_probability × margin`: the least improvement an adopted answer
    /// brings, weighted by how likely one is.
    pub expected_gain: f64,
    pub cost_micros: u64,
    /// Whether the estimate cleared the policy's bars, so a dispatch was made.
    pub cleared: bool,
    /// The substrate the estimate is about. Defaults to the QPU on a record
    /// written before the quantum-inspired rung existed, which is the only
    /// substrate an estimate was then made for.
    #[serde(default = "qpu")]
    pub substrate: Substrate,
    /// The share of trials whose answer verified; `None` before the first.
    /// A substrate that answers rarely is not repeatable however good its
    /// answers are when they come.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeatability: Option<f64>,
    /// Mean cycles from asking to reading an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_latency_cycles: Option<f64>,
    /// The Brier score of the router's past estimates at this size: how well
    /// it knew in advance whether the substrate would pay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_brier: Option<f64>,
}

const fn qpu() -> Substrate {
    Substrate::Qpu
}

/// The compute decision record: what the task was, where it ran, where it
/// was dispatched, what was declined and why. Journaled with the cycle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComputeDecision {
    pub task: CompiledTask,
    /// One verdict per member of [`Substrate::ALL`], in that order.
    pub verdicts: Vec<SubstrateVerdict>,
    /// Present whenever the quantum arm reached the estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub information_value: Option<InformationValue>,
    /// The quantum-inspired rung's estimate, present whenever that rung was
    /// enabled and the task eligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspired_information_value: Option<InformationValue>,
    /// One measurement per run this decision accounted for — the baseline,
    /// any candidate judged, any job abandoned — with its quality, cost and
    /// latency (R12-QUANT-4053). What the router's learning is folded from,
    /// live and on replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurements: Vec<crate::learning::SolverMeasurement>,
    /// Every candidate as an ensemble feature, not as authority (§11.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ensemble: Vec<crate::research::EnsembleFeature>,
    /// The §11.3 research domain the task belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_domain: Option<crate::research::ResearchDomain>,
    /// Whether a QPU result would count as advantage here, and what is unmet
    /// (§11.5).
    #[serde(default)]
    pub advantage: crate::research::AdvantageStanding,
}

impl ComputeDecision {
    pub fn verdict(&self, substrate: Substrate) -> Option<&SubstrateVerdict> {
        self.verdicts.iter().find(|v| v.substrate == substrate)
    }

    /// The substrate whose answer was used.
    pub fn answered_by(&self) -> Option<Substrate> {
        self.verdicts
            .iter()
            .find(|v| v.role == SubstrateRole::Answered)
            .map(|v| v.substrate)
    }

    /// Refuses a record that does not account for every declared substrate,
    /// or names no answer. Built by the router; checked here so that a record
    /// read back from the log is held to the same shape.
    pub fn validate(&self) -> Result<()> {
        for substrate in Substrate::ALL {
            let count = self
                .verdicts
                .iter()
                .filter(|v| v.substrate == substrate)
                .count();
            if count != 1 {
                return Err(Error::invalid(format!(
                    "a compute decision carries {count} verdict(s) for {}; it must carry exactly \
                     one per declared substrate, or a substrate's absence reads as a decision",
                    substrate.as_str()
                )));
            }
        }
        let answered = self
            .verdicts
            .iter()
            .filter(|v| v.role == SubstrateRole::Answered)
            .count();
        if answered != 1 {
            return Err(Error::invalid(format!(
                "a compute decision names {answered} answering substrate(s); exactly one answer \
                 is used per decision"
            )));
        }
        Ok(())
    }
}
