//! The router's measurements, learning, solver registry and research records
//! (R12-QUANT-4046, 4047, 4053; §11.3–11.5).
//!
//! Each test names the mutation that should make it fail. They drive the
//! router through `solve` and `replay`, the two entry points a deployment
//! reaches, rather than the helpers behind them.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_numerics::anneal::Qubo;
use qip_optimization_engine::problem::{Objective, PortfolioProblem};
use qip_optimization_engine::router::{
    ComputeRouter, QUANTUM_INSPIRED_SOLVER, RoutingDecision, RoutingPolicy, Solver,
};
use qip_optimization_engine::{
    AdvantageBoundary, AdvantageEvidence, ComputeDecision, MeasurementOutcome, ResearchDomain,
    Substrate, SubstrateRole,
};
use qip_quantum::provider::{ProviderCapabilities, QuantumProvider};
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use std::sync::Arc;

fn discrete(n: usize, cardinality: usize) -> Result<PortfolioProblem> {
    let assets: Vec<String> = (0..n).map(|i| format!("obj-A{i}")).collect();
    let covariance: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 0.04 } else { 0.012 }).collect())
        .collect();
    Ok(PortfolioProblem::new(assets, covariance)?
        .with_objective(Objective::MeanVariance)
        .with_expected_returns((0..n).map(|i| 0.05 + 0.01 * i as f64).collect())
        .with_risk_aversion(5.0)
        .with_bounds(vec![0.0; n], vec![0.3; n])
        .fully_invested(1.0)
        .with_cardinality(cardinality))
}

/// A heuristic baseline (the exhaustive limit sits below the size), so a
/// candidate is eligible at all: against a proven optimum nothing is sent.
fn heuristic_policy() -> RoutingPolicy {
    RoutingPolicy {
        minimum_assets_for_quantum: 4,
        exact_enumeration_limit: 2,
        ..RoutingPolicy::default()
    }
}

fn compute(decision: &RoutingDecision) -> Result<&ComputeDecision> {
    decision
        .compute
        .as_ref()
        .ok_or_else(|| Error::invalid("a decision carried no compute record"))
}

/// Returns exactly the baseline's selection with its true energy: a tie,
/// which is never adopted, so every trial is a trial without a win.
#[derive(Debug)]
struct Tying {
    assignment: Vec<u8>,
}

impl QuantumProvider for Tying {
    fn name(&self) -> &str {
        "tying"
    }
    fn is_available(&self) -> bool {
        true
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
            assignment: self.assignment.clone(),
            energy: qubo.evaluate(&self.assignment),
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: Vec::new(),
            evaluations: 1,
        })
    }
}

fn wait_for_job(router: &ComputeRouter) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while router.quantum_job_ready()? != Some(true) {
        assert!(
            std::time::Instant::now() < deadline,
            "the scripted job did not return within a minute"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    Ok(())
}

/// R12-QUANT-4053: every routed solve records quality, cost and latency per
/// run on the journaled decision, and the solver registry's view of the
/// solver that ran changes after it.
///
/// Mutation: in `ComputeRouter::solve`, delete the
/// `learned.record_registry(measurement)?` loop — the standing stays `None`
/// and the second assertion fires. Delete the baseline's entry from
/// `measurements` — the premise fires.
#[test]
fn every_solve_records_quality_cost_and_latency_per_run_and_the_registry_standing_moves()
-> Result<()> {
    let router = ComputeRouter::classical(1).with_policy(heuristic_policy())?;
    let problem = discrete(10, 4)?;
    let baseline_name = Solver::SimulatedAnnealing.as_str();
    assert!(
        router.solver_standing(baseline_name)?.is_none(),
        "premise: the registry has measured nothing before the first solve"
    );

    let decision = router.solve(&problem)?;
    assert_eq!(decision.chosen, Solver::SimulatedAnnealing, "premise");
    let record = compute(&decision)?;
    let baseline = record
        .measurements
        .iter()
        .find(|m| m.solver == baseline_name)
        .ok_or_else(|| Error::invalid("the baseline run was not measured"))?;
    assert_eq!(baseline.substrate, Substrate::Cpu);
    assert_eq!(baseline.outcome, MeasurementOutcome::Answered);
    assert!(baseline.used, "the baseline answered, so it was used");
    assert_eq!(
        baseline.quality.to_bits(),
        0.0_f64.to_bits(),
        "the baseline's improvement on itself"
    );
    assert_eq!((baseline.cost_micros, baseline.latency_cycles), (0, 0));

    let standing = router
        .solver_standing(baseline_name)?
        .ok_or_else(|| Error::invalid("the registry's view did not change after the run"))?;
    assert_eq!((standing.runs, standing.wins), (1, 1));

    router.solve(&problem)?;
    let after_two = router
        .solver_standing(baseline_name)?
        .ok_or_else(|| Error::invalid("standing vanished"))?;
    assert_eq!(after_two.runs, 2, "a second solve is a second run");
    Ok(())
}

/// §11.1 and §11.4: the quantum-inspired rung, when a deployment enables it,
/// runs through the solver registry on the same QUBO, is verified
/// classically, and is recorded as an ensemble feature beside the baseline —
/// not as a substrate silently declined.
///
/// Mutation: in `ComputeRouter::solve`, change `if
/// !self.policy.quantum_inspired` to `if true` — no quantum-inspired run
/// exists and the first assertion after the premise fires. Delete the
/// `ClassicalValidator` gate in `run_quantum_inspired` and return the
/// candidate's assignment unchecked — the run still appears, so this test
/// does not catch that; `qip-quantum`'s validator tests do.
#[test]
fn an_enabled_quantum_inspired_rung_runs_through_the_registry_and_is_recorded_as_a_feature()
-> Result<()> {
    let problem = discrete(12, 4)?;
    let disabled = ComputeRouter::classical(1)
        .with_policy(heuristic_policy())?
        .solve(&problem)?;
    assert!(
        disabled.run_for(Solver::QuantumInspired).is_none(),
        "premise: the default policy does not run the rung"
    );

    let router = ComputeRouter::classical(1).with_policy(RoutingPolicy {
        quantum_inspired: true,
        ..heuristic_policy()
    })?;
    let decision = router.solve(&problem)?;
    let inspired = decision
        .run_for(Solver::QuantumInspired)
        .ok_or_else(|| Error::invalid("the enabled quantum-inspired rung did not run"))?;
    assert_eq!(inspired.weights.len(), 12, "it solved this problem");
    assert!(
        !decision.runs[0].solver.is_quantum() && decision.runs[0].solver != Solver::QuantumInspired,
        "the classical baseline ran first"
    );

    let record = compute(&decision)?;
    let measured = record
        .measurements
        .iter()
        .find(|m| m.solver == QUANTUM_INSPIRED_SOLVER)
        .ok_or_else(|| Error::invalid("the quantum-inspired run was not measured"))?;
    assert_eq!(measured.substrate, Substrate::QuantumInspired);
    assert_eq!(measured.outcome, MeasurementOutcome::Answered);
    assert!(measured.predicted_win_probability.is_some());
    assert!(
        record
            .inspired_information_value
            .as_ref()
            .is_some_and(|value| value.cleared),
        "the dispatch estimate was recorded and cleared"
    );
    assert_eq!(
        record
            .ensemble
            .iter()
            .filter(|f| f.substrate == Substrate::QuantumInspired)
            .count(),
        1,
        "the candidate is an ensemble feature"
    );
    assert!(
        router
            .solver_standing(QUANTUM_INSPIRED_SOLVER)?
            .is_some_and(|s| s.runs == 1),
        "the registry's view of the quantum-inspired solver moved"
    );
    // Whatever the answer, a quantum-inspired result is never a quantum one.
    assert_eq!(decision.measured_quantum_advantage(), None);
    let verdict = record
        .verdict(Substrate::QuantumInspired)
        .ok_or_else(|| Error::invalid("no verdict for the rung"))?;
    assert!(
        matches!(
            verdict.role,
            SubstrateRole::Answered | SubstrateRole::Declined
        ),
        "{verdict:?}"
    );
    Ok(())
}

/// R12-QUANT-4047: the learning a router holds is rebuilt from its recorded
/// decisions, so a restarted router does not read a substrate that kept
/// losing as untried — it stays stopped.
///
/// Mutation: make `ComputeRouter::replay` skip `learned.absorb` — the
/// replayed record is empty, the equality fires, and the replayed router
/// submits a job it should not.
#[test]
fn a_replayed_router_holds_the_live_routers_learning_and_a_losing_substrate_stays_stopped()
-> Result<()> {
    let problem = discrete(12, 4)?;
    let policy = RoutingPolicy {
        minimum_win_probability: 0.3,
        ..heuristic_policy()
    };
    let baseline = ComputeRouter::classical(1)
        .with_policy(policy)?
        .solve(&problem)?;
    let tie: Vec<u8> = baseline
        .weights
        .iter()
        .map(|w| u8::from(w.abs() > 1e-9))
        .collect();
    let provider = Arc::new(Tying { assignment: tie });

    let live = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(provider.clone())?;
    let mut decisions = Vec::new();
    for _ in 0..6 {
        let decision = live.solve(&problem)?;
        if decision.quantum_dispatch.submitted.is_some() {
            wait_for_job(&live)?;
        }
        decisions.push(compute(&decision)?.clone());
    }
    let live_record = live.learned_record(Substrate::Qpu, 12)?;
    assert_eq!(
        (live_record.trials, live_record.wins),
        (2, 0),
        "premise: the live arm was tried twice, tied twice and stopped"
    );
    assert!(
        live_record.calibration_brier().is_some(),
        "the outcomes scored the router's own estimates"
    );
    // R12-QUANT-4053's latency half. Every QPU answer here was harvested by
    // a later solve than the one that submitted it, so each took at least a
    // cycle; the baseline test above only ever sees a latency of zero, which
    // a fold that dropped latency entirely would also report.
    assert!(
        live_record
            .mean_latency_cycles()
            .is_some_and(|cycles| cycles >= 1.0),
        "a harvested answer's latency was not learned: {live_record:?}"
    );

    let restarted = ComputeRouter::classical(1)
        .with_policy(policy)?
        .with_quantum(provider)?;
    assert_eq!(
        restarted.learned_record(Substrate::Qpu, 12)?.trials,
        0,
        "premise: a fresh router has learned nothing"
    );
    let folded = restarted.replay(decisions.iter())?;
    assert_eq!(folded, 6);
    assert_eq!(restarted.learned_record(Substrate::Qpu, 12)?, live_record);
    assert_eq!(
        restarted.solver_standing(Solver::Quantum.as_str())?,
        live.solver_standing(Solver::Quantum.as_str())?,
        "the registry's view replays too"
    );

    let first_after_restart = restarted.solve(&problem)?;
    assert!(
        first_after_restart.quantum_dispatch.submitted.is_none(),
        "the restarted router paid again for a substrate it had already learned loses"
    );
    Ok(())
}

/// §11.5: a QPU answer on a simulator never counts as advantage, however
/// often it wins; the same record on hardware does — so the boundary can
/// fire in both directions and is not a constant `false`.
///
/// Mutation: in `AdvantageBoundary::assess`, delete the `Some(true)` push —
/// the simulated record counts and the first assertion fires. Change
/// `counts: unmet.is_empty()` to `counts: false` — the hardware premise
/// fires.
#[test]
fn a_simulated_qpu_answer_never_counts_as_advantage_and_the_same_record_on_hardware_does()
-> Result<()> {
    let boundary = AdvantageBoundary::default();
    let record = AdvantageEvidence {
        qpu_answer_used: true,
        simulated: Some(true),
        trials: 100,
        wins: 90,
    };
    let simulated = boundary.assess(record);
    assert!(!simulated.counts);
    assert!(
        simulated.unmet.iter().any(|u| u.contains("simulator")),
        "{:?}",
        simulated.unmet
    );

    let hardware = boundary.assess(AdvantageEvidence {
        simulated: Some(false),
        ..record
    });
    assert!(
        hardware.counts,
        "premise: the boundary admits a sustained hardware win: {:?}",
        hardware.unmet
    );

    let too_few = boundary.assess(AdvantageEvidence {
        simulated: Some(false),
        trials: 3,
        wins: 3,
        ..record
    });
    assert!(!too_few.counts, "three lucky trials are not advantage");

    // Every decision carries the standing; a classical router's says why.
    let decision = ComputeRouter::classical(1).solve(&discrete(10, 4)?)?;
    let standing = &compute(&decision)?.advantage;
    assert!(!standing.counts);
    assert!(
        standing
            .unmet
            .iter()
            .any(|u| u == "no quantum provider is attached"),
        "{:?}",
        standing.unmet
    );
    Ok(())
}

/// §11.3: every routed task names its research domain and carries that
/// domain's required classical check among its verification properties.
///
/// Mutation: in `ComputeRouter::compile`, delete the `format!("{}: {}",
/// ROUTED_DOMAIN…)` entry — the containment assertion fires.
#[test]
fn every_routed_task_names_its_research_domain_and_carries_that_domains_classical_check()
-> Result<()> {
    assert_eq!(
        ResearchDomain::ALL
            .iter()
            .filter(|domain| domain.is_routed())
            .count(),
        1,
        "premise: one domain is routed today"
    );
    let decision = ComputeRouter::classical(1).solve(&discrete(10, 4)?)?;
    let record = compute(&decision)?;
    assert_eq!(
        record.research_domain,
        Some(ResearchDomain::CapitalPlacement)
    );
    let check = format!(
        "{}: {}",
        ResearchDomain::CapitalPlacement.as_str(),
        ResearchDomain::CapitalPlacement.required_check()
    );
    assert!(
        record.task.verification.iter().any(|v| v == &check),
        "{:?}",
        record.task.verification
    );
    Ok(())
}
