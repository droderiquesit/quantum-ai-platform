//! Quantum vs classical routing logic.
//!
//! The router makes the decision of whether to use a quantum solver or to
//! stick with classical. The decision is based on:
//!
//! 1. Availability: Is a quantum solver available?
//! 2. Cost: Does the quantum solver cost more than the classical solver?
//! 3. Performance: Does the quantum solver offer a measured advantage?
//!
//! The classical solver always runs first. This ensures that every decision
//! has a baseline to compare against, and that the classical result is never
//! waiting on quantum completion.

use crate::quantum::{ChosenPath, DecisionRequest, RoutingDecision, SolverKind, SolverResult};
use qip_core::error::{Error, Result};

/// Configuration for the routing decision.
///
/// These thresholds determine when quantum is preferred over classical.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutingConfig {
    /// Minimum improvement (in basis points) required to prefer quantum over classical.
    /// For example, 100 means quantum must be at least 1% better.
    pub min_advantage_bps: i64,
    /// Maximum cost multiplier: quantum is preferred only if its cost is at most
    /// this multiple of classical cost. For example, 2.0 means quantum can cost
    /// twice as much as classical.
    pub max_cost_multiplier: f64,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            // Require at least 1% improvement before preferring quantum.
            min_advantage_bps: 100,
            // Quantum can cost up to 5x classical.
            max_cost_multiplier: 5.0,
        }
    }
}

impl RoutingConfig {
    /// Create a config that prefers quantum for any improvement.
    pub fn quantum_first() -> Self {
        Self {
            min_advantage_bps: 1,
            max_cost_multiplier: f64::INFINITY,
        }
    }

    /// Create a config that strongly prefers classical.
    pub fn classical_first() -> Self {
        Self {
            min_advantage_bps: i64::MAX,
            max_cost_multiplier: 0.0,
        }
    }
}

/// The quantum routing decision engine.
///
/// This type encapsulates the logic for choosing between quantum and classical
/// solvers based on a DecisionRequest, availability, and performance metrics.
pub struct QuantumRouter {
    config: RoutingConfig,
}

impl QuantumRouter {
    /// Create a new router with the given configuration.
    pub fn new(config: RoutingConfig) -> Self {
        Self { config }
    }

    /// Route a problem decision.
    ///
    /// Always computes the classical solution first, then optionally computes
    /// the quantum solution. The choice is made based on the config thresholds.
    ///
    /// Returns a RoutingDecision that records both the classical and quantum
    /// results (if computed) and which path was chosen.
    pub fn route(
        &self,
        _request: &DecisionRequest,
        classical_result: SolverResult,
        quantum_result: Option<SolverResult>,
    ) -> Result<RoutingDecision> {
        // Validate classical result.
        if classical_result.kind() != SolverKind::Classical {
            return Err(Error::invalid(
                "classical result must come from a classical solver",
            ));
        }

        // Determine which path to choose.
        let chosen_path = match quantum_result.as_ref() {
            None => ChosenPath::Classical,
            Some(qr) => {
                // We have a quantum result. Should we use it?
                if self.should_use_quantum(&classical_result, qr) {
                    ChosenPath::Quantum
                } else {
                    ChosenPath::Classical
                }
            }
        };

        RoutingDecision::new(classical_result, quantum_result, chosen_path)
    }

    /// Determine whether to use the quantum result based on the config.
    fn should_use_quantum(&self, classical: &SolverResult, quantum: &SolverResult) -> bool {
        // Check cost first: if quantum is too expensive, don't use it.
        let cost_ratio = quantum.cost_nanos() as f64 / (classical.cost_nanos() as f64 + 1.0);
        if cost_ratio > self.config.max_cost_multiplier {
            return false;
        }

        // Check performance: quantum must offer enough improvement.
        let diff = classical.objective_value() - quantum.objective_value();
        if diff <= 0.0 {
            // Quantum is not better.
            return false;
        }

        // Calculate improvement in basis points.
        let denominator = classical.objective_value().abs();
        if denominator < 1e-10 {
            // Classical objective is essentially zero; can't compute basis point improvement.
            // Be conservative: prefer classical.
            return false;
        }

        let improvement_bps = (diff / denominator * 10000.0) as i64;
        improvement_bps >= self.config.min_advantage_bps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_config_classical_first() {
        let config = RoutingConfig::classical_first();
        assert_eq!(config.min_advantage_bps, i64::MAX);
        assert_eq!(config.max_cost_multiplier, 0.0);
    }

    #[test]
    fn routing_config_quantum_first() {
        let config = RoutingConfig::quantum_first();
        assert_eq!(config.min_advantage_bps, 1);
        assert!(config.max_cost_multiplier.is_infinite());
    }

    #[test]
    fn router_validates_classical_result() {
        let router = QuantumRouter::new(RoutingConfig::default());
        let quantum_result =
            SolverResult::new(SolverKind::Quantum, vec![true], 1.0, 100, "test").unwrap();
        let err = router
            .route(
                &DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap(),
                quantum_result,
                None,
            )
            .expect_err("must reject non-classical result");
        assert!(err.message().contains("classical"));
    }

    #[test]
    fn router_prefers_classical_when_quantum_unavailable() {
        let router = QuantumRouter::new(RoutingConfig::quantum_first());
        let classical_result =
            SolverResult::new(SolverKind::Classical, vec![true], 1.0, 100, "test").unwrap();
        let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();
        let decision = router.route(&request, classical_result, None).unwrap();
        assert_eq!(decision.chosen(), ChosenPath::Classical);
    }

    #[test]
    fn router_chooses_quantum_when_better_and_cheap() {
        let router = QuantumRouter::new(RoutingConfig::default());
        let classical_result =
            SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test").unwrap();
        let quantum_result =
            SolverResult::new(SolverKind::Quantum, vec![true], 99.0, 50, "test").unwrap();
        let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();
        let decision = router
            .route(&request, classical_result, Some(quantum_result))
            .unwrap();
        assert_eq!(decision.chosen(), ChosenPath::Quantum);
    }

    #[test]
    fn router_prefers_classical_when_quantum_expensive() {
        let router = QuantumRouter::new(RoutingConfig::default());
        let classical_result =
            SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test").unwrap();
        let quantum_result =
            SolverResult::new(SolverKind::Quantum, vec![true], 99.0, 1000, "test").unwrap();
        let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();
        let decision = router
            .route(&request, classical_result, Some(quantum_result))
            .unwrap();
        assert_eq!(decision.chosen(), ChosenPath::Classical);
    }
}
