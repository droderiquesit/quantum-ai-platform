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

/// What shape a task's answer takes, declared by the caller so that the
/// compiler — not the caller — decides the task's class (R12-QUANT-4048).
///
/// Before this the router wrote `TaskClass::CombinatorialOptimisation` into
/// every spec it compiled, so the class on the journaled record was a
/// constant asserting a classification nobody had made. Each flag here is a
/// fact about the answer a caller can check against its own code; the class
/// follows from them, and a set of facts no single class fits is refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskStructure {
    /// The answer is the choice of variables that minimises a declared
    /// objective. On its own this is combinatorial optimisation; a graph
    /// search and a fit may minimise something too, so it is compatible with
    /// either and contradicts neither.
    pub minimises_objective: bool,
    /// The answer is a path, cut or matching in a graph the variables
    /// describe.
    pub graph_structured: bool,
    /// The answer is a value at a future instant.
    pub predicts_ahead: bool,
    /// The answer is a set of draws from a distribution rather than a point.
    pub draws_samples: bool,
    /// The answer is a similarity between pairs of inputs.
    pub compares_pairs: bool,
    /// The answer is the trajectory of a system stepped through time.
    pub steps_dynamics: bool,
    /// The answer is an expression rather than a number.
    pub rewrites_expressions: bool,
    /// The answer is a set of parameters fitted to data.
    pub fits_parameters: bool,
}

impl TaskStructure {
    /// The one class these facts describe, or a refusal naming why none or
    /// more than one does. No catch-all: a structure that fits nothing is a
    /// task the router has no business routing under a guess.
    pub fn classify(&self) -> Result<TaskClass> {
        let shapes: Vec<TaskClass> = [
            (self.graph_structured, TaskClass::GraphSearch),
            (self.predicts_ahead, TaskClass::Forecasting),
            (self.draws_samples, TaskClass::Sampling),
            (self.compares_pairs, TaskClass::KernelMethod),
            (self.steps_dynamics, TaskClass::Simulation),
            (self.rewrites_expressions, TaskClass::SymbolicSolving),
            (self.fits_parameters, TaskClass::ModelTraining),
        ]
        .into_iter()
        .filter_map(|(declared, class)| declared.then_some(class))
        .collect();
        match shapes.as_slice() {
            [] if self.minimises_objective => Ok(TaskClass::CombinatorialOptimisation),
            [] => Err(Error::invalid(
                "a task structure declares no shape for its answer, so no class fits it; an \
                 unclassified task is refused rather than routed under a guess",
            )),
            [class] => Ok(*class),
            several => Err(Error::invalid(format!(
                "a task structure describes its answer as {} at once; one answer has one \
                 shape, so the declaration contradicts itself and is refused",
                several
                    .iter()
                    .map(TaskClass::as_str)
                    .collect::<Vec<_>>()
                    .join(" and ")
            ))),
        }
    }
}

/// What a caller declares about a task (§11.2 step 2). Every field is an
/// `Option` so that [`Self::compile`] can refuse the missing one by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskSpec {
    /// A class the caller asserts. Optional where `structure` is given, and
    /// checked against it when both are.
    pub class: Option<TaskClass>,
    /// The answer's shape, from which [`Self::compile`] infers the class.
    pub structure: Option<TaskStructure>,
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
        let class = match (self.class, self.structure) {
            (declared, Some(structure)) => {
                let inferred = structure.classify()?;
                if let Some(declared) = declared
                    && declared != inferred
                {
                    return Err(Error::invalid(format!(
                        "a task declared as {} has the structure of {}; the compiler classifies \
                         from the structure, so fix whichever is wrong rather than letting one \
                         override the other",
                        declared.as_str(),
                        inferred.as_str()
                    )));
                }
                inferred
            }
            (declared, None) => required(declared, "a task class")?,
        };
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
    /// What the expected gain is worth, in USD micro-units: `expected_gain`
    /// times the policy's `value_per_unit_gain_micros` (R12-QUANT-4051). The
    /// dispatch is declined when this is below `cost_micros`, so a job is
    /// sent only where the improvement it is likely to bring pays for it.
    #[serde(default)]
    pub expected_value_micros: u64,
    /// What the arm had already been billed when the estimate was made, the
    /// figure the cumulative ceiling is checked against (R12-QUANT-10068).
    #[serde(default)]
    pub spent_before_micros: u64,
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
    /// What the quantum job this decision submitted was billed, in USD
    /// micro-units; zero when none was sent. A job is paid for when it is
    /// sent whether or not its answer is ever read, so this — not a
    /// measurement folded at harvest — is what the cumulative spend ceiling
    /// is rebuilt from on replay (R12-QUANT-10068, 9046).
    #[serde(default)]
    pub quantum_billed_micros: u64,
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
