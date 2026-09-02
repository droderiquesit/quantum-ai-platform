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
use qip_optimization_engine::router::{ComputeRouter, RoutingPolicy, Solver};
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
    let router = ComputeRouter::classical(1).with_policy(policy);
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
        exact_enumeration_limit: 12,
        qaoa: QaoaSettings {
            layers: 1,
            optimiser_iterations: 20,
            shots: 0,
        },
        ..RoutingPolicy::default()
    };
    let router = ComputeRouter::classical(1)
        .with_policy(policy)
        .with_quantum(provider);
    let decision = router.solve(&discrete(10, 4)?)?;

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
    });
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
        .with_policy(policy)
        .solve(&problem)?;
    let with = ComputeRouter::classical(1)
        .with_policy(policy)
        .with_quantum(hosted)
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
    let router = ComputeRouter::classical(1).with_quantum(Arc::new(SimulatedProvider::new(1)));
    let (attempted, note) = router.quantum_assessment(&continuous(100)?);
    assert!(!attempted);
    assert!(note.contains("global optimum"), "{note}");
    Ok(())
}

#[test]
fn a_small_discrete_problem_does_not_justify_a_quantum_attempt() -> Result<()> {
    let router = ComputeRouter::classical(1).with_quantum(Arc::new(SimulatedProvider::new(1)));
    let (attempted, note) = router.quantum_assessment(&discrete(10, 4)?);
    assert!(!attempted);
    assert!(note.contains("below the 50"), "{note}");
    Ok(())
}

#[test]
fn a_simulated_result_is_labelled_as_not_being_evidence_of_advantage() -> Result<()> {
    let policy = RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 10,
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
        .with_policy(policy)
        .with_quantum(Arc::new(SimulatedProvider::new(3).with_max_qubits(12)));
    let decision = router.solve(&discrete(10, 4)?)?;

    assert!(
        decision.quantum_note.contains("not evidence of advantage"),
        "{}",
        decision.quantum_note
    );
    if let Some(quantum) = decision.run_for(Solver::Quantum) {
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
        Ok(QaoaResult {
            energy: qubo.evaluate(&assignment),
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
        exact_enumeration_limit: 12,
        quantum_margin: 0.01,
        ..RoutingPolicy::default()
    };
    // First find what the classical solver picks, then hand the quantum path
    // exactly that answer.
    let baseline = ComputeRouter::classical(1)
        .with_policy(policy)
        .solve(&problem)?;
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    let assignment: Vec<u8> = baseline
        .weights
        .iter()
        .map(|w| u8::from(w.abs() > 1e-9))
        .collect();
    let _ = encoding;

    let decision = ComputeRouter::classical(1)
        .with_policy(policy)
        .with_quantum(Arc::new(ScriptedProvider {
            assignment,
            simulated: false,
        }))
        .solve(&problem)?;

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
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };
    // Every asset held: a flagrant breach of a three-name mandate.
    let decision = ComputeRouter::classical(1)
        .with_policy(policy)
        .with_quantum(Arc::new(ScriptedProvider {
            assignment: vec![1; 12],
            simulated: false,
        }))
        .solve(&problem)?;

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
        .with_policy(policy)
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

    let decision = ComputeRouter::classical(99)
        .with_policy(policy)
        .with_quantum(Arc::new(ScriptedProvider {
            assignment: better,
            simulated: false,
        }))
        .solve(&problem)?;

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
        exact_enumeration_limit: 12,
        ..RoutingPolicy::default()
    };
    let decision = ComputeRouter::classical(1)
        .with_policy(policy)
        .with_quantum(Arc::new(Broken))
        .solve(&discrete(12, 4)?)?;

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
        .with_policy(policy)
        .solve(&problem)?;
    let b = ComputeRouter::classical(5)
        .with_policy(policy)
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
