//! The compute router.
//!
//! Chooses which solver runs a problem, and enforces the platform's rule about
//! quantum methods: **no quantum result is used without a classical baseline
//! solved on the same problem, and the classical result wins ties**.
//!
//! That is enforced mechanically, not by convention. [`ComputeRouter::solve`]
//! always runs the classical solver. It runs the quantum path only when the
//! problem has the structure that could justify it, and it returns the quantum
//! answer only when that answer is both feasible and strictly better by more
//! than a stated margin. Everything else — the benchmark, the margin, the
//! reason for the choice — is in [`RoutingDecision`], so a claim of advantage
//! always arrives with the evidence for it or not at all.
//!
//! The default is classical. A deployment with no quantum backend configured
//! behaves identically to one whose backend is unreachable, which is the
//! behaviour an operator should be able to rely on.

use crate::problem::{PortfolioProblem, QuboEncoding};
use qip_core::error::{Error, Result};
use qip_core::rng::Xoshiro256;
use qip_core::time::Duration;
use qip_numerics::anneal::{AnnealSettings, anneal, solve_exact};
use qip_numerics::qp::{QpSettings, solve as solve_qp};
use qip_quantum::provider::QuantumProvider;
use qip_quantum::qaoa::QaoaSettings;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
}

impl Solver {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::QuadraticProgram => "quadratic_program",
            Self::SimulatedAnnealing => "simulated_annealing",
            Self::ExactEnumeration => "exact_enumeration",
            Self::Quantum => "quantum",
        }
    }

    pub const fn is_quantum(&self) -> bool {
        matches!(self, Self::Quantum)
    }

    /// Whether the solver finds the global optimum rather than a good point.
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::ExactEnumeration)
    }
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
    /// Whether a quantum path was attempted at all, and why or why not.
    pub quantum_note: String,
}

impl RoutingDecision {
    /// The improvement the chosen answer gives over the classical baseline, as
    /// a fraction. Negative would mean the classical answer was better, which
    /// the router does not allow.
    pub fn improvement_over_classical(&self) -> f64 {
        let baseline = self.classical_objective;
        if baseline.abs() < 1e-12 {
            return 0.0;
        }
        // Objectives are minimised, so an improvement is a decrease.
        (baseline - self.objective) / baseline.abs()
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

/// How the router decides.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Assets below which a quantum path is never attempted.
    ///
    /// Under fifty, an exact or convex classical solve finishes in
    /// microseconds and any quantum formulation spends longer on encoding than
    /// the classical solver spends solving.
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
    /// Wall time the router will spend before returning what it has.
    pub time_budget: Duration,
    /// How much work the quantum path may do.
    ///
    /// Carried here rather than hard-coded in the solver call, because the
    /// depth and the angle-search budget are the two levers that decide what a
    /// QAOA attempt costs, and a caller paying for device time should be the
    /// one setting them.
    pub qaoa: QaoaSettings,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            minimum_assets_for_quantum: 50,
            quantum_on_continuous: false,
            quantum_margin: 0.01,
            exact_enumeration_limit: 22,
            time_budget: Duration::from_secs(60),
            qaoa: QaoaSettings::default(),
        }
    }
}

/// Routes a problem to a solver.
#[derive(Debug)]
pub struct ComputeRouter {
    policy: RoutingPolicy,
    quantum: Option<Arc<dyn QuantumProvider>>,
    seed: u64,
}

impl ComputeRouter {
    /// A classical-only router. The default configuration.
    pub fn classical(seed: u64) -> Self {
        Self {
            policy: RoutingPolicy::default(),
            quantum: None,
            seed,
        }
    }

    pub fn with_policy(mut self, policy: RoutingPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Attach a quantum provider. It is still only used where the policy
    /// permits and only when it wins by the margin.
    pub fn with_quantum(mut self, provider: Arc<dyn QuantumProvider>) -> Self {
        self.quantum = Some(provider);
        self
    }

    pub fn policy(&self) -> RoutingPolicy {
        self.policy
    }

    /// Whether a quantum path would be attempted for this problem, and why.
    ///
    /// Public so a caller can ask before committing to the cost, and so the
    /// reasoning is testable independently of running anything.
    pub fn quantum_assessment(&self, problem: &PortfolioProblem) -> (bool, String) {
        let Some(provider) = &self.quantum else {
            return (
                false,
                "no quantum provider is configured; the classical solver is the default"
                    .to_string(),
            );
        };
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

    /// Solve, always producing a classical baseline.
    pub fn solve(&self, problem: &PortfolioProblem) -> Result<RoutingDecision> {
        problem.validate()?;
        let mut runs = Vec::new();

        // The classical baseline is not optional. Even when a quantum path is
        // attempted and wins, the baseline is what the win is measured against.
        let classical = self.solve_classical(problem)?;
        let classical_objective = classical.objective;
        let classical_solver = classical.solver;
        let classical_weights = classical.weights.clone();
        let classical_usable = classical.is_usable();
        runs.push(classical);

        let (attempt_quantum, quantum_note) = self.quantum_assessment(problem);
        if attempt_quantum && let Some(provider) = &self.quantum {
            match self.solve_quantum(problem, provider.as_ref()) {
                Ok(run) => runs.push(run),
                Err(error) => {
                    // A failed quantum attempt is a note, not a failure: the
                    // classical answer is already in hand, which is the whole
                    // point of computing it first.
                    return Ok(RoutingDecision {
                        runs,
                        chosen: classical_solver,
                        weights: classical_weights,
                        objective: classical_objective,
                        classical_objective,
                        rationale: format!(
                            "the quantum attempt failed ({}), so the classical baseline stands",
                            error.message()
                        ),
                        quantum_note,
                    });
                }
            }
        }

        // Selection. The classical answer wins unless a quantum answer is
        // feasible and better by more than the margin.
        let quantum_run = runs.iter().find(|run| run.solver.is_quantum()).cloned();
        let (chosen, weights, objective, rationale) = match quantum_run {
            Some(quantum) if !quantum.is_usable() => (
                classical_solver,
                classical_weights,
                classical_objective,
                format!(
                    "the quantum answer violated {} by {:.4}, so the classical baseline stands",
                    quantum.violated_constraint, quantum.worst_violation
                ),
            ),
            Some(quantum) => {
                let improvement = if classical_objective.abs() < 1e-12 {
                    0.0
                } else {
                    (classical_objective - quantum.objective) / classical_objective.abs()
                };
                if improvement > self.policy.quantum_margin {
                    (
                        Solver::Quantum,
                        quantum.weights.clone(),
                        quantum.objective,
                        format!(
                            "the quantum answer improved on the classical baseline by {:.2}%, above the {:.2}% margin required",
                            improvement * 100.0,
                            self.policy.quantum_margin * 100.0
                        ),
                    )
                } else {
                    (
                        classical_solver,
                        classical_weights,
                        classical_objective,
                        format!(
                            "the quantum answer was {:.2}% better, inside the {:.2}% margin, so the classical baseline stands: a tie is not evidence",
                            improvement * 100.0,
                            self.policy.quantum_margin * 100.0
                        ),
                    )
                }
            }
            None => (
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

        Ok(RoutingDecision {
            runs,
            chosen,
            weights,
            objective,
            classical_objective,
            rationale,
            quantum_note,
        })
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

    fn solve_quantum(
        &self,
        problem: &PortfolioProblem,
        provider: &dyn QuantumProvider,
    ) -> Result<SolverRun> {
        let encoding = self.encoding_for(problem);
        let qubo = problem.to_qubo(&encoding)?;
        let result = provider.solve_qubo(&qubo, &self.policy.qaoa)?;
        let weights = encoding.to_weights(&result.assignment);

        let mut relaxations = vec![encoding.describe()];
        if provider.capabilities().simulated {
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
            feasible: worst_violation <= 1e-6,
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

/// Errors the router itself raises.
pub fn require_provider(
    provider: Option<&Arc<dyn QuantumProvider>>,
) -> Result<&dyn QuantumProvider> {
    provider
        .map(|p| p.as_ref())
        .ok_or_else(|| Error::unavailable("no quantum provider is configured"))
}
