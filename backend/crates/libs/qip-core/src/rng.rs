//! Deterministic pseudo-random number generation.
//!
//! Every stochastic component (synthetic markets, Monte Carlo, bootstrap, QAOA
//! sampling, agent tie-breaks) draws from a seeded [`Xoshiro256`]. The same seed
//! always produces the same run — a hard requirement for replay and for the
//! deterministic simulation tests.

/// Random source used across the platform.
pub trait Rng {
    fn next_u64(&mut self) -> u64;

    /// Uniform in `[0, 1)`, using the top 53 bits so every value is representable.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in `[lo, hi)`.
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }

    /// Uniform integer in `[0, n)`. Returns 0 when `n == 0`.
    ///
    /// Uses Lemire's multiply-shift with rejection, so the result is unbiased
    /// rather than skewed the way a plain modulo would be.
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u64();
            let m = u128::from(r) * u128::from(n);
            if (m as u64) >= threshold {
                return (m >> 64) as u64;
            }
        }
    }

    /// `true` with probability `p`.
    fn bernoulli(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Standard normal via the Box-Muller transform.
    fn normal(&mut self) -> f64 {
        // Guard against log(0); u1 is drawn from [0,1) so shift it into (0,1].
        let u1 = 1.0 - self.next_f64();
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    fn normal_with(&mut self, mean: f64, stddev: f64) -> f64 {
        mean + stddev * self.normal()
    }

    /// Exponential with the given rate.
    fn exponential(&mut self, rate: f64) -> f64 {
        if rate <= 0.0 {
            return f64::INFINITY;
        }
        -(1.0 - self.next_f64()).ln() / rate
    }

    /// Poisson count via Knuth's method. Suitable for the small means used for
    /// order-arrival and news-arrival intensities.
    fn poisson(&mut self, lambda: f64) -> u64 {
        if lambda <= 0.0 {
            return 0;
        }
        if lambda > 30.0 {
            // Normal approximation keeps the loop bounded for large intensities.
            return self.normal_with(lambda, lambda.sqrt()).max(0.0).round() as u64;
        }
        let l = (-lambda).exp();
        let mut k = 0u64;
        let mut p = 1.0;
        loop {
            p *= self.next_f64();
            if p <= l {
                return k;
            }
            k += 1;
        }
    }

    /// Student-t deviate with `nu` degrees of freedom, for fat-tailed shocks.
    fn student_t(&mut self, nu: f64) -> f64 {
        if nu <= 2.0 {
            return self.normal();
        }
        // Ratio of a normal to the root of a chi-square/nu, built from a
        // gamma(nu/2, 2) draw approximated by summing exponentials.
        let z = self.normal();
        let mut chi2 = 0.0;
        let k = nu.round().max(1.0) as u32;
        for _ in 0..k {
            let n = self.normal();
            chi2 += n * n;
        }
        z / (chi2 / f64::from(k)).sqrt()
    }

    /// Fisher-Yates shuffle in place.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below((i + 1) as u64) as usize;
            items.swap(i, j);
        }
    }

    /// Index sampled proportionally to `weights`. Returns `None` when empty or
    /// all weights are non-positive.
    fn weighted_index(&mut self, weights: &[f64]) -> Option<usize> {
        let total: f64 = weights.iter().filter(|w| w.is_finite() && **w > 0.0).sum();
        if total <= 0.0 {
            return None;
        }
        let mut target = self.next_f64() * total;
        for (i, w) in weights.iter().enumerate() {
            if !w.is_finite() || *w <= 0.0 {
                continue;
            }
            target -= w;
            if target <= 0.0 {
                return Some(i);
            }
        }
        weights.iter().rposition(|w| w.is_finite() && *w > 0.0)
    }
}

/// xoshiro256** — fast, well-distributed, and small enough to audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xoshiro256 {
    s: [u64; 4],
}

impl Xoshiro256 {
    /// Seed the generator, expanding a single `u64` with SplitMix64 so that
    /// even seeds 0, 1, 2 produce well-separated streams.
    pub fn seeded(seed: u64) -> Self {
        let mut sm = SplitMix64 { state: seed };
        Self {
            s: [sm.next(), sm.next(), sm.next(), sm.next()],
        }
    }

    /// Derive an independent stream from this one, for a sub-component.
    pub fn fork(&mut self, label: &str) -> Self {
        let mut mix = self.next_u64();
        for b in label.as_bytes() {
            mix = mix.rotate_left(7) ^ u64::from(*b).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        Self::seeded(mix)
    }
}

impl Rng for Xoshiro256 {
    fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }
}

/// SplitMix64, used only to expand seeds.
#[derive(Debug, Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
