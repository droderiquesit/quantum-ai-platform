//! ARCH-033 / ADR 0006: a classical baseline every time a quantum path runs.
//!
//! The previous version of this file had four tests and none of them could
//! fail on the property. `quantum_service_exists` was an empty body with a
//! comment saying so. `classical_baseline_computed_always` asserted that
//! `platform.rs` contained the word "baseline" *or* "classical", and skipped
//! itself if the file was missing. `quantum_results_validated_against_baseline`
//! ended its condition with `|| !content.is_empty()`. `qiskit_integration_for_qaoa`
//! asserted that `Cargo.lock` was non-empty. Deleting the router's classical
//! solve entirely would have passed all four.
//!
//! These drive the real chain: a `qip_quantum` provider attached to the
//! `qip_optimization_engine` router that the kernel's construction stage
//! calls, on a problem the router's own policy admits to the quantum path.
//!
//! Since the quantum arm went submit-and-poll, a quantum answer is judged in
//! the cycle *after* the one that asked, so the quantum path is driven across
//! two solves; and since no job is sent against a provably optimal baseline,
//! the baseline here is annealing (the exhaustive limit sits below the
//! problem size). The property is unchanged: whenever a quantum answer is
//! judged, a classical baseline was solved first on the same problem.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_optimization_engine::problem::{Objective, PortfolioProblem};
use qip_optimization_engine::router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver};
use qip_quantum::provider::SimulatedProvider;
use qip_quantum::qaoa::QaoaSettings;
use std::sync::Arc;

const ASSETS: usize = 10;

/// A discrete (cardinality-constrained) problem: the only kind the router
/// will send to a quantum path.
fn discrete_problem() -> Result<PortfolioProblem> {
    let assets: Vec<String> = (0..ASSETS).map(|i| format!("obj-A{i}")).collect();
    let covariance: Vec<Vec<f64>> = (0..ASSETS)
        .map(|i| {
            (0..ASSETS)
                .map(|j| if i == j { 0.04 } else { 0.012 })
                .collect()
        })
        .collect();
    Ok(PortfolioProblem::new(assets, covariance)?
        .with_objective(Objective::MeanVariance)
        .with_expected_returns((0..ASSETS).map(|i| 0.05 + 0.01 * i as f64).collect())
        .with_risk_aversion(5.0)
        .with_bounds(vec![0.0; ASSETS], vec![0.3; ASSETS])
        .fully_invested(1.0)
        .with_cardinality(4))
}

/// A policy that admits a ten-asset problem to the quantum path, with an
/// annealing baseline: the router sends no job against an exhaustive one,
/// because nothing can beat a proven optimum.
fn admitting_policy() -> RoutingPolicy {
    RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        qaoa: QaoaSettings {
            layers: 1,
            optimiser_iterations: 20,
            shots: 0,
        },
        ..RoutingPolicy::default()
    }
}

fn quantum_router(policy: RoutingPolicy) -> Result<ComputeRouter> {
    ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(SimulatedProvider::new(3).with_max_qubits(12)))
}

/// Two cycles, as the kernel drives them: the first solve submits and
/// answers classically, the job's worker finishes, the second judges it.
/// Asserts that the asking cycle adopted nothing — an inline call would.
fn judged_in_the_next_cycle(
    router: &ComputeRouter,
    problem: &PortfolioProblem,
) -> Result<RoutingDecision> {
    let first = router.solve(problem)?;
    assert!(
        first.quantum_dispatch.submitted.is_some(),
        "premise: the first cycle submitted a job: {}",
        first.quantum_note
    );
    assert!(
        first.run_for(Solver::Quantum).is_none(),
        "the cycle that asked also judged the answer, so the call was inline"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while router.quantum_job_ready()? != Some(true) {
        assert!(
            std::time::Instant::now() < deadline,
            "the simulated quantum job did not return within a minute"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let second = router.solve(problem)?;
    assert_eq!(
        second.quantum_dispatch.harvested,
        first.quantum_dispatch.submitted
    );
    Ok(second)
}

/// Mutation: in `ComputeRouter::solve`, move `let classical =
/// self.solve_classical(problem)?; ... runs.push(classical);` below the
/// quantum attempt — `runs[0]` is then the quantum run and the ordering
/// assertion fires. Deleting `runs.push(classical)` fails the count and the
/// "a classical run exists" assertion.
#[test]
fn when_the_quantum_path_runs_the_classical_baseline_is_solved_first_on_the_same_problem()
-> Result<()> {
    let problem = discrete_problem()?;
    let router = quantum_router(admitting_policy())?;
    let (attempt, note) = router.quantum_assessment(&problem);
    assert!(attempt, "premise: the policy admits this problem: {note}");

    let decision = judged_in_the_next_cycle(&router, &problem)?;

    // Premise: the quantum path actually ran, or "baseline beside it" is
    // satisfied by a run in which nothing else happened.
    let quantum = decision
        .run_for(Solver::Quantum)
        .unwrap_or_else(|| panic!("the quantum path did not run: {}", decision.rationale));
    assert_eq!(
        quantum.weights.len(),
        ASSETS,
        "the quantum run solved this problem"
    );

    assert_eq!(decision.runs.len(), 2, "one classical, one quantum");
    let baseline = &decision.runs[0];
    assert!(
        !baseline.solver.is_quantum(),
        "the first run is the classical baseline, solved before the quantum attempt"
    );
    assert_eq!(baseline.weights.len(), ASSETS, "same problem");
    assert_eq!(
        decision.classical_objective.to_bits(),
        baseline.objective.to_bits(),
        "the decision's baseline is the baseline run, not a number from elsewhere"
    );
    Ok(())
}

/// A quantum answer that does not clear the margin is not used and no
/// advantage is claimed. The margin here is `f64::MAX`, so no answer clears
/// it whatever the simulator returns and whether or not its answer is
/// feasible, so the test does not depend on the simulator being weak.
///
/// Mutation (always fires): in `ComputeRouter::solve`'s selection, replace
/// both `Some(quantum)` arms with one returning `(Solver::Quantum,
/// quantum.weights.clone(), quantum.objective, String::new())` — `chosen`
/// becomes `Quantum` and the `assert_ne!` fires. Mutation (fires when the
/// simulator's answer is feasible): replace `if improvement >
/// self.policy.quantum_margin` with `if true`.
#[test]
fn a_quantum_answer_inside_the_margin_is_never_chosen_and_no_advantage_is_claimed() -> Result<()> {
    let router = quantum_router(RoutingPolicy {
        quantum_margin: f64::MAX,
        ..admitting_policy()
    })?;
    let decision = judged_in_the_next_cycle(&router, &discrete_problem()?)?;
    // Premises: a quantum answer was judged, beside a classical baseline.
    assert!(
        decision.run_for(Solver::Quantum).is_some(),
        "{}",
        decision.rationale
    );
    assert!(!decision.runs[0].solver.is_quantum());

    assert_ne!(decision.chosen, Solver::Quantum, "{}", decision.rationale);
    assert_eq!(decision.measured_quantum_advantage(), None);
    assert_eq!(
        decision.objective.to_bits(),
        decision.classical_objective.to_bits()
    );
    assert!(decision.improvement_over_classical() >= 0.0);
    Ok(())
}

/// The default deployment has no provider. The quantum path does not run,
/// the classical answer is the only run, and the router says why in words an
/// operator can read.
///
/// Mutation: in `ComputeRouter::quantum_assessment`, change the no-provider
/// sentence — the equality fails. Making `solve` attempt quantum without a
/// provider is not expressible (it pattern-matches the provider), which is
/// the structural half of this guarantee.
#[test]
fn with_no_provider_configured_only_the_classical_baseline_runs_and_the_reason_is_recorded()
-> Result<()> {
    let decision = ComputeRouter::classical(1)
        .with_policy(admitting_policy())?
        .solve(&discrete_problem()?)?;
    assert_eq!(decision.runs.len(), 1);
    assert!(!decision.runs[0].solver.is_quantum());
    assert_ne!(decision.chosen, Solver::Quantum);
    assert_eq!(
        decision.quantum_note,
        "no quantum provider is configured; the classical solver is the default"
    );
    Ok(())
}
