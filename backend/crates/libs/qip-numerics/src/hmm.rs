//! Two-state Gaussian hidden Markov model.
//!
//! Fitted by Baum-Welch, decoded by Viterbi, and filtered forward for online
//! use. The platform uses it to infer volatility regimes from returns: the
//! observable is the return, the hidden state is the regime, and the model
//! recovers both the regime sequence and the transition probabilities.
//!
//! Two states rather than more, deliberately. A three- or four-state fit on a
//! few hundred observations will find states, and they will be noise; the
//! parameters of a two-state model are estimable from the data a strategy
//! actually has.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A fitted two-state Gaussian HMM.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GaussianHmm {
    /// Probability of starting in each state.
    pub initial: [f64; 2],
    /// `transition[i][j]` is the probability of moving from state i to j.
    pub transition: [[f64; 2]; 2],
    pub means: [f64; 2],
    pub variances: [f64; 2],
    /// Log likelihood of the training data under the fitted model.
    pub log_likelihood: f64,
    pub iterations: usize,
    pub converged: bool,
}

impl GaussianHmm {
    /// The state with the higher variance, which is the stressed regime.
    pub fn high_volatility_state(&self) -> usize {
        usize::from(self.variances[1] > self.variances[0])
    }

    /// Expected number of periods spent in a state before leaving it.
    ///
    /// The most interpretable output of the fit: a regime with an expected
    /// duration of two days is not a regime, it is noise the model has latched
    /// onto, and a strategy conditioned on it will trade constantly.
    pub fn expected_duration(&self, state: usize) -> f64 {
        let stay = self.transition[state][state].clamp(0.0, 0.999_999);
        1.0 / (1.0 - stay)
    }

    /// Whether both states persist long enough to be economically meaningful.
    pub fn states_are_persistent(&self, minimum_periods: f64) -> bool {
        (0..2).all(|state| self.expected_duration(state) >= minimum_periods)
    }

    /// Gaussian density of `x` under `state`.
    fn emission(&self, x: f64, state: usize) -> f64 {
        let variance = self.variances[state].max(1e-12);
        let z = x - self.means[state];
        (-0.5 * z * z / variance).exp() / (2.0 * std::f64::consts::PI * variance).sqrt()
    }

    /// Forward filtering: the probability of each state at each time, given
    /// observations up to that time.
    ///
    /// This is the online form — it uses no future data, which is what makes it
    /// usable for a decision made at that moment.
    pub fn filter(&self, observations: &[f64]) -> Vec<[f64; 2]> {
        let mut out = Vec::with_capacity(observations.len());
        if observations.is_empty() {
            return out;
        }
        let mut alpha = [
            self.initial[0] * self.emission(observations[0], 0),
            self.initial[1] * self.emission(observations[0], 1),
        ];
        normalise(&mut alpha);
        out.push(alpha);

        for observation in &observations[1..] {
            let mut next = [0.0f64; 2];
            for (j, slot) in next.iter_mut().enumerate() {
                let predicted: f64 = (0..2).map(|i| alpha[i] * self.transition[i][j]).sum();
                *slot = predicted * self.emission(*observation, j);
            }
            normalise(&mut next);
            alpha = next;
            out.push(alpha);
        }
        out
    }

    /// Most likely state sequence, by Viterbi decoding.
    ///
    /// Uses the whole series including the future, so it is for analysis and
    /// labelling history — never for a decision at a past date.
    pub fn decode(&self, observations: &[f64]) -> Vec<usize> {
        let n = observations.len();
        if n == 0 {
            return Vec::new();
        }
        // Work in logs: probabilities over a long series underflow otherwise.
        let mut delta = [
            self.initial[0].max(1e-300).ln() + self.emission(observations[0], 0).max(1e-300).ln(),
            self.initial[1].max(1e-300).ln() + self.emission(observations[0], 1).max(1e-300).ln(),
        ];
        let mut backpointers = vec![[0usize; 2]; n];

        for t in 1..n {
            let mut next = [f64::NEG_INFINITY; 2];
            for j in 0..2 {
                let mut best = f64::NEG_INFINITY;
                let mut best_state = 0usize;
                for (i, previous) in delta.iter().enumerate() {
                    let score = previous + self.transition[i][j].max(1e-300).ln();
                    if score > best {
                        best = score;
                        best_state = i;
                    }
                }
                next[j] = best + self.emission(observations[t], j).max(1e-300).ln();
                backpointers[t][j] = best_state;
            }
            delta = next;
        }

        let mut path = vec![0usize; n];
        path[n - 1] = usize::from(delta[1] > delta[0]);
        for t in (1..n).rev() {
            path[t - 1] = backpointers[t][path[t]];
        }
        path
    }

    /// Fit by Baum-Welch expectation maximisation.
    ///
    /// Initialised by splitting the data at its median absolute deviation, so
    /// the two states start out separated rather than identical — from
    /// identical starting parameters the algorithm has no gradient to follow.
    pub fn fit(observations: &[f64], max_iterations: usize, tolerance: f64) -> Result<Self> {
        let n = observations.len();
        if n < 20 {
            return Err(Error::invalid(
                "a two-state hidden Markov model needs at least twenty observations",
            ));
        }
        if !observations.iter().all(|x| x.is_finite()) {
            return Err(Error::numeric("observations contain non-finite values"));
        }

        let median = crate::stats::median(observations);
        let spread = crate::stats::median_absolute_deviation(observations).max(1e-9);
        let low: Vec<f64> = observations
            .iter()
            .copied()
            .filter(|x| (x - median).abs() <= spread)
            .collect();
        let high: Vec<f64> = observations
            .iter()
            .copied()
            .filter(|x| (x - median).abs() > spread)
            .collect();

        let mut model = Self {
            initial: [0.5, 0.5],
            transition: [[0.95, 0.05], [0.10, 0.90]],
            means: [
                if low.is_empty() {
                    median
                } else {
                    crate::stats::mean(&low)
                },
                if high.is_empty() {
                    median
                } else {
                    crate::stats::mean(&high)
                },
            ],
            variances: [
                crate::stats::variance(if low.len() > 2 { &low } else { observations }).max(1e-10),
                crate::stats::variance(if high.len() > 2 { &high } else { observations }).max(1e-9)
                    * 2.0,
            ],
            log_likelihood: f64::NEG_INFINITY,
            iterations: 0,
            converged: false,
        };

        let mut previous = f64::NEG_INFINITY;
        for iteration in 0..max_iterations {
            let (alpha, scales) = model.forward(observations);
            let beta = model.backward(observations, &scales);

            let log_likelihood: f64 = scales.iter().map(|s| s.max(1e-300).ln()).sum();
            model.log_likelihood = log_likelihood;
            model.iterations = iteration + 1;

            // Posterior state probabilities.
            let mut gamma = vec![[0.0f64; 2]; n];
            for t in 0..n {
                let mut row = [alpha[t][0] * beta[t][0], alpha[t][1] * beta[t][1]];
                normalise(&mut row);
                gamma[t] = row;
            }

            // Posterior transition probabilities.
            let mut xi_sum = [[0.0f64; 2]; 2];
            for t in 0..n - 1 {
                let mut xi = [[0.0f64; 2]; 2];
                let mut total = 0.0;
                for i in 0..2 {
                    for j in 0..2 {
                        let value = alpha[t][i]
                            * model.transition[i][j]
                            * model.emission(observations[t + 1], j)
                            * beta[t + 1][j];
                        xi[i][j] = value;
                        total += value;
                    }
                }
                if total > 1e-300 {
                    for i in 0..2 {
                        for j in 0..2 {
                            xi_sum[i][j] += xi[i][j] / total;
                        }
                    }
                }
            }

            // Maximisation.
            model.initial = gamma[0];
            for i in 0..2 {
                let denominator: f64 = (0..n - 1).map(|t| gamma[t][i]).sum();
                if denominator > 1e-12 {
                    for (j, probability) in model.transition[i].iter_mut().enumerate() {
                        *probability = (xi_sum[i][j] / denominator).clamp(1e-6, 1.0 - 1e-6);
                    }
                    normalise(&mut model.transition[i]);
                }

                let weight: f64 = (0..n).map(|t| gamma[t][i]).sum();
                if weight > 1e-12 {
                    let mean: f64 =
                        (0..n).map(|t| gamma[t][i] * observations[t]).sum::<f64>() / weight;
                    let variance: f64 = (0..n)
                        .map(|t| gamma[t][i] * (observations[t] - mean).powi(2))
                        .sum::<f64>()
                        / weight;
                    model.means[i] = mean;
                    // A variance floor stops a state from collapsing onto a
                    // single point, which sends the likelihood to infinity and
                    // the fit to nonsense.
                    model.variances[i] = variance.max(1e-10);
                }
            }

            if (log_likelihood - previous).abs() < tolerance {
                model.converged = true;
                break;
            }
            previous = log_likelihood;
        }

        // Order the states so index 1 is always the higher-variance regime,
        // which makes downstream interpretation stable across fits.
        if model.variances[0] > model.variances[1] {
            model.means.swap(0, 1);
            model.variances.swap(0, 1);
            model.initial.swap(0, 1);
            let t = model.transition;
            model.transition = [[t[1][1], t[1][0]], [t[0][1], t[0][0]]];
        }

        Ok(model)
    }

    /// Scaled forward pass. Returns the alphas and the scaling factors.
    fn forward(&self, observations: &[f64]) -> (Vec<[f64; 2]>, Vec<f64>) {
        let n = observations.len();
        let mut alpha = vec![[0.0f64; 2]; n];
        let mut scales = vec![1.0f64; n];

        for (i, weight) in alpha[0].iter_mut().enumerate() {
            *weight = self.initial[i] * self.emission(observations[0], i);
        }
        scales[0] = alpha[0].iter().sum::<f64>().max(1e-300);
        for weight in alpha[0].iter_mut() {
            *weight /= scales[0];
        }

        for t in 1..n {
            for j in 0..2 {
                let predicted: f64 = (0..2)
                    .map(|i| alpha[t - 1][i] * self.transition[i][j])
                    .sum();
                alpha[t][j] = predicted * self.emission(observations[t], j);
            }
            scales[t] = alpha[t].iter().sum::<f64>().max(1e-300);
            let scale = scales[t];
            for weight in alpha[t].iter_mut() {
                *weight /= scale;
            }
        }
        (alpha, scales)
    }

    /// Scaled backward pass, using the forward scaling factors.
    fn backward(&self, observations: &[f64], scales: &[f64]) -> Vec<[f64; 2]> {
        let n = observations.len();
        let mut beta = vec![[0.0f64; 2]; n];
        beta[n - 1] = [1.0 / scales[n - 1], 1.0 / scales[n - 1]];

        for t in (0..n - 1).rev() {
            for i in 0..2 {
                let value: f64 = (0..2)
                    .map(|j| {
                        self.transition[i][j]
                            * self.emission(observations[t + 1], j)
                            * beta[t + 1][j]
                    })
                    .sum();
                beta[t][i] = value / scales[t];
            }
        }
        beta
    }
}

fn normalise(values: &mut [f64; 2]) {
    let total = values[0] + values[1];
    if total > 1e-300 {
        values[0] /= total;
        values[1] /= total;
    } else {
        values[0] = 0.5;
        values[1] = 0.5;
    }
}
