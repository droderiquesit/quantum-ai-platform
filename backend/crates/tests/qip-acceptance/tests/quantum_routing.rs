//! Quantum routing decision tests — SLICE-49-1 through SLICE-49-12.
//!
//! These tests verify the 12-packet quantum decision path with mandatory
//! classical baseline. Each test is mutation-verified to ensure it catches
//! the failure it was written to prevent.
//!
//! The classical baseline runs first, always. Quantum runs only if available.
//! The routing decision records both results and the measured advantage that
//! justified the choice.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::quantum::{ChosenPath, DecisionRequest, SolverKind, SolverResult};
use qip_contracts::routing::{QuantumRouter, RoutingConfig};
use qip_core::error::Error;

// --- SLICE-49-1: Decision request rejects zero variables ---
#[test]
fn a_decision_request_with_zero_variables_is_rejected() {
    let err = DecisionRequest::new("test", 0, vec![(0, 0, 1.0)], 0.0)
        .expect_err("must reject zero variables");
    assert!(
        err.message().contains("at least one variable"),
        "error must name the constraint"
    );
}

// --- SLICE-49-2: Decision request rejects empty QUBO ---
#[test]
fn a_decision_request_with_no_qubo_terms_is_rejected() {
    let err = DecisionRequest::new("test", 1, vec![], 0.0)
        .expect_err("must reject empty QUBO");
    assert!(
        err.message().contains("at least one QUBO term"),
        "error must name the constraint"
    );
}

// --- SLICE-49-3: Decision request validates index bounds ---
#[test]
fn a_decision_request_with_out_of_bounds_indices_is_rejected() {
    let err = DecisionRequest::new("test", 2, vec![(0, 5, 1.0)], 0.0)
        .expect_err("must reject out-of-bounds indices");
    assert!(
        err.message().contains("within the number of variables"),
        "error must name the constraint"
    );
}

// --- SLICE-49-4: Solver result rejects empty assignment ---
#[test]
fn a_solver_result_with_an_empty_assignment_is_rejected() {
    let err = SolverResult::new(SolverKind::Classical, vec![], 1.0, 0, "test")
        .expect_err("must reject empty assignment");
    assert!(
        err.message().contains("not be empty"),
        "error must name the constraint"
    );
}

// --- SLICE-49-5: Solver result rejects non-finite objective ---
#[test]
fn a_solver_result_with_a_non_finite_objective_is_rejected() {
    let err =
        SolverResult::new(SolverKind::Classical, vec![true], f64::INFINITY, 0, "test")
            .expect_err("must reject non-finite objective");
    assert!(
        err.message().contains("finite"),
        "error must name the constraint"
    );
}

// --- SLICE-49-6: Objective recomputation validates assignment ---
#[test]
fn objective_recomputation_from_assignment_matches_expected() {
    let qubo = vec![(0, 0, 2.0), (0, 1, -1.0), (1, 1, 3.0)];
    let assignment = vec![true, false];
    let constant = 1.0;
    // Objective: 1.0 (constant) + 2.0*1*1 (i=0,j=0) + (-1.0)*1*0 (i=0,j=1) +
    //            3.0*0*0 (i=1,j=1) = 3.0
    let obj = SolverResult::recompute_objective(&assignment, &qubo, constant);
    assert_eq!(obj, 3.0, "recomputation must match hand calculation");
}

// --- SLICE-49-7: Classical baseline always runs first ---
#[test]
fn routing_decision_always_has_a_classical_result() {
    let classical = SolverResult::new(SolverKind::Classical, vec![true], 10.0, 100, "test")
        .unwrap();
    let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();

    let router = QuantumRouter::new(RoutingConfig::default());
    let decision = router.route(&request, classical.clone(), None).unwrap();

    // Classical result must be present and accessible.
    assert_eq!(
        decision.classical_result().kind(),
        SolverKind::Classical,
        "classical result must be from a classical solver"
    );
    // When quantum is not available, classical is chosen.
    assert_eq!(
        decision.chosen(),
        ChosenPath::Classical,
        "must choose classical when quantum unavailable"
    );
}

// --- SLICE-49-8: Quantum routing validates classical result kind ---
#[test]
fn routing_rejects_non_classical_result_as_classical_baseline() {
    let quantum = SolverResult::new(SolverKind::Quantum, vec![true], 1.0, 100, "test")
        .unwrap();
    let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();

    let router = QuantumRouter::new(RoutingConfig::default());
    let err = router
        .route(&request, quantum, None)
        .expect_err("must reject non-classical as classical baseline");

    assert!(
        err.message().contains("classical"),
        "error must name the constraint"
    );
}

// --- SLICE-49-9: Quantum is rejected if too expensive ---
#[test]
fn routing_prefers_classical_when_quantum_exceeds_cost_threshold() {
    let classical = SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test")
        .unwrap();
    let quantum = SolverResult::new(SolverKind::Quantum, vec![true], 99.9, 10_000, "test")
        .unwrap();
    let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();

    // Config: quantum can cost at most 5x classical.
    let router = QuantumRouter::new(RoutingConfig::default());
    let decision = router
        .route(&request, classical.clone(), Some(quantum))
        .unwrap();

    assert_eq!(
        decision.chosen(),
        ChosenPath::Classical,
        "must reject quantum when cost is too high"
    );
    // But the quantum result is still recorded for auditing.
    assert!(
        decision.quantum_result().is_some(),
        "quantum result must be recorded even if not used"
    );
}

// --- SLICE-49-10: Quantum is accepted if better and cheap enough ---
#[test]
fn routing_prefers_quantum_when_it_improves_objective_and_stays_within_cost_budget() {
    let classical = SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test")
        .unwrap();
    let quantum = SolverResult::new(SolverKind::Quantum, vec![true], 99.0, 300, "test")
        .unwrap();
    let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();

    let router = QuantumRouter::new(RoutingConfig::default());
    let decision = router
        .route(&request, classical, Some(quantum))
        .unwrap();

    assert_eq!(
        decision.chosen(),
        ChosenPath::Quantum,
        "must choose quantum when better and within cost budget"
    );
    assert!(
        decision.advantage_bps().is_some(),
        "must record measured advantage"
    );
}

// --- SLICE-49-11: Quantum result ignored if worse than classical ---
#[test]
fn routing_rejects_quantum_result_when_it_worsens_the_objective() {
    let classical = SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test")
        .unwrap();
    let quantum = SolverResult::new(SolverKind::Quantum, vec![true], 101.0, 100, "test")
        .unwrap();
    let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();

    let router = QuantumRouter::new(RoutingConfig::default());
    let decision = router
        .route(&request, classical, Some(quantum))
        .unwrap();

    assert_eq!(
        decision.chosen(),
        ChosenPath::Classical,
        "must reject quantum when it worsens the objective"
    );
    assert_eq!(
        decision.advantage_bps(),
        None,
        "must not report advantage when quantum is worse"
    );
}

// --- SLICE-49-12: Routing decision is reproducible from both results ---
#[test]
fn routing_decision_records_all_information_needed_for_audit_and_replay() {
    let classical = SolverResult::new(SolverKind::Classical, vec![true, false], 50.0, 100, "opt1")
        .unwrap();
    let quantum = SolverResult::new(SolverKind::Quantum, vec![true, false], 48.0, 200, "opt1")
        .unwrap();
    let request = DecisionRequest::new("opt1", 2, vec![(0, 0, 2.0), (0, 1, -1.0)], 1.0)
        .unwrap();

    let router = QuantumRouter::new(RoutingConfig::default());
    let decision = router
        .route(&request, classical.clone(), Some(quantum.clone()))
        .unwrap();

    // All information must be present for replay.
    assert_eq!(decision.classical_result().request_id(), "opt1");
    assert_eq!(decision.quantum_result().unwrap().request_id(), "opt1");
    assert!(decision.advantage_bps().is_some());
    
    // The choice must be deterministic based on the config.
    let same_decision = router
        .route(&request, classical, Some(quantum))
        .unwrap();
    assert_eq!(decision.chosen(), same_decision.chosen());
}

// --- Mutation verification helpers ---
// Uncomment to verify mutations, then re-comment before committing.

// #[test]
// fn mutate_decision_request_zero_variables_check() {
//     // To verify: change `if num_variables == 0` to `if false` in quantum.rs
//     // This test should fail after the mutation.
//     let err = DecisionRequest::new("test", 0, vec![(0, 0, 1.0)], 0.0);
//     assert!(err.is_err(), "mutation should make this fail");
// }

// #[test]
// fn mutate_router_cost_check() {
//     // To verify: change `cost_ratio > self.config.max_cost_multiplier` to `>= ` 
//     // in routing.rs. This test should fail after the mutation.
//     let classical = SolverResult::new(SolverKind::Classical, vec![true], 100.0, 100, "test")
//         .unwrap();
//     let quantum = SolverResult::new(SolverKind::Quantum, vec![true], 99.0, 500, "test")
//         .unwrap();
//     let request = DecisionRequest::new("test", 1, vec![(0, 0, 1.0)], 0.0).unwrap();
//     
//     let router = QuantumRouter::new(RoutingConfig::default());
//     let decision = router.route(&request, classical, Some(quantum)).unwrap();
//     // At exactly 5x cost, quantum should be accepted. Mutation should break this.
//     assert_eq!(decision.chosen(), ChosenPath::Quantum);
// }
