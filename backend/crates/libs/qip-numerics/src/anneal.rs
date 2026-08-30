//! Simulated annealing over binary quadratic problems.
//!
//! This is the honest classical baseline the quantum solvers are measured
//! against. Portfolio selection with a cardinality constraint is naturally a
//! QUBO; before claiming any quantum advantage the platform runs this, on the
//! same problem instance, with a comparable time budget.

use qip_core::rng::Rng;
use serde::{Deserialize, Serialize};

/// Upper-triangular QUBO: `minimise x'Qx` for `x` in `{0,1}^n`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Qubo {
    pub n: usize,
    /// `entries[(i, j)]` with `i <= j`; the diagonal holds the linear terms.
    pub entries: Vec<(usize, usize, f64)>,
    /// Constant offset, so objective values stay comparable across encodings.
    pub offset: f64,
}

impl Qubo {
    pub fn new(n: usize) -> Self {
        Self {
            n,
            entries: Vec::new(),
            offset: 0.0,
        }
    }

    /// Add `weight` to the coefficient of `x_i x_j` (or `x_i` when `i == j`).
    pub fn add(&mut self, i: usize, j: usize, weight: f64) {
        let (i, j) = if i <= j { (i, j) } else { (j, i) };
        if let Some(existing) = self.entries.iter_mut().find(|(a, b, _)| *a == i && *b == j) {
            existing.2 += weight;
        } else {
            self.entries.push((i, j, weight));
        }
    }

    pub fn add_linear(&mut self, i: usize, weight: f64) {
        self.add(i, i, weight);
    }

    /// Objective value at a binary assignment.
    pub fn evaluate(&self, x: &[u8]) -> f64 {
        let mut total = self.offset;
        for &(i, j, w) in &self.entries {
            if i >= x.len() || j >= x.len() {
                continue;
            }
            if x[i] == 1 && x[j] == 1 {
                total += w;
            }
        }
        total
    }

    /// Change in objective from flipping bit `k`, without a full re-evaluation.
    pub fn delta(&self, x: &[u8], k: usize) -> f64 {
        let mut delta = 0.0;
        let turning_on = x[k] == 0;
        for &(i, j, w) in &self.entries {
            if i == k && j == k {
                delta += w;
            } else if i == k {
                if x[j] == 1 {
                    delta += w;
                }
            } else if j == k && x[i] == 1 {
                delta += w;
            }
        }
        if turning_on { delta } else { -delta }
    }

    /// Dense matrix form, for the quantum backends.
    pub fn to_dense(&self) -> crate::matrix::Matrix {
        let mut m = crate::matrix::Matrix::zeros(self.n, self.n);
        for &(i, j, w) in &self.entries {
            m.add_to(i, j, w);
        }
        m
    }

    /// Convert to Ising form `(h, J, constant)` with spins in `{-1, +1}`.
    ///
    /// Uses `x_i = (1 + s_i) / 2`, the convention every quantum backend here
    /// expects.
    pub fn to_ising(&self) -> (Vec<f64>, Vec<(usize, usize, f64)>, f64) {
        let mut h = vec![0.0; self.n];
        let mut j_terms: Vec<(usize, usize, f64)> = Vec::new();
        let mut constant = self.offset;

        for &(i, j, w) in &self.entries {
            if i == j {
                h[i] += w / 2.0;
                constant += w / 2.0;
            } else {
                j_terms.push((i, j, w / 4.0));
                h[i] += w / 4.0;
                h[j] += w / 4.0;
                constant += w / 4.0;
            }
        }
        (h, j_terms, constant)
    }
}

/// Annealing schedule and effort.
#[derive(Clone, Debug)]
pub struct AnnealSettings {
    pub sweeps: usize,
    pub restarts: usize,
    pub start_temperature: f64,
    pub end_temperature: f64,
}

impl Default for AnnealSettings {
    fn default() -> Self {
        Self {
            sweeps: 2_000,
            restarts: 8,
            start_temperature: 4.0,
            end_temperature: 0.01,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnnealResult {
    pub assignment: Vec<u8>,
    pub energy: f64,
    pub sweeps_run: usize,
    pub restarts_run: usize,
}

/// Anneal a QUBO to a low-energy assignment.
pub fn anneal<R: Rng>(qubo: &Qubo, settings: &AnnealSettings, rng: &mut R) -> AnnealResult {
    let n = qubo.n;
    if n == 0 {
        return AnnealResult {
            assignment: Vec::new(),
            energy: qubo.offset,
            sweeps_run: 0,
            restarts_run: 0,
        };
    }

    let mut best = vec![0u8; n];
    let mut best_energy = qubo.evaluate(&best);

    for _restart in 0..settings.restarts.max(1) {
        let mut x: Vec<u8> = (0..n).map(|_| u8::from(rng.bernoulli(0.5))).collect();
        let mut energy = qubo.evaluate(&x);

        for sweep in 0..settings.sweeps {
            let progress = sweep as f64 / settings.sweeps.max(1) as f64;
            // Geometric cooling: fast early exploration, fine late refinement.
            let temperature = settings.start_temperature
                * (settings.end_temperature / settings.start_temperature).powf(progress);
            for _ in 0..n {
                let k = rng.below(n as u64) as usize;
                let delta = qubo.delta(&x, k);
                let accept = delta <= 0.0
                    || (temperature > 0.0 && rng.next_f64() < (-delta / temperature).exp());
                if accept {
                    x[k] ^= 1;
                    energy += delta;
                }
            }
            if energy < best_energy {
                best_energy = energy;
                best.copy_from_slice(&x);
            }
        }
    }

    AnnealResult {
        assignment: best,
        energy: best_energy,
        sweeps_run: settings.sweeps,
        restarts_run: settings.restarts.max(1),
    }
}

/// Exact solve by enumeration. Only for `n <= 24`; used to certify that the
/// heuristics and the quantum backends find true optima on small instances.
pub fn solve_exact(qubo: &Qubo) -> Option<AnnealResult> {
    if qubo.n > 24 {
        return None;
    }
    let n = qubo.n;
    let mut best = vec![0u8; n];
    let mut best_energy = f64::INFINITY;
    for mask in 0u32..(1u32 << n) {
        let x: Vec<u8> = (0..n).map(|i| ((mask >> i) & 1) as u8).collect();
        let energy = qubo.evaluate(&x);
        if energy < best_energy {
            best_energy = energy;
            best = x;
        }
    }
    Some(AnnealResult {
        assignment: best,
        energy: best_energy,
        sweeps_run: 0,
        restarts_run: 1,
    })
}
