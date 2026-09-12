//! Frequency estimation in bounded memory, with the error it makes declared
//! up front (§22.2, §56.3 rule 30).
//!
//! §22.2's table asks for a count-min sketch for "frequency and cardinality:
//! kilobytes, bounded error", and §56.3's rule 30 that "every streaming
//! estimator declares its error bound". Until this module nothing in the
//! workspace declared one, and §22.4's mitigation for "sketch or reservoir
//! error affects a model" — "bounds are declared and monitored" — had
//! nothing to bound. This is the one sketch, and the bound is the first thing
//! it is built from.
//!
//! # The guarantee, stated exactly
//!
//! A [`CountMinSketch`] built from an [`ErrorBound`] `(ε, δ)` holds `d = ⌈ln
//! 1/δ⌉` rows of `w = ⌈e/ε⌉` counters. For any key, after `N` total
//! increments, the estimate `ĉ` and the true count `c` satisfy
//!
//! ```text
//! c <= ĉ                      always, and
//! ĉ <= c + ε·N                with probability at least 1 - δ.
//! ```
//!
//! The first line is structural: every row's counter for a key is only ever
//! incremented by that key or by keys that collide with it, so the minimum
//! over rows never falls below the truth. The second is Cormode and
//! Muthukrishnan's bound, and it assumes the `d` hash functions are drawn
//! from a pairwise-independent family. The family here — a 64-bit FNV-1a
//! digest of the key, then multiply-shift with a distinct odd multiplier per
//! row — is the standard practical approximation to that assumption and not
//! a proof of it, which is why the bound is *tested* on a skewed stream as
//! well as stated, and why a consumer is given [`ErrorBound::tolerable_for`]
//! to refuse a sketch whose declared error is too wide for the decision it is
//! feeding rather than trusting the arithmetic alone.
//!
//! # What it is for here
//!
//! Memory that does not grow with the number of keys. A research campaign
//! counting how many bars each subject contributed across its fetched
//! extents holds one of these — `w·d` counters, a few kilobytes — whatever
//! the universe grows to, and attaches the bound to the campaign manifest
//! beside the statistic so a reader knows how far the number may be off. The
//! consumer that fits on the count refuses when `ε·N` exceeds the tolerance
//! it can afford. That refusal is the mitigation the blueprint's table names.
//!
//! Deterministic, like everything in this crate: the same stream produces
//! the same counters on every machine, so a manifest's statistic can be
//! recomputed from the log and compared.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// The `(ε, δ)` a sketch is built from and answers to.
///
/// `ε` is the overestimate, as a fraction of everything counted; `δ` is the
/// probability of exceeding it. Both are open-interval fractions, refused at
/// zero (a sketch with no error is not a sketch, it is a hash map) and at one
/// (a bound that permits any answer bounds nothing).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBound {
    epsilon: f64,
    delta: f64,
}

impl ErrorBound {
    pub fn new(epsilon: f64, delta: f64) -> Result<Self> {
        for (name, value) in [("epsilon", epsilon), ("delta", delta)] {
            if !value.is_finite() || value <= 0.0 || value >= 1.0 {
                return Err(Error::invalid(format!(
                    "a sketch's {name} must lie strictly between 0 and 1, not {value}: zero \
                     would declare an estimator with no error, which is not an estimator, and \
                     one would declare a bound that admits any answer"
                )));
            }
        }
        Ok(Self { epsilon, delta })
    }

    pub const fn epsilon(&self) -> f64 {
        self.epsilon
    }

    pub const fn delta(&self) -> f64 {
        self.delta
    }

    /// Counters per row: `⌈e/ε⌉`.
    pub fn width(&self) -> usize {
        (std::f64::consts::E / self.epsilon).ceil() as usize
    }

    /// Rows: `⌈ln(1/δ)⌉`, and never fewer than one.
    pub fn depth(&self) -> usize {
        ((1.0 / self.delta).ln().ceil() as usize).max(1)
    }

    /// The most an estimate may exceed the truth by, after `total`
    /// increments, with probability at least `1 - δ`.
    pub fn absolute_error(&self, total: u64) -> f64 {
        self.epsilon * total as f64
    }

    /// Whether a consumer that can tolerate an overestimate of at most
    /// `tolerance` may rely on this sketch after `total` increments.
    ///
    /// The consumer's refusal, in one place: a fit that needs to tell 256
    /// observations from 240 cannot be fed by a sketch whose declared error
    /// at this volume is 50, and saying so is the difference between an
    /// estimator with a bound and one with a number.
    pub fn tolerable_for(&self, total: u64, tolerance: f64) -> bool {
        tolerance.is_finite() && tolerance >= 0.0 && self.absolute_error(total) <= tolerance
    }

    /// One line for a manifest.
    pub fn describe(&self) -> String {
        format!(
            "count-min (epsilon {}, delta {}): estimates never undercount and overcount by at \
             most epsilon x total with probability at least {}",
            self.epsilon,
            self.delta,
            1.0 - self.delta
        )
    }
}

/// See the module doc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CountMinSketch {
    bound: ErrorBound,
    width: usize,
    depth: usize,
    /// Row-major, `depth * width` counters.
    counts: Vec<u64>,
    /// One odd multiplier per row, derived deterministically from the row
    /// index so two sketches with the same bound hash identically.
    multipliers: Vec<u64>,
    total: u64,
}

impl CountMinSketch {
    pub fn new(bound: ErrorBound) -> Self {
        let width = bound.width();
        let depth = bound.depth();
        let multipliers = (0..depth as u64)
            .map(|row| {
                splitmix64(0x9E37_79B9_7F4A_7C15 ^ row.wrapping_mul(0xD1B5_4A32_D192_ED03)) | 1
            })
            .collect();
        Self {
            bound,
            width,
            depth,
            counts: vec![0; width * depth],
            multipliers,
            total: 0,
        }
    }

    pub const fn bound(&self) -> ErrorBound {
        self.bound
    }

    /// Counters held — the memory the sketch costs, independent of how many
    /// keys it has seen.
    pub fn cells(&self) -> usize {
        self.counts.len()
    }

    /// Everything counted so far, which is what the bound is a fraction of.
    pub const fn total(&self) -> u64 {
        self.total
    }

    pub fn increment(&mut self, key: &str) {
        self.add(key, 1);
    }

    pub fn add(&mut self, key: &str, by: u64) {
        let digest = fnv1a(key.as_bytes());
        for row in 0..self.depth {
            let index = self.index(row, digest);
            self.counts[index] = self.counts[index].saturating_add(by);
        }
        self.total = self.total.saturating_add(by);
    }

    /// The estimate for `key`: never below its true count, and above it by
    /// at most `ε·total` with probability `1 - δ`.
    pub fn estimate(&self, key: &str) -> u64 {
        let digest = fnv1a(key.as_bytes());
        (0..self.depth)
            .map(|row| self.counts[self.index(row, digest)])
            .min()
            .unwrap_or(0)
    }

    /// The most `estimate` may currently exceed any key's true count by.
    pub fn absolute_error(&self) -> f64 {
        self.bound.absolute_error(self.total)
    }

    fn index(&self, row: usize, digest: u64) -> usize {
        // Multiply-shift: the high bits of an odd-multiplier product are the
        // well-mixed ones, so the row's slot is taken from the top of the
        // 64-bit product and reduced into the width.
        let mixed = digest.wrapping_mul(self.multipliers[row]);
        row * self.width + ((mixed >> 32) as usize % self.width)
    }
}

/// FNV-1a over the key bytes: a fast, well-distributed 64-bit digest that
/// every row then mixes with its own multiplier.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// The splitmix64 finaliser, for turning a row index into a multiplier that
/// shares no low-bit structure with its neighbours.
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the bound it is exercising still has
// to assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// The declared bound holds on a heavily skewed stream: no key is ever
    /// undercounted, and no key is overcounted by more than `ε·N`. Skewed
    /// rather than uniform because a Zipf-shaped stream — a few subjects
    /// with most of the bars, a long tail with few — is what a research
    /// campaign actually counts, and it is the shape on which collisions with
    /// a heavy key do the most damage to a light one.
    ///
    /// Mutated by taking the `max` over rows instead of the `min` in
    /// `estimate` — confirmed the overcount assertion then fails on the tail
    /// keys, then restored. Also mutated by halving `width()` — confirmed
    /// the bound is then exceeded, then restored.
    #[test]
    fn an_estimate_never_undercounts_and_stays_within_the_declared_bound_on_a_skewed_stream()
    -> Result<()> {
        let bound = ErrorBound::new(0.01, 0.01)?;
        let mut sketch = CountMinSketch::new(bound);
        // Premise: the sketch is genuinely small — a few hundred counters per
        // row, a handful of rows — so the bound is doing work.
        assert_eq!(bound.width(), 272);
        assert_eq!(bound.depth(), 5);
        assert_eq!(sketch.cells(), 272 * 5);

        let keys = 600usize;
        let mut truth = std::collections::BTreeMap::new();
        for rank in 0..keys {
            let count = (20_000 / (rank + 1)) as u64 + 1;
            let key = format!("subject-{rank:04}");
            sketch.add(&key, count);
            truth.insert(key, count);
        }
        let total: u64 = truth.values().sum();
        assert_eq!(
            sketch.total(),
            total,
            "the sketch must know what it counted"
        );
        assert!(
            keys > sketch.bound().width(),
            "premise: more keys than counters per row, so collisions must occur"
        );

        let allowed = bound.absolute_error(total);
        let mut worst = 0.0_f64;
        for (key, count) in &truth {
            let estimate = sketch.estimate(key);
            assert!(
                estimate >= *count,
                "{key}: estimated {estimate} below the true count {count}; a count-min sketch \
                 can never undercount"
            );
            worst = worst.max((estimate - count) as f64);
        }
        assert!(
            worst <= allowed,
            "the worst overcount {worst} exceeds the declared bound {allowed} (epsilon x total)"
        );
        // And the estimate for a key never seen is bounded the same way.
        assert!(sketch.estimate("never-counted") as f64 <= allowed);
        Ok(())
    }

    /// A bound outside the open unit interval is refused on either axis.
    #[test]
    fn a_bound_outside_the_unit_interval_is_refused() {
        for (epsilon, delta) in [
            (0.0, 0.1),
            (1.0, 0.1),
            (0.1, 0.0),
            (0.1, 1.0),
            (f64::NAN, 0.1),
        ] {
            let error = ErrorBound::new(epsilon, delta)
                .expect_err(&format!("({epsilon}, {delta}) was accepted as a bound"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
        }
    }

    /// The consumer's refusal: the same sketch is usable for a coarse
    /// decision and refused for a fine one, and the line between them is the
    /// declared bound at the volume actually counted — not a guess.
    ///
    /// Mutated by making `tolerable_for` return `true` unconditionally —
    /// confirmed the refused half then fails, then restored.
    #[test]
    fn a_consumer_refuses_a_sketch_whose_declared_error_exceeds_its_tolerance() -> Result<()> {
        let bound = ErrorBound::new(0.01, 0.05)?;
        // After 10,000 increments the declared error is 100.
        assert!(
            (bound.absolute_error(10_000) - 100.0).abs() < 1e-9,
            "epsilon x total is {}",
            bound.absolute_error(10_000)
        );
        assert!(
            bound.tolerable_for(10_000, 250.0),
            "a consumer that can absorb an overcount of 250 may rely on this sketch"
        );
        assert!(
            !bound.tolerable_for(10_000, 16.0),
            "a consumer that must tell 256 from 240 cannot rely on a sketch off by up to 100"
        );
        assert!(!bound.tolerable_for(10_000, f64::NAN));
        assert!(!bound.tolerable_for(10_000, -1.0));
        Ok(())
    }
}
