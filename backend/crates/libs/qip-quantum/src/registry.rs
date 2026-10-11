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
    /// Rungs a caller runs outside the registry — the compute router's own
    /// classical baselines and its gateway-fronted QAOA arm — named here so
    /// their outcomes can be recorded against a name the registry knows.
    declared: BTreeMap<String, SolverKind>,
    /// The registry's measured view of each solver (R12-QUANT-4053). Empty
    /// until something is recorded: a standing nobody measured is absent,
    /// not zero.
    standings: BTreeMap<String, SolverStanding>,
}

/// One routed run's outcome, as the router measured it (§11.2 step 5).
///
/// Every figure is a statistic and stays `f64`; `cost_micros` is the
/// provider's declared price in USD micro-units, never converted into one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolverObservation {
    /// Variables in the problem the run answered.
    pub problem_size: usize,
    /// Improvement over the classical baseline, as a fraction. Zero for the
    /// baseline itself.
    pub quality: f64,
    /// Whether the run produced an answer that passed classical verification.
    pub verified: bool,
    /// Whether its answer was the one used.
    pub won: bool,
    /// Declared cost of the run.
    pub cost_micros: u64,
    /// Cycles between asking and reading the answer; zero for a run answered
    /// in the cycle that asked.
    pub latency_cycles: u32,
}

/// The registry's measured view of one solver: what it has cost, how long it
/// has taken, how often its answer verified and how often it was used.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SolverStanding {
    pub runs: u32,
    pub verified: u32,
    pub wins: u32,
    /// Sum of `quality` over verified runs; [`Self::mean_quality`] divides.
    pub quality_sum: f64,
    pub cost_micros: u64,
    /// Sum of `latency_cycles` over every run.
    pub latency_cycles: u64,
}

impl SolverStanding {
    /// Mean improvement over the baseline across verified runs, or `None`
    /// when no run verified — a mean of nothing is not zero.
    pub fn mean_quality(&self) -> Option<f64> {
        (self.verified > 0).then(|| self.quality_sum / f64::from(self.verified))
    }

    /// Mean cycles from asking to reading, or `None` before the first run.
    pub fn mean_latency_cycles(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.latency_cycles as f64 / f64::from(self.runs))
    }

    /// The share of runs whose answer verified, or `None` before the first.
    pub fn repeatability(&self) -> Option<f64> {
        (self.runs > 0).then(|| f64::from(self.verified) / f64::from(self.runs))
    }
}

impl SolverRegistry {
    /// Build a registry with a classical baseline and an optional set of
    /// additional solvers.
    pub fn builder(classical: Arc<dyn QuboSolver>) -> SolverRegistryBuilder {
        SolverRegistryBuilder {
            solvers: BTreeMap::new(),
            classical_name: classical.name().to_string(),
            classical,
            declared: BTreeMap::new(),
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

    /// The kind of a registered or declared solver.
    pub fn kind_of(&self, name: &str) -> Option<SolverKind> {
        self.solvers
            .get(name)
            .map(|solver| solver.kind())
            .or_else(|| self.declared.get(name).copied())
    }

    /// Record one run's outcome against the solver that produced it.
    ///
    /// Refuses a name the registry neither holds nor declares, and a quality
    /// that is not finite: a standing for a solver nobody registered is a
    /// record nobody can join back to a solver, and a non-finite mean poisons
    /// every later one.
    pub fn record(&mut self, solver: &str, observation: &SolverObservation) -> Result<()> {
        if self.kind_of(solver).is_none() {
            return Err(Error::invalid(format!(
                "no solver named '{solver}' is registered or declared, so its outcome has \
                 nothing to update; register or declare it first. The registry holds: {}",
                self.known_names().join(", ")
            )));
        }
        if !observation.quality.is_finite() {
            return Err(Error::invalid(format!(
                "a quality of {} for '{solver}' is not finite; refuse the run upstream rather \
                 than averaging it into the standing",
                observation.quality
            )));
        }
        let standing = self.standings.entry(solver.to_string()).or_default();
        standing.runs = standing.runs.saturating_add(1);
        if observation.verified {
            standing.verified = standing.verified.saturating_add(1);
            standing.quality_sum += observation.quality;
        }
        if observation.won {
            standing.wins = standing.wins.saturating_add(1);
        }
        standing.cost_micros = standing.cost_micros.saturating_add(observation.cost_micros);
        standing.latency_cycles = standing
            .latency_cycles
            .saturating_add(u64::from(observation.latency_cycles));
        Ok(())
    }

    /// The measured view of one solver, or `None` before its first record.
    pub fn standing(&self, solver: &str) -> Option<&SolverStanding> {
        self.standings.get(solver)
    }

    /// Every measured standing, in name order.
    pub fn standings(&self) -> &BTreeMap<String, SolverStanding> {
        &self.standings
    }

    fn known_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.solvers.keys().cloned().collect();
        names.extend(self.declared.keys().cloned());
        names.sort();
        names.dedup();
        names
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
    declared: BTreeMap<String, SolverKind>,
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

    /// Declare a rung its caller runs outside the registry, so its outcomes
    /// can be recorded. A declared name is never returned by
    /// [`SolverRegistry::select`]: the registry cannot run what it does not
    /// hold, and a name it could return and not run would be a fallback that
    /// fails at the moment it is needed.
    pub fn with_declared(mut self, name: impl Into<String>, kind: SolverKind) -> Self {
        self.declared.insert(name.into(), kind);
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
            declared: self.declared,
            standings: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)] // the assertion is the deliverable in a test
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
        let classical: Arc<dyn QuboSolver> = Arc::new(ClassicalSolver::exhaustive(20));
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
        let classical: Arc<dyn QuboSolver> = Arc::new(ClassicalSolver::exhaustive(20));
        let qi: Arc<dyn QuboSolver> = Arc::new(QuantumInspiredSolver::new(1));

        let registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(Arc::clone(&qi))
            .build();

        // Verify that selecting a registered quantum-inspired solver works
        let selected = registry.select("quantum-inspired-path-integral")?;
        assert_eq!(selected.kind(), SolverKind::QuantumInspired);
        assert!(selected.is_available());

        // Both solvers are available and registered
        assert_eq!(registry.available().len(), 2);
        assert_eq!(registry.len(), 2);
        Ok(())
    }

    /// The classical baseline is always available.
    #[test]
    fn the_classical_baseline_is_always_available() -> Result<()> {
        let classical: Arc<dyn QuboSolver> = Arc::new(ClassicalSolver::descent(1, 4));
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

    /// The registry's view of a solver changes when an outcome is recorded
    /// against it (R12-QUANT-4053), and an outcome for a solver it neither
    /// holds nor declares is refused rather than filed under a new name.
    ///
    /// Mutation: in `SolverRegistry::record`, delete the `if observation.won`
    /// increment — `wins` stays zero and the tuple assertion fires. Delete
    /// the `kind_of(solver).is_none()` refusal — the unknown name is accepted
    /// and the `expect_err` fires.
    #[test]
    fn recording_an_outcome_moves_the_standing_and_an_unknown_solver_is_refused() -> Result<()> {
        let classical: Arc<dyn QuboSolver> = Arc::new(ClassicalSolver::exhaustive(20));
        let mut registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(Arc::new(QuantumInspiredSolver::new(1)))
            .with_declared("quantum", SolverKind::Quantum)
            .build();
        assert!(
            registry.standing("quantum").is_none(),
            "premise: nothing has been measured yet"
        );

        registry.record(
            "quantum",
            &SolverObservation {
                problem_size: 23,
                quality: 0.02,
                verified: true,
                won: true,
                cost_micros: 7,
                latency_cycles: 3,
            },
        )?;
        let standing = registry
            .standing("quantum")
            .copied()
            .ok_or_else(|| Error::invalid("the recorded standing is missing"))?;
        assert_eq!((standing.runs, standing.verified, standing.wins), (1, 1, 1));
        assert_eq!(standing.cost_micros, 7);
        assert_eq!(standing.mean_latency_cycles(), Some(3.0));
        assert_eq!(standing.mean_quality(), Some(0.02));

        let refusal = registry
            .record(
                "nobody",
                &SolverObservation {
                    problem_size: 1,
                    quality: 0.0,
                    verified: true,
                    won: false,
                    cost_micros: 0,
                    latency_cycles: 0,
                },
            )
            .expect_err("an unregistered solver was given a standing");
        assert_eq!(refusal.code(), "invalid");
        assert!(registry.standing("nobody").is_none());
        Ok(())
    }

    /// Registry selection is deterministic.
    #[test]
    fn registry_selection_is_deterministic_and_prefers_available_solvers() -> Result<()> {
        let classical: Arc<dyn QuboSolver> = Arc::new(ClassicalSolver::exhaustive(20));
        let qi: Arc<dyn QuboSolver> = Arc::new(QuantumInspiredSolver::new(3));

        let registry = SolverRegistry::builder(Arc::clone(&classical))
            .with_solver(Arc::clone(&qi))
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
