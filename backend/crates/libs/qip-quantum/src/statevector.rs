//! A statevector simulator.
//!
//! Enough of one to run the variational circuits the platform actually uses:
//! state preparation, the single-qubit rotations, CNOT, and expectation values
//! of diagonal Hamiltonians. It is exact — no sampling noise unless sampling is
//! asked for — which makes a QAOA run reproducible and a test meaningful.
//!
//! The size limit is physics, not implementation. A statevector needs `2ⁿ`
//! complex amplitudes, so twenty qubits is sixteen megabytes and thirty is
//! sixteen gigabytes. [`MAX_QUBITS`] draws the line where the allocation stops
//! being reasonable, and the constructor refuses beyond it rather than
//! attempting it — a simulator that dies inside an optimiser takes the
//! optimiser with it.

use qip_core::error::{Error, Result};
use qip_core::rng::Rng;
use serde::{Deserialize, Serialize};

/// The largest circuit this simulator will attempt.
///
/// Twenty-four qubits is 2²⁴ complex amplitudes: 256MB at f64 pairs. Past that
/// the allocation is more likely to fail than to finish.
pub const MAX_QUBITS: usize = 24;

/// A complex amplitude.
///
/// Written out rather than pulled from a crate: the operations needed are
/// addition, multiplication and modulus, and the dependency policy is that a
/// dependency has to earn its place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };
    pub const ONE: Self = Self { re: 1.0, im: 0.0 };

    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// `e^{iθ}`.
    pub fn phase(theta: f64) -> Self {
        Self {
            re: theta.cos(),
            im: theta.sin(),
        }
    }

    pub fn scale(self, factor: f64) -> Self {
        Self {
            re: self.re * factor,
            im: self.im * factor,
        }
    }

    pub fn conjugate(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// Squared modulus: the measurement probability of this amplitude.
    pub fn norm_squared(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn modulus(self) -> f64 {
        self.norm_squared().sqrt()
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, other: Self) -> Self {
        Self {
            re: self.re + other.re,
            im: self.im + other.im,
        }
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, other: Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, other: Self) -> Self {
        Self {
            re: self.re * other.re - self.im * other.im,
            im: self.re * other.im + self.im * other.re,
        }
    }
}

/// A pure state of `n` qubits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateVector {
    qubits: usize,
    amplitudes: Vec<Complex>,
}

impl StateVector {
    /// All qubits in |0⟩.
    pub fn zero(qubits: usize) -> Result<Self> {
        if qubits == 0 {
            return Err(Error::invalid("a state needs at least one qubit"));
        }
        if qubits > MAX_QUBITS {
            return Err(Error::invalid(format!(
                "{qubits} qubits needs {} amplitudes, beyond the {MAX_QUBITS}-qubit limit this simulator will attempt",
                1u64 << qubits.min(63)
            )));
        }
        let mut amplitudes = vec![Complex::ZERO; 1 << qubits];
        amplitudes[0] = Complex::ONE;
        Ok(Self { qubits, amplitudes })
    }

    /// The uniform superposition, the standard QAOA starting state.
    pub fn uniform(qubits: usize) -> Result<Self> {
        let mut state = Self::zero(qubits)?;
        let amplitude = 1.0 / (state.amplitudes.len() as f64).sqrt();
        for value in state.amplitudes.iter_mut() {
            *value = Complex::new(amplitude, 0.0);
        }
        Ok(state)
    }

    pub fn qubits(&self) -> usize {
        self.qubits
    }

    pub fn dimension(&self) -> usize {
        self.amplitudes.len()
    }

    pub fn amplitudes(&self) -> &[Complex] {
        &self.amplitudes
    }

    /// Probability of measuring the given basis state.
    pub fn probability(&self, basis: usize) -> f64 {
        self.amplitudes
            .get(basis)
            .map(|amplitude| amplitude.norm_squared())
            .unwrap_or(0.0)
    }

    /// Total probability, which must stay at one.
    ///
    /// Checked in tests after every gate: a gate that does not preserve the
    /// norm is not unitary, and a non-unitary "quantum" simulator produces
    /// results that cannot occur.
    pub fn total_probability(&self) -> f64 {
        self.amplitudes.iter().map(|a| a.norm_squared()).sum()
    }

    /// The most likely basis state, and its probability.
    pub fn most_likely(&self) -> (usize, f64) {
        self.amplitudes
            .iter()
            .enumerate()
            .map(|(index, amplitude)| (index, amplitude.norm_squared()))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0, 0.0))
    }

    /// Hadamard on one qubit.
    pub fn hadamard(&mut self, qubit: usize) -> Result<()> {
        self.check_qubit(qubit)?;
        let factor = std::f64::consts::FRAC_1_SQRT_2;
        self.apply_single(qubit, |a0, a1| {
            ((a0 + a1).scale(factor), (a0 - a1).scale(factor))
        });
        Ok(())
    }

    /// Pauli-X: the bit flip.
    pub fn pauli_x(&mut self, qubit: usize) -> Result<()> {
        self.check_qubit(qubit)?;
        self.apply_single(qubit, |a0, a1| (a1, a0));
        Ok(())
    }

    /// Pauli-Z: a sign on |1⟩.
    pub fn pauli_z(&mut self, qubit: usize) -> Result<()> {
        self.check_qubit(qubit)?;
        self.apply_single(qubit, |a0, a1| (a0, a1.scale(-1.0)));
        Ok(())
    }

    /// Rotation about X by `theta`.
    pub fn rotate_x(&mut self, qubit: usize, theta: f64) -> Result<()> {
        self.check_qubit(qubit)?;
        let (cos, sin) = ((theta / 2.0).cos(), (theta / 2.0).sin());
        self.apply_single(qubit, |a0, a1| {
            (
                a0.scale(cos) + a1 * Complex::new(0.0, -sin),
                a0 * Complex::new(0.0, -sin) + a1.scale(cos),
            )
        });
        Ok(())
    }

    /// Rotation about Z by `theta`.
    pub fn rotate_z(&mut self, qubit: usize, theta: f64) -> Result<()> {
        self.check_qubit(qubit)?;
        let minus = Complex::phase(-theta / 2.0);
        let plus = Complex::phase(theta / 2.0);
        self.apply_single(qubit, |a0, a1| (a0 * minus, a1 * plus));
        Ok(())
    }

    /// CNOT: flip `target` where `control` is one.
    pub fn cnot(&mut self, control: usize, target: usize) -> Result<()> {
        self.check_qubit(control)?;
        self.check_qubit(target)?;
        if control == target {
            return Err(Error::invalid("CNOT needs two distinct qubits"));
        }
        let control_mask = 1usize << control;
        let target_mask = 1usize << target;
        for index in 0..self.amplitudes.len() {
            // Swap each pair once, by only acting from the side where the
            // target bit is zero.
            if index & control_mask != 0 && index & target_mask == 0 {
                self.amplitudes.swap(index, index | target_mask);
            }
        }
        Ok(())
    }

    /// A phase `e^{iθ}` on basis states where both qubits are one.
    ///
    /// The two-qubit primitive a QUBO's quadratic terms map onto.
    pub fn controlled_phase(&mut self, a: usize, b: usize, theta: f64) -> Result<()> {
        self.check_qubit(a)?;
        self.check_qubit(b)?;
        let mask = (1usize << a) | (1usize << b);
        let phase = Complex::phase(theta);
        for (index, amplitude) in self.amplitudes.iter_mut().enumerate() {
            if index & mask == mask {
                *amplitude = *amplitude * phase;
            }
        }
        Ok(())
    }

    /// Apply a phase proportional to a diagonal cost function.
    ///
    /// This is the QAOA cost layer for any objective the platform can evaluate
    /// classically on a bit string, which covers every QUBO.
    pub fn apply_diagonal_phase<F: Fn(usize) -> f64>(&mut self, gamma: f64, cost: F) {
        for (index, amplitude) in self.amplitudes.iter_mut().enumerate() {
            *amplitude = *amplitude * Complex::phase(-gamma * cost(index));
        }
    }

    /// The QAOA mixing layer: an X rotation on every qubit.
    pub fn apply_mixer(&mut self, beta: f64) -> Result<()> {
        for qubit in 0..self.qubits {
            self.rotate_x(qubit, 2.0 * beta)?;
        }
        Ok(())
    }

    /// Expectation of a diagonal observable.
    pub fn expectation<F: Fn(usize) -> f64>(&self, observable: F) -> f64 {
        self.amplitudes
            .iter()
            .enumerate()
            .map(|(index, amplitude)| amplitude.norm_squared() * observable(index))
            .sum()
    }

    /// Sample basis states, as a real device would.
    pub fn sample<R: Rng>(&self, shots: usize, rng: &mut R) -> Vec<usize> {
        let weights: Vec<f64> = self.amplitudes.iter().map(|a| a.norm_squared()).collect();
        (0..shots)
            .map(|_| rng.weighted_index(&weights).unwrap_or(0))
            .collect()
    }

    /// Renormalise, correcting the drift a long circuit accumulates.
    pub fn normalise(&mut self) {
        let norm = self.total_probability().sqrt();
        if norm > 1e-300 {
            for amplitude in self.amplitudes.iter_mut() {
                *amplitude = amplitude.scale(1.0 / norm);
            }
        }
    }

    fn check_qubit(&self, qubit: usize) -> Result<()> {
        if qubit >= self.qubits {
            return Err(Error::invalid(format!(
                "qubit {qubit} is outside a {}-qubit register",
                self.qubits
            )));
        }
        Ok(())
    }

    /// Apply a 2×2 unitary to one qubit across every amplitude pair.
    fn apply_single<F: Fn(Complex, Complex) -> (Complex, Complex)>(&mut self, qubit: usize, f: F) {
        let mask = 1usize << qubit;
        for index in 0..self.amplitudes.len() {
            if index & mask != 0 {
                continue;
            }
            let partner = index | mask;
            let (a0, a1) = (self.amplitudes[index], self.amplitudes[partner]);
            let (b0, b1) = f(a0, a1);
            self.amplitudes[index] = b0;
            self.amplitudes[partner] = b1;
        }
    }
}
