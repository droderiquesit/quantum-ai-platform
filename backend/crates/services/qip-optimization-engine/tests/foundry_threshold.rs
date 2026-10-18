//! R12-QUANT-4028: the Foundry's registered per-workload promotion threshold is
//! the bar the router holds a quantum answer to.
//!
//! Before this wiring the threshold was validated at `Foundry::register` and
//! then never read: the router compared against `RoutingPolicy::quantum_margin`,
//! which `validate` lets be zero, so "a positive threshold, ties go to
//! classical" was enforced only on a declaration nothing consulted. Every test
//! here sets the policy margin to zero, so the only thing that can hold a
//! quantum answer back is the Foundry's number.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_numerics::anneal::Qubo;
use qip_optimization_engine::problem::{Objective, PortfolioProblem, QuboEncoding};
use qip_optimization_engine::router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver};
use qip_quantum::foundry::{FamilyDeclaration, FamilyKind, Foundry};
use qip_quantum::provider::{ProviderCapabilities, QuantumProvider};
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use std::sync::Arc;

/// A provider that returns the answer the test wants and nothing else. No
/// network, no randomness, no billing.
#[derive(Debug)]
struct Scripted {
    assignment: Vec<u8>,
    available: bool,
}

impl QuantumProvider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            max_qubits: 64,
            simulated: true,
            noisy: false,
            typical_queue: qip_core::time::Duration::ZERO,
            cost_per_job_micros: 0,
        }
    }
    fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        Ok(QaoaResult {
            energy: qubo.evaluate(&self.assignment),
            assignment: self.assignment.clone(),
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: vec![0.0, 0.0],
            evaluations: 1,
        })
    }
}

fn problem() -> Result<PortfolioProblem> {
    let n = 12;
    let assets: Vec<String> = (0..n).map(|i| format!("obj-A{i}")).collect();
    let cov: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 0.04 } else { 0.012 }).collect())
        .collect();
    Ok(PortfolioProblem::new(assets, cov)?
        .with_objective(Objective::MeanVariance)
        .with_expected_returns((0..n).map(|i| 0.05 + 0.01 * i as f64).collect())
        .with_risk_aversion(5.0)
        .with_bounds(vec![0.0; n], vec![0.3; n])
        .fully_invested(1.0)
        .with_cardinality(4))
}

/// Margin zero on purpose: `RoutingPolicy::validate` accepts it, and it is the
/// configuration under which the unwired threshold let a tie through.
fn zero_margin_policy() -> RoutingPolicy {
    RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        quantum_margin: 0.0,
        ..RoutingPolicy::default()
    }
}

fn foundry_with(threshold: f64) -> Result<Foundry> {
    let mut f = Foundry::new();
    f.register(
        FamilyKind::PortfolioAllocation,
        &FamilyDeclaration {
            classical_baseline: Some("annealing".into()),
            objective: Some("PortfolioProblem::objective_at".into()),
            promotion_threshold: Some(threshold),
            encoding: Some("PortfolioProblem::to_qubo".into()),
        },
    )?;
    Ok(f)
}

/// A seed whose annealing baseline leaves room, and the best strictly better
/// feasible four-name answer over it, with the improvement it gives.
fn better_answer(problem: &PortfolioProblem) -> Result<(u64, Vec<u8>, f64)> {
    let encoding = QuboEncoding::equal_weight_bounded(4, 1.0, 0.3);
    for seed in 1..200u64 {
        let baseline = ComputeRouter::classical(seed)
            .with_policy(zero_margin_policy())?
            .solve(problem)?
            .classical_objective;
        let mut best: Option<(Vec<u8>, f64)> = None;
        for a in 0..12 {
            for b in (a + 1)..12 {
                for c in (b + 1)..12 {
                    for d in (c + 1)..12 {
                        let mut assignment = vec![0u8; 12];
                        for i in [a, b, c, d] {
                            assignment[i] = 1;
                        }
                        let o = problem.objective_at(&encoding.to_weights(&assignment));
                        if o < best.as_ref().map_or(baseline, |x| x.1) - 1e-9 {
                            best = Some((assignment, o));
                        }
                    }
                }
            }
        }
        if let Some((assignment, o)) = best {
            return Ok((seed, assignment, (baseline - o) / baseline.abs()));
        }
    }
    panic!("no seed left the annealing baseline room to improve");
}

fn wait(router: &ComputeRouter) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while router.quantum_job_ready().ok().flatten() != Some(true) {
        assert!(
            std::time::Instant::now() < deadline,
            "the job never returned"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Submit on one cycle, judge on the next, as the kernel drives the arm.
fn two_cycles(router: &ComputeRouter, p: &PortfolioProblem) -> Result<RoutingDecision> {
    let first = router.solve(p)?;
    assert!(
        first.quantum_dispatch.submitted.is_some(),
        "{}",
        first.quantum_note
    );
    wait(router);
    let second = router.solve(p)?;
    // Premise: the answer was judged, beside a classical baseline.
    assert!(
        second.run_for(Solver::Quantum).is_some(),
        "{}",
        second.rationale
    );
    assert!(!second.runs[0].solver.is_quantum());
    Ok(second)
}

fn router_for(
    seed: u64,
    assignment: &[u8],
    threshold: f64,
    policy: RoutingPolicy,
) -> Result<ComputeRouter> {
    ComputeRouter::classical(seed)
        .with_policy(policy)?
        .with_foundry(foundry_with(threshold)?)
        .with_quantum(Arc::new(Scripted {
            assignment: assignment.to_vec(),
            available: true,
        }))
}

/// Mutations: in `ComputeRouter::margin_for`, return the policy margin alone
/// (ignore the foundry) and the exactly-at and below assertions fire; in
/// `adopts`, change `gain > margin` to `gain >= margin` and the exactly-at
/// assertion fires.
#[test]
fn a_quantum_answer_is_adopted_only_when_it_beats_the_foundry_threshold_strictly() -> Result<()> {
    let p = problem()?;
    let (seed, answer, gain) = better_answer(&p)?;
    assert!(gain > 1e-9, "gain {gain}");

    // Just below the gain: the quantum answer clears it.
    let below = router_for(seed, &answer, gain * 0.999, zero_margin_policy())?;
    let d = two_cycles(&below, &p)?;
    assert_eq!(d.chosen, Solver::Quantum, "{}", d.rationale);

    // Exactly at the gain, the same gain bit for bit: a tie, which stays
    // classical. The policy margin is zero, so only the foundry holds it.
    let at = router_for(seed, &answer, gain, zero_margin_policy())?;
    assert_eq!(at.quantum_promotion_margin()?, Some(gain));
    let d = two_cycles(&at, &p)?;
    assert_ne!(d.chosen, Solver::Quantum, "{}", d.rationale);
    assert!(
        d.rationale.contains("a tie is not evidence"),
        "{}",
        d.rationale
    );
    assert_eq!(d.objective.to_bits(), d.classical_objective.to_bits());

    // Above the gain: loses, so changing only the registered number flipped
    // the decision between the two runs above and this one.
    let above = router_for(seed, &answer, gain * 1.001, zero_margin_policy())?;
    let d = two_cycles(&above, &p)?;
    assert_ne!(d.chosen, Solver::Quantum, "{}", d.rationale);
    Ok(())
}

#[test]
fn the_stricter_of_the_policy_margin_and_the_foundry_threshold_governs() -> Result<()> {
    let p = problem()?;
    let (seed, answer, gain) = better_answer(&p)?;
    // Foundry lenient, policy strict.
    let strict_policy = RoutingPolicy {
        quantum_margin: gain * 2.0,
        ..zero_margin_policy()
    };
    let r = router_for(seed, &answer, gain * 0.5, strict_policy)?;
    assert_eq!(r.quantum_promotion_margin()?, Some(gain * 2.0));
    assert_ne!(two_cycles(&r, &p)?.chosen, Solver::Quantum);
    // Foundry strict, policy lenient.
    let r = router_for(seed, &answer, gain * 2.0, zero_margin_policy())?;
    assert_eq!(r.quantum_promotion_margin()?, Some(gain * 2.0));
    assert_ne!(two_cycles(&r, &p)?.chosen, Solver::Quantum);
    Ok(())
}

#[test]
fn a_foundry_that_registers_no_portfolio_workload_leaves_the_classical_answer_standing()
-> Result<()> {
    let p = problem()?;
    let (seed, answer, _) = better_answer(&p)?;
    // Attached while registered, then the foundry is replaced by an empty one.
    let router =
        router_for(seed, &answer, 0.01, zero_margin_policy())?.with_foundry(Foundry::new());
    assert_eq!(router.quantum_promotion_margin()?, None);
    let d = router.solve(&p)?;
    assert!(d.quantum_dispatch.submitted.is_none());
    assert!(
        d.quantum_note.contains("registers no portfolio-allocation"),
        "{}",
        d.quantum_note
    );
    assert!(!d.chosen.is_quantum());
    assert!(
        !d.runs.is_empty() && !d.runs[0].solver.is_quantum(),
        "classical baseline still computed"
    );
    Ok(())
}

#[test]
fn an_unreachable_provider_leaves_the_classical_answer_standing_with_the_reason() -> Result<()> {
    let p = problem()?;
    let router = ComputeRouter::classical(1)
        .with_policy(zero_margin_policy())?
        .with_foundry(foundry_with(0.01)?)
        .with_quantum(Arc::new(Scripted {
            assignment: vec![0; 12],
            available: false,
        }))?;
    let d = router.solve(&p)?;
    assert!(
        d.quantum_note.contains("not reachable"),
        "{}",
        d.quantum_note
    );
    assert!(d.quantum_dispatch.submitted.is_none());
    assert!(!d.chosen.is_quantum());
    assert!(!d.runs[0].solver.is_quantum());
    Ok(())
}

#[test]
fn attaching_a_quantum_arm_to_a_foundry_without_the_family_is_refused() -> Result<()> {
    let r = ComputeRouter::classical(1)
        .with_policy(zero_margin_policy())?
        .with_foundry(Foundry::new())
        .with_quantum(Arc::new(Scripted {
            assignment: vec![0; 12],
            available: true,
        }));
    assert!(r.is_err());
    Ok(())
}

/// R12-QUANT-1076: the foundry's registration names the encoder, and the
/// router refuses a family whose declared encoder is not the one its quantum
/// arm calls. Without the comparison the declaration is free text: a family
/// could be admitted with an encoding nobody wrote, and the registry would
/// still read as a family with an encoding. The platform's own foundry must
/// pass the same check, or the guard would only prove it refuses everything.
#[test]
fn a_portfolio_family_declaring_an_encoder_the_router_does_not_run_is_refused() -> Result<()> {
    let mut foreign = Foundry::new();
    foreign.register(
        FamilyKind::PortfolioAllocation,
        &FamilyDeclaration {
            classical_baseline: Some("annealing".into()),
            objective: Some("PortfolioProblem::objective_at".into()),
            promotion_threshold: Some(0.01),
            encoding: Some("PortfolioProblem::to_ising".into()),
        },
    )?;
    let refused = ComputeRouter::classical(1)
        .with_policy(zero_margin_policy())?
        .with_foundry(foreign)
        .with_quantum(Arc::new(Scripted {
            assignment: vec![0; 12],
            available: true,
        }));
    match refused {
        Err(error) => assert!(
            error.message().contains("`PortfolioProblem::to_ising`")
                && error.message().contains("`PortfolioProblem::to_qubo`"),
            "the refusal does not name both encoders: {}",
            error.message()
        ),
        Ok(_) => panic!("a family declaring an encoder the router does not run was admitted"),
    }

    let admitted = ComputeRouter::classical(1)
        .with_policy(zero_margin_policy())?
        .with_foundry(Foundry::platform()?)
        .with_quantum(Arc::new(Scripted {
            assignment: vec![0; 12],
            available: true,
        }));
    assert!(
        admitted.is_ok(),
        "the platform's own foundry declares an encoder the router refuses: {:?}",
        admitted.err()
    );
    Ok(())
}
