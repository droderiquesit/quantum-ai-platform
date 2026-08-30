//! Tests for the quantum library.
//!
//! The simulator's job is to be *correct*, not fast: if it is not unitary, or
//! its expectations are wrong, then every QAOA result it produces is fiction.
//! Most of these tests check exactly that.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_core::rng::Xoshiro256;
use qip_core::testing::approx_eq;
use qip_numerics::anneal::{Qubo, solve_exact};
use qip_quantum::provider::{
    HostedConfig, HostedProvider, ProviderCapabilities, QuantumProvider, SimulatedProvider,
};
use qip_quantum::qaoa::{QaoaSettings, solve};
use qip_quantum::statevector::{Complex, MAX_QUBITS, StateVector};

// --- the simulator ----------------------------------------------------------

#[test]
fn a_fresh_register_is_in_the_zero_state() -> Result<()> {
    let state = StateVector::zero(3)?;
    assert_eq!(state.dimension(), 8);
    assert!(approx_eq(state.probability(0), 1.0, 1e-15));
    for basis in 1..8 {
        assert!(approx_eq(state.probability(basis), 0.0, 1e-15));
    }
    Ok(())
}

#[test]
fn every_gate_preserves_the_total_probability() -> Result<()> {
    // A gate that does not preserve the norm is not unitary, and a
    // non-unitary "quantum" simulator produces results that cannot occur.
    /// A named gate to apply, so the loop below can report which one broke.
    type Gate = Box<dyn Fn(&mut StateVector) -> Result<()>>;

    let mut state = StateVector::zero(4)?;
    let checks: Vec<(&str, Gate)> = vec![
        ("hadamard", Box::new(|s: &mut StateVector| s.hadamard(0))),
        ("pauli_x", Box::new(|s: &mut StateVector| s.pauli_x(1))),
        ("pauli_z", Box::new(|s: &mut StateVector| s.pauli_z(2))),
        (
            "rotate_x",
            Box::new(|s: &mut StateVector| s.rotate_x(3, 0.7)),
        ),
        (
            "rotate_z",
            Box::new(|s: &mut StateVector| s.rotate_z(0, 1.3)),
        ),
        ("cnot", Box::new(|s: &mut StateVector| s.cnot(0, 1))),
        (
            "controlled_phase",
            Box::new(|s: &mut StateVector| s.controlled_phase(1, 2, 0.9)),
        ),
    ];
    for (name, gate) in checks {
        gate(&mut state)?;
        assert!(
            approx_eq(state.total_probability(), 1.0, 1e-12),
            "{name} left the total probability at {}",
            state.total_probability()
        );
    }
    Ok(())
}

#[test]
fn hadamard_creates_an_equal_superposition_and_undoes_itself() -> Result<()> {
    let mut state = StateVector::zero(1)?;
    state.hadamard(0)?;
    assert!(approx_eq(state.probability(0), 0.5, 1e-12));
    assert!(approx_eq(state.probability(1), 0.5, 1e-12));
    // H is its own inverse.
    state.hadamard(0)?;
    assert!(approx_eq(state.probability(0), 1.0, 1e-12));
    Ok(())
}

#[test]
fn cnot_produces_a_bell_state() -> Result<()> {
    // The standard entanglement check: half the amplitude on |00⟩ and half on
    // |11⟩, and nothing on the states where the qubits disagree.
    let mut state = StateVector::zero(2)?;
    state.hadamard(0)?;
    state.cnot(0, 1)?;
    assert!(approx_eq(state.probability(0b00), 0.5, 1e-12));
    assert!(approx_eq(state.probability(0b11), 0.5, 1e-12));
    assert!(approx_eq(state.probability(0b01), 0.0, 1e-12));
    assert!(approx_eq(state.probability(0b10), 0.0, 1e-12));
    Ok(())
}

#[test]
fn a_full_x_rotation_returns_the_state_up_to_a_global_phase() -> Result<()> {
    // Rx(2π) is -I: the probabilities return, the amplitude flips sign. A
    // simulator that got this wrong would be off by a sign in every
    // interference term.
    let mut state = StateVector::zero(1)?;
    state.rotate_x(0, 2.0 * std::f64::consts::PI)?;
    assert!(approx_eq(state.probability(0), 1.0, 1e-12));
    assert!(
        state.amplitudes()[0].re < 0.0,
        "Rx(2pi) must be -I, not I: {:?}",
        state.amplitudes()[0]
    );
    Ok(())
}

#[test]
fn expectation_of_a_diagonal_observable_is_the_probability_weighted_sum() -> Result<()> {
    let mut state = StateVector::zero(2)?;
    state.hadamard(0)?;
    state.hadamard(1)?;
    // Uniform over four states; the observable returns the basis index.
    let expectation = state.expectation(|index| index as f64);
    assert!(approx_eq(expectation, 1.5, 1e-12), "{expectation}");
    Ok(())
}

#[test]
fn a_register_beyond_the_limit_is_refused_rather_than_attempted() {
    // Twenty-five qubits is 2^25 amplitudes. Attempting the allocation inside
    // an optimiser would take the optimiser down with it.
    let error = StateVector::zero(MAX_QUBITS + 1).unwrap_err();
    assert!(error.message().contains("beyond"), "{}", error.message());
    assert!(StateVector::zero(0).is_err());
}

#[test]
fn a_gate_on_a_qubit_outside_the_register_is_refused() -> Result<()> {
    let mut state = StateVector::zero(2)?;
    assert!(state.hadamard(5).is_err());
    assert!(state.cnot(0, 0).is_err(), "CNOT needs distinct qubits");
    Ok(())
}

#[test]
fn sampling_reproduces_the_amplitude_distribution() -> Result<()> {
    let mut state = StateVector::zero(2)?;
    state.hadamard(0)?;
    let mut rng = Xoshiro256::seeded(9);
    let samples = state.sample(20_000, &mut rng);
    let zeros = samples.iter().filter(|s| **s == 0).count() as f64 / 20_000.0;
    assert!(
        (zeros - 0.5).abs() < 0.02,
        "expected about half zeros, got {zeros}"
    );
    Ok(())
}

#[test]
fn complex_arithmetic_is_correct() {
    let a = Complex::new(1.0, 2.0);
    let b = Complex::new(3.0, -1.0);
    let product = a * b;
    // (1+2i)(3-i) = 3 - i + 6i - 2i² = 5 + 5i
    assert!(approx_eq(product.re, 5.0, 1e-15));
    assert!(approx_eq(product.im, 5.0, 1e-15));
    assert!(approx_eq(a.norm_squared(), 5.0, 1e-15));
    // e^{iπ} = -1
    let phase = Complex::phase(std::f64::consts::PI);
    assert!(approx_eq(phase.re, -1.0, 1e-12));
    assert!(approx_eq(phase.im, 0.0, 1e-12));
}

// --- QAOA -------------------------------------------------------------------

/// A small QUBO whose optimum is known by enumeration.
fn max_cut_triangle() -> Qubo {
    // Max-cut on a triangle: any two-versus-one split cuts two edges, which is
    // the best possible. As a minimisation, the objective is -cut.
    let mut qubo = Qubo::new(3);
    for (i, j) in [(0, 1), (1, 2), (0, 2)] {
        // -(x_i + x_j - 2 x_i x_j) for each edge.
        qubo.add_linear(i, -1.0);
        qubo.add_linear(j, -1.0);
        qubo.add(i, j, 2.0);
    }
    qubo
}

#[test]
fn qaoa_finds_the_known_optimum_of_a_small_problem() -> Result<()> {
    let qubo = max_cut_triangle();
    let exact = solve_exact(&qubo).expect("a three-variable problem enumerates");
    let mut rng = Xoshiro256::seeded(5);
    let result = solve(&qubo, &QaoaSettings::default(), &mut rng)?;

    assert!(
        approx_eq(result.energy, exact.energy, 1e-9),
        "QAOA found {} against the optimum {}",
        result.energy,
        exact.energy
    );
    assert!(result.success_probability > 0.0);
    Ok(())
}

#[test]
fn the_qaoa_expectation_is_worse_than_its_best_sample() -> Result<()> {
    // The expectation is the average over the whole distribution, which
    // includes bad states. A caller uses the best sample and checks it
    // classically; quoting the expectation as the result would be misleading.
    let qubo = max_cut_triangle();
    let mut rng = Xoshiro256::seeded(7);
    let result = solve(&qubo, &QaoaSettings::default(), &mut rng)?;
    assert!(
        result.expectation >= result.energy - 1e-9,
        "expectation {} below the best sample {}",
        result.expectation,
        result.energy
    );
    Ok(())
}

#[test]
fn more_layers_do_not_make_the_result_worse() -> Result<()> {
    // A monotonicity check on the formulation rather than a claim about
    // convergence: deeper circuits can express everything shallower ones can.
    let qubo = max_cut_triangle();
    let shallow = solve(
        &qubo,
        &QaoaSettings {
            layers: 1,
            ..QaoaSettings::default()
        },
        &mut Xoshiro256::seeded(3),
    )?;
    let deep = solve(
        &qubo,
        &QaoaSettings {
            layers: 4,
            ..QaoaSettings::default()
        },
        &mut Xoshiro256::seeded(3),
    )?;
    assert!(deep.energy <= shallow.energy + 1e-9);
    Ok(())
}

#[test]
fn a_qaoa_run_is_reproducible() -> Result<()> {
    let qubo = max_cut_triangle();
    let a = solve(&qubo, &QaoaSettings::default(), &mut Xoshiro256::seeded(11))?;
    let b = solve(&qubo, &QaoaSettings::default(), &mut Xoshiro256::seeded(11))?;
    assert_eq!(a.assignment, b.assignment);
    assert_eq!(a.angles, b.angles);
    Ok(())
}

#[test]
fn an_approximation_ratio_against_a_zero_optimum_is_withheld() -> Result<()> {
    // Dividing by zero would produce an infinity that reads as a spectacular
    // result.
    let qubo = Qubo::new(2);
    let result = solve(&qubo, &QaoaSettings::default(), &mut Xoshiro256::seeded(1))?;
    assert!(result.approximation_ratio(0.0).is_none());
    Ok(())
}

// --- providers --------------------------------------------------------------

#[test]
fn the_simulator_is_available_and_says_it_is_a_simulator() {
    let provider = SimulatedProvider::new(1);
    assert!(provider.is_available());
    let capabilities = provider.capabilities();
    assert!(
        capabilities.simulated,
        "a simulated result must never be presentable as hardware evidence"
    );
    assert!(!capabilities.noisy);
    assert_eq!(capabilities.cost_per_job_micros, 0);
}

#[test]
fn the_simulator_refuses_a_problem_it_cannot_hold() {
    let provider = SimulatedProvider::new(1).with_max_qubits(4);
    let qubo = Qubo::new(10);
    let error = provider
        .solve_qubo(&qubo, &QaoaSettings::default())
        .unwrap_err();
    assert!(
        error.message().contains("qubit limit"),
        "{}",
        error.message()
    );
}

fn hosted() -> HostedConfig {
    HostedConfig {
        vendor: "ibm-quantum".to_string(),
        backend: "ibm_torino".to_string(),
        credential_env: "QIP_QUANTUM_TOKEN".to_string(),
        endpoint: "https://api.quantum.ibm.com".to_string(),
        max_qubits: 133,
        cost_per_job_micros: 1_500_000,
    }
}

#[test]
fn the_hosted_provider_reports_exactly_what_is_missing() {
    let provider = HostedProvider::new(hosted());
    assert!(!provider.is_available());

    let requirement = provider.requirement();
    assert!(requirement.contains("QIP_QUANTUM_TOKEN"), "{requirement}");
    assert!(requirement.contains("ibm-quantum"), "{requirement}");
    assert!(
        requirement.contains("classical solver"),
        "an operator needs to know what happens instead: {requirement}"
    );

    // And it refuses rather than pretending to submit a job.
    let error = provider
        .solve_qubo(&Qubo::new(4), &QaoaSettings::default())
        .unwrap_err();
    assert!(error.message().contains("not usable"));
}

#[test]
fn a_credential_alone_does_not_make_the_hosted_provider_usable() {
    // The transport is missing too, and reporting availability on a credential
    // alone would make a deployment fail at the first optimisation instead of
    // at start-up.
    let provider = HostedProvider::with_credential(hosted(), true);
    assert!(!provider.is_available());
    let requirement = provider.requirement();
    assert!(!requirement.contains("QIP_QUANTUM_TOKEN"));
    assert!(requirement.contains("HTTPS transport"), "{requirement}");
}

#[test]
fn hosted_capabilities_describe_real_hardware_rather_than_a_simulator() {
    let provider = HostedProvider::new(hosted());
    let capabilities: ProviderCapabilities = provider.capabilities();
    assert!(!capabilities.simulated);
    assert!(
        capabilities.noisy,
        "real hardware has noise this build does not model"
    );
    assert!(capabilities.cost_per_job_micros > 0);
}
