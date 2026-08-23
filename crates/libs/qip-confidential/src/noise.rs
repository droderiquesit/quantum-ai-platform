//! Calibrated noise, drawn from a seeded stream, reproducible for ever.
//!
//! The noise added to a release is the Laplace mechanism: a draw from
//! `Laplace(0, sensitivity / epsilon)`, where the sensitivity is the most one
//! cell's contribution can move the statistic. That ratio is the whole
//! calibration — a bigger sensitivity or a smaller epsilon means more noise —
//! and both halves of it come from things the caller declared before seeing the
//! data. Nothing here is sized from the data itself, because a scale computed
//! from the data is a statistic about the data, and the ones you would reach
//! for (the maximum, the range) are the most disclosive statistics in the set.
//!
//! # The stream is a function of the question
//!
//! `qip-core`'s first rule forbids ambient randomness, so the draw comes from a
//! [`Xoshiro256`] seeded from the gate's seed and the question's fingerprint.
//! That has a consequence worth stating on its own, because it is a defence
//! rather than a side effect: **the same question always gets the same
//! answer.** Asking one more time cannot average the noise away, because there
//! is no second draw to average with. What the budget is for is the *other*
//! attack — a thousand slightly different questions, each with an honest fresh
//! draw, averaged to recover what a thousand identical ones could not.
//!
//! Two details of the fingerprint follow from this and are enforced in
//! [`crate::query::Query::fingerprint`]:
//!
//! * the cohort label is **not** in it — if it were, relabelling the same
//!   question would draw fresh noise, and two answers averaged halve it;
//! * epsilon **is** in it — a single draw reused at two different scales gives
//!   `value + b₁·Z` and `value + b₂·Z`, which is two linear equations in two
//!   unknowns and hands over the value exactly. The price of separating them
//!   is that sweeping epsilon draws fresh noise each time, and only the budget
//!   stops that.
//!
//! # What the seed is
//!
//! Key material, operationally, with none of the machinery that phrase usually
//! implies. Anyone holding the seed can recompute the noise for any question
//! and subtract it, recovering the true statistic exactly.
//! [`noise_for`] is public so that this is a fact a reader can see and a test
//! can demonstrate, rather than a property hidden behind a private function
//! and therefore easy to forget. This crate does not manage, rotate or protect
//! the seed; it takes one.
//!
//! # Floating point
//!
//! The privacy guarantee of the Laplace mechanism is stated for real
//! arithmetic. Implemented in `f64` it is known to leak through the low bits:
//! the set of representable outputs depends on the true value, so a determined
//! attacker reading the last bits of a release can learn more than epsilon
//! says (Mironov, *On Significance of the Least Significant Bits in
//! Differential Privacy*, 2012). [`snap`] implements the rounding half of the
//! published mitigation and nothing else — see its documentation for exactly
//! how far that goes.

use crate::query::Fingerprint;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256;
use qip_core::rng::{Rng, Xoshiro256};
use serde::Serialize;

/// The most one cell's contribution can move a statistic.
///
/// Derived from the declared bounds and the contributor count — both public,
/// both fixed before the data is looked at. See
/// [`crate::query::Statistic::sensitivity`] for the neighbour model this is
/// computed under, which is the assumption everything downstream rests on.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct Sensitivity(f64);

impl Sensitivity {
    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() || value <= 0.0 {
            return Err(Error::numeric(format!(
                "sensitivity {value} is not a positive finite number; a statistic no cell can \
                 move is not a statistic worth noising, and one that can be moved infinitely \
                 cannot be noised at all"
            )));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

/// The `b` of `Laplace(0, b)`: sensitivity divided by epsilon.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct NoiseScale(f64);

impl NoiseScale {
    pub fn calibrate(sensitivity: Sensitivity, epsilon: crate::budget::Epsilon) -> Result<Self> {
        let scale = sensitivity.get() / epsilon.get();
        if !scale.is_finite() || scale <= 0.0 {
            return Err(Error::numeric(format!(
                "a noise scale of {scale} is not usable; sensitivity {} over epsilon {epsilon}",
                sensitivity.get()
            )));
        }
        Ok(Self(scale))
    }

    pub const fn get(self) -> f64 {
        self.0
    }

    /// The standard deviation of the noise, which is what a reader of a release
    /// actually wants when asking how much to trust the number: `b√2`.
    pub fn standard_deviation(self) -> f64 {
        self.0 * std::f64::consts::SQRT_2
    }
}

/// The noise this seed adds to this question at this scale.
///
/// Deterministic in all three arguments and in nothing else. Public on
/// purpose: the reproducibility the platform requires and the confidentiality
/// this crate provides are in tension exactly here, and the honest resolution
/// is to make the tension visible. Whoever holds the seed holds every release's
/// noise, and therefore every release's true value.
///
/// The scale is not mixed into the stream because it does not need to be: it is
/// a function of the bounds, the contributor count and epsilon, and all three
/// are in the fingerprint.
pub fn noise_for(seed: u64, fingerprint: &Fingerprint, scale: NoiseScale) -> f64 {
    // SHA-256 as a mixing function, not as a security primitive: it gives each
    // distinct question its own stream. It is used rather than the cheaper
    // `Xoshiro256::fork` because a collision between two questions' streams
    // would mean identical noise on both, and identical noise cancels in a
    // difference — which is precisely the recovery this crate refuses to allow.
    let mut material = Vec::with_capacity(64);
    material.extend_from_slice(b"qip-confidential/noise/v1");
    material.extend_from_slice(&seed.to_le_bytes());
    material.extend_from_slice(fingerprint.as_bytes());
    let digest = sha256(&material);

    let mut stream = [0u8; 8];
    stream.copy_from_slice(&digest[..8]);
    let mut rng = Xoshiro256::seeded(u64::from_le_bytes(stream));

    // A Laplace deviate is the difference of two unit exponentials. Built from
    // `Rng::exponential` rather than from an inverse CDF over a uniform draw so
    // that the `ln(0)` at the edge of the interval — an infinite noise draw, on
    // one seed in 2⁵³ — is not reachable: `exponential` draws from (0, 1].
    scale.get() * (rng.exponential(1.0) - rng.exponential(1.0))
}

/// How far below the noise scale the released value is rounded: 2⁻²⁰ of it.
const GRANULARITY_SHIFT: i32 = 20;

/// Round a released value onto a coarse grid.
///
/// This is the rounding half of the snapping mechanism, and it is here for one
/// reason: a naive floating-point Laplace draw leaves structure in the low bits
/// of the released value that depends on the true value, so an attacker reading
/// the full `f64` can learn more than epsilon accounts for. Mapping many
/// representable outputs onto one grid point destroys that structure.
///
/// **Exactly how far this goes.** The grid is a power of two at 2⁻²⁰ of the
/// noise scale, so rounding it costs about a millionth of a standard deviation
/// — statistically free. The published mitigation also clamps the output and
/// derives the granularity from the clamping bound; neither is implemented
/// here. And when the released magnitude exceeds the noise scale by more than
/// about 2³², the float grid near the value is already coarser than this one
/// and the rounding does nothing at all. Treat the low-bit channel as
/// mitigated, not closed.
///
/// The granularity is derived from the noise scale, which is public — it is on
/// every [`crate::release::Release`] — so the grid itself leaks nothing. Public
/// for the same reason [`noise_for`] is: with both, a release can be
/// reproduced end to end from the seed and the inputs, which is what replay
/// requires and what the seed-holder can do anyway.
pub fn snap(value: f64, scale: NoiseScale) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let exponent = scale.get().log2().floor();
    if !exponent.is_finite() {
        return value;
    }
    let exponent = exponent.clamp(-1000.0, 1000.0) as i32 - GRANULARITY_SHIFT;
    let granularity = 2f64.powi(exponent);
    if !granularity.is_finite() || granularity <= 0.0 {
        return value;
    }
    let quotient = value / granularity;
    // Past 2⁵³ the grid is finer than the floats themselves; rounding onto it
    // is a no-op that would only lose precision to no purpose.
    if !quotient.is_finite() || quotient.abs() > 9_007_199_254_740_992.0 {
        return value;
    }
    quotient.round() * granularity
}
