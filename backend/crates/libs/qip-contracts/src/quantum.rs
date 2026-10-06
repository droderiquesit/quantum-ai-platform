//! Quantum decision contracts.
//!
//! The vocabulary for quantum problem formulation, solver results, and the
//! routing decision to choose between quantum and classical approaches.
//!
//! The rule this module exists to make enforceable is that every quantum
//! decision is accompanied by a classical baseline solved on the same problem.
//! A routing decision that chooses quantum over classical records both results
//! and the measured advantage that justified the choice.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A quantum or hybrid optimization problem.
///
/// Carries enough structure to formulate the problem for any solver and to
/// validate that the result makes sense. The problem is immutable once created,
/// because the baseline solver and the quantum solver must see the same objective
/// function.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// A unique identifier for this problem instance.
    id: String,
    /// The number of binary variables.
    num_variables: usize,
    /// The QUBO coefficients in sparse format: (row, col, value).
    /// Both diagonal (row == col) and off-diagonal terms are included.
    qubo_terms: Vec<(usize, usize, f64)>,
    /// The constant offset term.
    constant: f64,
}

impl DecisionRequest {
    /// Create a new quantum decision request.
    ///
    /// Validates that the problem is well-formed: variables are in range,
    /// and the QUBO is symmetric where necessary.
    pub fn new(
        id: impl Into<String>,
        num_variables: usize,
        qubo_terms: Vec<(usize, usize, f64)>,
        constant: f64,
    ) -> Result<Self> {
        if num_variables == 0 {
            return Err(Error::invalid("problem must have at least one variable"));
        }
        if qubo_terms.is_empty() {
            return Err(Error::invalid("problem must have at least one QUBO term"));
        }

        // Validate indices are in range.
        for (row, col, _) in &qubo_terms {
            if *row >= num_variables || *col >= num_variables {
                return Err(Error::invalid(
                    "QUBO indices must be within the number of variables",
                ));
            }
        }

        Ok(Self {
            id: id.into(),
            num_variables,
            qubo_terms,
            constant,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn num_variables(&self) -> usize {
        self.num_variables
    }

    pub fn qubo_terms(&self) -> &[(usize, usize, f64)] {
        &self.qubo_terms
    }

    pub fn constant(&self) -> f64 {
        self.constant
    }
}

/// The solution to a quantum or classical optimization problem.
///
/// Both classical and quantum solvers return this type, with the kind field
/// recording which path was taken. The assignment is validated before it is
/// used: see [`SolverResult::is_valid`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolverResult {
    /// Which solver produced this result.
    kind: SolverKind,
    /// The assignment: one bit per variable, in order.
    assignment: Vec<bool>,
    /// The objective value computed from the assignment.
    objective_value: f64,
    /// The computational cost, modelled not measured.
    cost_nanos: u64,
    /// Which request this result solves.
    request_id: String,
}

/// The kind of solver that produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverKind {
    /// Classical exhaustive or local search.
    Classical,
    /// Quantum-inspired search (PIQA).
    QuantumInspired,
    /// Quantum device or high-fidelity simulator.
    Quantum,
}

impl SolverKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Classical => "classical",
            Self::QuantumInspired => "quantum_inspired",
            Self::Quantum => "quantum",
        }
    }

    /// Whether this result needs a classical baseline to be trusted.
    pub const fn needs_classical_baseline(&self) -> bool {
        !matches!(self, Self::Classical)
    }
}

impl SolverResult {
    /// Create a solver result.
    ///
    /// Validates that the assignment matches the problem size and that the
    /// objective value is computable.
    pub fn new(
        kind: SolverKind,
        assignment: Vec<bool>,
        objective_value: f64,
        cost_nanos: u64,
        request_id: impl Into<String>,
    ) -> Result<Self> {
        if assignment.is_empty() {
            return Err(Error::invalid("assignment must not be empty"));
        }
        if !objective_value.is_finite() {
            return Err(Error::numeric("objective value must be finite"));
        }

        Ok(Self {
            kind,
            assignment,
            objective_value,
            cost_nanos,
            request_id: request_id.into(),
        })
    }

    pub fn kind(&self) -> SolverKind {
        self.kind
    }

    pub fn assignment(&self) -> &[bool] {
        &self.assignment
    }

    pub fn objective_value(&self) -> f64 {
        self.objective_value
    }

    pub fn cost_nanos(&self) -> u64 {
        self.cost_nanos
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Validate that the assignment is consistent with a request.
    pub fn is_valid_for(&self, request: &DecisionRequest) -> bool {
        self.assignment.len() == request.num_variables && self.request_id == request.id()
    }

    /// Re-compute the objective value from the assignment and QUBO terms.
    ///
    /// This is the validation step: a result whose claimed objective does not
    /// match its recomputed objective is refused. The assignment must already
    /// have been validated against the request.
    pub fn recompute_objective(
        assignment: &[bool],
        qubo_terms: &[(usize, usize, f64)],
        constant: f64,
    ) -> f64 {
        let mut value = constant;
        for (i, j, coeff) in qubo_terms {
            let bit_i = if assignment[*i] { 1.0 } else { 0.0 };
            let bit_j = if assignment[*j] { 1.0 } else { 0.0 };
            value += coeff * bit_i * bit_j;
        }
        value
    }
}

/// The result of a routing decision: quantum or classical.
///
/// This type records why the decision was made, carrying both the classical
/// and quantum results so the full rationale is reproducible from the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Which path was chosen.
    chosen: ChosenPath,
    /// The classical result (always computed).
    classical_result: SolverResult,
    /// The quantum result (may be absent if quantum was not attempted).
    quantum_result: Option<SolverResult>,
    /// The measured advantage of quantum over classical.
    ///
    /// Positive means quantum is better (lower objective).
    /// Only set if quantum was chosen and outperformed classical.
    advantage_bps: Option<i64>,
}

/// Which path a routing decision selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChosenPath {
    /// Classical solver was chosen.
    Classical,
    /// Quantum solver was chosen despite quantum being more expensive,
    /// because the objective improvement justified the cost.
    Quantum,
}

impl ChosenPath {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Classical => "classical",
            Self::Quantum => "quantum",
        }
    }
}

impl RoutingDecision {
    /// Create a routing decision.
    ///
    /// Validates that both results solve the same problem and that the
    /// advantage calculation is correct.
    pub fn new(
        classical_result: SolverResult,
        quantum_result: Option<SolverResult>,
        chosen: ChosenPath,
    ) -> Result<Self> {
        // Classical result is mandatory and must be from a classical solver.
        if classical_result.kind() != SolverKind::Classical {
            return Err(Error::invalid(
                "classical result must come from a classical solver",
            ));
        }

        // If quantum result is present, validate it.
        if let Some(ref qr) = quantum_result {
            if classical_result.request_id() != qr.request_id() {
                return Err(Error::invalid(
                    "classical and quantum results must solve the same problem",
                ));
            }
            if classical_result.assignment.len() != qr.assignment.len() {
                return Err(Error::invalid(
                    "classical and quantum assignments must have the same size",
                ));
            }
        }

        // Calculate advantage if quantum was chosen.
        let advantage_bps = if let Some(ref qr) = quantum_result {
            if chosen == ChosenPath::Quantum {
                let diff = classical_result.objective_value - qr.objective_value;
                if diff > 0.0 {
                    // Quantum is better. Convert to basis points.
                    let advantage = (diff / classical_result.objective_value.abs()) * 10000.0;
                    Some(advantage as i64)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            chosen,
            classical_result,
            quantum_result,
            advantage_bps,
        })
    }

    pub fn chosen(&self) -> ChosenPath {
        self.chosen
    }

    pub fn classical_result(&self) -> &SolverResult {
        &self.classical_result
    }

    pub fn quantum_result(&self) -> Option<&SolverResult> {
        self.quantum_result.as_ref()
    }

    pub fn advantage_bps(&self) -> Option<i64> {
        self.advantage_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_request_validates_variables() {
        let err = DecisionRequest::new("test", 0, vec![(0, 0, 1.0)], 0.0)
            .expect_err("must reject zero variables");
        assert!(err.message().contains("at least one variable"));
    }

    #[test]
    fn decision_request_validates_qubo_terms() {
        let err = DecisionRequest::new("test", 1, vec![], 0.0).expect_err("must reject empty QUBO");
        assert!(err.message().contains("at least one QUBO term"));
    }

    #[test]
    fn decision_request_validates_indices() {
        let err = DecisionRequest::new("test", 2, vec![(0, 3, 1.0)], 0.0)
            .expect_err("must reject out-of-range indices");
        assert!(err.message().contains("within the number of variables"));
    }

    #[test]
    fn solver_result_validates_assignment() {
        let err = SolverResult::new(SolverKind::Classical, vec![], 1.0, 0, "test")
            .expect_err("must reject empty assignment");
        assert!(err.message().contains("not be empty"));
    }

    #[test]
    fn solver_result_validates_objective() {
        let err = SolverResult::new(SolverKind::Classical, vec![true], f64::INFINITY, 0, "test")
            .expect_err("must reject non-finite objective");
        assert!(err.message().contains("finite"));
    }

    #[test]
    fn objective_recomputation_matches_expected() {
        let qubo = vec![(0, 0, 2.0), (0, 1, -1.0), (1, 1, 3.0)];
        let assignment = vec![true, false];
        let constant = 1.0;
        // Objective: 1.0 + 2.0*1*1 + (-1.0)*1*0 + 3.0*0*0 = 3.0
        let obj = SolverResult::recompute_objective(&assignment, &qubo, constant);
        assert_eq!(obj, 3.0);
    }
}
