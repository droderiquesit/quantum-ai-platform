//! A registry of available solvers.
//!
//! The registry exposes all installed solvers through a common interface,
//! allowing the routing decision to select among them based on availability,
//! cost, and measured performance.
//!
//! Every solver is registered at deployment time, and the registry itself is
//! immutable once constructed. This ensures that the set of available solvers
//! is known and cannot be changed dynamically.

use crate::QuboSolver;
use qip_contracts::quantum::{DecisionRequest, SolverResult};
use qip_core::error::{Error, Result};
use std::sync::Arc;

/// A registry of available QUBO solvers.
///
/// The registry holds references to all solvers that can be used to solve
/// problems. It is responsible for:
/// 1. Tracking which solvers are available in this deployment
/// 2. Exposing the classical solver (mandatory)
/// 3. Exposing optional quantum solvers (if configured)
/// 4. Routing a problem to the appropriate solver
pub trait SolverRegistry: Send + Sync {
    /// Get the classical solver (always available).
    fn classical_solver(&self) -> Arc<dyn QuboSolver>;

    /// Get the quantum-inspired solver if available.
    fn quantum_inspired_solver(&self) -> Option<Arc<dyn QuboSolver>>;

    /// Get the quantum provider if available.
    fn quantum_provider(&self) -> Option<Arc<dyn QuboSolver>>;

    /// List all registered solvers.
    fn all_solvers(&self) -> Vec<Arc<dyn QuboSolver>>;

    /// Solve a problem with the classical solver.
    fn solve_classical(&self, request: &DecisionRequest) -> Result<SolverResult> {
        let solver = self.classical_solver();
        self.solve_with_solver(solver, request)
    }

    /// Solve a problem with the quantum provider if available.
    fn solve_quantum(&self, request: &DecisionRequest) -> Result<Option<SolverResult>> {
        match self.quantum_provider() {
            Some(solver) => {
                let result = self.solve_with_solver(solver, request)?;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    /// Solve a problem with a specific solver.
    ///
    /// This is a helper method that handles the common pattern of converting
    /// a solver's output into our SolverResult type.
    fn solve_with_solver(
        &self,
        solver: Arc<dyn QuboSolver>,
        request: &DecisionRequest,
    ) -> Result<SolverResult>;
}

/// A local registry with classical, quantum-inspired, and optional quantum solvers.
#[allow(missing_debug_implementations)]
pub struct LocalRegistry {
    classical: Arc<dyn QuboSolver>,
    quantum_inspired: Option<Arc<dyn QuboSolver>>,
    quantum: Option<Arc<dyn QuboSolver>>,
}

impl LocalRegistry {
    /// Create a new registry with a classical solver and optional additional solvers.
    pub fn new(
        classical: Arc<dyn QuboSolver>,
        quantum_inspired: Option<Arc<dyn QuboSolver>>,
        quantum: Option<Arc<dyn QuboSolver>>,
    ) -> Result<Self> {
        if !classical.is_available() {
            return Err(Error::denied(
                "classical solver must be available when registering",
            ));
        }

        Ok(Self {
            classical,
            quantum_inspired,
            quantum,
        })
    }
}

impl SolverRegistry for LocalRegistry {
    fn classical_solver(&self) -> Arc<dyn QuboSolver> {
        Arc::clone(&self.classical)
    }

    fn quantum_inspired_solver(&self) -> Option<Arc<dyn QuboSolver>> {
        self.quantum_inspired.as_ref().map(Arc::clone)
    }

    fn quantum_provider(&self) -> Option<Arc<dyn QuboSolver>> {
        self.quantum.as_ref().map(Arc::clone)
    }

    fn all_solvers(&self) -> Vec<Arc<dyn QuboSolver>> {
        let mut solvers = vec![Arc::clone(&self.classical)];
        if let Some(ref qi) = self.quantum_inspired {
            solvers.push(Arc::clone(qi));
        }
        if let Some(ref q) = self.quantum {
            solvers.push(Arc::clone(q));
        }
        solvers
    }

    fn solve_with_solver(
        &self,
        _solver: Arc<dyn QuboSolver>,
        _request: &DecisionRequest,
    ) -> Result<SolverResult> {
        // Convert the DecisionRequest to the format the solver expects.
        // For now, this is a placeholder that returns an error.
        // The actual implementation will depend on how the solver interface
        // expects to receive problems.
        Err(Error::io("solver integration not yet implemented"))
    }
}

/// A builder for constructing a SolverRegistry.
///
/// This builder ensures all required solvers are available and properly
/// configured before the registry is created.
#[allow(missing_debug_implementations)]
pub struct SolverRegistryBuilder {
    classical: Option<Arc<dyn QuboSolver>>,
    quantum_inspired: Option<Arc<dyn QuboSolver>>,
    quantum: Option<Arc<dyn QuboSolver>>,
}

impl SolverRegistryBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            classical: None,
            quantum_inspired: None,
            quantum: None,
        }
    }

    /// Set the classical solver (mandatory).
    pub fn with_classical(mut self, solver: Arc<dyn QuboSolver>) -> Self {
        self.classical = Some(solver);
        self
    }

    /// Set the quantum-inspired solver (optional).
    pub fn with_quantum_inspired(mut self, solver: Arc<dyn QuboSolver>) -> Self {
        self.quantum_inspired = Some(solver);
        self
    }

    /// Set the quantum provider (optional).
    pub fn with_quantum(mut self, solver: Arc<dyn QuboSolver>) -> Self {
        self.quantum = Some(solver);
        self
    }

    /// Build the registry.
    ///
    /// Returns an error if no classical solver has been configured.
    pub fn build(self) -> Result<LocalRegistry> {
        let classical = self
            .classical
            .ok_or_else(|| Error::denied("classical solver is required"))?;

        LocalRegistry::new(classical, self.quantum_inspired, self.quantum)
    }
}

impl Default for SolverRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_requires_available_classical_solver() {
        // We can't easily test this without a mock solver, but this documents
        // the requirement: a registry cannot be created without a working
        // classical solver.
    }

    #[test]
    fn builder_requires_classical_solver() {
        let builder = SolverRegistryBuilder::new();
        let result = builder.build();
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("classical"));
    }
}
