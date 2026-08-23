//! The three-way solver comparison, and the two rules it exists to enforce.
//!
//! A classical baseline is always computed, and a non-classical answer is
//! classically re-evaluated before anything may use it. Both are asserted here
//! against a solver that lies about its own result, because a rule that only
//! holds for well-behaved callers is not a rule.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::Decimal;
use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::Duration;
use qip_numerics::anneal::{AnnealSettings, Qubo, solve_exact};
use qip_quantum::benchmark::{ClassicalValidator, SolverBenchmark};
use qip_quantum::provider::{QuantumProvider, SimulatedProvider};
use qip_quantum::qaoa::QaoaSettings;
use qip_quantum::solver::{
    ClassicalSolver, IbmQuantumConfig, IbmQuantumSolver, ProviderSolver, QuantumInspiredSolver,
    QuboSolver, QueuePolicy, SearchTrace, SolverCandidate, SolverCostModel, SolverEffort,
    SolverKind, is_local_optimum,
};
use std::sync::Arc;

/// A dense random spin glass: the standard hard instance for single-flip local
/// search, because its landscape is rugged rather than merely large.
fn spin_glass(n: usize, seed: u64) -> Qubo {
    let mut rng = Xoshiro256::seeded(seed);
    let mut qubo = Qubo::new(n);
    for i in 0..n {
        qubo.add_linear(i, rng.uniform(-1.0, 1.0));
        for j in (i + 1)..n {
            qubo.add(i, j, rng.uniform(-1.0, 1.0));
        }
    }
    qubo
}

fn ibm_config() -> IbmQuantumConfig {
    IbmQuantumConfig {
        api_token_env: "IBM_QUANTUM_API_TOKEN".to_string(),
        instance_crn: "crn:v1:bluemix:public:quantum-computing:us-east:a/ACCOUNT:INSTANCE::"
            .to_string(),
        channel: "ibm_quantum_platform".to_string(),
        backend: "ibm_torino".to_string(),
        queue: QueuePolicy {
            maximum_wait: Duration::from_mins(45),
            shots: 4_096,
            maximum_circuits: 300,
            mode: "session".to_string(),
        },
        max_qubits: 133,
        price_micros_per_job: 1_600_000,
    }
}

/// A solver that returns a worthless assignment and claims it is excellent.
///
/// The point of the whole validation layer. Nothing about its interface says
/// it is lying, which is why the check cannot be a matter of trusting the
/// implementation.
#[derive(Debug)]
struct LyingQuantumSolver {
    claim: f64,
}

impl QuboSolver for LyingQuantumSolver {
    fn name(&self) -> &str {
        "lying-device"
    }

    fn kind(&self) -> SolverKind {
        SolverKind::Quantum
    }

    fn is_available(&self) -> bool {
        true
    }

    fn cost_model(&self) -> SolverCostModel {
        SolverCostModel::in_process(1)
    }

    fn solve(&self, qubo: &Qubo, _effort: &SolverEffort) -> Result<SolverCandidate> {
        Ok(SolverCandidate {
            solver: self.name().to_string(),
            kind: SolverKind::Quantum,
            // Every variable off, which on these instances is nowhere near
            // optimal, together with a claim that it is spectacular.
            assignment: vec![0u8; qubo.n],
            claimed_objective: self.claim,
            evaluations: 1,
            trace: SearchTrace {
                family: "fabricated".to_string(),
                moves_proposed: 0,
                moves_accepted: 0,
                uphill_accepted: 0,
                restarts: 0,
                replicas: 1,
                local_optimum: false,
            },
        })
    }
}

// --- the baseline rule ------------------------------------------------------

#[test]
fn the_benchmark_computes_a_classical_baseline_even_when_a_quantum_solver_is_available()
-> Result<()> {
    // A simulated quantum provider is available and enters the benchmark. The
    // baseline is computed anyway, and it is not an option on the report: a
    // report without one has no representation.
    let qubo = spin_glass(8, 3);
    let quantum = ProviderSolver::new(
        Arc::new(SimulatedProvider::new(9)),
        QaoaSettings {
            layers: 2,
            optimiser_iterations: 60,
            shots: 0,
        },
    );
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(quantum))
        .with_repeats(1)
        .run(&qubo)?;

    assert!(
        report.classical_baseline.usable_solution().is_some(),
        "the baseline produced no validated answer: {:?}",
        report.classical_baseline.refusal
    );
    assert_eq!(report.classical_baseline.kind, SolverKind::Classical);
    assert!(
        report
            .entrants
            .iter()
            .any(|record| record.kind == SolverKind::Quantum),
        "the quantum entrant did not run at all, so this test proves nothing"
    );
    // And the sentence the platform would publish names the baseline.
    let baseline = report.baseline_objective().expect("a baseline objective");
    assert!(
        report.claim().contains(&format!("{baseline:.6}")),
        "the claim omitted the baseline: {}",
        report.claim()
    );
    Ok(())
}

#[test]
fn a_benchmark_whose_classical_baseline_fails_reports_nothing_at_all() {
    // The ordering rule, from the other side: if the baseline cannot be
    // computed there is no report, rather than a report of whatever the
    // quantum path happened to return.
    let qubo = spin_glass(12, 5);
    let outcome = SolverBenchmark::new(ClassicalSolver::exhaustive(4))
        .with_solver(Arc::new(QuantumInspiredSolver::new(1)))
        .with_repeats(1)
        .run(&qubo);
    let error = outcome.expect_err("a benchmark without a baseline must not produce a report");
    assert_eq!(error.code(), "numeric");
    assert!(
        error.message().contains("classical baseline"),
        "the refusal does not say the baseline is the reason: {error}"
    );
}

// --- the validation rule ----------------------------------------------------

#[test]
fn a_solution_that_fails_classical_validation_is_refused_however_good_the_claim() -> Result<()> {
    let qubo = spin_glass(10, 2);
    let liar = LyingQuantumSolver { claim: -1_000.0 };
    let candidate = liar.solve(&qubo, &SolverEffort::default())?;

    let refusal = ClassicalValidator::default()
        .validate(&qubo, &candidate)
        .expect_err("a fabricated objective must not validate");
    assert_eq!(refusal.code(), "guard");
    assert!(
        refusal.message().contains("-1000.000000"),
        "the refusal does not quote the claim it rejected: {refusal}"
    );
    Ok(())
}

#[test]
fn a_refused_answer_never_reaches_the_benchmark_choice() -> Result<()> {
    // The liar claims an objective no assignment on this problem attains. Its
    // record carries no solution, and the classical baseline is chosen.
    let qubo = spin_glass(10, 2);
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(LyingQuantumSolver { claim: -1_000.0 }))
        .with_repeats(2)
        .run(&qubo)?;

    let liar = report
        .record_for("lying-device")
        .expect("the liar entered the benchmark");
    assert!(
        liar.usable_solution().is_none(),
        "a refused answer was reported as usable"
    );
    assert!(
        liar.refusal.is_some(),
        "no reason was recorded for the refusal"
    );
    assert_eq!(liar.reliability.produced, 2, "the liar did answer twice");
    assert_eq!(
        liar.reliability.validated, 0,
        "a fabricated answer must never count as validated"
    );
    assert_eq!(report.chosen, report.classical_baseline.solver);
    assert!(
        report.measured_advantage(0.0).is_none(),
        "a refused answer was credited with an advantage"
    );
    Ok(())
}

#[test]
fn a_validated_solution_reports_the_recomputed_objective_rather_than_the_claim() -> Result<()> {
    // A claim inside tolerance is accepted, and the number that survives is
    // still the one the classical evaluator computed.
    let qubo = spin_glass(8, 4);
    let assignment = vec![1u8, 0, 1, 1, 0, 0, 1, 0];
    let truth = qubo.evaluate(&assignment);
    let candidate = SolverCandidate {
        solver: "nearly-right".to_string(),
        kind: SolverKind::QuantumInspired,
        assignment,
        claimed_objective: truth + 1e-9,
        evaluations: 1,
        trace: SearchTrace {
            family: "test".to_string(),
            moves_proposed: 0,
            moves_accepted: 0,
            uphill_accepted: 0,
            restarts: 0,
            replicas: 1,
            local_optimum: false,
        },
    };
    let validated = ClassicalValidator::default().validate(&qubo, &candidate)?;
    assert!(
        (validated.objective() - truth).abs() < 1e-15,
        "the validated objective is the claim, not the recomputation"
    );
    assert!(validated.validation().discrepancy > 0.0);
    Ok(())
}

#[test]
fn an_assignment_of_the_wrong_shape_is_refused_before_it_is_scored() -> Result<()> {
    let qubo = spin_glass(6, 1);
    let candidate = SolverCandidate {
        solver: "wrong-shape".to_string(),
        kind: SolverKind::Quantum,
        assignment: vec![1u8, 0, 1],
        claimed_objective: -99.0,
        evaluations: 1,
        trace: SearchTrace {
            family: "test".to_string(),
            moves_proposed: 0,
            moves_accepted: 0,
            uphill_accepted: 0,
            restarts: 0,
            replicas: 1,
            local_optimum: false,
        },
    };
    let refusal = ClassicalValidator::default()
        .validate(&qubo, &candidate)
        .expect_err("a three-bit answer to a six-variable problem is not an answer");
    assert_eq!(refusal.code(), "guard");
    Ok(())
}

// --- the quantum-inspired solver is a different solver ----------------------

#[test]
fn the_quantum_inspired_search_is_structurally_a_different_search() -> Result<()> {
    let qubo = spin_glass(16, 7);
    let effort = SolverEffort {
        sweeps: 200,
        restarts: 1,
        seed: 7,
    };
    let descent = ClassicalSolver::descent(11, 1).solve(&qubo, &effort)?;
    let annealing =
        ClassicalSolver::annealing(11, AnnealSettings::default()).solve(&qubo, &effort)?;
    let inspired = QuantumInspiredSolver::new(11).solve(&qubo, &effort)?;

    assert!(inspired.trace.is_distinct_from(&descent.trace));
    assert!(inspired.trace.is_distinct_from(&annealing.trace));
    assert_eq!(inspired.trace.family, "path-integral-quantum-annealing");
    assert!(
        inspired.trace.replicas > 1,
        "a single-configuration search has no imaginary-time direction, so it is not this algorithm"
    );
    assert_eq!(
        descent.trace.replicas, 1,
        "the classical searches are single-configuration"
    );
    assert_eq!(
        descent.trace.uphill_accepted, 0,
        "steepest descent never accepts a worse move; that is what traps it"
    );
    assert!(
        inspired.trace.uphill_accepted > 0,
        "a path-integral run that never went uphill did not explore"
    );
    assert_eq!(inspired.kind, SolverKind::QuantumInspired);
    assert!(
        inspired.kind.needs_a_classical_baseline(),
        "a heuristic borrowed from physics needs a baseline just as a device does"
    );
    Ok(())
}

#[test]
fn the_quantum_inspired_search_escapes_a_local_optimum_that_traps_classical_descent() -> Result<()>
{
    // The claim is made precise rather than left as an impression. On this
    // instance the descent's answer is a *strict* local optimum — no single
    // flip improves it — and it is strictly worse than the enumerated global
    // optimum. So the descent stopped because the landscape trapped it, not
    // because it ran out of budget, which is the only version of "trapped"
    // worth asserting.
    let qubo = spin_glass(16, 7);
    let effort = SolverEffort {
        sweeps: 200,
        restarts: 1,
        seed: 7,
    };
    let optimum = solve_exact(&qubo).expect("sixteen variables enumerate");

    let descent = ClassicalSolver::descent(11, 1).solve(&qubo, &effort)?;
    assert!(
        is_local_optimum(&qubo, &descent.assignment),
        "the descent did not stop at a local optimum, so it was not trapped"
    );
    assert!(
        descent.claimed_objective > optimum.energy + 1e-9,
        "the descent found the optimum on this instance, so it demonstrates no trap"
    );

    let inspired = QuantumInspiredSolver::new(11).solve(&qubo, &effort)?;
    assert!(
        inspired.claimed_objective <= optimum.energy + 1e-9,
        "the path-integral search reached {:.6} against an optimum of {:.6}",
        inspired.claimed_objective,
        optimum.energy
    );
    assert!(
        inspired.claimed_objective < descent.claimed_objective,
        "the path-integral search did not improve on the trapped descent"
    );
    Ok(())
}

#[test]
fn the_quantum_inspired_solver_is_never_reported_as_quantum() -> Result<()> {
    let qubo = spin_glass(10, 1);
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(QuantumInspiredSolver::new(3)))
        .with_repeats(1)
        .run(&qubo)?;
    let record = report
        .record_for("quantum-inspired-path-integral")
        .expect("the entrant ran");
    assert_eq!(record.kind, SolverKind::QuantumInspired);
    assert!(
        !record.kind.is_quantum(),
        "a classical Monte Carlo algorithm was reported as a quantum result"
    );
    Ok(())
}

// --- the IBM port -----------------------------------------------------------

#[test]
fn the_ibm_port_reports_unavailable_and_names_every_missing_item() {
    let solver = IbmQuantumSolver::new(ibm_config());
    // Named explicitly: the port wears both traits, and both must refuse.
    assert!(!QuboSolver::is_available(&solver));
    assert!(!QuantumProvider::is_available(&solver));

    let requirement = QuboSolver::requirement(&solver);
    for item in [
        // the credential, and where it comes from
        "IBM_QUANTUM_API_TOKEN",
        // the instance the token is scoped to
        "crn:v1:bluemix:public:quantum-computing:us-east:a/ACCOUNT:INSTANCE::",
        // the specific device
        "ibm_torino",
        // the channel and the client that is missing
        "ibm_quantum_platform",
        "Qiskit Runtime",
        // the queue policy the job would run under
        "session",
        "4096 shot(s)",
        "300 circuit(s)",
    ] {
        assert!(
            requirement.contains(item),
            "the requirement does not name {item}: {requirement}"
        );
    }

    let refusal = QuboSolver::solve(&solver, &spin_glass(6, 1), &SolverEffort::default())
        .expect_err("an unconfigured device must not appear to solve anything");
    assert_eq!(refusal.code(), "unavailable");

    // The same port, through the provider trait the compute router already
    // takes, says the same thing.
    let provider_refusal =
        QuantumProvider::solve_qubo(&solver, &spin_glass(6, 1), &QaoaSettings::default())
            .expect_err("the provider face must refuse too");
    assert_eq!(provider_refusal.code(), "unavailable");
    assert_eq!(provider_refusal.message(), refusal.message());
}

#[test]
fn an_ibm_backend_with_a_token_and_an_instance_is_still_unavailable_without_a_transport() {
    // The availability logic is the real one: supplying the two things a
    // deployment can supply does not make a client appear.
    let solver = IbmQuantumSolver::with_credentials(ibm_config(), true, true);
    assert!(!QuboSolver::is_available(&solver));
    let requirement = QuboSolver::requirement(&solver);
    assert!(
        !requirement.contains("IBM_QUANTUM_API_TOKEN"),
        "a present credential is still being reported as missing: {requirement}"
    );
    assert!(
        requirement.contains("Qiskit Runtime"),
        "the missing client is not named: {requirement}"
    );
}

#[test]
fn an_unavailable_solver_is_reported_with_its_requirement_and_no_solution() -> Result<()> {
    let qubo = spin_glass(10, 6);
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(IbmQuantumSolver::new(ibm_config())))
        .with_repeats(1)
        .run(&qubo)?;

    let record = report.record_for("ibm_torino").expect("the port is listed");
    assert!(!record.available);
    assert!(record.usable_solution().is_none());
    assert!(record.requirement.contains("ibm_torino"));
    assert_eq!(
        record.price,
        Decimal::ZERO,
        "a job that never ran was billed"
    );
    assert_eq!(record.runtime, Duration::ZERO);
    assert!(
        report.notes.iter().any(|note| note.contains("ibm_torino")),
        "an operator reading the notes would not learn the device was skipped"
    );
    // And the baseline still stands.
    assert!(report.classical_baseline.usable_solution().is_some());
    Ok(())
}

// --- the comparison itself --------------------------------------------------

#[test]
fn the_benchmark_compares_quality_runtime_cost_and_reliability() -> Result<()> {
    let qubo = spin_glass(12, 8);
    let device_cost = SolverCostModel {
        nanos_per_evaluation: 0,
        queue: Duration::from_mins(30),
        price_micros_per_job: 1_500_000,
    };
    let quantum = ProviderSolver::new(
        Arc::new(SimulatedProvider::new(4)),
        QaoaSettings {
            layers: 2,
            optimiser_iterations: 40,
            shots: 0,
        },
    )
    .with_cost(device_cost);

    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(QuantumInspiredSolver::new(2)))
        .with_solver(Arc::new(quantum))
        .with_repeats(3)
        .run(&qubo)?;

    assert!(
        report.baseline_is_exact,
        "the baseline for twelve variables is exact"
    );
    for record in report.records() {
        // Quality is measured against the same baseline for everyone.
        if let Some(quality) = record.quality {
            assert!(
                (quality.baseline_objective - report.baseline_objective().unwrap_or_default())
                    .abs()
                    < 1e-12
            );
            assert!(
                quality.approximation_ratio.is_some(),
                "the baseline is exact, so a ratio to the optimum is available"
            );
        }
        assert!(record.reliability.attempts <= 3);
    }

    let device = report
        .record_for("statevector-simulator")
        .expect("the device ran");
    assert!(
        device.runtime >= Duration::from_mins(30),
        "the modelled runtime does not include the queue"
    );
    assert_eq!(
        device.price,
        Decimal::from_scaled(1_500_000, 6).expect("an exact price"),
        "the price is not exact money"
    );

    let inspired = report
        .record_for("quantum-inspired-path-integral")
        .expect("the inspired solver ran");
    assert_eq!(inspired.reliability.attempts, 3);
    assert!(inspired.reliability.validated > 0);
    assert!(inspired.reliability.success_rate() > 0.0);
    assert!((inspired.reliability.validation_rate() - 1.0).abs() < 1e-12);
    Ok(())
}

#[test]
fn every_solver_is_reproducible_under_a_fixed_seed() -> Result<()> {
    let qubo = spin_glass(14, 12);
    let effort = SolverEffort {
        sweeps: 120,
        restarts: 2,
        seed: 99,
    };
    for solver in [
        Box::new(ClassicalSolver::descent(5, 3)) as Box<dyn QuboSolver>,
        Box::new(ClassicalSolver::annealing(5, AnnealSettings::default())),
        Box::new(QuantumInspiredSolver::new(5)),
    ] {
        let first = solver.solve(&qubo, &effort)?;
        let second = solver.solve(&qubo, &effort)?;
        assert_eq!(
            first,
            second,
            "{} produced two different answers from one seed",
            solver.name()
        );
    }
    Ok(())
}

#[test]
fn the_whole_benchmark_is_reproducible_under_a_fixed_seed() -> Result<()> {
    let qubo = spin_glass(10, 21);
    let build = || {
        SolverBenchmark::new(ClassicalSolver::exhaustive(20))
            .with_solver(Arc::new(QuantumInspiredSolver::new(6)))
            .with_effort(SolverEffort {
                sweeps: 80,
                restarts: 1,
                seed: 4_242,
            })
            .with_repeats(2)
    };
    let first = build().run(&qubo)?;
    let second = build().run(&qubo)?;
    assert_eq!(first, second, "the benchmark is not reproducible");
    Ok(())
}
