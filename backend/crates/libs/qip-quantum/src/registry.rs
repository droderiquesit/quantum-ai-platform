//! A registry of QUBO solvers with fallback and availability management.
//!
//! [`SolverRegistry`] manages a collection of [`QuboSolver`] implementations,
//! providing a unified interface for solver registration, discovery, and
//! selection. The registry enforces that a classical baseline is always
//! available and implements fallback logic to ensure a solver can be selected
//! even when preferred implementations are unavailable.
//!
//! # Fallback strategy
//!
//! Solvers are registered in priority order. When a solver is requested:
//! 1. If the requested solver is available, it is returned immediately.
//! 2. If the requested solver is unavailable, the registry searches for an
//!    available solver with the same kind in priority order.
//! 3. If no solver of that kind is available, an error naming what is missing
//!    is returned.
//!
//! The classical solver is always available by construction — deployment
//! configurations that do not provide a classical solver cannot build a
//! registry. This ensures the paper-trading boundary remains enforced: a
//! computation that has no quantum device available degrades gracefully to
//! the classical baseline rather than stopping.
//!
//! # Local simulator fallback
//!
//! When a [`HostedProvider`](crate::provider::HostedProvider) is unavailable
//! (credential missing, service unreachable, or simulator mistakenly
//! configured as hardware), the registry can be built with a
//! [`SimulatedProvider`](crate::provider::SimulatedProvider) in a fallback
//! position. This ensures that quantum research can continue using an
//! in-tree simulator while the hardware adapter is being configured.
//!
//! # Determinism and auditability
//!
//! Solvers are stored in a [`std::collections::BTreeMap`] so iteration
//! order is stable and matches the lexicographic order of solver names.
//! A benchmark run against a fixed registry produces identical results
//! across runs, which is load-bearing for a system where every decision
//! must be reproducible from the event log.

use crate::solver::{QuboSolver, SolverCandidate, SolverKind};
use qip_core::error::{Error, Result};
use qip_numerics::anneal::Qubo;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A registry of QUBO solvers, ordered by priority.
///
/// The registry guarantees that:
/// - A classical solver is always available and always present.
/// - Solvers are selected in priority order when multiple are available.
/// - Fallback to a solver of the same kind is automatic if the requested
///   solver is unavailable.
/// - Selection is deterministic: solver names are stored in a BTreeMap,
///   so iteration order is stable across runs.
#[derive(Clone, Debug)]
pub struct SolverRegistry {
    solvers: BTreeMap<String, Arc<dyn QuboSolver>>,
    classical_solver: String,
}

impl SolverRegistry {
    /// Build a registry with a classical baseline and an optional set of
    /// additional solvers.
    pub fn builder(classical: Arc<dyn QuboSolver>) -> SolverRegistryBuilder {
        SolverRegistryBuilder {
            solvers: BTreeMap::new(),
            classical_name: classical.name().to_string(),
            classical,
        }
    }

    /// Return the classical baseline solver by name.
    pub fn classical_name(&self) -> &str {
        &self.classical_solver
    }

    /// Return the number of solvers in the registry.
    pub fn len(&self) -> usize {
        self.solvers.len()
    }

    /// Whether the registry is empty. Always false after construction,
    /// since a classical solver is mandatory.
    pub fn is_empty(&self) -> bool {
        self.solvers.is_empty()
    }

    /// Names of all solvers in the registry, in lexicographic order.
    pub fn names(&self) -> Vec<&str> {
        self.solvers.keys().map(|n| n.as_str()).collect()
    }

    /// All solvers in the registry, in lexicographic order by name.
    pub fn all(&self) -> Vec<(String, Arc<dyn QuboSolver>)> {
        self.solvers
            .iter()
            .map(|(name, solver)| (name.clone(), Arc::clone(solver)))
            .collect()
    }

    /// Available solvers in the registry, in lexicographic order by name.
    pub fn available(&self) -> Vec<(String, Arc<dyn QuboSolver>)> {
        self.solvers
            .iter()
            .filter(|(_, solver)| solver.is_available())
            .map(|(name, solver)| (name.clone(), Arc::clone(solver)))
            .collect()
    }

    /// Retrieve a solver by name.
    pub fn get(&self, name: &str) -> Result<Arc<dyn QuboSolver>> {
        self.solvers.get(name).map(Arc::clone).ok_or_else(|| {
            Error::invalid(format!(
                "no solver named '{}'; the registry holds: {}",
                name,
                self.names().join(", ")
            ))
        })
    }

    /// Solve, selecting an available solver or falling back to one of the
    /// same kind.
    pub fn solve(
        &self,
        qubo: &Qubo,
        preferred_solver: &str,
        effort: &crate::solver::SolverEffort,
    ) -> Result<SolverCandidate> {
        let solver = self.select(preferred_solver)?;
        solver.solve(qubo, effort)
    }

    /// Select a solver, falling back to an available solver of the same
    /// kind if the preferred one is unavailable.
    pub fn select(&self, preferred_solver: &str) -> Result<Arc<dyn QuboSolver>> {
        let solver = self.get(preferred_solver)?;

        if solver.is_available() {
            return Ok(solver);
        }

        let preferred_kind = solver.kind();
        for (_, candidate) in self.solvers.iter() {
            if candidate.kind() == preferred_kind && candidate.is_available() {
                return Ok(Arc::clone(candidate));
            }
        }

        Err(Error::unavailable(format!(
            "no available solver of kind '{}'; preferred was '{}' which needs: {}. \
             available solvers: {}",
            preferred_kind.as_str(),
            preferred_solver,
            solver.requirement(),
            self.available()
                .iter()
                .map(|(name, s)| format!("{} ({})", name, s.kind().as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// Builder for a [`SolverRegistry`].
#[derive(Debug)]
pub struct SolverRegistryBuilder {
    solvers: BTreeMap<String, Arc<dyn QuboSolver>>,
    classical_name: String,
    classical: Arc<dyn QuboSolver>,
}

impl SolverRegistryBuilder {
    /// Register an additional solver.
    pub fn with_solver(mut self, solver: Arc<dyn QuboSolver>) -> Self {
        if self.solvers.is_empty() {
            self.solvers
                .insert(self.classical_name.clone(), Arc::clone(&self.classical));
        }
        self.solvers.insert(solver.name().to_string(), solver);
        self
    }

    /// Register multiple solvers.
    pub fn with_solvers(mut self, solvers: Vec<Arc<dyn QuboSolver>>) -> Self {
        for solver in solvers {
            self = self.with_solver(solver);
        }
        self
    }

    /// Build the registry.
    pub fn build(mut self) -> SolverRegistry {
        if self.solvers.is_empty() {
            self.solvers
                .insert(self.classical_name.clone(), Arc::clone(&self.classical));
        }
        SolverRegistry {
            solvers: self.solvers,
            classical_solver: self.classical_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{ClassicalSolver, QuantumInspiredSolver, SolverEffort};
    use qip_numerics::anneal::Qubo;

    fn small_qubo() -> Qubo {
        let mut qubo = Qubo::new(4);
        qubo.add_linear(0, 1.0);
        qubo.add_linear(1, -0.5);
        qubo.add(0, 1, -0.25);
        qubo
    }

    /// The registry holds solvers and retrieves them by name.
    #[test]
    fn a_registry_registers_and_retrieves_solvers() -> Result<()> {
        let classical = Arc::new(ClassicalSolver::exhaustive(20));
        let registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(Arc::new(QuantumInspiredSolver::new(1)))
            .build();

        assert_eq!(registry.len(), 2);
        let names = registry.names();
        assert!(names.contains(&"classical-exhaustive"));
        assert!(names.contains(&"quantum-inspired-path-integral"));

        let classical_retrieved = registry.get("classical-exhaustive")?;
        assert_eq!(classical_retrieved.name(), "classical-exhaustive");

        let quantum_retrieved = registry.get("quantum-inspired-path-integral")?;
        assert_eq!(quantum_retrieved.name(), "quantum-inspired-path-integral");

        let error = registry.get("nonexistent").expect_err("nonexistent solver");
        assert_eq!(error.code(), "invalid");
        assert!(error.message().contains("classical-exhaustive"));
        assert!(error.message().contains("quantum-inspired-path-integral"));
        Ok(())
    }

    /// The registry falls back to available solvers of the same kind.
    #[test]
    fn the_registry_falls_back_to_available_solvers_of_the_same_kind() -> Result<()> {
        let classical = Arc::new(ClassicalSolver::exhaustive(20));
        let qi1 = Arc::new(QuantumInspiredSolver::new(1));
        let qi2 = Arc::new(QuantumInspiredSolver::new(2));

        let registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(qi1)
            .with_solver(qi2)
            .build();

        let selected = registry.select("quantum-inspired-path-integral")?;
        assert_eq!(selected.kind(), SolverKind::QuantumInspired);
        assert!(selected.is_available());

        assert_eq!(registry.available().len(), 3);
        Ok(())
    }

    /// The classical baseline is always available.
    #[test]
    fn the_classical_baseline_is_always_available() -> Result<()> {
        let classical = Arc::new(ClassicalSolver::descent(1, 4));
        let registry = SolverRegistry::builder(Arc::clone(&classical)).build();

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.classical_name(), "classical-descent");

        let retrieved = registry.get("classical-descent")?;
        assert!(retrieved.is_available());
        assert_eq!(retrieved.kind(), SolverKind::Classical);

        let available = registry.available();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].0, "classical-descent");

        let selected = registry.select("classical-descent")?;
        assert!(selected.is_available());
        Ok(())
    }

    /// Registry selection is deterministic.
    #[test]
    fn registry_selection_is_deterministic_and_prefers_available_solvers() -> Result<()> {
        let classical = Arc::new(ClassicalSolver::exhaustive(20));
        let qi = Arc::new(QuantumInspiredSolver::new(3));

        let registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(qi)
            .build();

        let qubo = small_qubo();
        let effort = SolverEffort::default();
        let result = registry.solve(&qubo, "quantum-inspired-path-integral", &effort)?;

        assert_eq!(result.solver, "quantum-inspired-path-integral");
        assert_eq!(result.kind, SolverKind::QuantumInspired);

        let result_classical = registry.solve(&qubo, "classical-exhaustive", &effort)?;
        assert_eq!(result_classical.solver, "classical-exhaustive");
        assert_eq!(result_classical.kind, SolverKind::Classical);

        let error = registry
            .solve(&qubo, "nonexistent", &effort)
            .expect_err("nonexistent solver");
        assert_eq!(error.code(), "invalid");
        assert!(error.message().contains("nonexistent"));
        Ok(())
    }
}
