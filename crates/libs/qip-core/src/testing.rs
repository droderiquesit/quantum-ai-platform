//! A small deterministic property-testing harness.
//!
//! The platform takes no third-party test dependencies, so randomised property
//! checks are driven from the same seeded [`Xoshiro256`] the rest of the system
//! uses. A failure reports the exact seed and case index, which reproduces it
//! on demand — the practical part of shrinking, without the machinery.

use crate::decimal::Decimal;
use crate::rng::{Rng, Xoshiro256};
use crate::time::Timestamp;

/// Runs a predicate over generated cases and panics with a reproducible seed.
#[derive(Debug, Clone)]
pub struct Property {
    name: String,
    seed: u64,
    cases: usize,
}

impl Property {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            seed: 0x5EED_1234_ABCD_0001,
            cases: 512,
        }
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn cases(mut self, cases: usize) -> Self {
        self.cases = cases;
        self
    }

    /// Generate `cases` values with `gen` and assert `check` holds for each.
    ///
    /// `check` returns `Err(reason)` to fail, so the message can describe what
    /// the offending value actually did.
    pub fn for_all<T, G, C>(&self, mut generate: G, mut check: C)
    where
        T: std::fmt::Debug,
        G: FnMut(&mut Xoshiro256) -> T,
        C: FnMut(&T) -> Result<(), String>,
    {
        let mut rng = Xoshiro256::seeded(self.seed);
        for case in 0..self.cases {
            let value = generate(&mut rng);
            if let Err(reason) = check(&value) {
                panic!(
                    "property `{}` failed on case {case} (seed {:#x}): {reason}\n  value: {value:?}",
                    self.name, self.seed
                );
            }
        }
    }
}

/// Decimal in `[-10^6, 10^6]` with up to nine fractional digits.
pub fn any_decimal(rng: &mut Xoshiro256) -> Decimal {
    let units = rng.below(2_000_001) as i128 - 1_000_000;
    let frac = rng.below(1_000_000_000) as i128;
    let sign = if units < 0 { -1 } else { 1 };
    Decimal::from_raw(units * crate::decimal::SCALE + sign * frac)
}

/// Decimal in `(0, 10^6]`, useful as a divisor or a price.
pub fn any_positive_decimal(rng: &mut Xoshiro256) -> Decimal {
    let units = rng.below(1_000_000) as i128;
    let frac = rng.below(999_999_999) as i128 + 1;
    Decimal::from_raw(units * crate::decimal::SCALE + frac)
}

/// A small decimal, where rounding behaviour is most visible.
pub fn any_small_decimal(rng: &mut Xoshiro256) -> Decimal {
    Decimal::from_raw(rng.below(2_000_000) as i128 - 1_000_000)
}

/// Timestamp inside 1970-2100.
pub fn any_timestamp(rng: &mut Xoshiro256) -> Timestamp {
    Timestamp::from_nanos(rng.below(4_102_444_800_000_000_000) as i64)
}

/// Finite `f64` in `[-1e6, 1e6]`.
pub fn any_f64(rng: &mut Xoshiro256) -> f64 {
    rng.uniform(-1e6, 1e6)
}

/// A vector of `n` finite returns, roughly daily-equity scaled.
pub fn any_returns(rng: &mut Xoshiro256, n: usize) -> Vec<f64> {
    (0..n).map(|_| rng.normal_with(0.0004, 0.012)).collect()
}

/// Assert two floats agree within an absolute-or-relative tolerance.
pub fn approx_eq(a: f64, b: f64, tol: f64) -> bool {
    if a.is_nan() || b.is_nan() {
        return false;
    }
    let diff = (a - b).abs();
    diff <= tol || diff <= tol * a.abs().max(b.abs())
}

/// [`approx_eq`] as a `Result`, for use inside [`Property::for_all`].
pub fn check_approx(label: &str, a: f64, b: f64, tol: f64) -> Result<(), String> {
    if approx_eq(a, b, tol) {
        Ok(())
    } else {
        Err(format!("{label}: {a} != {b} (tol {tol})"))
    }
}
