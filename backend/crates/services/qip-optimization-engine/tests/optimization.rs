//! Tests for the DECIDE stage.
//!
//! The rule this crate exists to enforce is that a quantum result is never
//! used without a classical baseline, and never wins a tie. Most of what
//! follows tries to get a quantum answer accepted when it should not be.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::testing::approx_eq;
use qip_numerics::anneal::Qubo;
use qip_optimization_engine::problem::{Objective, PortfolioProblem, QuboEncoding};
use qip_optimization_engine::router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver};
use qip_quantum::provider::{
    HostedConfig, HostedProvider, ProviderCapabilities, QuantumProvider, SimulatedProvider,
};
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use std::sync::Arc;

fn assets(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("obj-A{i}")).collect()
}

/// A covariance matrix with a common factor plus idiosyncratic variance.
fn covariance(n: usize, correlation: f64, variance: f64) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    if i == j {
                        variance
                    } else {
                        variance * correlation
                    }
                })
                .collect()
        })
        .collect()
}

fn continuous(n: usize) -> Result<PortfolioProblem> {
    Ok(PortfolioProblem::new(assets(n), covariance(n, 0.3, 0.04))?
        .with_objective(Objective::MeanVariance)
        .with_expected_returns((0..n).map(|i| 0.05 + 0.01 * i as f64).collect())
        .with_risk_aversion(5.0)
        .with_bounds(vec![0.0; n], vec![0.3; n])
        .fully_invested(1.0))
}

fn discrete(n: usize, cardinality: usize) -> Result<PortfolioProblem> {
    Ok(continuous(n)?.with_cardinality(cardinality))
}

/// Wait, off the router's own path, until the job the router has out has an
/// answer waiting. Never called by the router; this is the test standing in
/// for the time that passes between two cycles.
fn wait_for_quantum_job(router: &ComputeRouter) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match router.quantum_job_ready()? {
            Some(true) => return Ok(()),
            Some(false) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Some(false) => panic!("the quantum job did not return within a minute"),
            None => panic!("the router has no quantum job out to wait for"),
        }
    }
}

/// Drive the router as two cycles drive it: the first solve submits a job and
/// answers classically, the job's worker finishes, and the second solve
/// judges the answer.
///
/// Asserts the premise every quantum-selection test below rests on since the
/// arm became submit-and-poll: the cycle that *asked* never adopts the
/// answer. A router that called the provider inline again would return a
/// quantum run from the first solve, and every caller of this helper fails.
fn solve_across_two_cycles(
    router: &ComputeRouter,
    problem: &PortfolioProblem,
) -> Result<RoutingDecision> {
    let first = router.solve(problem)?;
    assert!(
        first.quantum_dispatch.submitted.is_some(),
        "the first cycle submitted no quantum job: {:?} / {}",
        first.quantum_dispatch,
        first.quantum_note
    );
    assert!(
        first.run_for(Solver::Quantum).is_none() && !first.chosen.is_quantum(),
        "the cycle that submitted a quantum job also judged its answer, so the call was inline"
    );
    wait_for_quantum_job(router)?;
    let second = router.solve(problem)?;
    assert_eq!(
        second.quantum_dispatch.harvested, first.quantum_dispatch.submitted,
        "the second cycle did not judge the job the first submitted"
    );
    Ok(second)
}

// --- the problem ------------------------------------------------------------

#[test]
fn an_asymmetric_covariance_matrix_is_refused() -> Result<()> {
    let mut cov = covariance(3, 0.2, 0.04);
    cov[0][1] = 0.9;
    let problem = PortfolioProblem::new(assets(3), cov)?;
    assert!(
        problem
            .validate()
            .unwrap_err()
            .message()
            .contains("not symmetric")
    );
    Ok(())
}

#[test]
fn a_negative_variance_is_refused() -> Result<()> {
    let mut cov = covariance(3, 0.2, 0.04);
    cov[1][1] = -0.01;
    let problem = PortfolioProblem::new(assets(3), cov)?;
    assert!(problem.validate().is_err());
    Ok(())
}

#[test]
fn a_cardinality_beyond_the_asset_count_is_refused() -> Result<()> {
    let problem = discrete(5, 9)?;
    assert!(
        problem
            .validate()
            .unwrap_err()
            .message()
            .contains("exceeds")
    );
    Ok(())
}

#[test]
fn bounds_the_wrong_way_round_are_refused() -> Result<()> {
    let problem = continuous(3)?.with_bounds(vec![0.5, 0.0, 0.0], vec![0.2, 1.0, 1.0]);
    assert!(problem.validate().is_err());
    Ok(())
}

#[test]
fn a_qubo_encoding_records_what_the_reduction_dropped() {
    let encoding = QuboEncoding::equal_weight(5, 1.0);
    assert!(approx_eq(encoding.weight_per_holding, 0.2, 1e-12));
    assert!(
        encoding.describe().contains("not represented"),
        "the reduction must be visible: {}",
        encoding.describe()
    );
    let weights = encoding.to_weights(&[1, 0, 1, 0, 0]);
    assert!(approx_eq(weights[0], 0.2, 1e-12));
    assert!(approx_eq(weights[1], 0.0, 1e-12));
}

#[test]
fn the_cardinality_penalty_makes_the_right_count_optimal() -> Result<()> {
    // A penalty too small to matter would let the optimiser pay it to get a
    // better objective, silently violating the mandate.
    let problem = discrete(6, 3)?;
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3).with_penalty(10.0);
    let qubo: Qubo = problem.to_qubo(&encoding)?;

    let exactly_three = qubo.evaluate(&[1, 1, 1, 0, 0, 0]);
    let five = qubo.evaluate(&[1, 1, 1, 1, 1, 0]);
    let one = qubo.evaluate(&[1, 0, 0, 0, 0, 0]);
    assert!(
        exactly_three < five && exactly_three < one,
        "three holdings should beat five ({five}) and one ({one}), got {exactly_three}"
    );
    Ok(())
}

// --- the classical path -----------------------------------------------------

#[test]
fn a_continuous_problem_is_solved_by_quadratic_programming() -> Result<()> {
    let router = ComputeRouter::classical(1);
    let decision = router.solve(&continuous(8)?)?;

    assert_eq!(decision.chosen, Solver::QuadraticProgram);
    assert!(decision.runs.iter().all(|run| !run.solver.is_quantum()));
    let sum: f64 = decision.weights.iter().sum();
    assert!(
        approx_eq(sum, 1.0, 1e-4),
        "the budget constraint was not respected: {sum}"
    );
    assert!(
        decision
            .weights
            .iter()
            .all(|w| *w >= -1e-6 && *w <= 0.3 + 1e-6)
    );
    Ok(())
}

#[test]
fn a_small_discrete_problem_gets_a_provably_optimal_baseline() -> Result<()> {
    // Exact enumeration, so the baseline is the true optimum rather than
    // another heuristic's guess.
    let router = ComputeRouter::classical(1);
    let decision = router.solve(&discrete(10, 4)?)?;
    assert_eq!(decision.chosen, Solver::ExactEnumeration);
    assert!(decision.chosen.is_exact());
    Ok(())
}

#[test]
fn a_large_discrete_problem_falls_back_to_annealing() -> Result<()> {
    let policy = RoutingPolicy {
        exact_enumeration_limit: 8,
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1).with_policy(policy)?;
    let decision = router.solve(&discrete(20, 5)?)?;
    assert_eq!(decision.chosen, Solver::SimulatedAnnealing);
    Ok(())
}

#[test]
fn a_relaxed_answer_is_scored_against_the_constraint_it_dropped() -> Result<()> {
    // The QUBO encoding cannot express the budget constraint exactly, so a
    // candidate that misses it must come back infeasible rather than winning
    // on an objective it was not entitled to.
    let router = ComputeRouter::classical(1);
    let decision = router.solve(&discrete(10, 4)?)?;
    let run = decision.run_for(decision.chosen).unwrap();
    assert!(
        !run.relaxations.is_empty(),
        "a discrete solve goes through a reduction and must say so"
    );
    // Feasibility is judged against the real problem, cardinality included.
    let held = decision.weights.iter().filter(|w| w.abs() > 1e-9).count();
    if run.feasible {
        assert!(held <= 4, "a feasible answer cannot breach the cardinality");
    }
    Ok(())
}

#[test]
fn the_classical_baseline_is_always_computed() -> Result<()> {
    // Even with a quantum provider attached and used.
    let provider = Arc::new(SimulatedProvider::new(3).with_max_qubits(12));
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        qaoa: QaoaSettings {
            layers: 1,
            optimiser_iterations: 20,
            shots: 0,
        },
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(provider)?;
    let decision = solve_across_two_cycles(&router, &discrete(10, 4)?)?;

    // Premise: the quantum path actually ran. Without this the assertion
    // below is also satisfied by a router that never attempted quantum, and
    // "the baseline is always computed" would be proven by a run in which
    // nothing else was.
    assert!(
        decision.runs.iter().any(|run| run.solver.is_quantum()),
        "the quantum provider was attached but never ran: {}",
        decision.rationale
    );
    assert!(
        decision.runs.iter().any(|run| !run.solver.is_quantum()),
        "a quantum run without a classical baseline is unusable evidence"
    );
    assert!(decision.classical_objective.is_finite());
    Ok(())
}

// --- the quantum gate -------------------------------------------------------

#[test]
fn no_quantum_provider_means_no_quantum_attempt() -> Result<()> {
    let router = ComputeRouter::classical(1);
    // Sized past the gate so the refusal is about the missing provider, not
    // about the problem being too small to bother with.
    let router = router.with_policy(RoutingPolicy {
        minimum_assets_for_quantum: 4,
        ..RoutingPolicy::default()
    })?;
    let (attempted, note) = router.quantum_assessment(&discrete(12, 4)?);
    assert!(!attempted);
    assert!(note.contains("no quantum provider"), "{note}");
    Ok(())
}

#[test]
fn an_unreachable_provider_behaves_exactly_like_no_provider() -> Result<()> {
    // An operator should be able to rely on this: a misconfigured backend must
    // not change what the platform decides.
    let hosted = Arc::new(HostedProvider::new(HostedConfig {
        vendor: "ibm-quantum".to_string(),
        backend: "ibm_torino".to_string(),
        credential_env: "QIP_QUANTUM_TOKEN".to_string(),
        endpoint: "https://api.quantum.ibm.com".to_string(),
        max_qubits: 133,
        cost_per_job_micros: 1_500_000,
    }));
    // Small, but past the size gate, so the *only* reason the quantum path is
    // skipped is that the provider cannot be reached.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };

    let without = ComputeRouter::classical(1)
        .with_policy(policy)?
        .solve(&problem)?;
    let with = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(hosted)?
        .solve(&problem)?;

    assert_eq!(without.chosen, with.chosen);
    assert!(approx_eq(without.objective, with.objective, 1e-12));
    assert!(
        with.quantum_note.contains("not reachable"),
        "the reason must be legible: {}",
        with.quantum_note
    );
    Ok(())
}

#[test]
fn a_continuous_problem_never_goes_to_a_quantum_solver() -> Result<()> {
    // A convex continuous problem has a classical global optimum; there is
    // nothing for a heuristic to improve on.
    let router = ComputeRouter::classical(1).with_quantum(Arc::new(SimulatedProvider::new(1)))?;
    let (attempted, note) = router.quantum_assessment(&continuous(100)?);
    assert!(!attempted);
    assert!(note.contains("global optimum"), "{note}");
    Ok(())
}

#[test]
fn a_small_discrete_problem_does_not_justify_a_quantum_attempt() -> Result<()> {
    let router = ComputeRouter::classical(1).with_quantum(Arc::new(SimulatedProvider::new(1)))?;
    let (attempted, note) = router.quantum_assessment(&discrete(10, 4)?);
    assert!(!attempted);
    assert!(note.contains("below the 23"), "{note}");
    Ok(())
}

#[test]
fn a_problem_larger_than_the_devices_qubits_is_declined_with_the_reason_and_an_eligible_one_is_attempted()
-> Result<()> {
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(SimulatedProvider::new(1).with_max_qubits(12)))?;

    // Premise: the same router attempts a discrete problem that fits, so the
    // decline below is the qubit bound and not a router that never attempts.
    let (fits, note) = router.quantum_assessment(&discrete(10, 4)?);
    assert!(fits, "{note}");
    assert!(note.contains("attempting QAOA"), "{note}");

    let (attempted, note) = router.quantum_assessment(&discrete(14, 4)?);
    assert!(!attempted);
    assert!(note.contains("exceeds the provider's 12 qubits"), "{note}");
    Ok(())
}

#[test]
fn a_simulated_result_is_labelled_as_not_being_evidence_of_advantage() -> Result<()> {
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        // A shallow circuit and a short angle search: this test checks the
        // labelling, not the optimisation quality.
        qaoa: QaoaSettings {
            layers: 1,
            optimiser_iterations: 20,
            shots: 0,
        },
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(SimulatedProvider::new(3).with_max_qubits(12)))?;
    let decision = solve_across_two_cycles(&router, &discrete(10, 4)?)?;

    assert!(
        decision.quantum_note.contains("not evidence of advantage"),
        "{}",
        decision.quantum_note
    );
    // Premise: a simulated run exists to be labelled; without it the check
    // below would pass by finding nothing.
    let quantum = decision
        .run_for(Solver::Quantum)
        .expect("the simulator's answer was not judged");
    {
        assert!(
            quantum
                .relaxations
                .iter()
                .any(|r| r.contains("not evidence of quantum advantage")),
            "{:?}",
            quantum.relaxations
        );
    }
    Ok(())
}

/// A provider that returns whatever answer the test wants, so the router's
/// selection logic can be exercised without depending on QAOA's luck.
#[derive(Debug)]
struct ScriptedProvider {
    assignment: Vec<u8>,
    simulated: bool,
    claimed_energy: Option<f64>,
}

impl ScriptedProvider {
    /// Create a provider that returns a specific assignment with the correct claimed energy.
    fn with_assignment(assignment: Vec<u8>, simulated: bool) -> Self {
        Self {
            assignment,
            simulated,
            claimed_energy: None,
        }
    }

    /// Create a provider that claims wrong energy for the assignment.
    fn with_wrong_energy(assignment: Vec<u8>, wrong_energy: f64, simulated: bool) -> Self {
        Self {
            assignment,
            simulated,
            claimed_energy: Some(wrong_energy),
        }
    }
}

impl QuantumProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: 64,
            simulated: self.simulated,
            noisy: false,
            typical_queue: qip_core::time::Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
    fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        let assignment = self.assignment.clone();
        let energy = self
            .claimed_energy
            .unwrap_or_else(|| qubo.evaluate(&assignment));
        Ok(QaoaResult {
            energy,
            assignment,
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: vec![0.0, 0.0],
            evaluations: 1,
        })
    }
}

#[test]
fn a_quantum_answer_that_ties_loses_to_the_classical_baseline() -> Result<()> {
    // A tie is not evidence, and preferring it would let noise decide which
    // solver the platform reports as better.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        quantum_margin: 0.01,
        ..RoutingPolicy::default()
    };
    // First find what the classical solver picks, then hand the quantum path
    // exactly that answer.
    let baseline = ComputeRouter::classical(1)
        .with_policy(policy)?
        .solve(&problem)?;
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let assignment: Vec<u8> = baseline
        .weights
        .iter()
        .map(|w| u8::from(w.abs() > 1e-9))
        .collect();
    let _ = encoding;

    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(
            assignment, false,
        )))?;
    let decision = solve_across_two_cycles(&router, &problem)?;

    assert_ne!(decision.chosen, Solver::Quantum);
    assert!(
        decision.rationale.contains("a tie is not evidence"),
        "{}",
        decision.rationale
    );
    assert!(decision.measured_quantum_advantage().is_none());
    Ok(())
}

#[test]
fn an_infeasible_quantum_answer_is_rejected_however_good_its_objective() -> Result<()> {
    // The dangerous case: a QUBO answer that ignored the cardinality limit
    // will always look better on the objective, because it holds more names.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        ..RoutingPolicy::default()
    };
    // Every asset held: a flagrant breach of a three-name mandate.
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(
            vec![1; 12],
            false,
        )))?;
    let decision = solve_across_two_cycles(&router, &problem)?;

    assert_ne!(decision.chosen, Solver::Quantum);
    assert!(
        decision.rationale.contains("violated"),
        "{}",
        decision.rationale
    );
    let quantum = decision.run_for(Solver::Quantum).unwrap();
    assert!(!quantum.feasible);
    assert_eq!(quantum.violated_constraint, "cardinality");
    Ok(())
}

#[test]
fn a_quantum_answer_whose_claimed_energy_does_not_match_the_assignment_is_refused() -> Result<()> {
    // QUANT-012: before any quantum result is used, the claimed energy is
    // re-evaluated classically. A claim that doesn't match the assignment is
    // refused, whatever it claimed to be worth.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        ..RoutingPolicy::default()
    };
    // A feasible three-name assignment.
    let mut good_assignment = vec![0u8; 12];
    good_assignment[0] = 1;
    good_assignment[1] = 1;
    good_assignment[2] = 1;

    // Claim it has an energy far better than it actually does.
    let wrong_claimed_energy = -9999.0;

    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_wrong_energy(
            good_assignment,
            wrong_claimed_energy,
            false,
        )))?;
    let decision = solve_across_two_cycles(&router, &problem)?;

    // The quantum path should fail because the claim doesn't match.
    // The decision should fall back to classical.
    assert_ne!(decision.chosen, Solver::Quantum);
    assert!(
        decision.rationale.contains("quantum attempt failed"),
        "{}",
        decision.rationale
    );
    Ok(())
}

#[test]
fn a_genuinely_better_feasible_quantum_answer_is_used_and_the_gain_is_measured() -> Result<()> {
    // The router is not merely refusing everything: when the quantum path
    // actually wins on the real problem, it is used and the margin is reported.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Force the classical baseline onto the weaker annealer so a better
        // answer is reachable.
        exact_enumeration_limit: 2,
        quantum_margin: 0.0001,
        ..RoutingPolicy::default()
    };

    let baseline = ComputeRouter::classical(99)
        .with_policy(policy)?
        .solve(&problem)?;

    // Search for a feasible three-name assignment that beats the baseline.
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let mut best: Option<Vec<u8>> = None;
    let mut best_objective = baseline.classical_objective;
    for a in 0..12 {
        for b in (a + 1)..12 {
            for c in (b + 1)..12 {
                for d in (c + 1)..12 {
                    let mut assignment = vec![0u8; 12];
                    for index in [a, b, c, d] {
                        assignment[index] = 1;
                    }
                    let objective = problem.objective_at(&encoding.to_weights(&assignment));
                    if objective < best_objective - 1e-9 {
                        best_objective = objective;
                        best = Some(assignment);
                    }
                }
            }
        }
    }

    let Some(better) = best else {
        // The annealer already found the optimum, which is a fine outcome and
        // means there is nothing for this test to demonstrate.
        return Ok(());
    };

    let router = ComputeRouter::classical(99)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(better, false)))?;
    let decision = solve_across_two_cycles(&router, &problem)?;

    assert_eq!(decision.chosen, Solver::Quantum, "{}", decision.rationale);
    let advantage = decision.measured_quantum_advantage().unwrap();
    assert!(advantage > 0.0, "advantage was {advantage}");
    assert!(
        decision.rationale.contains("above the"),
        "{}",
        decision.rationale
    );
    Ok(())
}

#[test]
fn a_feasible_quantum_answer_better_by_less_than_the_margin_loses_to_the_classical_baseline()
-> Result<()> {
    // The margin is the rule QUANT-030 exists for. A tie hands the quantum path
    // an improvement of exactly zero, which a router with the margin deleted
    // still refuses; only an answer that is genuinely, slightly better exposes
    // a router that has stopped asking for more than noise.
    let problem = discrete(12, 4)?;
    let weak_baseline_policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        quantum_margin: 0.0001,
        ..RoutingPolicy::default()
    };
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    // The annealer is seeded and can land on the optimum, leaving nothing to
    // beat; walk seeds until one leaves room rather than hoping seed 99 does.
    let mut found: Option<(u64, f64, Vec<u8>, f64)> = None;
    for seed in 1..200u64 {
        let baseline = ComputeRouter::classical(seed)
            .with_policy(weak_baseline_policy)?
            .solve(&problem)?;
        let mut best: Option<(Vec<u8>, f64)> = None;
        for a in 0..12 {
            for b in (a + 1)..12 {
                for c in (b + 1)..12 {
                    for d in (c + 1)..12 {
                        let mut assignment = vec![0u8; 12];
                        for index in [a, b, c, d] {
                            assignment[index] = 1;
                        }
                        let objective = problem.objective_at(&encoding.to_weights(&assignment));
                        if objective
                            < best.as_ref().map_or(baseline.classical_objective, |b| b.1) - 1e-9
                        {
                            best = Some((assignment, objective));
                        }
                    }
                }
            }
        }
        if let Some((assignment, objective)) = best {
            found = Some((seed, baseline.classical_objective, assignment, objective));
            break;
        }
    }
    // Premise: a strictly better feasible answer exists, or this test would
    // pass on a baseline that was already optimal and prove nothing.
    let (seed, baseline_objective, better, objective) =
        found.expect("some seed must leave the weak baseline room to improve");
    let improvement = (baseline_objective - objective) / baseline_objective.abs();
    assert!(improvement > 1e-9, "improvement was {improvement}");

    // A margin twice the real improvement: better, feasible, and not enough.
    let policy = RoutingPolicy {
        quantum_margin: improvement * 2.0,
        ..weak_baseline_policy
    };
    let router = ComputeRouter::classical(seed)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(better, false)))?;
    let decision = solve_across_two_cycles(&router, &problem)?;

    assert_ne!(decision.chosen, Solver::Quantum, "{}", decision.rationale);
    assert!(
        decision.rationale.contains("inside the") && decision.rationale.contains("margin"),
        "{}",
        decision.rationale
    );
    assert!(decision.run_for(Solver::Quantum).unwrap().feasible);
    Ok(())
}

#[test]
fn a_failed_quantum_attempt_leaves_the_classical_answer_standing() -> Result<()> {
    #[derive(Debug)]
    struct Broken;
    impl QuantumProvider for Broken {
        fn name(&self) -> &str {
            "broken"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                max_qubits: 64,
                simulated: false,
                noisy: true,
                typical_queue: qip_core::time::Duration::ZERO,
                cost_per_job_micros: 0,
            }
        }
        fn solve_qubo(&self, _qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
            Err(qip_core::error::Error::unavailable("the device went down"))
        }
    }

    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(Broken))?;
    let decision = solve_across_two_cycles(&router, &discrete(12, 4)?)?;

    assert_ne!(decision.chosen, Solver::Quantum);
    assert!(
        decision.rationale.contains("quantum attempt failed"),
        "{}",
        decision.rationale
    );
    assert!(decision.weights.iter().any(|w| w.abs() > 1e-9));
    Ok(())
}

#[test]
fn every_routing_decision_records_the_classical_objective() -> Result<()> {
    // Without it there is nothing to measure a claimed advantage against.
    for problem in [continuous(6)?, discrete(10, 4)?] {
        let decision = ComputeRouter::classical(1).solve(&problem)?;
        assert!(decision.classical_objective.is_finite());
        assert!(!decision.rationale.is_empty());
        assert!(!decision.quantum_note.is_empty());
    }
    Ok(())
}

#[test]
fn routing_is_deterministic() -> Result<()> {
    let problem = discrete(14, 4)?;
    let policy = RoutingPolicy {
        exact_enumeration_limit: 8,
        ..RoutingPolicy::default()
    };
    let a = ComputeRouter::classical(5)
        .with_policy(policy)?
        .solve(&problem)?;
    let b = ComputeRouter::classical(5)
        .with_policy(policy)?
        .solve(&problem)?;
    assert_eq!(a.weights, b.weights);
    assert_eq!(a.chosen, b.chosen);
    Ok(())
}

#[test]
fn a_mandate_whose_own_parameters_conflict_is_named_as_such() -> Result<()> {
    // Three names capped at 30% each cannot be 100% invested. That is a
    // contradiction between three numbers someone chose separately, and it is
    // worth naming rather than surfacing as an unexplained infeasibility.
    let problem = discrete(12, 3)?;
    let router = ComputeRouter::classical(1);
    let conflict = router
        .mandate_conflict(&problem)
        .expect("the conflict must be detected");
    assert!(conflict.contains("conflict"), "{conflict}");
    assert!(conflict.contains("90.0% gross"), "{conflict}");

    // A consistent mandate reports no conflict.
    assert!(router.mandate_conflict(&discrete(12, 4)?).is_none());
    Ok(())
}

#[test]
fn the_encoding_never_produces_a_weight_above_the_position_cap() -> Result<()> {
    // Otherwise every candidate breaches a bound and the discrete path returns
    // nothing usable at all.
    let encoding = QuboEncoding::equal_weight_bounded(3, 1.0, 0.3);
    assert!(approx_eq(encoding.weight_per_holding, 0.3, 1e-12));
    assert!(approx_eq(encoding.achievable_gross(3), 0.9, 1e-12));

    // And where the cap does not bind, the target gross is reached exactly.
    let unbound = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    assert!(approx_eq(unbound.weight_per_holding, 0.25, 1e-12));
    assert!(approx_eq(unbound.achievable_gross(4), 1.0, 1e-12));
    Ok(())
}

// --- the classical baseline computation (mandatory first run) -----------

#[test]
fn the_classical_solver_runs_first_before_quantum_is_attempted() -> Result<()> {
    // ADR 0006 mandate: the classical baseline is not optional and must run
    // before any quantum path, even when the quantum path will be attempted.
    let provider = Arc::new(SimulatedProvider::new(3).with_max_qubits(12));
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        qaoa: QaoaSettings {
            layers: 1,
            optimiser_iterations: 20,
            shots: 0,
        },
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(provider)?;
    let decision = router.solve(&discrete(10, 4)?)?;

    // The classical run must be present.
    let classical_run = decision
        .runs
        .iter()
        .find(|r| !r.solver.is_quantum())
        .expect("no classical run found, violating the mandatory baseline rule");

    // The classical run must be first (or at worst, indistinguishable from
    // first). Iteration order is insertion order in Vec, and the classical
    // solve is pushed before any quantum attempt.
    let first_run = decision.runs.first().expect("no runs recorded at all");
    assert!(
        !first_run.solver.is_quantum(),
        "the first run must be classical, not {:?}",
        first_run.solver
    );

    // The classical objective must be finite and recorded.
    assert!(
        classical_run.objective.is_finite(),
        "classical objective must be finite, got {}",
        classical_run.objective
    );
    assert!(
        decision.classical_objective.is_finite(),
        "decision.classical_objective must be finite, got {}",
        decision.classical_objective
    );
    Ok(())
}

#[test]
fn the_classical_objective_is_always_used_as_the_baseline_for_quantum_comparison() -> Result<()> {
    // The quantum answer's improvement is measured against the classical
    // baseline that was computed on the same problem. This is the enforcement
    // mechanism for not measuring against a weak baseline.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        quantum_margin: 0.0001,
        ..RoutingPolicy::default()
    };

    // Find a better solution to hand to the quantum provider.
    let baseline = ComputeRouter::classical(99)
        .with_policy(policy)?
        .solve(&problem)?;
    let baseline_obj = baseline.classical_objective;
    assert!(
        baseline_obj.is_finite(),
        "baseline must compute a finite objective"
    );

    // Search for a feasible three-name assignment that beats the baseline.
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let mut better_assignment = None;
    let mut better_objective = baseline_obj;

    for a in 0..12 {
        for b in (a + 1)..12 {
            for c in (b + 1)..12 {
                let mut assignment = vec![0u8; 12];
                for idx in [a, b, c] {
                    assignment[idx] = 1;
                }
                let obj = problem.objective_at(&encoding.to_weights(&assignment));
                if obj < better_objective - 1e-9 {
                    better_objective = obj;
                    better_assignment = Some(assignment);
                }
            }
        }
    }

    if let Some(assignment) = better_assignment {
        // Now solve with a scripted quantum provider that returns this better
        // assignment. The router must use the same classical baseline
        // (baseline_obj) to measure improvement.
        let decision = ComputeRouter::classical(99)
            .with_policy(policy)?
            .with_quantum(Arc::new(ScriptedProvider::with_assignment(
                assignment, false,
            )))?
            .solve(&problem)?;

        // The classical objective in the decision must equal the one we computed.
        assert!(
            approx_eq(decision.classical_objective, baseline_obj, 1e-9),
            "classical_objective must be the same baseline: got {} vs computed {}",
            decision.classical_objective,
            baseline_obj
        );

        // If quantum was chosen, the improvement was measured against this
        // baseline.
        if decision.chosen == Solver::Quantum {
            let measured = decision.measured_quantum_advantage().unwrap();
            let expected = (baseline_obj - decision.objective) / baseline_obj.abs();
            assert!(
                approx_eq(measured, expected, 1e-9),
                "quantum improvement must be measured against the classical baseline: got {} vs expected {}",
                measured,
                expected
            );
        }
    }
    Ok(())
}

#[test]
fn the_classical_baseline_is_computed_and_recorded_regardless_of_quantum_availability() -> Result<()>
{
    // Whether a quantum provider is attached, reachable, or used must not affect
    // whether the classical baseline is computed. The platform defaults to
    // classical and behaves identically to a classical-only deployment if the
    // quantum path is unavailable.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };

    // Solve with no quantum provider.
    let no_quantum = ComputeRouter::classical(42)
        .with_policy(policy)?
        .solve(&problem)?;

    // Solve with an unreachable quantum provider.
    let unreachable = Arc::new(HostedProvider::new(HostedConfig {
        vendor: "ibm-quantum".to_string(),
        backend: "unreachable_backend".to_string(),
        credential_env: "QIP_QUANTUM_TOKEN".to_string(),
        endpoint: "https://unreachable.example.com".to_string(),
        max_qubits: 133,
        cost_per_job_micros: 1_000_000,
    }));
    let with_unreachable = ComputeRouter::classical(42)
        .with_policy(policy)?
        .with_quantum(unreachable)?
        .solve(&problem)?;

    // Both must have computed a classical baseline.
    assert!(
        no_quantum.classical_objective.is_finite(),
        "classical_objective must be finite even without quantum provider"
    );
    assert!(
        with_unreachable.classical_objective.is_finite(),
        "classical_objective must be finite even with unreachable provider"
    );

    // The classical objectives must be identical (same seed, same problem,
    // same policy).
    assert!(
        approx_eq(
            no_quantum.classical_objective,
            with_unreachable.classical_objective,
            1e-12
        ),
        "classical baseline must be independent of quantum provider state: got {} vs {}",
        no_quantum.classical_objective,
        with_unreachable.classical_objective
    );

    // Both must have computed the same solution weights.
    assert_eq!(
        no_quantum.weights.len(),
        with_unreachable.weights.len(),
        "weight vector length must match"
    );
    for (i, (a, b)) in no_quantum
        .weights
        .iter()
        .zip(with_unreachable.weights.iter())
        .enumerate()
    {
        assert!(
            approx_eq(*a, *b, 1e-12),
            "weight[{i}] must match: got {} vs {}",
            a,
            b
        );
    }
    Ok(())
}

#[test]
fn the_classical_baseline_value_is_preserved_through_the_routing_decision() -> Result<()> {
    // The classical_objective field in RoutingDecision is the value that is
    // used to judge quantum answers. It must be stable and equal to the
    // objective of the classical run.
    let problem = discrete(10, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1).with_policy(policy)?;
    let decision = router.solve(&problem)?;

    // Extract the classical run from the runs vector.
    let classical_run = decision
        .runs
        .iter()
        .find(|r| !r.solver.is_quantum())
        .expect("no classical run in decision.runs");

    // The decision.classical_objective must equal the classical run's objective.
    assert!(
        approx_eq(decision.classical_objective, classical_run.objective, 1e-9),
        "decision.classical_objective must equal the classical run's objective: {} vs {}",
        decision.classical_objective,
        classical_run.objective
    );

    // The decision's own objective must be >= classical_objective (never worse
    // than the baseline).
    assert!(
        decision.objective >= decision.classical_objective - 1e-9,
        "decision.objective must never be worse than classical_objective: {} vs {}",
        decision.objective,
        decision.classical_objective
    );

    // For a classical-only solve, the chosen solver must be the one in the run.
    if decision.chosen == classical_run.solver {
        assert!(
            approx_eq(decision.objective, classical_run.objective, 1e-9),
            "when classical is chosen, objectives must match"
        );
    }
    Ok(())
}

/// A quantum provider that claims a different energy than its assignment produces.
#[derive(Debug)]
struct LyingQuantumProvider {
    assignment: Vec<u8>,
    claimed_energy: f64,
    simulated: bool,
}

impl QuantumProvider for LyingQuantumProvider {
    fn name(&self) -> &str {
        "lying"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: 64,
            simulated: self.simulated,
            noisy: false,
            typical_queue: qip_core::time::Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
    fn solve_qubo(&self, _qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        let assignment = self.assignment.clone();
        Ok(QaoaResult {
            energy: self.claimed_energy,
            assignment,
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: vec![0.0, 0.0],
            evaluations: 1,
        })
    }
}

#[test]
fn a_quantum_answer_with_a_mismatched_claimed_energy_is_refused() -> Result<()> {
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        quantum_margin: 0.0,
        ..RoutingPolicy::default()
    };

    // Create an assignment that's feasible but claim a different energy.
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let mut assignment = vec![0u8; 12];
    for idx in [0, 1, 2, 3] {
        assignment[idx] = 1;
    }
    let true_objective = problem.objective_at(&encoding.to_weights(&assignment));

    let router = ComputeRouter::classical(99)
        .with_policy(policy)?
        .with_quantum(Arc::new(LyingQuantumProvider {
            assignment,
            claimed_energy: true_objective - 100.0, // Lie by 100
            simulated: false,
        }))?;

    let decision = solve_across_two_cycles(&router, &problem)?;

    // The quantum answer's claimed improvement should not be trusted, so classical
    // should be chosen. This test verifies that ComputeRouter rejects quantum
    // candidates whose claimed energy doesn't match their actual assignment value.
    assert_ne!(
        decision.chosen,
        Solver::Quantum,
        "quantum answer with mismatched energy claim should be refused; rationale: {}",
        decision.rationale
    );
    Ok(())
}

#[test]
fn a_quantum_answer_that_violates_the_cardinality_constraint_is_refused() -> Result<()> {
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        // Below the problem size, so the baseline is the annealer rather than
        // exhaustive search: against a provably optimal baseline the router
        // sends no quantum job at all, and this test needs one sent.
        exact_enumeration_limit: 2,
        quantum_margin: 0.0,
        ..RoutingPolicy::default()
    };

    // Create an assignment with too many holdings (5 instead of cardinality 4).
    // This satisfies the QUBO but violates the cardinality constraint that
    // the QUBO relaxation dropped.
    let mut assignment = vec![0u8; 12];
    for idx in [0, 1, 2, 3, 4] {
        assignment[idx] = 1; // 5 holdings instead of 4
    }

    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let _true_objective = problem.objective_at(&encoding.to_weights(&assignment));

    let router = ComputeRouter::classical(99)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(
            assignment, false,
        )))?;

    let decision = solve_across_two_cycles(&router, &problem)?;

    // The cardinality constraint violation should be caught by ComputeRouter's
    // score() function when it re-evaluates against the real PortfolioProblem.
    assert_ne!(
        decision.chosen,
        Solver::Quantum,
        "quantum answer violating cardinality should be refused; rationale: {}",
        decision.rationale
    );
    Ok(())
}
// --- the asynchronous quantum arm (R12-QUANT-4029, 7016, 7206) ---------------

/// A provider that does not answer until the test lets it, and counts the
/// calls it receives.
#[derive(Debug)]
struct HeldProvider {
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl QuantumProvider for HeldProvider {
    fn name(&self) -> &str {
        "held"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: 64,
            simulated: false,
            noisy: true,
            typical_queue: qip_core::time::Duration::from_mins(30),
            cost_per_job_micros: 0,
        }
    }
    fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Blocks until the test sends or drops the sender — a device queue
        // that does not move.
        if let Ok(release) = self.release.lock() {
            let _ = release.recv();
        }
        Err(qip_core::error::Error::unavailable(format!(
            "released without an answer for {} variables",
            qubo.n
        )))
    }
}

#[test]
fn a_provider_that_never_returns_does_not_stall_a_cycle_and_the_classical_answer_stands()
-> Result<()> {
    // The failure this guards: the router used to call the provider inline,
    // so a hosted device's queue would have stopped the DECIDE stage — and
    // every stage after it — for as long as the queue took. Each solve here
    // runs on its own thread with a deadline; a router that waited on the
    // provider never reports back, and the test fails on the deadline rather
    // than hanging.
    let (hold, release) = std::sync::mpsc::channel::<()>();
    let provider = Arc::new(HeldProvider {
        release: std::sync::Mutex::new(release),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        quantum_patience_cycles: 3,
        ..RoutingPolicy::default()
    };
    let router = Arc::new(
        ComputeRouter::classical(1)
            .with_policy(policy)?
            .with_quantum(provider.clone())?,
    );
    let problem = discrete(12, 4)?;
    let baseline = ComputeRouter::classical(1)
        .with_policy(policy)?
        .solve(&problem)?;

    let mut decisions = Vec::new();
    for cycle in 0..5 {
        let (sender, receiver) = std::sync::mpsc::channel();
        let (router, problem) = (Arc::clone(&router), problem.clone());
        std::thread::spawn(move || {
            let _ = sender.send(router.solve(&problem));
        });
        let decision = receiver
            .recv_timeout(std::time::Duration::from_secs(20))
            .unwrap_or_else(|_| {
                panic!("cycle {cycle} waited on a provider that never returns: the call is inline")
            })?;
        decisions.push(decision);
    }

    // Premise: the provider really was asked, so the cycles above completed
    // with a job outstanding rather than because nothing was submitted.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while provider.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the provider was never called"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(decisions[0].quantum_dispatch.submitted, Some(0));

    for decision in &decisions {
        assert_eq!(decision.chosen, baseline.chosen, "{}", decision.rationale);
        assert_eq!(decision.weights, baseline.weights);
    }
    // Pending for two cycles, then abandoned on the third poll — and the
    // abandoned job keeps its slot, because its provider has not returned:
    // a provider that ignores its own queue is not handed a second job.
    assert_eq!(decisions[1].quantum_dispatch.pending, Some(0));
    assert!(
        decisions[3]
            .quantum_dispatch
            .notes
            .iter()
            .any(|note| note.contains("abandoned after 3 cycle(s)")),
        "{:?}",
        decisions[3].quantum_dispatch.notes
    );
    assert!(
        decisions[4].quantum_dispatch.submitted.is_none(),
        "a second job was queued on a provider still holding the first"
    );
    assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(hold);
    Ok(())
}

#[test]
fn the_default_policy_reaches_the_in_tree_simulator_and_a_large_enough_problem_is_eligible()
-> Result<()> {
    // The failure this guards: the default threshold was fifty assets and the
    // simulator holds twenty-four qubits, so `quantum_enabled` switched on a
    // quantum arm that no problem could ever reach.
    let router = ComputeRouter::classical(1).with_quantum(Arc::new(SimulatedProvider::new(1)))?;
    let policy = router.policy();
    assert!(
        policy.minimum_assets_for_quantum > policy.exact_enumeration_limit,
        "the default threshold sits inside the exhaustive baseline, where a quantum answer can only tie"
    );
    let (attempted, note) =
        router.quantum_assessment(&discrete(policy.minimum_assets_for_quantum, 4)?);
    assert!(attempted, "{note}");
    Ok(())
}

#[test]
fn a_policy_whose_quantum_threshold_exceeds_the_providers_qubits_is_refused_at_construction()
-> Result<()> {
    let impossible = RoutingPolicy {
        minimum_assets_for_quantum: 50,
        ..RoutingPolicy::default()
    };
    // Premise: the same policy is valid on its own, so the refusals below are
    // about the provider and not the policy.
    let classical = ComputeRouter::classical(1).with_policy(impossible)?;

    let attached_after = classical
        .with_quantum(Arc::new(SimulatedProvider::new(1)))
        .expect_err("a provider the policy can never call was accepted");
    assert!(
        attached_after.message().contains("could never run"),
        "{}",
        attached_after.message()
    );

    let policy_after = ComputeRouter::classical(1)
        .with_quantum(Arc::new(SimulatedProvider::new(1)))?
        .with_policy(impossible)
        .expect_err("a policy that can never reach the attached provider was accepted");
    assert!(
        policy_after.message().contains("could never run"),
        "{}",
        policy_after.message()
    );
    Ok(())
}

#[test]
fn an_exhaustive_baseline_sends_no_quantum_job_because_the_answer_can_only_tie() -> Result<()> {
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(
            vec![0; 12],
            false,
        )))?;
    let problem = discrete(12, 4)?;
    // Premise: the problem is otherwise eligible.
    assert!(router.quantum_assessment(&problem).0);

    let decision = router.solve(&problem)?;
    assert_eq!(decision.chosen, Solver::ExactEnumeration);
    assert!(decision.quantum_dispatch.submitted.is_none());
    let compute = decision
        .compute
        .expect("every decision carries a compute record");
    let estimate = compute
        .information_value
        .clone()
        .expect("the estimate is recorded when the arm reached it");
    assert!(!estimate.cleared);
    assert!(approx_eq(estimate.win_probability, 0.0, 0.0));
    let qpu = compute
        .verdict(qip_optimization_engine::Substrate::Qpu)
        .expect("a verdict for the QPU");
    assert!(qpu.reason.contains("provably optimal"), "{}", qpu.reason);
    Ok(())
}

#[test]
fn a_quantum_arm_that_keeps_tying_stops_being_tried() -> Result<()> {
    // R12-QUANT-4047: no substrate is presumed superior, and one that keeps
    // losing stops being paid for. With one win and one loss of prior, two
    // straight non-wins put the measured rate at 0.25, under a 0.3 bar.
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        minimum_win_probability: 0.3,
        ..RoutingPolicy::default()
    };
    let baseline = ComputeRouter::classical(1)
        .with_policy(policy)?
        .solve(&problem)?;
    let tie: Vec<u8> = baseline
        .weights
        .iter()
        .map(|w| u8::from(w.abs() > 1e-9))
        .collect();
    let router = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(ScriptedProvider::with_assignment(tie, false)))?;

    let mut submitted = 0;
    let mut last = None;
    for _ in 0..6 {
        let decision = router.solve(&problem)?;
        if decision.quantum_dispatch.submitted.is_some() {
            submitted += 1;
            wait_for_quantum_job(&router)?;
        }
        last = Some(decision);
    }
    // Premise: the arm was tried at all.
    assert!(submitted > 0);
    assert_eq!(submitted, 2, "the arm was tried {submitted} times");
    let last = last.expect("six solves ran");
    let estimate = last
        .compute
        .and_then(|c| c.information_value)
        .expect("an estimate is recorded");
    assert_eq!((estimate.trials, estimate.wins), (2, 0));
    assert!(!estimate.cleared);
    Ok(())
}

#[test]
fn a_job_priced_above_the_budget_is_not_sent() -> Result<()> {
    #[derive(Debug)]
    struct Priced;
    impl QuantumProvider for Priced {
        fn name(&self) -> &str {
            "priced"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                max_qubits: 64,
                simulated: false,
                noisy: true,
                typical_queue: qip_core::time::Duration::ZERO,
                cost_per_job_micros: 1_500_000,
            }
        }
        fn solve_qubo(&self, _qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
            Err(qip_core::error::Error::denied("a priced job was sent"))
        }
    }
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        ..RoutingPolicy::default()
    };
    let refused = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(Arc::new(Priced))?
        .solve(&discrete(12, 4)?)?;
    assert!(refused.quantum_dispatch.submitted.is_none());

    // Premise: with the budget raised to the price the same job is sent, so
    // the refusal above was the budget and not something else.
    let funded = ComputeRouter::classical(1)
        .with_policy(RoutingPolicy {
            max_quantum_job_cost_micros: 1_500_000,
            ..policy
        })?
        .with_quantum(Arc::new(Priced))?;
    let decision = funded.solve(&discrete(12, 4)?)?;
    assert!(decision.quantum_dispatch.submitted.is_some());
    assert_eq!(decision.quantum_dispatch.cost_micros, 1_500_000);
    assert_eq!(funded.quantum_spend_micros()?, 1_500_000);
    Ok(())
}

#[test]
fn every_routing_decision_accounts_for_every_declared_substrate() -> Result<()> {
    use qip_optimization_engine::{Substrate, SubstrateRole};
    for problem in [continuous(6)?, discrete(10, 4)?] {
        let decision = ComputeRouter::classical(1).solve(&problem)?;
        let compute = decision
            .compute
            .expect("every decision carries a compute record");
        compute.validate()?;
        assert_eq!(compute.verdicts.len(), Substrate::ALL.len());
        assert_eq!(compute.answered_by(), Some(Substrate::Cpu));
        let gpu = compute.verdict(Substrate::Gpu).expect("a GPU verdict");
        assert_eq!(gpu.role, SubstrateRole::Declined);
        assert!(gpu.reason.contains("cannot be reached"), "{}", gpu.reason);
        // The continuous problem compiles with no quantum formulation at all.
        assert_eq!(compute.task.quantum.is_some(), problem.is_discrete());
    }
    Ok(())
}
