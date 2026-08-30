//! The Quantum Approximate Optimization Algorithm.
//!
//! QAOA alternates a cost layer, which phases each basis state by its
//! objective value, with a mixer that spreads amplitude between them. The
//! angles are found classically — Nelder-Mead over `2p` parameters — and the
//! quantum part is the expectation evaluation.
//!
//! **What this is and is not.** Run against
//! [`crate::statevector::StateVector`] it is an exact simulation, which is
//! useful for testing the formulation and worthless as evidence of quantum
//! advantage: simulating the circuit costs more than solving the problem. Run
//! against real hardware it is subject to noise this does not model. Either
//! way [`QaoaResult`] carries the classical baseline it was measured against,
//! because a QAOA result quoted without one says nothing.

use crate::statevector::StateVector;
use qip_core::error::Result;
use qip_core::rng::Rng;
use qip_numerics::anneal::Qubo;
use qip_numerics::search::nelder_mead;
use serde::{Deserialize, Serialize};

/// How QAOA is run.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct QaoaSettings {
    /// Circuit depth. Each layer adds two parameters and, in principle,
    /// quality; in practice the classical optimisation gets harder faster than
    /// the answer gets better, and past about six layers it stops paying.
    pub layers: usize,
    /// Iterations of the classical angle search.
    pub optimiser_iterations: usize,
    /// Shots when sampling. Zero reads the exact distribution, which only a
    /// simulator can do.
    pub shots: usize,
}

impl Default for QaoaSettings {
    fn default() -> Self {
        Self {
            layers: 3,
            optimiser_iterations: 300,
            shots: 0,
        }
    }
}

/// What a QAOA run produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QaoaResult {
    /// Best bit string found.
    pub assignment: Vec<u8>,
    /// Its objective value.
    pub energy: f64,
    /// Expectation of the objective over the final state — what the algorithm
    /// actually optimises, and usually worse than the best sample.
    pub expectation: f64,
    /// Probability of measuring the best assignment.
    pub success_probability: f64,
    pub layers: usize,
    /// The angles found, gammas then betas.
    pub angles: Vec<f64>,
    /// Objective evaluations spent on the classical angle search.
    pub evaluations: usize,
}

impl QaoaResult {
    /// Approximation ratio against a known optimum, where one is available.
    ///
    /// Returns `None` rather than a ratio when the optimum is zero: dividing
    /// by it would produce an infinity that reads as a spectacular result.
    pub fn approximation_ratio(&self, optimum: f64) -> Option<f64> {
        if optimum.abs() < 1e-12 {
            return None;
        }
        Some(self.energy / optimum)
    }
}

/// Run QAOA against a QUBO on the statevector simulator.
pub fn solve<R: Rng>(qubo: &Qubo, settings: &QaoaSettings, rng: &mut R) -> Result<QaoaResult> {
    let n = qubo.n;
    let mut state = StateVector::zero(n)?;
    let _ = &mut state;

    let cost = |index: usize| -> f64 {
        let bits: Vec<u8> = (0..n).map(|q| ((index >> q) & 1) as u8).collect();
        qubo.evaluate(&bits)
    };

    // Counted through a Cell so the objective can stay an `Fn`, which is what
    // the derivative-free optimiser takes.
    let evaluations = std::cell::Cell::new(0usize);
    let objective = |angles: &[f64]| -> f64 {
        let Ok(state) = prepare(n, angles, settings.layers, &cost) else {
            return f64::INFINITY;
        };
        state.expectation(cost)
    };

    // Start from a linear ramp: gammas increasing, betas decreasing. This is
    // the standard initialisation and it matters — from random angles the
    // classical search often converges to a local optimum no better than
    // guessing.
    let start: Vec<f64> = (0..settings.layers)
        .map(|layer| 0.5 * (layer + 1) as f64 / settings.layers as f64)
        .chain(
            (0..settings.layers)
                .map(|layer| 0.5 * (settings.layers - layer) as f64 / settings.layers as f64),
        )
        .collect();

    let (angles, _) = nelder_mead(
        |x| {
            evaluations.set(evaluations.get() + 1);
            objective(x)
        },
        &start,
        0.3,
        settings.optimiser_iterations,
        1e-8,
    );

    let state = prepare(n, &angles, settings.layers, &cost)?;
    let expectation = state.expectation(cost);

    // The best sample, not the expectation, is what a caller would use: QAOA
    // is run to produce a candidate, and the candidate is checked classically.
    let (best_index, success_probability) = if settings.shots == 0 {
        best_by_energy(&state, &cost)
    } else {
        let samples = state.sample(settings.shots, rng);
        let best = samples
            .iter()
            .copied()
            .min_by(|a, b| {
                cost(*a)
                    .partial_cmp(&cost(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        let hits = samples.iter().filter(|s| **s == best).count();
        (best, hits as f64 / settings.shots as f64)
    };

    let assignment: Vec<u8> = (0..n).map(|q| ((best_index >> q) & 1) as u8).collect();
    Ok(QaoaResult {
        energy: qubo.evaluate(&assignment),
        assignment,
        expectation,
        success_probability,
        layers: settings.layers,
        angles,
        evaluations: evaluations.get(),
    })
}

/// Build the QAOA state for a set of angles.
fn prepare<F: Fn(usize) -> f64>(
    qubits: usize,
    angles: &[f64],
    layers: usize,
    cost: &F,
) -> Result<StateVector> {
    let mut state = StateVector::uniform(qubits)?;
    for layer in 0..layers {
        let gamma = angles.get(layer).copied().unwrap_or(0.0);
        let beta = angles.get(layers + layer).copied().unwrap_or(0.0);
        state.apply_diagonal_phase(gamma, cost);
        state.apply_mixer(beta)?;
    }
    // A long circuit accumulates rounding drift; renormalising keeps the
    // expectation an expectation.
    state.normalise();
    Ok(state)
}

/// The lowest-cost basis state with non-negligible amplitude, and its
/// probability.
fn best_by_energy<F: Fn(usize) -> f64>(state: &StateVector, cost: &F) -> (usize, f64) {
    let mut best = 0usize;
    let mut best_cost = f64::INFINITY;
    for index in 0..state.dimension() {
        // Ignore states the circuit gives essentially no amplitude: a
        // vanishing-probability optimum is not one a device would ever return.
        if state.probability(index) < 1e-12 {
            continue;
        }
        let value = cost(index);
        if value < best_cost {
            best_cost = value;
            best = index;
        }
    }
    (best, state.probability(best))
}
