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

/// The most counters a sketch may hold: 65,536 `u64`s, half a megabyte.
///
/// The module doc promises kilobytes and the promise has to be a number
/// somewhere, because `w·d` is `⌈e/ε⌉ · ⌈ln 1/δ⌉` and both factors grow
/// without limit as their fraction shrinks: `ε = 10⁻⁹` alone asks for 2.7
/// billion counters per row, and a bound small enough for its product to
/// overflow `usize` would, without this ceiling, either abort the process in
/// `vec!` or wrap to a sketch far smaller than the bound it claims. The
/// campaign's own bound (`ε = 0.001, δ = 0.01`) is 13,595 counters, so the
/// ceiling is roughly four times what the one production consumer asks for.
pub const MAX_COUNTERS: usize = 65_536;

/// The `(ε, δ)` a sketch is built from and answers to.
///
/// `ε` is the overestimate, as a fraction of everything counted; `δ` is the
/// probability of exceeding it. Both are open-interval fractions, refused at
/// zero (a sketch with no error is not a sketch, it is a hash map) and at one
/// (a bound that permits any answer bounds nothing), and the pair is refused
/// when the counters it implies exceed [`MAX_COUNTERS`].
///
/// [`ErrorBound::new`] is the only way in, on the wire as well as in code:
/// deserialisation goes through the same constructor, so a manifest edited
/// by hand to declare `epsilon: 0` is refused at the read rather than
/// producing a sketch that abides by no bound at all.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ErrorBoundWire")]
pub struct ErrorBound {
    epsilon: f64,
    delta: f64,
}

/// The wire shape, validated through [`ErrorBound::new`] on the way in.
#[derive(Deserialize)]
struct ErrorBoundWire {
    epsilon: f64,
    delta: f64,
}

impl TryFrom<ErrorBoundWire> for ErrorBound {
    type Error = Error;

    fn try_from(wire: ErrorBoundWire) -> Result<Self> {
        Self::new(wire.epsilon, wire.delta)
    }
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
        let bound = Self { epsilon, delta };
        // The product is checked rather than computed: a pair small enough
        // to overflow `usize` is refused here for the same reason as one
        // that merely exceeds the ceiling, and neither reaches `vec!`.
        match bound.width().checked_mul(bound.depth()) {
            Some(counters) if counters <= MAX_COUNTERS => Ok(bound),
            counters => Err(Error::invalid(format!(
                "a sketch with epsilon {epsilon} and delta {delta} needs {} counters — {} per \
                 row across {} rows — and this platform bounds a sketch at {MAX_COUNTERS}; a \
                 bound that tight is a hash map wearing a sketch's name, and it is refused \
                 rather than allocated",
                counters.map_or_else(|| "more than usize::MAX".to_string(), |n| n.to_string()),
                bound.width(),
                bound.depth()
            ))),
        }
    }

    pub const fn epsilon(&self) -> f64 {
        self.epsilon
    }

    pub const fn delta(&self) -> f64 {
        self.delta
    }

    /// Counters per row: `⌈e/ε⌉`. The cast saturates for an `ε` too small to
    /// represent, which is what lets [`Self::new`] refuse it by arithmetic.
    pub fn width(&self) -> usize {
        (std::f64::consts::E / self.epsilon).ceil() as usize
    }

    /// Rows: `⌈ln(1/δ)⌉`. At least one without a floor being written down:
    /// `δ` is strictly below one, so `1/δ` is strictly above one and its
    /// logarithm strictly positive, and the ceiling of a positive number is
    /// at least one. A `.max(1)` used to sit here and could never fire.
    pub fn depth(&self) -> usize {
        (1.0 / self.delta).ln().ceil() as usize
    }

    /// The counters a sketch built from this bound holds: `width × depth`,
    /// which [`Self::new`] has already checked against [`MAX_COUNTERS`].
    pub fn counters(&self) -> usize {
        self.width().saturating_mul(self.depth())
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
    /// A sketch under `bound`. Infallible on purpose: an [`ErrorBound`] can
    /// only be built through [`ErrorBound::new`], which has already refused
    /// a pair whose counters exceed [`MAX_COUNTERS`] or overflow, so the
    /// allocation below is bounded by the type and not by a check repeated
    /// here.
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
            counts: vec![0; bound.counters()],
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
    ///
    /// A note for whoever changes a hash constant. The `(ε, δ)` guarantee is
    /// probabilistic — it holds with probability `1 − δ`, here 99% — and
    /// this test is one draw from that distribution: the 600 keys, the
    /// FNV-1a seed, the splitmix multipliers and the row count together fix
    /// which keys collide with which. Changing any hash constant is a fresh
    /// draw, and a fresh draw can land in the 1% and fail the overcount
    /// assertion below without the sketch being wrong. If that happens,
    /// the honest responses are to widen the stream and re-check the
    /// failure rate against `δ`, or to change the constant back; the
    /// dishonest one is to loosen `allowed`.
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

    /// The memory bound, and the overflow behind it. A bound whose counters
    /// would exceed [`MAX_COUNTERS`] is refused by name; a bound so tight
    /// that `width × depth` overflows `usize` is refused the same way rather
    /// than wrapping to a small sketch or aborting in `vec!`; and the same
    /// refusal reaches the wire, so a serialised bound cannot smuggle in a
    /// pair the constructor would have refused.
    ///
    /// Mutated by replacing the `checked_mul` match with `Ok(bound)` —
    /// confirmed every refusal below then admits, and the wire half too,
    /// then restored.
    #[test]
    fn a_bound_whose_counters_exceed_the_ceiling_or_overflow_is_refused() -> Result<()> {
        // Premise: the ceiling admits the campaign's own bound with room.
        let campaign = ErrorBound::new(0.001, 0.01)?;
        assert_eq!(campaign.counters(), 2719 * 5);
        assert!(campaign.counters() < MAX_COUNTERS);

        // Past the ceiling by arithmetic alone: 2.7 billion counters per
        // row, one row.
        let too_tight =
            ErrorBound::new(1e-9, 0.5).expect_err("a billion-counter row was allocated");
        assert_eq!(too_tight.code(), "invalid", "got {too_tight:?}");
        assert!(
            too_tight.message().contains(&MAX_COUNTERS.to_string()),
            "the refusal does not name the ceiling: {too_tight}"
        );

        // The overflow edge: an epsilon so small the width saturates to
        // `usize::MAX`, and a delta small enough that the depth is above one,
        // so the product overflows rather than merely exceeding.
        let saturated = ErrorBound::new(f64::MIN_POSITIVE, 1e-6)
            .expect_err("an overflowing counter product was accepted");
        assert!(
            saturated.message().contains("more than usize::MAX"),
            "the refusal does not say the product overflowed: {saturated}"
        );

        // And the wire is gated the same way: the pair `new` refuses is
        // refused by `serde_json` too, because deserialisation goes through
        // `new`.
        let smuggled: std::result::Result<ErrorBound, _> =
            serde_json::from_str(r#"{"epsilon":1e-9,"delta":0.5}"#);
        assert!(
            smuggled.is_err(),
            "a bound the constructor refuses was accepted off the wire"
        );
        let round_trip: ErrorBound = serde_json::from_str(&serde_json::to_string(&campaign)?)?;
        assert_eq!(round_trip, campaign);
        Ok(())
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
