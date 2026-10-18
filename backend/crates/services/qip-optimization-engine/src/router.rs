//! The compute router.
//!
//! Chooses which solver runs a problem, and enforces the platform's rule about
//! quantum methods: **no quantum result is used without a classical baseline
//! solved on the same problem, and the classical result wins ties**.
//!
//! That is enforced mechanically, not by convention. [`ComputeRouter::solve`]
//! always runs the classical solver, and the classical answer is the one every
//! cycle starts from. A quantum answer replaces it only when that answer is
//! feasible on the real problem and strictly better by more than a stated
//! margin. Everything else — the benchmark, the margin, the reason for the
//! choice, which substrates were declined and why — is in
//! [`RoutingDecision`], so a claim of advantage always arrives with the
//! evidence for it or not at all.
//!
//! # The quantum arm never blocks the cycle
//!
//! The router used to call the provider inline and wait for it. With the
//! simulator that was milliseconds; with [`qip_quantum::HostedProvider`] — the
//! same trait — it would have been a device queue the adapter itself declares
//! as thirty minutes, with the whole cycle stopped behind it. So the arm is
//! **submit-and-poll** through [`QuantumGateway`]:
//!
//! 1. Every solve computes the classical baseline first, as before.
//! 2. It then polls the one job it may have in flight. A job that has
//!    returned is verified classically — its claimed energy recomputed on the
//!    QUBO it was given, its weights re-scored on *this* cycle's problem —
//!    and adopted only if it beats *this* cycle's baseline by the margin. A
//!    job still pending changes nothing: the classical answer stands. A job
//!    past its patience is abandoned and its eventual answer discarded.
//! 3. Where the problem is eligible and the measured record justifies it, it
//!    submits a fresh job for the next cycle to judge, and returns at once.
//!
//! A quantum answer is therefore only ever adopted in a cycle *after* the one
//! that asked for it (ADR 0006, R12-QUANT-4029). It is re-scored on the
//! current problem rather than trusted from the old one, which is what makes
//! adopting a late answer honest: a selection of names that was good last
//! cycle is used only if it is still better today, on today's numbers. A job
//! is matched to a problem by its asset list and encoding; a universe that
//! changed in between makes the answer stale, and it is dropped unread.
//!
//! Which cycle adopts an answer depends on when a worker finishes, so it is
//! not a function of the inputs alone. That is why the dispatch record —
//! which job was harvested, after how many cycles, which was submitted — is
//! on the decision and reaches the journal: the log, not a re-run, is what
//! reproduces the choice.
//!
//! The default is classical. A deployment with no quantum backend configured
//! behaves identically to one whose backend is unreachable, which is the
//! behaviour an operator should be able to rely on.

use crate::compute::{
    CompiledTask, ComputeDecision, Formulation, InformationValue, Substrate, SubstrateRole,
    SubstrateVerdict, TaskClass, TaskSpec,
};
use crate::learning::{MeasurementOutcome, RoutingLearning, SolverMeasurement, SubstrateRecord};
use crate::problem::{PortfolioProblem, QuboEncoding};
use crate::research::{
    AdvantageBoundary, AdvantageEvidence, EnsembleFeature, ResearchDomain, selection_overlap,
};
use qip_core::error::{Error, Result};
use qip_core::rng::Xoshiro256;
use qip_core::time::Duration;
use qip_numerics::anneal::{AnnealSettings, Qubo, anneal, solve_exact};
use qip_numerics::qp::{QpSettings, solve as solve_qp};
use qip_quantum::benchmark::ClassicalValidator;
use qip_quantum::foundry::{FamilyKind, FamilyStatus, Foundry, JobClass};
use qip_quantum::gateway::{Delivery, GatewayLimits, JobPoll, JobTicket, QuantumGateway};
use qip_quantum::provider::QuantumProvider;
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use qip_quantum::registry::{SolverObservation, SolverRegistry, SolverStanding};
use qip_quantum::solver::{ClassicalSolver, QuantumInspiredSolver, SolverEffort, SolverKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// The worst constraint violation a run may carry and still be feasible.
/// One constant, read by the scorer and declared on the compiled task, so the
/// tolerance the record states is the one the comparison used.
pub const FEASIBILITY_TOLERANCE: f64 = 1e-6;

/// How far a provider's claimed QUBO energy may sit from the energy its own
/// assignment recomputes to. The same figure as
/// `qip_quantum::ClassicalValidator`.
pub const ENERGY_TOLERANCE: f64 = 1e-6;

/// Variables up to which the classical baseline is exhaustive.
const DEFAULT_EXACT_ENUMERATION_LIMIT: usize = 22;

/// Which solver produced an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Solver {
    /// Convex quadratic programming by ADMM.
    QuadraticProgram,
    /// Classical simulated annealing over the QUBO.
    SimulatedAnnealing,
    /// Exhaustive search. Exact, and only viable for small problems.
    ExactEnumeration,
    /// QAOA on a quantum provider.
    Quantum,
    /// Path-integral quantum annealing over the same QUBO, run through the
    /// solver registry. A classical algorithm on this CPU: never a quantum
    /// result, and never the baseline.
    QuantumInspired,
}

impl Solver {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::QuadraticProgram => "quadratic_program",
            Self::SimulatedAnnealing => "simulated_annealing",
            Self::ExactEnumeration => "exact_enumeration",
            Self::Quantum => "quantum",
            Self::QuantumInspired => "quantum_inspired",
        }
    }

    /// The substrate the solver runs on (§11.1).
    pub const fn substrate(&self) -> Substrate {
        match self {
            Self::QuadraticProgram | Self::SimulatedAnnealing | Self::ExactEnumeration => {
                Substrate::Cpu
            }
            Self::Quantum => Substrate::Qpu,
            Self::QuantumInspired => Substrate::QuantumInspired,
        }
    }

    pub const fn is_quantum(&self) -> bool {
        matches!(self, Self::Quantum)
    }

    /// Whether the solver finds the global optimum rather than a good point.
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::ExactEnumeration)
    }

    /// The compiled formulation a classical solver ran, or a refusal for the
    /// quantum solver, which is never a baseline.
    fn classical_formulation(&self) -> Result<Formulation> {
        match self {
            Self::QuadraticProgram => Ok(Formulation::QuadraticProgram),
            Self::SimulatedAnnealing => Ok(Formulation::SimulatedAnnealing),
            Self::ExactEnumeration => Ok(Formulation::ExhaustiveEnumeration),
            Self::Quantum => Err(Error::invalid(
                "the quantum solver cannot be the classical baseline (ADR 0006)",
            )),
            Self::QuantumInspired => Err(Error::invalid(
                "the quantum-inspired search is a candidate measured against the baseline, and a \
                 candidate measured against itself measures nothing (ADR 0006)",
            )),
        }
    }
}

/// The registry name of the quantum-inspired rung: the name
/// [`QuantumInspiredSolver`] reports, so the registry runs it by the name it
/// records it under.
pub const QUANTUM_INSPIRED_SOLVER: &str = "quantum-inspired-path-integral";

/// The §11.3 research domain every problem this router solves belongs to:
/// allocation is capital placement.
const ROUTED_DOMAIN: ResearchDomain = ResearchDomain::CapitalPlacement;

/// The win rate at or above which a measured QPU record is ahead more often
/// than not — the §11.5 boundary's second half.
const ADVANTAGE_WIN_RATE: f64 = 0.5;

fn default_advantage_minimum_trials() -> u32 {
    AdvantageBoundary::default().minimum_trials
}

/// One solver's attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverRun {
    pub solver: Solver,
    /// The weights found.
    pub weights: Vec<f64>,
    /// The problem's objective at those weights, comparable across solvers
    /// because every run is scored by the same function.
    pub objective: f64,
    /// Whether every constraint is satisfied.
    pub feasible: bool,
    /// The worst constraint violation, and which constraint.
    pub worst_violation: f64,
    pub violated_constraint: String,
    /// Iterations, sweeps or evaluations, depending on the solver.
    pub work: usize,
    /// Whether the run converged by its own criterion.
    pub converged: bool,
    /// If the run solved a relaxation or a coarsened problem rather than the
    /// real one, what was dropped.
    pub relaxations: Vec<String>,
}

impl SolverRun {
    /// Whether this run's answer may be used at all.
    pub fn is_usable(&self) -> bool {
        self.feasible && self.weights.iter().all(|w| w.is_finite())
    }
}

/// What the quantum arm did this solve: which job it judged, which it
/// submitted, what is still out, and what it cost (R12-QUANT-4053).
///
/// Latency is counted in cycles — the polls a job waited before its answer
/// was read — because the router reads no clock and a cycle is the unit in
/// which an answer's lateness costs anything. Cost is the provider's declared
/// price per job in USD micro-units, charged on submission: a job is billed
/// when it is sent, whether or not its answer is ever used.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuantumDispatch {
    /// The job whose answer was read this solve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvested: Option<u64>,
    /// How many cycles that job waited before its answer was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvested_after_cycles: Option<u32>,
    /// The job submitted this solve, for a later cycle to judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted: Option<u64>,
    /// A job still out after this solve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<u64>,
    /// The job given up on this solve because it ran past its patience: a
    /// trial that produced no answer, and so a failed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandoned: Option<u64>,
    /// The job the gateway no longer holds: also an outcome, and a failed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<u64>,
    /// What the job submitted this solve cost, in USD micro-units.
    #[serde(default)]
    pub cost_micros: u64,
    /// What happened, in order, in the router's own words.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Why the router chose what it chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Every run, including the ones not chosen.
    pub runs: Vec<SolverRun>,
    /// The solver whose answer is returned.
    pub chosen: Solver,
    /// The weights returned.
    pub weights: Vec<f64>,
    /// The objective of the returned answer.
    pub objective: f64,
    /// The classical baseline's objective, always present.
    pub classical_objective: f64,
    /// The reason, in a sentence, for a decision record.
    pub rationale: String,
    /// Whether a quantum path was eligible at all, and why or why not.
    pub quantum_note: String,
    /// What the asynchronous quantum arm did this solve.
    #[serde(default)]
    pub quantum_dispatch: QuantumDispatch,
    /// The compiled task and one verdict per declared substrate. `None` only
    /// on a record written before the compiler existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<ComputeDecision>,
}

impl RoutingDecision {
    /// The improvement the chosen answer gives over the classical baseline, as
    /// a fraction. Negative would mean the classical answer was better, which
    /// the router does not allow.
    pub fn improvement_over_classical(&self) -> f64 {
        improvement(self.classical_objective, self.objective)
    }

    /// Whether a quantum advantage was actually measured on this problem.
    ///
    /// The only statement about quantum performance the platform will make,
    /// and it is per-problem: a method that helped once has not been shown to
    /// help in general.
    pub fn measured_quantum_advantage(&self) -> Option<f64> {
        if !self.chosen.is_quantum() {
            return None;
        }
        Some(self.improvement_over_classical())
    }

    pub fn run_for(&self, solver: Solver) -> Option<&SolverRun> {
        self.runs.iter().find(|run| run.solver == solver)
    }
}

/// Objectives are minimised, so an improvement is a decrease, as a fraction
/// of the baseline. Zero against a zero baseline rather than a division.
fn improvement(baseline: f64, objective: f64) -> f64 {
    if baseline.abs() < 1e-12 {
        return 0.0;
    }
    (baseline - objective) / baseline.abs()
}

fn default_minimum_win_probability() -> f64 {
    0.05
}

fn default_patience() -> u32 {
    GatewayLimits::default().patience_polls
}

/// How the router decides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Assets below which a quantum path is never attempted.
    ///
    /// The default is one more than [`Self::exact_enumeration_limit`], and
    /// that is the reason for it: at or below the enumeration limit the
    /// classical baseline is exhaustive and provably optimal, so a quantum
    /// answer can at best tie and a tie is never adopted. It used to be fifty,
    /// which was more than the in-tree simulator's 24 qubits — so with the
    /// default policy no problem was ever both large enough and small enough,
    /// and the quantum arm `quantum_enabled` switched on could never run.
    /// [`ComputeRouter::with_quantum`] now refuses that combination rather
    /// than accepting a provider it can never call.
    pub minimum_assets_for_quantum: usize,
    /// Whether a quantum path may be attempted on a continuous problem.
    ///
    /// False by design: a convex continuous problem has a classical global
    /// optimum, and there is nothing for a heuristic to improve on.
    pub quantum_on_continuous: bool,
    /// Fractional improvement a quantum answer must show over the classical
    /// baseline to be used.
    ///
    /// Non-zero on purpose. A quantum result that ties is not evidence of
    /// anything, and preferring it would let noise decide which solver the
    /// platform reports as better.
    pub quantum_margin: f64,
    /// Variables below which exact enumeration is used as the baseline.
    ///
    /// 2²² is four million evaluations, which is a second or so, and it gives
    /// the router a *provably* optimal baseline to measure against instead of
    /// another heuristic's guess.
    pub exact_enumeration_limit: usize,
    /// Wall time the task is declared to have, recorded as the compiled
    /// task's deadline.
    pub time_budget: Duration,
    /// How much work the quantum path may do.
    ///
    /// Carried here rather than hard-coded in the solver call, because the
    /// depth and the angle-search budget are the two levers that decide what a
    /// QAOA attempt costs, and a caller paying for device time should be the
    /// one setting them.
    pub qaoa: QaoaSettings,
    /// The measured win rate below which the quantum arm stops being tried at
    /// a problem size (R12-QUANT-4047). With one win and one loss of prior an
    /// untried size reads as one half, and the default of 5% is crossed after
    /// nineteen straight non-wins — a substrate that consistently loses stops
    /// being paid for.
    #[serde(default = "default_minimum_win_probability")]
    pub minimum_win_probability: f64,
    /// The most one quantum job may cost, in USD micro-units.
    ///
    /// Zero by default, which admits only a free provider — the in-tree
    /// simulator. A hosted device declares a price, and spending on it is a
    /// budget an operator sets rather than one the router assumes.
    #[serde(default)]
    pub max_quantum_job_cost_micros: u64,
    /// Cycles a quantum job may stay pending before it is abandoned.
    #[serde(default = "default_patience")]
    pub quantum_patience_cycles: u32,
    /// Whether the quantum-inspired rung may run (§11.1). Off by default:
    /// a deployment opts in at its composition root, and a router nobody
    /// configured behaves as it always has.
    ///
    /// The rung runs only where the quantum arm could — a discrete task whose
    /// classical baseline is a heuristic — because against an exhaustive
    /// baseline a candidate can at best tie, and a tie is never adopted. It
    /// runs synchronously, in process, at no declared price; its answer is
    /// verified classically and adopted only by the same margin rule.
    #[serde(default)]
    pub quantum_inspired: bool,
    /// Trials at a problem size before a QPU win rate can count as advantage
    /// (§11.5).
    #[serde(default = "default_advantage_minimum_trials")]
    pub advantage_minimum_trials: u32,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            minimum_assets_for_quantum: DEFAULT_EXACT_ENUMERATION_LIMIT + 1,
            quantum_on_continuous: false,
            quantum_margin: 0.01,
            exact_enumeration_limit: DEFAULT_EXACT_ENUMERATION_LIMIT,
            time_budget: Duration::from_secs(60),
            qaoa: QaoaSettings::default(),
            minimum_win_probability: default_minimum_win_probability(),
            max_quantum_job_cost_micros: 0,
            quantum_patience_cycles: default_patience(),
            quantum_inspired: false,
            advantage_minimum_trials: default_advantage_minimum_trials(),
        }
    }
}

impl RoutingPolicy {
    /// Refuse a policy that cannot mean what it says.
    pub fn validate(&self) -> Result<()> {
        if self.minimum_assets_for_quantum == 0 {
            return Err(Error::invalid(
                "minimum_assets_for_quantum is zero; a problem with no assets has nothing to \
                 encode, so set the smallest size a quantum attempt is worth",
            ));
        }
        if !self.quantum_margin.is_finite() || self.quantum_margin < 0.0 {
            return Err(Error::invalid(format!(
                "a quantum margin of {} is not a non-negative finite fraction; a negative margin \
                 would adopt a quantum answer worse than the classical baseline",
                self.quantum_margin
            )));
        }
        if self.exact_enumeration_limit > 30 {
            return Err(Error::invalid(format!(
                "an exact enumeration limit of {} asks for more than 2^30 evaluations per cycle; \
                 that is not a baseline, it is a stalled cycle — use 22 or less",
                self.exact_enumeration_limit
            )));
        }
        if self.time_budget.is_zero() {
            return Err(Error::invalid(
                "a zero time budget is a deadline that expires before any solver starts",
            ));
        }
        if !self.minimum_win_probability.is_finite()
            || !(0.0..1.0).contains(&self.minimum_win_probability)
        {
            return Err(Error::invalid(format!(
                "minimum_win_probability {} is outside [0, 1); at one or above the quantum arm \
                 could never be tried, which is a disabled arm reading as an enabled one",
                self.minimum_win_probability
            )));
        }
        if self.quantum_patience_cycles == 0 {
            return Err(Error::invalid(
                "quantum_patience_cycles is zero, which abandons every job before it can return",
            ));
        }
        if self.advantage_minimum_trials == 0 {
            return Err(Error::invalid(
                "advantage_minimum_trials is zero, which would let one lucky answer count as \
                 quantum advantage; set the number of trials a win rate needs to mean anything",
            ));
        }
        Ok(())
    }

    fn advantage_boundary(&self) -> AdvantageBoundary {
        AdvantageBoundary {
            minimum_trials: self.advantage_minimum_trials,
            minimum_win_rate: ADVANTAGE_WIN_RATE,
        }
    }

    /// Refuse a policy that can never send a problem to this provider.
    fn coherent_with(&self, provider: &dyn QuantumProvider) -> Result<()> {
        let capabilities = provider.capabilities();
        if self.minimum_assets_for_quantum > capabilities.max_qubits {
            return Err(Error::invalid(format!(
                "the routing policy attempts quantum only from {} assets, and {} holds at most {} \
                 qubits: no problem is both large enough and small enough, so the quantum arm \
                 could never run. Lower minimum_assets_for_quantum to at most {} or attach a \
                 larger provider — the router refuses rather than carrying a provider it can \
                 never call",
                self.minimum_assets_for_quantum,
                provider.name(),
                capabilities.max_qubits,
                capabilities.max_qubits
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PendingJob {
    ticket: JobTicket,
    /// Asset list and encoding: what a later problem must share for this
    /// job's answer to be about it.
    key: String,
    /// The QUBO the provider was given, which its claimed energy is checked
    /// against.
    qubo: Qubo,
    /// What the job was billed when it was sent, carried to the measurement
    /// of its outcome so the run is costed where it is judged.
    cost_micros: u64,
    /// The win probability the router estimated when it sent the job, which
    /// the outcome scores the router's calibration against.
    predicted_win_probability: f64,
}

#[derive(Debug, Default)]
struct ArmState {
    pending: Option<PendingJob>,
    spent_micros: u64,
}

/// What the router has measured, held behind one lock: the learned routing
/// state per substrate and size, and the solver registry's standings per
/// solver. Updated together by [`Learned::absorb`] and by nothing else, so
/// the two views cannot disagree about which runs happened.
#[derive(Debug)]
struct Learned {
    learning: RoutingLearning,
    registry: SolverRegistry,
}

impl Learned {
    fn new(seed: u64) -> Self {
        // The registry holds the one rung it runs itself — the
        // quantum-inspired search — beside a classical searcher it never
        // selects for the router (the router's baseline is its own, solved
        // first), and declares the router's own rungs so each run's outcome
        // has a name to be recorded against.
        let registry = SolverRegistry::builder(Arc::new(ClassicalSolver::annealing(
            seed,
            AnnealSettings::default(),
        )))
        .with_solver(Arc::new(QuantumInspiredSolver::new(seed)))
        .with_declared(Solver::QuadraticProgram.as_str(), SolverKind::Classical)
        .with_declared(Solver::SimulatedAnnealing.as_str(), SolverKind::Classical)
        .with_declared(Solver::ExactEnumeration.as_str(), SolverKind::Classical)
        .with_declared(Solver::Quantum.as_str(), SolverKind::Quantum)
        .build();
        Self {
            learning: RoutingLearning::default(),
            registry,
        }
    }

    /// Fold one decision's measurements into both views, as a replay does.
    ///
    /// The live solve applies the same two per-measurement folds —
    /// [`RoutingLearning::absorb`] and [`Self::record_registry`] — but folds
    /// a harvested QPU outcome into the learning before it estimates the
    /// next dispatch, so the estimate already knows how the last job ended.
    /// Both folds are per measurement and order-free within a decision, so
    /// the state a replay of the whole decision reaches is the state the
    /// live solve reached.
    fn absorb(&mut self, measurements: &[SolverMeasurement]) -> Result<()> {
        for measurement in measurements {
            self.record_registry(measurement)?;
        }
        self.learning.absorb(measurements);
        Ok(())
    }

    fn record_registry(&mut self, measurement: &SolverMeasurement) -> Result<()> {
        self.registry.record(
            &measurement.solver,
            &SolverObservation {
                problem_size: measurement.problem_size,
                quality: if measurement.quality.is_finite() {
                    measurement.quality
                } else {
                    0.0
                },
                verified: measurement.verified,
                won: measurement.used,
                cost_micros: measurement.cost_micros,
                latency_cycles: measurement.latency_cycles,
            },
        )
    }
}

/// Refuse a promotion threshold that is not positive and finite, rather than
/// clamping it. [`Foundry::register`] already refuses one, but the router is the
/// reader that acts on the number, and a threshold of zero promotes a tie
/// (R12-QUANT-4028), so it checks what it reads.
fn checked_threshold(threshold: f64) -> Result<f64> {
    if threshold.is_finite() && threshold > 0.0 {
        Ok(threshold)
    } else {
        Err(Error::invalid(format!(
            "the foundry's promotion threshold {threshold} is not positive and finite; a zero or \
             negative margin lets noise decide which solver is reported better, so register a \
             positive threshold for the workload"
        )))
    }
}

/// The name the registry records a router rung under.
fn registry_name(solver: Solver) -> &'static str {
    match solver {
        Solver::QuantumInspired => QUANTUM_INSPIRED_SOLVER,
        other => other.as_str(),
    }
}

/// A QPU job's outcome as a measurement. `judged` is `(feasible, (beat the
/// baseline by the margin, improvement))` where an answer was scored.
fn quantum_measurement(
    pending: &PendingJob,
    outcome: MeasurementOutcome,
    judged: Option<(bool, (bool, f64))>,
    latency_cycles: u32,
) -> SolverMeasurement {
    let (verified, beat_baseline, quality) = match judged {
        Some((feasible, (beat, gain))) => (feasible, beat, if feasible { gain } else { 0.0 }),
        None => (false, false, 0.0),
    };
    SolverMeasurement {
        solver: Solver::Quantum.as_str().to_string(),
        substrate: Substrate::Qpu,
        problem_size: pending.qubo.n,
        outcome,
        verified,
        beat_baseline,
        used: false,
        quality,
        cost_micros: pending.cost_micros,
        latency_cycles,
        predicted_win_probability: Some(pending.predicted_win_probability),
    }
}

/// What the solve hands the decision record beyond the verdict inputs.
struct Extras {
    inspired_note: String,
    inspired_information_value: Option<InformationValue>,
    measurements: Vec<SolverMeasurement>,
    ensemble: Vec<EnsembleFeature>,
    advantage: crate::research::AdvantageStanding,
}

#[derive(Debug)]
struct QuantumArm {
    gateway: QuantumGateway,
    state: Mutex<ArmState>,
}

impl QuantumArm {
    fn new(provider: Arc<dyn QuantumProvider>, policy: &RoutingPolicy) -> Result<Self> {
        Ok(Self {
            gateway: QuantumGateway::new(
                provider,
                GatewayLimits {
                    max_in_flight: 1,
                    patience_polls: policy.quantum_patience_cycles,
                },
            )?,
            state: Mutex::new(ArmState::default()),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, ArmState>> {
        self.state.lock().map_err(|_| {
            Error::guard(
                "the quantum arm's state was poisoned by a panicking holder; the classical \
                 baseline cannot be compared against a record that may be half-written",
            )
        })
    }
}

/// Routes a problem to a solver.
#[derive(Debug)]
pub struct ComputeRouter {
    policy: RoutingPolicy,
    quantum: Option<QuantumArm>,
    learned: Mutex<Learned>,
    seed: u64,
    /// The Quantum Foundry whose registered per-workload promotion threshold
    /// the adoption decision reads (R12-QUANT-4028). `None` until a quantum
    /// provider is attached or a foundry is supplied; then the platform's own.
    foundry: Option<Foundry>,
}

impl ComputeRouter {
    /// A classical-only router. The default configuration.
    pub fn classical(seed: u64) -> Self {
        Self {
            policy: RoutingPolicy::default(),
            quantum: None,
            learned: Mutex::new(Learned::new(seed)),
            seed,
            foundry: None,
        }
    }

    fn learned(&self) -> Result<MutexGuard<'_, Learned>> {
        self.learned.lock().map_err(|_| {
            Error::guard(
                "the router's learned routing state was poisoned by a panicking holder; a \
                 dispatch estimated from a half-written record is a guess, so the solve stops",
            )
        })
    }

    /// Fold recorded decisions into the router's learning and its solver
    /// registry, as a restarted process does over its journal
    /// (R12-QUANT-4047). The same fold the live solve applies, so a replayed
    /// router holds what the live one held.
    ///
    /// Refuses a measurement naming a solver this router does not know,
    /// rather than skipping it: a record the router cannot account for is a
    /// record written by a different router, and learning half of a history
    /// is a history nobody wrote.
    pub fn replay<'a>(
        &self,
        decisions: impl IntoIterator<Item = &'a ComputeDecision>,
    ) -> Result<usize> {
        let mut learned = self.learned()?;
        let mut folded = 0usize;
        for decision in decisions {
            learned.absorb(&decision.measurements)?;
            folded += 1;
        }
        Ok(folded)
    }

    /// The learned record for a substrate at a problem size.
    pub fn learned_record(
        &self,
        substrate: Substrate,
        problem_size: usize,
    ) -> Result<SubstrateRecord> {
        Ok(self.learned()?.learning.record(substrate, problem_size))
    }

    /// The solver registry's measured standing for a solver, by the name the
    /// registry records it under, or `None` before its first run.
    pub fn solver_standing(&self, solver: &str) -> Result<Option<SolverStanding>> {
        Ok(self.learned()?.registry.standing(solver).copied())
    }

    /// Every standing the registry holds, in name order.
    pub fn solver_standings(&self) -> Result<BTreeMap<String, SolverStanding>> {
        Ok(self.learned()?.registry.standings().clone())
    }

    /// Replace the policy, refusing one that is invalid on its own or that
    /// can never reach an already attached provider.
    pub fn with_policy(mut self, policy: RoutingPolicy) -> Result<Self> {
        policy.validate()?;
        if let Some(arm) = self.quantum.take() {
            let provider = Arc::clone(arm.gateway.provider());
            policy.coherent_with(provider.as_ref())?;
            self.quantum = Some(QuantumArm::new(provider, &policy)?);
        }
        self.policy = policy;
        Ok(self)
    }

    /// Attach a quantum provider behind a gateway. It is still only used
    /// where the policy permits and only when it wins by the margin, and it is
    /// never called on the caller's thread.
    ///
    /// Refused when the policy's size threshold exceeds the provider's
    /// qubits, and when the foundry does not register the portfolio
    /// allocation family — the router has no baseline, objective or margin
    /// for any other, and a family the foundry records as not built must not
    /// acquire a quantum path by being attached here.
    pub fn with_quantum(mut self, provider: Arc<dyn QuantumProvider>) -> Result<Self> {
        self.policy.coherent_with(provider.as_ref())?;
        let foundry = match self.foundry.take() {
            Some(foundry) => foundry,
            None => Foundry::platform()?,
        };
        match foundry.status(FamilyKind::PortfolioAllocation) {
            Some(FamilyStatus::Registered(family)) => {
                checked_threshold(family.promotion_threshold())?;
                if family.encoding() != PortfolioProblem::QUBO_ENCODER {
                    return Err(Error::denied(format!(
                        "the quantum foundry registers portfolio allocation with the encoding \
                         `{}`, but the router's quantum arm encodes through `{}`; register the \
                         family with the encoder the router runs",
                        family.encoding(),
                        PortfolioProblem::QUBO_ENCODER
                    )));
                }
            }
            other => {
                return Err(Error::denied(format!(
                    "the quantum foundry does not register portfolio allocation (it reports \
                     {other:?}), so the router has no admitted family to route a quantum job \
                     under; register the family with its baseline and margin first"
                )));
            }
        }
        self.quantum = Some(QuantumArm::new(provider, &self.policy)?);
        self.foundry = Some(foundry);
        Ok(self)
    }

    /// Replace the foundry the promotion threshold is read from.
    ///
    /// Attach it before [`Self::with_quantum`] and an unregistered portfolio
    /// family is refused there. Replace it after, and a foundry that no longer
    /// registers the family is not an error: every solve then runs the
    /// classical baseline alone and records why in `quantum_note`, because a
    /// quantum answer with no admitted threshold to clear has nothing to be
    /// judged against.
    pub fn with_foundry(mut self, foundry: Foundry) -> Self {
        self.foundry = Some(foundry);
        self
    }

    /// The margin a quantum answer must beat the baseline by, as the adoption
    /// decision will read it: see [`Self::margin_for`] for the precedence.
    /// `None` when the foundry admits no portfolio-allocation workload.
    ///
    /// Public so a test at the composition seam can prove which number the
    /// router reads, rather than inferring it from a decision.
    pub fn quantum_promotion_margin(&self) -> Result<Option<f64>> {
        self.margin_for(Solver::Quantum)
    }

    /// The margin a candidate from `solver` must beat the baseline by, or
    /// `None` when the foundry admits no workload for it.
    ///
    /// Precedence: the stricter of [`RoutingPolicy::quantum_margin`] and the
    /// foundry's registered threshold. The policy margin is a deployment-wide
    /// floor an operator may raise; the foundry threshold is the workload's
    /// own admission condition. Taking the larger can only make adoption
    /// rarer, so neither source can loosen the other, and a policy margin of
    /// zero (which `validate` allows) can no longer promote a tie.
    ///
    /// The quantum-inspired rung is held to the same threshold when the family
    /// is registered; when it is not, only the policy margin applies, because
    /// that rung runs in process and needs no quantum provider's admission.
    fn margin_for(&self, solver: Solver) -> Result<Option<f64>> {
        let registered = match &self.foundry {
            Some(foundry) => match foundry.status(FamilyKind::PortfolioAllocation) {
                Some(FamilyStatus::Registered(family)) => {
                    Some(checked_threshold(family.promotion_threshold())?)
                }
                _ => None,
            },
            None => None,
        };
        Ok(match (solver, registered) {
            (_, Some(threshold)) => Some(threshold.max(self.policy.quantum_margin)),
            (Solver::QuantumInspired, None) => Some(self.policy.quantum_margin),
            (_, None) => None,
        })
    }

    pub fn policy(&self) -> RoutingPolicy {
        self.policy
    }

    /// The gateway in front of the attached provider, for an operator's view
    /// of its counters.
    pub fn quantum_gateway(&self) -> Option<&QuantumGateway> {
        self.quantum.as_ref().map(|arm| &arm.gateway)
    }

    /// Whether the job in flight has an answer waiting, without reading it or
    /// counting a cycle against its patience. `None` when no job is out.
    ///
    /// Never blocks. A test uses it to wait for a worker before driving the
    /// next solve; the cycle has no reason to call it.
    pub fn quantum_job_ready(&self) -> Result<Option<bool>> {
        let Some(arm) = &self.quantum else {
            return Ok(None);
        };
        let state = arm.lock()?;
        match &state.pending {
            Some(pending) => Ok(Some(arm.gateway.is_ready(pending.ticket)?)),
            None => Ok(None),
        }
    }

    /// Total declared cost of every job this router has submitted, in USD
    /// micro-units.
    pub fn quantum_spend_micros(&self) -> Result<u64> {
        match &self.quantum {
            Some(arm) => Ok(arm.lock()?.spent_micros),
            None => Ok(0),
        }
    }

    /// Whether a quantum path would be attempted for this problem, and why.
    ///
    /// Public so a caller can ask before committing to the cost, and so the
    /// reasoning is testable independently of running anything.
    pub fn quantum_assessment(&self, problem: &PortfolioProblem) -> (bool, String) {
        let Some(arm) = &self.quantum else {
            return (
                false,
                "no quantum provider is configured; the classical solver is the default"
                    .to_string(),
            );
        };
        let provider = arm.gateway.provider();
        if !provider.is_available() {
            return (
                false,
                format!(
                    "the {} provider is not reachable: {}",
                    provider.name(),
                    provider.requirement()
                ),
            );
        }
        if !problem.is_discrete() && !self.policy.quantum_on_continuous {
            return (
                false,
                "the problem is continuous and convex, so the classical solver finds the global optimum and there is nothing to improve on"
                    .to_string(),
            );
        }
        if problem.dimension() < self.policy.minimum_assets_for_quantum {
            return (
                false,
                format!(
                    "{} assets is below the {} at which a quantum formulation could pay; the encoding alone would cost more than the classical solve",
                    problem.dimension(),
                    self.policy.minimum_assets_for_quantum
                ),
            );
        }
        match self.margin_for(Solver::Quantum) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    false,
                    "the quantum foundry registers no portfolio-allocation workload, so there is no \
                     promotion threshold for a quantum answer to clear; the classical baseline stands"
                        .to_string(),
                );
            }
            Err(error) => return (false, error.message().to_string()),
        }
        let capabilities = provider.capabilities();
        if problem.dimension() > capabilities.max_qubits {
            return (
                false,
                format!(
                    "{} variables exceeds the provider's {} qubits",
                    problem.dimension(),
                    capabilities.max_qubits
                ),
            );
        }
        let note = if capabilities.simulated {
            format!(
                "attempting QAOA on {}, a simulator: a result here validates the formulation and is not evidence of advantage",
                provider.name()
            )
        } else {
            format!("attempting QAOA on {}", provider.name())
        };
        (true, note)
    }

    /// Solve, always producing a classical baseline, and never waiting on a
    /// quantum provider.
    pub fn solve(&self, problem: &PortfolioProblem) -> Result<RoutingDecision> {
        problem.validate()?;
        let size = problem.dimension();
        let mut runs = Vec::new();

        // The classical baseline is not optional. It is the answer this cycle
        // uses unless a verified candidate beats it.
        let classical = self.solve_classical(problem)?;
        let classical_objective = classical.objective;
        let classical_solver = classical.solver;
        let classical_weights = classical.weights.clone();
        let baseline_weights = classical.weights.clone();
        let classical_usable = classical.is_usable();
        runs.push(classical);

        let task = self.compile(problem, classical_solver)?;
        let (attempt_quantum, quantum_note) = self.quantum_assessment(problem);
        let mut dispatch = QuantumDispatch::default();
        let mut quantum_failure: Option<String> = None;
        let mut information_value: Option<InformationValue> = None;
        let mut qpu_declined = quantum_note.clone();
        // Every run this solve accounts for, the baseline first. `used` is
        // settled once the selection is known.
        let mut measurements = vec![SolverMeasurement {
            solver: classical_solver.as_str().to_string(),
            substrate: Substrate::Cpu,
            problem_size: size,
            outcome: MeasurementOutcome::Answered,
            verified: classical_usable,
            beat_baseline: false,
            used: false,
            quality: 0.0,
            cost_micros: 0,
            latency_cycles: 0,
            predicted_win_probability: None,
        }];
        let mut learned = self.learned()?;

        if let Some(arm) = &self.quantum {
            let mut state = arm.lock()?;
            let key = self.problem_key(problem);

            // Harvest: read the one job that may be out, without waiting.
            if let Some(pending) = state.pending.take() {
                let mut harvested: Option<SolverMeasurement> = None;
                match arm.gateway.poll(pending.ticket) {
                    Ok(JobPoll::Pending { polls }) => {
                        dispatch.pending = Some(pending.ticket.id());
                        dispatch.notes.push(format!(
                            "quantum job {} is still out after {polls} cycle(s); the classical \
                             baseline stands",
                            pending.ticket.id()
                        ));
                        qpu_declined = format!(
                            "quantum job {} is still out, and one job is asked per question",
                            pending.ticket.id()
                        );
                        state.pending = Some(pending);
                    }
                    Ok(JobPoll::Abandoned { polls }) => {
                        dispatch.abandoned = Some(pending.ticket.id());
                        dispatch.notes.push(format!(
                            "quantum job {} was abandoned after {polls} cycle(s) without an \
                             answer; whatever it returns later is discarded, and the classical \
                             baseline stands",
                            pending.ticket.id()
                        ));
                        // An answer that never came is a trial without a win.
                        harvested = Some(quantum_measurement(
                            &pending,
                            MeasurementOutcome::Abandoned,
                            None,
                            polls,
                        ));
                    }
                    Ok(JobPoll::Completed { polls, result }) => {
                        dispatch.harvested = Some(pending.ticket.id());
                        dispatch.harvested_after_cycles = Some(polls);
                        // `polls` counts the polls that found the job still
                        // running, so an answer ready at the first harvest
                        // reads zero — what an inline call would read. It was
                        // harvested a cycle after the one that sent it, and
                        // the learning's latency is cycles since submission,
                        // so the harvesting cycle counts.
                        let latency = polls.saturating_add(1);
                        if pending.key != key {
                            dispatch.notes.push(format!(
                                "quantum job {} answered a problem over a different universe or \
                                 encoding; the answer is stale and is dropped unread",
                                pending.ticket.id()
                            ));
                            harvested = Some(quantum_measurement(
                                &pending,
                                MeasurementOutcome::Stale,
                                None,
                                latency,
                            ));
                        } else {
                            let simulated = arm.gateway.provider().capabilities().simulated;
                            match self.judge(problem, &pending.qubo, result, simulated) {
                                Ok(run) => {
                                    let judged = self.adopts(&run, classical_objective)?;
                                    harvested = Some(quantum_measurement(
                                        &pending,
                                        MeasurementOutcome::Answered,
                                        Some((run.is_usable(), judged)),
                                        latency,
                                    ));
                                    dispatch.notes.push(format!(
                                        "quantum job {} answered after {polls} cycle(s) and was \
                                         re-scored on this cycle's problem",
                                        pending.ticket.id()
                                    ));
                                    runs.push(run);
                                }
                                Err(error) => {
                                    harvested = Some(quantum_measurement(
                                        &pending,
                                        MeasurementOutcome::Refused,
                                        None,
                                        latency,
                                    ));
                                    quantum_failure = Some(error.message().to_string());
                                }
                            }
                        }
                    }
                    Err(error) => {
                        dispatch.lost = Some(pending.ticket.id());
                        dispatch.notes.push(format!(
                            "the gateway no longer holds quantum job {}: {}",
                            pending.ticket.id(),
                            error.message()
                        ));
                        harvested = Some(quantum_measurement(
                            &pending,
                            MeasurementOutcome::Lost,
                            None,
                            0,
                        ));
                    }
                }
                // Folded into the learning before the dispatch below reads
                // it, so the estimate this cycle makes already knows how the
                // last job ended. The registry is updated at the end with
                // every other run, once `used` is known; the fold is
                // per-measurement, so the order changes nothing a replay of
                // the whole decision would not also reach.
                if let Some(measurement) = harvested {
                    learned.learning.absorb(std::slice::from_ref(&measurement));
                    measurements.push(measurement);
                }
            }

            // Dispatch: one fresh job for a later cycle, where the estimate
            // clears the policy's bars.
            if attempt_quantum && state.pending.is_none() {
                let record = learned.learning.record(Substrate::Qpu, size);
                let cost = arm.gateway.provider().capabilities().cost_per_job_micros;
                let (estimate, refusal) =
                    self.estimate(Substrate::Qpu, record, task.classical, cost);
                let predicted = estimate.win_probability;
                information_value = Some(estimate);
                match refusal {
                    Some(reason) => qpu_declined = reason,
                    None => {
                        let encoding = self.encoding_for(problem);
                        let qubo = problem.to_qubo(&encoding)?;
                        match arm.gateway.submit(
                            JobClass::Optimisation.name(),
                            Delivery::Polled,
                            qubo.clone(),
                            self.policy.qaoa,
                        ) {
                            Ok(ticket) => {
                                dispatch.submitted = Some(ticket.id());
                                dispatch.pending = Some(ticket.id());
                                dispatch.cost_micros = cost;
                                state.spent_micros = state.spent_micros.saturating_add(cost);
                                dispatch.notes.push(format!(
                                    "submitted quantum job {}; a later cycle judges its answer \
                                     against that cycle's baseline",
                                    ticket.id()
                                ));
                                state.pending = Some(PendingJob {
                                    ticket,
                                    key,
                                    qubo,
                                    cost_micros: cost,
                                    predicted_win_probability: predicted,
                                });
                            }
                            Err(error) => {
                                qpu_declined =
                                    format!("the gateway refused the job: {}", error.message());
                                dispatch.notes.push(qpu_declined.clone());
                            }
                        }
                    }
                }
            }
        }

        // The quantum-inspired rung (§11.1): synchronous, in process, on the
        // same QUBO, and only where its estimate clears the same bars.
        let mut inspired_information_value: Option<InformationValue> = None;
        let inspired_note: String;
        let mut inspired_measurement: Option<SolverMeasurement> = None;
        if !self.policy.quantum_inspired {
            inspired_note =
                "the quantum-inspired rung is not enabled in this deployment's routing \
                             policy; path-integral annealing stays benchmarked in qip-quantum"
                    .to_string();
        } else if task.quantum.is_none() {
            inspired_note = "the compiled task has no QUBO formulation for the quantum-inspired \
                             search to run on"
                .to_string();
        } else {
            let record = learned.learning.record(Substrate::QuantumInspired, size);
            let (estimate, refusal) =
                self.estimate(Substrate::QuantumInspired, record, task.classical, 0);
            let predicted = estimate.win_probability;
            inspired_information_value = Some(estimate);
            match refusal {
                Some(reason) => inspired_note = reason,
                None => {
                    let (run, measurement, note) = self.run_quantum_inspired(
                        &learned.registry,
                        problem,
                        classical_objective,
                        predicted,
                    )?;
                    inspired_note = note;
                    inspired_measurement = Some(measurement);
                    if let Some(run) = run {
                        runs.push(run);
                    }
                }
            }
        }

        // Selection. The classical answer wins unless a candidate is
        // feasible and better by more than the margin.
        let quantum_run = runs.iter().find(|run| run.solver.is_quantum()).cloned();
        let (mut chosen, mut weights, mut objective, mut rationale) = match (
            quantum_failure,
            quantum_run,
        ) {
            (Some(failure), _) => (
                classical_solver,
                classical_weights,
                classical_objective,
                format!("the quantum attempt failed ({failure}), so the classical baseline stands"),
            ),
            (None, Some(quantum)) if !quantum.is_usable() => (
                classical_solver,
                classical_weights,
                classical_objective,
                format!(
                    "the quantum answer violated {} by {:.4}, so the classical baseline stands",
                    quantum.violated_constraint, quantum.worst_violation
                ),
            ),
            (None, Some(quantum)) => {
                let (adopt, gain) = self.adopts(&quantum, classical_objective)?;
                let margin = self.margin_for(Solver::Quantum)?;
                if adopt {
                    (
                        Solver::Quantum,
                        quantum.weights.clone(),
                        quantum.objective,
                        format!(
                            "the quantum answer improved on the classical baseline by {:.2}%, above the {:.2}% margin required",
                            gain * 100.0,
                            margin.unwrap_or(0.0) * 100.0
                        ),
                    )
                } else if let Some(margin) = margin {
                    (
                        classical_solver,
                        classical_weights,
                        classical_objective,
                        format!(
                            "the quantum answer was {:.2}% better, inside the {:.2}% margin, so the classical baseline stands: a tie is not evidence",
                            gain * 100.0,
                            margin * 100.0
                        ),
                    )
                } else {
                    (
                        classical_solver,
                        classical_weights,
                        classical_objective,
                        "the quantum foundry registers no portfolio-allocation workload, so a \
                         quantum answer has no promotion threshold to clear and the classical \
                         baseline stands"
                            .to_string(),
                    )
                }
            }
            (None, None) => (
                classical_solver,
                classical_weights,
                classical_objective,
                if classical_usable {
                    format!("solved classically by {}", classical_solver.as_str())
                } else {
                    format!(
                        "no feasible solution found; the best {} result is returned and is infeasible",
                        classical_solver.as_str()
                    )
                },
            ),
        };
        // The quantum-inspired candidate is held to the same rule, and
        // replaces whatever stands only by beating the baseline by the margin
        // *and* the standing answer outright.
        if let Some(inspired) = runs
            .iter()
            .find(|run| run.solver == Solver::QuantumInspired)
            .cloned()
        {
            let (adopt, gain) = self.adopts(&inspired, classical_objective)?;
            let inspired_margin = self.margin_for(Solver::QuantumInspired)?.unwrap_or(0.0);
            if adopt && inspired.objective < objective {
                rationale = format!(
                    "the quantum-inspired answer improved on the classical baseline by {:.2}%, \
                     above the {:.2}% margin required{}",
                    gain * 100.0,
                    inspired_margin * 100.0,
                    if chosen.is_quantum() {
                        ", and beat the quantum answer outright"
                    } else {
                        ""
                    }
                );
                chosen = Solver::QuantumInspired;
                weights = inspired.weights.clone();
                objective = inspired.objective;
            }
        }

        // Settle which run was used, then fold everything not yet folded.
        if let Some(measurement) = inspired_measurement {
            measurements.push(measurement);
        }
        let used = registry_name(chosen);
        for measurement in &mut measurements {
            measurement.used =
                measurement.solver == used && measurement.outcome == MeasurementOutcome::Answered;
        }
        let unfolded: Vec<SolverMeasurement> = measurements
            .iter()
            .filter(|m| m.substrate != Substrate::Qpu)
            .cloned()
            .collect();
        learned.learning.absorb(&unfolded);
        for measurement in &measurements {
            learned.record_registry(measurement)?;
        }

        let ensemble = runs
            .iter()
            .map(|run| EnsembleFeature {
                substrate: run.solver.substrate(),
                solver: run.solver.as_str().to_string(),
                objective: run.objective,
                gap_to_baseline: improvement(classical_objective, run.objective),
                selection_overlap: selection_overlap(&run.weights, &baseline_weights),
                feasible: run.is_usable(),
            })
            .collect();
        let qpu_record = learned.learning.record(Substrate::Qpu, size);
        let advantage = self.policy.advantage_boundary().assess(AdvantageEvidence {
            qpu_answer_used: chosen.is_quantum(),
            simulated: self
                .quantum
                .as_ref()
                .map(|arm| arm.gateway.provider().capabilities().simulated),
            trials: qpu_record.trials,
            wins: qpu_record.wins,
        });
        drop(learned);

        let compute = self.compute_decision(
            task,
            chosen,
            classical_solver,
            &dispatch,
            qpu_declined,
            information_value,
            Extras {
                inspired_note,
                inspired_information_value,
                measurements,
                ensemble,
                advantage,
            },
        );

        Ok(RoutingDecision {
            runs,
            chosen,
            weights,
            objective,
            classical_objective,
            rationale,
            quantum_note,
            quantum_dispatch: dispatch,
            compute: Some(compute),
        })
    }

    /// Run the quantum-inspired search through the solver registry, verify
    /// its claimed energy classically, and score it on the real problem.
    ///
    /// A run that fails, or whose claim does not verify, returns no
    /// [`SolverRun`] and a `Refused` measurement: a trial without a win.
    fn run_quantum_inspired(
        &self,
        registry: &SolverRegistry,
        problem: &PortfolioProblem,
        classical_objective: f64,
        predicted: f64,
    ) -> Result<(Option<SolverRun>, SolverMeasurement, String)> {
        let mut measurement = SolverMeasurement {
            solver: QUANTUM_INSPIRED_SOLVER.to_string(),
            substrate: Substrate::QuantumInspired,
            problem_size: problem.dimension(),
            outcome: MeasurementOutcome::Refused,
            verified: false,
            beat_baseline: false,
            used: false,
            quality: 0.0,
            cost_micros: 0,
            latency_cycles: 0,
            predicted_win_probability: Some(predicted),
        };
        let encoding = self.encoding_for(problem);
        let qubo = problem.to_qubo(&encoding)?;
        let effort = SolverEffort::default().with_seed(self.seed);
        let candidate = match registry.solve(&qubo, QUANTUM_INSPIRED_SOLVER, &effort) {
            Ok(candidate) => candidate,
            Err(error) => {
                let note = format!(
                    "the quantum-inspired search failed ({}); the baseline stands",
                    error.message()
                );
                return Ok((None, measurement, note));
            }
        };
        // The same classical gate a QPU answer goes through: the claimed
        // energy recomputed from the assignment, refused beyond tolerance.
        let validated = match ClassicalValidator::new(ENERGY_TOLERANCE)?.validate(&qubo, &candidate)
        {
            Ok(validated) => validated,
            Err(error) => {
                let note = format!(
                    "the quantum-inspired answer failed classical verification ({}) and is \
                     refused",
                    error.message()
                );
                return Ok((None, measurement, note));
            }
        };
        let weights = encoding.to_weights(validated.assignment());
        let run = self.score(
            problem,
            Solver::QuantumInspired,
            weights,
            candidate.evaluations,
            true,
            vec![
                encoding.describe(),
                "quantum-inspired path-integral annealing over the QUBO: a classical algorithm \
                 on this CPU, and not evidence of quantum advantage"
                    .to_string(),
            ],
        );
        let (beat, gain) = self.adopts(&run, classical_objective)?;
        measurement.outcome = MeasurementOutcome::Answered;
        measurement.verified = run.is_usable();
        measurement.beat_baseline = beat;
        measurement.quality = if run.is_usable() { gain } else { 0.0 };
        let note = format!(
            "ran on the same QUBO, verified classically and re-scored on the real problem: {:.2}% \
             against the baseline{}",
            gain * 100.0,
            if run.is_usable() { "" } else { ", infeasible" }
        );
        Ok((Some(run), measurement, note))
    }

    /// Whether a candidate would be adopted over this baseline, and its
    /// improvement. One predicate for the selection and for the measured
    /// record, so the record cannot count as a win what the selection refused.
    ///
    /// The bar is [`Self::margin_for`]: the stricter of the policy margin and
    /// the foundry's registered threshold, compared with a strict `>` so a tie
    /// stays classical. A workload the foundry does not admit adopts nothing.
    fn adopts(&self, candidate: &SolverRun, classical_objective: f64) -> Result<(bool, f64)> {
        let gain = improvement(classical_objective, candidate.objective);
        let adopted = match self.margin_for(candidate.solver)? {
            Some(margin) => candidate.is_usable() && gain > margin,
            None => false,
        };
        Ok((adopted, gain))
    }

    /// The expected-information-value estimate, and the reason a dispatch is
    /// declined when it is.
    fn estimate(
        &self,
        substrate: Substrate,
        record: SubstrateRecord,
        classical: Formulation,
        cost_micros: u64,
    ) -> (InformationValue, Option<String>) {
        let win_probability = if classical.is_exact() {
            0.0
        } else {
            record.win_probability()
        };
        let what = match substrate {
            Substrate::Qpu => "quantum",
            _ => substrate.as_str(),
        };
        let refusal = if classical.is_exact() {
            Some(format!(
                "the classical baseline is exhaustive and provably optimal at this size, so a \
                 {what} answer can at best tie and a tie is never adopted; no job is sent"
            ))
        } else if win_probability < self.policy.minimum_win_probability {
            Some(format!(
                "the {what} arm has won {} of {} trial(s) at this size, a measured win rate of \
                 {:.3} below the {:.3} the policy requires; it stops being tried",
                record.wins, record.trials, win_probability, self.policy.minimum_win_probability
            ))
        } else if cost_micros > self.policy.max_quantum_job_cost_micros {
            Some(format!(
                "a job costs {cost_micros} micro-USD, above the {} the policy allows per job",
                self.policy.max_quantum_job_cost_micros
            ))
        } else {
            None
        };
        (
            InformationValue {
                trials: record.trials,
                wins: record.wins,
                win_probability,
                expected_gain: win_probability * self.policy.quantum_margin,
                cost_micros,
                cleared: refusal.is_none(),
                substrate,
                repeatability: record.repeatability(),
                mean_latency_cycles: record.mean_latency_cycles(),
                calibration_brier: record.calibration_brier(),
            },
            refusal,
        )
    }

    /// What a later problem must share with a job's for the answer to be
    /// about it.
    fn problem_key(&self, problem: &PortfolioProblem) -> String {
        format!(
            "{}|{}",
            problem.assets.join(","),
            self.encoding_for(problem).describe()
        )
    }

    /// Verify a returned candidate classically, then score it on the real,
    /// current problem (R12-QUANT-4052).
    fn judge(
        &self,
        problem: &PortfolioProblem,
        qubo: &Qubo,
        result: Result<QaoaResult>,
        simulated: bool,
    ) -> Result<SolverRun> {
        let result = result?;
        if result.assignment.len() != qubo.n || qubo.n != problem.dimension() {
            return Err(Error::guard(format!(
                "quantum provider returned {} bit(s) for a {}-variable QUBO on a {}-asset problem; \
                 the result is refused",
                result.assignment.len(),
                qubo.n,
                problem.dimension()
            )));
        }
        // The provider's energy is a number it computed; recomputing it on
        // the QUBO it was given guards against a claim that does not match
        // the assignment. This is the classical validation gate QUANT-012
        // requires before any quantum result is used.
        let recomputed = qubo.evaluate(&result.assignment);
        let discrepancy = (recomputed - result.energy).abs();
        if !discrepancy.is_finite() || discrepancy > ENERGY_TOLERANCE {
            return Err(Error::guard(format!(
                "quantum provider claimed QUBO energy {:.6} but the assignment is worth {:.6}: a gap of {:.3e} exceeds tolerance {:.3e}. The result is refused.",
                result.energy, recomputed, discrepancy, ENERGY_TOLERANCE
            )));
        }

        let encoding = self.encoding_for(problem);
        let weights = encoding.to_weights(&result.assignment);
        let mut relaxations = vec![
            encoding.describe(),
            "solved asynchronously on an earlier cycle's figures and re-scored on this cycle's \
             problem"
                .to_string(),
        ];
        if simulated {
            relaxations.push(
                "run on a simulator: this validates the formulation and is not evidence of quantum advantage"
                    .to_string(),
            );
        }
        Ok(self.score(
            problem,
            Solver::Quantum,
            weights,
            result.evaluations,
            true,
            relaxations,
        ))
    }

    /// The compiled task for a portfolio problem (§11.2 steps 1–3).
    fn compile(&self, problem: &PortfolioProblem, classical: Solver) -> Result<CompiledTask> {
        let mut constraints: Vec<String> = problem
            .constraints
            .iter()
            .map(|constraint| constraint.label.clone())
            .collect();
        constraints.push("per-asset lower and upper bounds".to_string());
        if let Some(cardinality) = problem.cardinality {
            constraints.push(format!("at most {cardinality} holdings"));
        }
        TaskSpec {
            class: Some(TaskClass::CombinatorialOptimisation),
            variables: Some(problem.dimension()),
            constraints: Some(constraints),
            objective: Some(format!(
                "PortfolioProblem::objective_at ({:?})",
                problem.objective
            )),
            tolerance: Some(FEASIBILITY_TOLERANCE),
            deadline: Some(self.policy.time_budget),
            verification: Some(vec![
                format!(
                    "a quantum candidate's claimed QUBO energy is recomputed from its assignment \
                     and refused beyond {ENERGY_TOLERANCE:e}"
                ),
                "every candidate is re-scored on the real problem, bounds, constraints and \
                 cardinality included, and refused if infeasible"
                    .to_string(),
                "a candidate replaces the classical baseline only when better by more than the \
                 policy's margin"
                    .to_string(),
                // §11.3: the research domain's required classical check, on
                // the record beside the candidates it was applied to.
                format!(
                    "{}: {}",
                    ROUTED_DOMAIN.as_str(),
                    ROUTED_DOMAIN.required_check()
                ),
            ]),
            classical: Some(classical.classical_formulation()?),
            // The router discretises a continuous problem when the policy
            // lets the quantum arm near one, so the compiled task says so.
            discrete: problem.is_discrete() || self.policy.quantum_on_continuous,
        }
        .compile()
    }

    /// One verdict per declared substrate.
    fn compute_decision(
        &self,
        task: CompiledTask,
        chosen: Solver,
        classical_solver: Solver,
        dispatch: &QuantumDispatch,
        qpu_declined: String,
        information_value: Option<InformationValue>,
        extras: Extras,
    ) -> ComputeDecision {
        let quantum_chosen = chosen.is_quantum();
        let inspired_chosen = chosen == Solver::QuantumInspired;
        let inspired_ran = extras
            .ensemble
            .iter()
            .any(|feature| feature.substrate == Substrate::QuantumInspired);
        let mut verdicts = Vec::with_capacity(Substrate::ALL.len());
        for substrate in Substrate::ALL {
            let (role, reason) = match substrate {
                Substrate::Cpu => (
                    if quantum_chosen || inspired_chosen {
                        SubstrateRole::Baseline
                    } else {
                        SubstrateRole::Answered
                    },
                    format!(
                        "the classical baseline by {}, computed every cycle (ADR 0006)",
                        classical_solver.as_str()
                    ),
                ),
                // Declined, no device: a valid verdict, and the only honest
                // one. No in-tree backend drives an accelerator, so nothing
                // is routed to one and nothing claims acceleration it did not
                // get.
                Substrate::Gpu | Substrate::Tpu => (
                    SubstrateRole::Declined,
                    format!(
                        "declined, no device: no {} backend is attached to this workspace; \
                         nothing is routed to a substrate that cannot be reached",
                        substrate.as_str()
                    ),
                ),
                Substrate::QuantumInspired => {
                    if inspired_chosen {
                        (
                            SubstrateRole::Answered,
                            format!(
                                "a verified quantum-inspired answer beat the baseline by more \
                                 than the margin; {}",
                                extras.inspired_note
                            ),
                        )
                    } else {
                        (SubstrateRole::Declined, extras.inspired_note.clone())
                    }
                }
                Substrate::Qpu => {
                    if quantum_chosen {
                        (
                            SubstrateRole::Answered,
                            format!(
                                "a verified answer from quantum job {} beat the baseline by more \
                                 than the margin",
                                dispatch.harvested.unwrap_or_default()
                            ),
                        )
                    } else if let Some(ticket) = dispatch.submitted {
                        (
                            SubstrateRole::Dispatched,
                            format!("quantum job {ticket} submitted; judged in a later cycle"),
                        )
                    } else if task.quantum.is_none() {
                        (
                            SubstrateRole::Declined,
                            "the compiled task has no quantum formulation".to_string(),
                        )
                    } else {
                        (SubstrateRole::Declined, qpu_declined.clone())
                    }
                }
                Substrate::ParallelEnsemble => match dispatch.submitted {
                    Some(ticket) => (
                        SubstrateRole::Dispatched,
                        format!(
                            "the CPU baseline answered now while quantum job {ticket} runs beside \
                             it; the ensemble is resolved when the job returns"
                        ),
                    ),
                    None if inspired_ran => (
                        SubstrateRole::Declined,
                        "the CPU baseline and the quantum-inspired search ran side by side on the \
                         same QUBO; their outputs are recorded as ensemble features and are not \
                         combined into an answer (§11.4)"
                            .to_string(),
                    ),
                    None => (
                        SubstrateRole::Declined,
                        "only one substrate was asked this cycle".to_string(),
                    ),
                },
            };
            verdicts.push(SubstrateVerdict {
                substrate,
                role,
                reason,
            });
        }
        ComputeDecision {
            task,
            verdicts,
            information_value,
            inspired_information_value: extras.inspired_information_value,
            measurements: extras.measurements,
            ensemble: extras.ensemble,
            research_domain: Some(ROUTED_DOMAIN),
            advantage: extras.advantage,
        }
    }

    /// The classical baseline.
    ///
    /// Exact enumeration for a small discrete problem, annealing for a large
    /// one, quadratic programming for a continuous one.
    fn solve_classical(&self, problem: &PortfolioProblem) -> Result<SolverRun> {
        if problem.is_discrete() {
            let encoding = self.encoding_for(problem);
            let qubo = problem.to_qubo(&encoding)?;

            if problem.dimension() <= self.policy.exact_enumeration_limit
                && let Some(exact) = solve_exact(&qubo)
            {
                let weights = encoding.to_weights(&exact.assignment);
                return Ok(self.score(
                    problem,
                    Solver::ExactEnumeration,
                    weights,
                    1usize << problem.dimension(),
                    true,
                    vec![encoding.describe()],
                ));
            }

            let mut rng = Xoshiro256::seeded(self.seed).fork("anneal");
            let settings = AnnealSettings::default();
            let result = anneal(&qubo, &settings, &mut rng);
            let weights = encoding.to_weights(&result.assignment);
            return Ok(self.score(
                problem,
                Solver::SimulatedAnnealing,
                weights,
                result.sweeps_run,
                true,
                vec![encoding.describe()],
            ));
        }

        let program = problem.to_quadratic_program()?;
        let solution = solve_qp(&program, &QpSettings::default())?;
        Ok(self.score(
            problem,
            Solver::QuadraticProgram,
            solution.x,
            solution.iterations,
            solution.converged,
            Vec::new(),
        ))
    }

    fn encoding_for(&self, problem: &PortfolioProblem) -> QuboEncoding {
        let cardinality = problem.cardinality.unwrap_or(problem.dimension());
        // The gross exposure comes from the budget constraint when there is
        // one, so the discretisation lands where the continuous solution
        // would. Defaulting to one otherwise.
        let gross = problem
            .constraints
            .iter()
            .find(|c| c.label == "budget")
            .and_then(|c| c.upper)
            .unwrap_or(1.0);
        // The tightest position cap in the mandate. Ignoring it would make
        // every candidate the encoding produces breach a bound, so the whole
        // discrete path would return nothing usable.
        let cap = problem
            .upper_bounds
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        QuboEncoding::equal_weight_bounded(cardinality, gross, cap)
    }

    /// Whether a discrete mandate's own parameters can be satisfied together.
    ///
    /// `cardinality × position_cap < gross` is a contradiction between three
    /// numbers someone chose separately, and it is worth naming as such rather
    /// than letting it surface as an unexplained infeasibility.
    pub fn mandate_conflict(&self, problem: &PortfolioProblem) -> Option<String> {
        let cardinality = problem.cardinality?;
        let gross = problem
            .constraints
            .iter()
            .find(|c| c.label == "budget")
            .and_then(|c| c.lower)?;
        let cap = problem
            .upper_bounds
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let reachable = cap * cardinality as f64;
        (reachable < gross - 1e-9).then(|| {
            format!(
                "{cardinality} holdings capped at {:.1}% each reach {:.1}% gross, short of the {:.1}% the budget constraint requires; the mandate's own parameters conflict",
                cap * 100.0,
                reachable * 100.0,
                gross * 100.0
            )
        })
    }

    /// Score a candidate against the *real* problem, including the constraints
    /// a relaxation dropped.
    ///
    /// This is what makes the comparison honest: a QUBO answer that ignored
    /// the budget constraint is measured against the budget constraint anyway,
    /// and comes back infeasible.
    fn score(
        &self,
        problem: &PortfolioProblem,
        solver: Solver,
        weights: Vec<f64>,
        work: usize,
        converged: bool,
        relaxations: Vec<String>,
    ) -> SolverRun {
        let objective = problem.objective_at(&weights);
        let (worst_violation, violated_constraint) = worst_violation(problem, &weights);
        SolverRun {
            solver,
            weights,
            objective,
            feasible: worst_violation <= FEASIBILITY_TOLERANCE,
            worst_violation,
            violated_constraint,
            work,
            converged,
            relaxations,
        }
    }
}

/// The worst constraint violation of a weight vector, and which constraint.
///
/// Includes bounds and the cardinality limit, which the QP relaxation does not
/// carry — so a relaxed answer is scored against the mandate it would actually
/// have to satisfy.
fn worst_violation(problem: &PortfolioProblem, weights: &[f64]) -> (f64, String) {
    let n = problem.dimension();
    if weights.len() != n {
        return (f64::INFINITY, "dimension".to_string());
    }
    let mut worst = 0.0_f64;
    let mut label = String::new();

    let consider = |violation: f64, name: &str, worst: &mut f64, label: &mut String| {
        if violation > *worst {
            *worst = violation;
            *label = name.to_string();
        }
    };

    for (i, weight) in weights.iter().enumerate() {
        consider(
            (problem.lower_bounds[i] - weight).max(0.0),
            &format!("lower bound on {}", problem.assets[i]),
            &mut worst,
            &mut label,
        );
        consider(
            (weight - problem.upper_bounds[i]).max(0.0),
            &format!("upper bound on {}", problem.assets[i]),
            &mut worst,
            &mut label,
        );
    }

    for constraint in &problem.constraints {
        let value: f64 = (0..n)
            .map(|i| constraint.coefficients[i] * weights[i])
            .sum();
        if let Some(lower) = constraint.lower {
            consider(
                (lower - value).max(0.0),
                &constraint.label,
                &mut worst,
                &mut label,
            );
        }
        if let Some(upper) = constraint.upper {
            consider(
                (value - upper).max(0.0),
                &constraint.label,
                &mut worst,
                &mut label,
            );
        }
    }

    if let Some(cardinality) = problem.cardinality {
        let held = weights.iter().filter(|w| w.abs() > 1e-9).count();
        consider(
            held.saturating_sub(cardinality) as f64,
            "cardinality",
            &mut worst,
            &mut label,
        );
    }

    if label.is_empty() {
        label = "none".to_string();
    }
    (worst, label)
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::checked_threshold;

    /// A threshold the router reads that is zero, negative or not a number is
    /// refused, not raised to a floor: a clamped margin is one nobody chose.
    ///
    /// Mutation: change `threshold > 0.0` to `threshold >= 0.0` in
    /// `checked_threshold` and the zero case fires.
    #[test]
    fn a_non_positive_or_non_finite_threshold_is_refused_at_routing_not_clamped() {
        for bad in [0.0, -0.0, -0.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(checked_threshold(bad).is_err(), "{bad} was accepted");
        }
        assert_eq!(checked_threshold(0.01).ok(), Some(0.01));
    }
}
