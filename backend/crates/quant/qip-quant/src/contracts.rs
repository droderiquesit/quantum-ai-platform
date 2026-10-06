//! Quantum optimization contracts.
//!
//! [`DecisionRequest`] is a request to solve a quantum optimization problem,
//! paired with metadata about what the requestor needs to know. [`SolverResult`]
//! is the answer: an assignment, what kind of solver produced it, and the effort
//! expended. [`SolverRegistry`] is a trait for registering solvers and solving
//! requests, enforcing that solvers are named and retrievable.
//!
//! These contracts are the narrow interface between the strategy layer and the
//! quantum optimization layer: a strategy proposes targets, the portfolio engine
//! asks which solvers are available, picks one, and sends a decision request; the
//! solver returns a result, and the risk engine audits the assignment before
//! submitting orders.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_numerics::anneal::Qubo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A request to solve a quantum optimization problem.
///
/// Carries the problem itself (a QUBO), effort constraints, and metadata about
/// the request context — the portfolio it applies to, the moment it was made,
/// and the strategy that is proposing it. Solvers use context to explain their
/// choices; a request without context produces a meaningless result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// The quadratic unconstrained binary optimization problem to solve.
    pub problem: Qubo,
    /// The portfolio this optimization applies to. Used for attribution.
    pub portfolio_id: String,
    /// The strategy requesting the solve. Used for explanation.
    pub strategy_id: String,
    /// When the request was made, in the cycle clock.
    pub as_of: Timestamp,
    /// How many variables the problem has. Pre-computed for validation.
    pub num_variables: usize,
    /// Optional context about the optimization — the factor the strategy is
    /// optimizing for, the constraints it holds, any human notes. Recorded on
    /// the proposal but does not constrain the solve.
    pub context: BTreeMap<String, String>,
}

impl DecisionRequest {
    /// Create a request to solve a QUBO.
    ///
    /// Refuses if the problem's size does not match the stated variable count,
    /// or if the portfolio or strategy identifier is empty. A QUBO with zero
    /// variables is invalid: the smallest meaningful problem is one variable.
    pub fn new(
        problem: Qubo,
        portfolio_id: String,
        strategy_id: String,
        as_of: Timestamp,
    ) -> Result<Self> {
        if portfolio_id.is_empty() {
            return Err(Error::invalid("portfolio_id must not be empty"));
        }
        if strategy_id.is_empty() {
            return Err(Error::invalid("strategy_id must not be empty"));
        }

        let num_variables = problem.n;
        if num_variables == 0 {
            return Err(Error::invalid(
                "QUBO must have at least one variable to optimize",
            ));
        }

        Ok(Self {
            problem,
            portfolio_id,
            strategy_id,
            as_of,
            num_variables,
            context: BTreeMap::new(),
        })
    }

    /// Add a context field to the request.
    pub fn with_context(mut self, key: String, value: String) -> Self {
        self.context.insert(key, value);
        self
    }

    /// Validate that the request is sound before handing to a solver.
    ///
    /// A valid request has:
    /// - A problem with at least one variable
    /// - A problem whose size matches the declared variable count
    /// - Non-empty portfolio and strategy identifiers
    ///
    /// This is called before solving, so a defect in the request is caught
    /// before a solver spends time on it.
    pub fn validate(&self) -> Result<()> {
        if self.portfolio_id.is_empty() {
            return Err(Error::invalid("portfolio_id is empty"));
        }
        if self.strategy_id.is_empty() {
            return Err(Error::invalid("strategy_id is empty"));
        }
        if self.num_variables == 0 {
            return Err(Error::invalid("num_variables must be > 0"));
        }
        if self.problem.n != self.num_variables {
            return Err(Error::invalid(
                "problem size does not match num_variables declaration",
            ));
        }
        Ok(())
    }
}

/// The result of solving a quantum optimization problem.
///
/// Carries the solution itself (an assignment), metadata about what produced it
/// (solver name, kind, effort), and optional context about its quality relative
/// to a classical baseline. A result without attribution is unauditble.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverResult {
    /// Which solver produced this result.
    pub solver_name: String,
    /// An assignment: which variable is set to which value.
    pub assignment: Vec<u8>,
    /// The objective function value of the assignment.
    pub objective: f64,
    /// How long the solve took, in modelled time (not wall-clock).
    pub duration_nanos: u64,
    /// How much work was done: evaluations, sweeps, queue time.
    pub effort_metrics: BTreeMap<String, u64>,
    /// The monetary cost of solving, in microunits.
    pub cost_micros: u64,
    /// Optional metadata about the quality of the result — gap to known lower
    /// bound, reliability metrics if this was the average of multiple runs, etc.
    pub quality: BTreeMap<String, f64>,
}

impl SolverResult {
    /// Create a result from a solve.
    ///
    /// Refuses if the assignment size does not match the problem size declared
    /// in the request, or if the solver name is empty. Every result must name
    /// its source so the choice is auditable.
    pub fn new(
        solver_name: String,
        assignment: Vec<u8>,
        objective: f64,
        request: &DecisionRequest,
    ) -> Result<Self> {
        if solver_name.is_empty() {
            return Err(Error::invalid("solver_name must not be empty"));
        }
        if assignment.len() != request.num_variables {
            return Err(Error::invalid(format!(
                "assignment size {} does not match request size {}",
                assignment.len(),
                request.num_variables
            )));
        }

        // Validate that each assignment value is either 0 or 1.
        for (i, &val) in assignment.iter().enumerate() {
            if val > 1 {
                return Err(Error::invalid(format!(
                    "assignment[{}] = {} is not binary",
                    i, val
                )));
            }
        }

        Ok(Self {
            solver_name,
            assignment,
            objective,
            duration_nanos: 0,
            effort_metrics: BTreeMap::new(),
            cost_micros: 0,
            quality: BTreeMap::new(),
        })
    }

    /// Set the modelled duration of solving.
    pub fn with_duration(mut self, nanos: u64) -> Self {
        self.duration_nanos = nanos;
        self
    }

    /// Add an effort metric to the result.
    pub fn with_metric(mut self, name: String, value: u64) -> Self {
        self.effort_metrics.insert(name, value);
        self
    }

    /// Set the cost of solving.
    pub fn with_cost(mut self, cost_micros: u64) -> Self {
        self.cost_micros = cost_micros;
        self
    }

    /// Add a quality measure to the result.
    pub fn with_quality(mut self, name: String, value: f64) -> Self {
        self.quality.insert(name, value);
        self
    }

    /// Validate that the result is consistent with the request it answers.
    pub fn validate(&self, request: &DecisionRequest) -> Result<()> {
        if self.solver_name.is_empty() {
            return Err(Error::invalid("solver_name is empty"));
        }
        if self.assignment.len() != request.num_variables {
            return Err(Error::invalid("assignment size does not match request"));
        }
        for (i, &val) in self.assignment.iter().enumerate() {
            if val > 1 {
                return Err(Error::invalid(format!(
                    "assignment[{}] = {} is not binary",
                    i, val
                )));
            }
        }
        Ok(())
    }
}

/// A registry for quantum solvers.
///
/// A solver registry manages a named set of solvers and permits solving a
/// decision request with any registered solver. A solver is registered by name;
/// the same solver instance can be looked up by name later.
///
/// The registry enforces that every solver has a name, that lookups are by that
/// name, and that solving produces a result that is validated against the
/// request. These are structural guarantees about who produced an assignment and
/// whether they answered the question that was asked.
pub trait SolverRegistry: Send + Sync {
    /// Register a named solver.
    ///
    /// Refuses if the name is empty, or if the name is already registered.
    /// Registering the same solver twice is likely an error, even if the
    /// instances are identical.
    fn register(&mut self, name: String, solver: Box<dyn QuantumSolverEngine>) -> Result<()>;

    /// Look up a solver by name.
    fn get(&self, name: &str) -> Option<&dyn QuantumSolverEngine>;

    /// Solve a decision request with a named solver.
    ///
    /// Returns an error if the solver is not registered, or if the solver
    /// itself refuses the request. The returned result is validated against
    /// the request before it is returned.
    fn solve(&self, solver_name: &str, request: &DecisionRequest) -> Result<SolverResult>;

    /// List the names of all registered solvers.
    fn list_solvers(&self) -> Vec<String>;
}

/// A quantum solver engine capable of solving a decision request.
///
/// A solver engine is the actual implementation that a registry holds.
/// It takes a decision request and produces a solution. The solver is
/// responsible for validating its inputs and producing a result that
/// correctly answers the request.
pub trait QuantumSolverEngine: Send + Sync + std::fmt::Debug {
    /// The name of this solver, for attribution.
    fn name(&self) -> &str;

    /// Solve a decision request.
    ///
    /// The request is guaranteed to have been validated before this is called.
    /// The solver should refuse the request if it cannot handle the problem size,
    /// the time constraints, or any other aspect.
    fn solve(&self, request: &DecisionRequest) -> Result<SolverResult>;
}
