//! Streaming estimators in bounded memory, each declaring the error it makes
//! (§21.1, §22.2, §56.3 rule 30).
//!
//! §22.2's table asks for four things this crate did not have: quantiles from
//! "a t-digest or KLL sketch, few KB per distribution, bounded error";
//! cardinality from a HyperLogLog, "kilobytes, bounded error"; and
//! "representative samples" from weighted reservoir sampling at a "fixed row
//! count". §21.1 states what they are all for — "maintained in the node in
//! constant memory", roughly 320 KB for 200 features, against terabytes a
//! year of raw capture — and states the condition the whole arrangement rests
//! on: "every estimator declares an error bound, and one that drifts past it
//! marks every model depending on it as degraded."
//!
//! That sentence is the reason each type here is built *from* a bound rather
//! than merely carrying one. [`Compression`], [`Precision`] and a reservoir's
//! capacity are each refused outside a range, each implies the memory the
//! estimator costs, and each answers `tolerable_for` so a consumer can refuse
//! a statistic whose declared error is too wide for the decision it would
//! feed. An error bound that cannot be exceeded is not a bound, and an
//! estimator whose memory is not refused past a ceiling is not a sketch.
//!
//! # What is deliberately not here
//!
//! **Frequency.** [`crate::sketch::CountMinSketch`] is the count-min sketch
//! and [`crate::sketch::ErrorBound`] is its `(ε, δ)`; neither is
//! reimplemented here, and the HyperLogLog below shares that module's
//! [`crate::sketch::fnv1a`] and [`crate::sketch::splitmix64`] rather than
//! carrying a second hash. Two sketches summarising one stream must agree
//! about what a key hashes to, and the way to guarantee that is to have one
//! implementation rather than two that happen to match today.
//!
//! **Moments.** [`crate::stats::RunningStats`] is Welford's accumulator, and
//! [`crate::stats::ewma`] the exponentially weighted update. Both predate
//! this module.
//!
//! # Determinism
//!
//! Like everything in this crate: the same stream in the same order produces
//! identical bits on every machine. The reservoir's randomness is a seeded
//! splitmix64 counter and never the operating system's, so a sample that fed
//! a fit can be reproduced from the log rather than merely described.

use crate::sketch::{fnv1a, splitmix64};
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// t-digest
// ---------------------------------------------------------------------------

/// The loosest compression a digest may be built at.
///
/// Below twenty the declared rank error at the median is worse than one in
/// six, which is wider than any consumer in this platform declares a
/// tolerance for. Refused rather than admitted with a warning: a digest
/// nobody may use is a digest nobody should build.
pub const TDIGEST_MIN_COMPRESSION: f64 = 20.0;

/// The tightest compression a digest may be built at, and so the memory
/// ceiling.
///
/// A digest allocates `2δ + 1` centroids and buffers as many again, each a
/// pair of `f64`s — at `δ = 1000` that is 4,002 pairs, 64 KB. The module doc
/// promises "few KB per distribution" and §21.1 budgets ~320 KB for two
/// hundred features together, so a compression that alone would spend a fifth
/// of that budget is the outer edge of what the promise can carry. Above it
/// the value is refused
/// rather than allocated — the same discipline
/// [`crate::sketch::MAX_COUNTERS`] applies to the count-min sketch, and for
/// the same reason: an unbounded `vec!` inside an estimator whose whole
/// purpose is bounded memory is a contradiction that aborts the process
/// rather than returning an error.
pub const TDIGEST_MAX_COMPRESSION: f64 = 1_000.0;

/// A t-digest's `δ`: how finely it resolves the distribution, and so how much
/// memory it costs and how far a quantile may be wrong.
///
/// Refused outside `[TDIGEST_MIN_COMPRESSION, TDIGEST_MAX_COMPRESSION]` and
/// refused if not finite. Deserialises through the same constructor, so a
/// manifest edited by hand to declare `compression: 0` is refused at the read
/// rather than producing a digest that abides by no bound — and, worse, one
/// whose centroid ceiling is `1` and whose every quantile is the stream mean.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CompressionWire")]
pub struct Compression {
    delta: f64,
}

/// The wire shape, validated through [`Compression::new`] on the way in.
#[derive(Deserialize)]
struct CompressionWire {
    delta: f64,
}

impl TryFrom<CompressionWire> for Compression {
    type Error = Error;

    fn try_from(wire: CompressionWire) -> Result<Self> {
        Self::new(wire.delta)
    }
}

impl Compression {
    pub fn new(delta: f64) -> Result<Self> {
        if !delta.is_finite()
            || !(TDIGEST_MIN_COMPRESSION..=TDIGEST_MAX_COMPRESSION).contains(&delta)
        {
            return Err(Error::invalid(format!(
                "a t-digest's compression must lie between {TDIGEST_MIN_COMPRESSION} and \
                 {TDIGEST_MAX_COMPRESSION}, not {delta}; below the floor no consumer here \
                 declares a tolerance the digest could meet, and above the ceiling one \
                 distribution alone would spend the whole per-node memory budget §21.1 sets \
                 for two hundred features"
            )));
        }
        Ok(Self { delta })
    }

    pub const fn value(&self) -> f64 {
        self.delta
    }

    /// The most centroids a digest at this compression may hold: `2δ + 1`.
    ///
    /// Three numbers, and the gaps between them are the point. The k1 scale
    /// function spans `δ/2`, so a single sweep over sorted points spends at
    /// least one unit of `k` per centroid and cannot produce more than `δ/2`
    /// of them. A *merging* digest does not do one sweep: each compression
    /// starts from centroids whose boundaries were fixed under a smaller
    /// total, so the count settles above the ideal — measured at 63 for
    /// `δ = 100` on a million heavy-tailed observations, between `δ/2` and
    /// `δ`. The ceiling is twice `δ` so that the guard in
    /// [`TDigest::compress`] stays a guard rather than becoming the working
    /// path, and `a_digest_s_memory_does_not_grow_with_its_stream` asserts
    /// the measured count stays inside half of it.
    ///
    /// An earlier draft of this module used the scale function `4q(1−q)/δ`,
    /// which bounds each centroid's *weight* and not their *number* — the
    /// integral of its reciprocal diverges — so the sweep ran past the
    /// ceiling on a hundred thousand points, the guard fired on every
    /// compression, and the digest reported the 10th percentile of the unit
    /// interval as 0.17. A bound on the size of each part is not a bound on
    /// how many parts there are.
    pub fn centroid_ceiling(&self) -> usize {
        // f64 → usize at the memory boundary: `delta` is finite and at most
        // 1,000 by construction, so the conversion is exact and small.
        (2.0 * self.delta).ceil() as usize + 1
    }

    /// The bytes a digest at this compression costs, centroids and buffer
    /// together — the number "few KB per distribution" has to be.
    pub fn bytes(&self) -> usize {
        2 * self.centroid_ceiling() * std::mem::size_of::<Centroid>()
    }

    /// The rank error a quantile estimate may carry at `q`: `2π√(q(1−q))/δ`.
    ///
    /// Rank rather than value error, and that is the whole shape of the
    /// guarantee: a t-digest does not promise that the value it returns for
    /// `q` is close to the true `q`-quantile in the units of the data — on a
    /// distribution with a flat region it cannot — it promises that the value
    /// it returns sits at a rank within this much of `q`. A consumer that
    /// needs a value tolerance must convert one to the other through its own
    /// knowledge of the distribution, and this function will not do it for
    /// them.
    ///
    /// The figure is the k1 scale function's own, not a number chosen to be
    /// comfortable. A centroid at rank `q` spans `Δq` where `k'(q)·Δq = 1`,
    /// and `k'(q) = δ / (π√(4q(1−q)))`, so one centroid covers
    /// `2π√(q(1−q))/δ` of the rank axis and an estimate interpolated inside
    /// it cannot be further out than that. Widest at the median (`π/δ`) and
    /// tightening toward both tails by the square root — the property the
    /// digest is chosen for, since the 99th percentile of a loss distribution
    /// is where a risk figure is read.
    ///
    /// This function returned `4q(1−q)/δ` in an earlier draft, which is the
    /// *other* scale function's shape and is between three and eight times
    /// tighter than the implementation can deliver. A bound an estimator
    /// cannot meet is worse than none: every consumer refusing on it refuses
    /// correct answers, and every consumer relying on it is relying on
    /// arithmetic nobody checked.
    ///
    /// Refuses a `q` outside `[0, 1]` rather than clamping it. A clamp here
    /// would hand back the error bound for a quantile the caller did not ask
    /// about, which is the most dangerous possible answer.
    pub fn rank_error(&self, q: f64) -> Result<f64> {
        if !q.is_finite() || !(0.0..=1.0).contains(&q) {
            return Err(Error::invalid(format!(
                "a quantile must lie in [0, 1], not {q}; the rank error is a function of q and \
                 a clamped q would state the error of a quantile nobody asked for"
            )));
        }
        Ok(2.0 * std::f64::consts::PI * (q * (1.0 - q)).sqrt() / self.delta)
    }

    /// Whether a consumer needing rank accuracy of `tolerance` at `q` may
    /// rely on a digest at this compression.
    ///
    /// The consumer's refusal, in one place — the counterpart of
    /// [`crate::sketch::ErrorBound::tolerable_for`]. `false` rather than an
    /// error for an unreadable `q` or tolerance, because "may I rely on it"
    /// has a safe answer and it is no.
    pub fn tolerable_for(&self, q: f64, tolerance: f64) -> bool {
        if !tolerance.is_finite() || tolerance < 0.0 {
            return false;
        }
        self.rank_error(q).is_ok_and(|error| error <= tolerance)
    }

    /// One line for a manifest.
    pub fn describe(&self) -> String {
        format!(
            "t-digest (compression {}): a quantile is returned at a rank within \
             2*pi*sqrt(q(1-q))/{} of the one asked for, at most {} at the median, in at most {} \
             bytes",
            self.delta,
            self.delta,
            std::f64::consts::PI / self.delta,
            self.bytes()
        )
    }
}

/// One cluster of the distribution: a weighted mean.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Centroid {
    pub mean: f64,
    pub weight: f64,
}

impl Centroid {
    fn absorb(&mut self, other: Self) {
        let weight = self.weight + other.weight;
        // Weighted mean rather than a running sum of products, so a long
        // stream of like-signed values does not drift the centroid by
        // accumulating a large intermediate.
        self.mean = (self.mean * self.weight + other.mean * other.weight) / weight;
        self.weight = weight;
    }
}

/// Quantiles of a stream in memory that does not grow with it.
///
/// See the module doc. The accuracy claim is [`Compression::rank_error`]'s and
/// is tested against a known distribution rather than only asserted.
///
/// Reading a quantile takes `&mut self` on purpose. A digest holds unmerged
/// points in a buffer between compressions, and a read that answered from the
/// centroids alone would answer from a different population than the next
/// read — two answers to one question, from the same object, with nothing in
/// the type saying which was which. Settling the buffer first means there is
/// one code path to the centroids and one population behind every answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TDigestWire")]
pub struct TDigest {
    compression: Compression,
    /// Sorted by mean, never longer than [`Compression::centroid_ceiling`].
    centroids: Vec<Centroid>,
    /// Points not yet merged, never longer than the same ceiling.
    buffer: Vec<Centroid>,
    total_weight: f64,
    count: u64,
    /// The extremes seen, so the tails interpolate to a value that actually
    /// occurred rather than to the outermost centroid's mean. `None` on an
    /// empty digest — not a sentinel, because `f64::INFINITY` here would be
    /// returned as a quantile by the interpolation below.
    minimum: Option<f64>,
    maximum: Option<f64>,
}

/// The wire shape, held to its own compression's geometry on the way in.
#[derive(Deserialize)]
struct TDigestWire {
    compression: Compression,
    centroids: Vec<Centroid>,
    buffer: Vec<Centroid>,
    total_weight: f64,
    count: u64,
    minimum: Option<f64>,
    maximum: Option<f64>,
}

impl TryFrom<TDigestWire> for TDigest {
    type Error = Error;

    fn try_from(wire: TDigestWire) -> Result<Self> {
        let ceiling = wire.compression.centroid_ceiling();
        for (name, held) in [
            ("centroid", wire.centroids.len()),
            ("buffer", wire.buffer.len()),
        ] {
            if held > ceiling {
                return Err(Error::invalid(format!(
                    "a serialised t-digest holds {held} {name}(s) and its own compression ({}) \
                     bounds it at {ceiling}; a digest larger than the memory its bound declares \
                     abides by no bound at all, and is refused rather than read",
                    wire.compression.value()
                )));
            }
        }
        let mut sum = 0.0;
        for centroid in wire.centroids.iter().chain(wire.buffer.iter()) {
            if !centroid.mean.is_finite() || !centroid.weight.is_finite() || centroid.weight <= 0.0
            {
                return Err(Error::invalid(format!(
                    "a serialised t-digest carries a centroid of mean {} and weight {}; a \
                     non-finite mean propagates into every quantile read from the digest and a \
                     weight at or below zero divides by zero in the interpolation, so it is \
                     refused",
                    centroid.mean, centroid.weight
                )));
            }
            sum += centroid.weight;
        }
        // The declared total is what every quantile is a fraction of. A total
        // that disagrees with the weights returns a quantile for a rank the
        // digest never held, which reads exactly like a correct answer.
        if (sum - wire.total_weight).abs() > 1e-6 * wire.total_weight.abs().max(1.0) {
            return Err(Error::invalid(format!(
                "a serialised t-digest declares a total weight of {} and its centroids carry \
                 {sum}; every quantile is a fraction of the declared total, so a digest whose \
                 weights disagree with it answers for a rank it never held",
                wire.total_weight
            )));
        }
        Ok(Self {
            compression: wire.compression,
            centroids: wire.centroids,
            buffer: wire.buffer,
            total_weight: wire.total_weight,
            count: wire.count,
            minimum: wire.minimum,
            maximum: wire.maximum,
        })
    }
}

impl TDigest {
    /// An empty digest at `compression`.
    ///
    /// Infallible: a [`Compression`] can only be built through its own
    /// constructor, which has already refused a value whose centroid ceiling
    /// would exceed the memory this module promises, so the two allocations
    /// below are bounded by the type rather than by a check repeated here.
    pub fn new(compression: Compression) -> Self {
        let ceiling = compression.centroid_ceiling();
        Self {
            compression,
            centroids: Vec::with_capacity(ceiling),
            buffer: Vec::with_capacity(ceiling),
            total_weight: 0.0,
            count: 0,
            minimum: None,
            maximum: None,
        }
    }

    pub const fn compression(&self) -> Compression {
        self.compression
    }

    /// Observations absorbed. Not the memory: that is [`Self::bytes`], and it
    /// does not move.
    pub const fn count(&self) -> u64 {
        self.count
    }

    pub const fn total_weight(&self) -> f64 {
        self.total_weight
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Centroids currently held — at most [`Compression::centroid_ceiling`].
    pub fn centroids(&self) -> usize {
        self.centroids.len()
    }

    /// The memory the digest occupies, which is a function of its compression
    /// and of nothing else.
    pub fn bytes(&self) -> usize {
        (self.centroids.capacity() + self.buffer.capacity()) * std::mem::size_of::<Centroid>()
    }

    /// Absorb one observation.
    ///
    /// Refuses a non-finite value rather than absorbing it: one NaN in a
    /// centroid makes every quantile read from the digest afterwards a NaN,
    /// and the digest carries no record of which observation did it.
    pub fn add(&mut self, value: f64) -> Result<()> {
        self.add_weighted(value, 1.0)
    }

    /// Absorb one observation carrying `weight`.
    ///
    /// Refuses a non-finite or non-positive weight for the same reason a
    /// reservoir does: a weight of zero is an observation that is present in
    /// the count and absent from every quantile, which is a state no reader
    /// can interpret.
    pub fn add_weighted(&mut self, value: f64, weight: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "a t-digest will not absorb {value}: one non-finite centroid makes every \
                 quantile read from the digest afterwards non-finite, and the digest keeps no \
                 record of which observation did it"
            )));
        }
        if !weight.is_finite() || weight <= 0.0 {
            return Err(Error::invalid(format!(
                "a t-digest observation's weight must be finite and above zero, not {weight}; a \
                 zero-weight point counts toward nothing and a negative one subtracts mass the \
                 stream never carried"
            )));
        }
        self.buffer.push(Centroid {
            mean: value,
            weight,
        });
        self.total_weight += weight;
        self.count += 1;
        self.minimum = Some(self.minimum.map_or(value, |held| held.min(value)));
        self.maximum = Some(self.maximum.map_or(value, |held| held.max(value)));
        if self.buffer.len() >= self.compression.centroid_ceiling() {
            self.compress();
        }
        Ok(())
    }

    /// Merge the buffer into the centroids.
    ///
    /// Public so a caller about to serialise or measure a digest can settle it
    /// first; called automatically whenever the buffer fills and before every
    /// read.
    pub fn compress(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let ceiling = self.compression.centroid_ceiling();
        let mut points = std::mem::take(&mut self.centroids);
        points.append(&mut self.buffer);
        // `total_cmp` rather than `partial_cmp`: the means are all finite by
        // construction, and a total order means no `unwrap` and no ordering
        // that depends on where a NaN happened to sit.
        points.sort_by(|a, b| a.mean.total_cmp(&b.mean));

        let total = self.total_weight;
        let mut out: Vec<Centroid> = Vec::with_capacity(ceiling);
        let mut iter = points.into_iter();
        let Some(mut current) = iter.next() else {
            self.centroids = out;
            return;
        };
        let delta = self.compression.value();
        let mut weight_so_far = 0.0_f64;
        // Where the centroid being built starts, on the k axis.
        let mut k_start = k_scale(0.0, delta);
        for next in iter {
            let q_end = (weight_so_far + current.weight + next.weight) / total;
            // One unit of k is one centroid's budget. `k` spans exactly `δ/2`
            // across the whole rank axis, so a sweep spending at least a unit
            // on each centroid cannot produce more than `δ/2` of them — which
            // is the bound the ceiling is twice.
            let admits = k_scale(q_end, delta) - k_start <= 1.0;
            // The second arm is the memory guard. The scale function keeps the
            // sweep at about half the ceiling on every stream the suite
            // exercises — `a_digest_s_memory_does_not_grow_with_its_stream`
            // asserts that on a million points — but a bound argued from a
            // scale function is a bound nobody can check at run time, and this
            // one can: once the output is one short of the ceiling the last
            // centroid absorbs whatever remains, so the vector cannot exceed
            // the memory the compression declared however adversarial the
            // order of arrival.
            if admits || out.len() + 1 >= ceiling {
                current.absorb(next);
            } else {
                weight_so_far += current.weight;
                out.push(current);
                current = next;
                k_start = k_scale(weight_so_far / total, delta);
            }
        }
        out.push(current);
        self.centroids = out;
    }

    /// The value at rank `q`, within [`Compression::rank_error`] of it.
    ///
    /// Refuses a `q` outside `[0, 1]`, and refuses an empty digest rather
    /// than returning a NaN or a zero: "the median of nothing" has no answer,
    /// and both sentinels read downstream as a number somebody computed.
    pub fn quantile(&mut self, q: f64) -> Result<f64> {
        if !q.is_finite() || !(0.0..=1.0).contains(&q) {
            return Err(Error::invalid(format!(
                "a quantile must lie in [0, 1], not {q}; a clamped q would answer a question \
                 nobody asked and read exactly like an answer to the one they did"
            )));
        }
        self.compress();
        let (Some(minimum), Some(maximum)) = (self.minimum, self.maximum) else {
            return Err(Error::invalid(
                "a t-digest holding no observation has no quantile; a zero or a NaN returned \
                 here reads downstream as a figure somebody computed, so the digest refuses \
                 instead"
                    .to_string(),
            ));
        };
        let Some(first) = self.centroids.first() else {
            return Err(Error::invalid(
                "a t-digest holding observations but no centroid cannot answer a quantile; this \
                 is a defect in the digest rather than in the request, and it is reported rather \
                 than papered over with the minimum"
                    .to_string(),
            ));
        };
        let target = q * self.total_weight;
        // Below the first centroid's centre of mass: interpolate down to the
        // smallest value actually seen, so the 0th percentile is a real
        // observation rather than the first cluster's mean.
        if target < first.weight / 2.0 {
            let span = first.weight / 2.0;
            return Ok(minimum + (first.mean - minimum) * (target / span));
        }
        let mut cumulative = first.weight / 2.0;
        for pair in self.centroids.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            // Centre to centre: half of each centroid's weight lies between
            // them, which is what makes the interpolation linear in rank.
            let span = (left.weight + right.weight) / 2.0;
            if target < cumulative + span {
                let along = (target - cumulative) / span;
                return Ok(left.mean + (right.mean - left.mean) * along);
            }
            cumulative += span;
        }
        let Some(last) = self.centroids.last() else {
            return Ok(maximum);
        };
        let span = last.weight / 2.0;
        let along = ((target - cumulative) / span).min(1.0);
        Ok(last.mean + (maximum - last.mean) * along)
    }

    /// Where `value` sits in the distribution, as a rank in `[0, 1]`.
    ///
    /// The inverse question to [`Self::quantile`], and the one a drift check
    /// asks: not "what is the median" but "at what rank does the median of
    /// some other sample fall in this one". Stated in rank units so it can be
    /// compared against [`Compression::rank_error`] directly, without a
    /// conversion through the units of the data that nothing here could
    /// justify.
    pub fn rank_of(&mut self, value: f64) -> Result<f64> {
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "a t-digest cannot rank {value}; a non-finite value has no place in the \
                 distribution and a rank returned for one would be a comparison nobody made"
            )));
        }
        self.compress();
        if self.total_weight <= 0.0 {
            return Err(Error::invalid(
                "a t-digest holding no observation ranks nothing; a rank of zero returned here \
                 would read as 'smaller than everything seen' rather than 'nothing was seen'"
                    .to_string(),
            ));
        }
        let mut below = 0.0_f64;
        for centroid in &self.centroids {
            if centroid.mean < value {
                below += centroid.weight;
            } else if centroid.mean > value {
                break;
            } else {
                // Ties land at the centre of the equal mass, which is the
                // convention that makes the rank of the median of a symmetric
                // distribution one half rather than something either side of
                // it.
                below += centroid.weight / 2.0;
                break;
            }
        }
        Ok(below / self.total_weight)
    }
}

/// Dunning's k1 scale function: the rank axis `[0, 1]` stretched onto
/// `[−δ/4, +δ/4]` by `k(q) = (δ/2π)·asin(2q − 1)`.
///
/// Two properties, and both are load-bearing. It is *steep* at the ends, so
/// one unit of k buys a narrow band of rank there and the tails get many small
/// centroids — which is the reason to choose a t-digest at all, since the 99th
/// percentile of a loss distribution is where a risk figure is read. And its
/// range is *finite*: `k(1) − k(0) = δ/2` exactly, so a sweep that spends at
/// least one unit per centroid produces at most `δ/2` of them, and the memory
/// is bounded by a fact about the function rather than by hope.
///
/// The second property is what the module's first draft lacked. A rule
/// bounding each centroid's weight by `4q(1−q)/δ` bounds no count at all: the
/// integral of its reciprocal diverges at both ends, so the centroid list grew
/// past its ceiling, the guard in [`TDigest::compress`] fired on every merge,
/// and the digest answered the 10th percentile of the unit interval with 0.17.
fn k_scale(q: f64, delta: f64) -> f64 {
    // `asin` is undefined outside [-1, 1]; `q` reaches here only as a ratio of
    // weights against their own total, so the argument is in range, and the
    // clamp is a guard against a floating-point overshoot of the last ratio
    // rather than a correction of a caller's input.
    delta / (2.0 * std::f64::consts::PI) * (2.0 * q - 1.0).clamp(-1.0, 1.0).asin()
}

// ---------------------------------------------------------------------------
// HyperLogLog
// ---------------------------------------------------------------------------

/// The coarsest register count a HyperLogLog may be built at: `2⁴ = 16`
/// registers, a relative standard error of 26%.
///
/// Below this the bias corrections the estimator is derived under stop
/// holding at all, and the answer is not a wide estimate but an arbitrary
/// one.
pub const HLL_MIN_PRECISION: u8 = 4;

/// The finest: `2¹⁶ = 65,536` registers, one byte each, 64 KB — a relative
/// standard error of 0.41%.
///
/// The ceiling that makes "kilobytes" a number. §21.1 budgets ~320 KB per
/// node for two hundred features together, so a single cardinality estimator
/// costing a fifth of it is the outer edge; above this the precision is
/// refused rather than allocated, which is the discipline
/// [`crate::sketch::MAX_COUNTERS`] applies to the count-min sketch.
pub const HLL_MAX_PRECISION: u8 = 16;

/// A HyperLogLog's register count, as a power of two, and the error that
/// follows from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PrecisionWire")]
pub struct Precision {
    bits: u8,
}

/// The wire shape, validated through [`Precision::new`] on the way in.
#[derive(Deserialize)]
struct PrecisionWire {
    bits: u8,
}

impl TryFrom<PrecisionWire> for Precision {
    type Error = Error;

    fn try_from(wire: PrecisionWire) -> Result<Self> {
        Self::new(wire.bits)
    }
}

impl Precision {
    pub fn new(bits: u8) -> Result<Self> {
        if !(HLL_MIN_PRECISION..=HLL_MAX_PRECISION).contains(&bits) {
            return Err(Error::invalid(format!(
                "a HyperLogLog's precision must lie between {HLL_MIN_PRECISION} and \
                 {HLL_MAX_PRECISION}, not {bits}; below the floor the bias corrections the \
                 estimator is derived under do not hold and the answer is arbitrary rather than \
                 merely wide, and above the ceiling one estimator would spend a fifth of the \
                 whole per-node memory budget §21.1 sets for two hundred features"
            )));
        }
        Ok(Self { bits })
    }

    pub const fn bits(&self) -> u8 {
        self.bits
    }

    /// `2^bits`. The memory in bytes too, since a register is one byte.
    pub const fn registers(&self) -> usize {
        1usize << self.bits
    }

    /// The standard error of the estimate, as a fraction of the cardinality:
    /// `1.04/√m`.
    ///
    /// Flajolet's constant. A *relative* error, unlike the count-min sketch's
    /// `ε·N`, which is why the two bounds are separate types rather than one:
    /// a bound that is a fraction of the answer and a bound that is a
    /// fraction of the stream behave differently under every consumer
    /// decision, and a single type carrying both would let a caller apply the
    /// wrong one without the compiler noticing.
    pub fn relative_standard_error(&self) -> f64 {
        // usize → f64 at the statistics boundary: a register count of at most
        // 65,536 converts exactly.
        1.04 / (self.registers() as f64).sqrt()
    }

    /// How far the estimate may sit from the truth at a cardinality of
    /// `cardinality`, with probability about 95%: `1.96σ`, two-sided.
    ///
    /// Stated at a confidence rather than as a bare standard error because a
    /// consumer refusing on "one sigma" is refusing at a third of the time.
    pub fn absolute_error(&self, cardinality: f64) -> f64 {
        1.96 * self.relative_standard_error() * cardinality.abs()
    }

    /// Whether a consumer that can absorb an error of `tolerance` at a
    /// cardinality of `cardinality` may rely on this precision.
    pub fn tolerable_for(&self, cardinality: f64, tolerance: f64) -> bool {
        tolerance.is_finite()
            && tolerance >= 0.0
            && cardinality.is_finite()
            && self.absolute_error(cardinality) <= tolerance
    }

    /// One line for a manifest.
    pub fn describe(&self) -> String {
        format!(
            "HyperLogLog (precision {}, {} registers, {} bytes): a cardinality estimate with a \
             relative standard error of {:.4}",
            self.bits,
            self.registers(),
            self.registers(),
            self.relative_standard_error()
        )
    }

    /// The bias-correction constant for this register count.
    fn alpha(&self) -> f64 {
        let m = self.registers() as f64;
        match self.registers() {
            16 => 0.673,
            32 => 0.697,
            64 => 0.709,
            _ => 0.7213 / (1.0 + 1.079 / m),
        }
    }

    /// The largest leading-zero count a hash can produce at this precision:
    /// `64 − bits + 1`. A register above it is a value no hash produced.
    const fn max_rank(&self) -> u8 {
        64 - self.bits + 1
    }
}

/// Distinct keys in a stream, counted in memory fixed by the precision.
///
/// See the module doc. The accuracy claim is [`Precision`]'s and is tested
/// against streams of known cardinality rather than only asserted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "HyperLogLogWire")]
pub struct HyperLogLog {
    precision: Precision,
    /// One byte per register, `2^bits` of them and never any other number.
    registers: Vec<u8>,
}

/// The wire shape, held to its precision's geometry on the way in.
#[derive(Deserialize)]
struct HyperLogLogWire {
    precision: Precision,
    registers: Vec<u8>,
}

impl TryFrom<HyperLogLogWire> for HyperLogLog {
    type Error = Error;

    fn try_from(wire: HyperLogLogWire) -> Result<Self> {
        if wire.registers.len() != wire.precision.registers() {
            return Err(Error::invalid(format!(
                "a serialised HyperLogLog holds {} register(s) and its own precision implies {}; \
                 a register vector shorter than the index space panics on the first add and a \
                 longer one holds counts no key can reach, so it is refused rather than read",
                wire.registers.len(),
                wire.precision.registers()
            )));
        }
        let ceiling = wire.precision.max_rank();
        if let Some(rogue) = wire.registers.iter().find(|value| **value > ceiling) {
            return Err(Error::invalid(format!(
                "a serialised HyperLogLog carries a register of {rogue} and no hash at precision \
                 {} can produce a rank above {ceiling}; such a register drives the harmonic mean \
                 to an estimate far above any cardinality the stream held, so it is refused",
                wire.precision.bits()
            )));
        }
        Ok(Self {
            precision: wire.precision,
            registers: wire.registers,
        })
    }
}

impl HyperLogLog {
    /// An empty estimator at `precision`.
    ///
    /// Infallible: [`Precision::new`] has already refused a value whose
    /// register count would exceed the memory this module promises.
    pub fn new(precision: Precision) -> Self {
        Self {
            precision,
            registers: vec![0; precision.registers()],
        }
    }

    pub const fn precision(&self) -> Precision {
        self.precision
    }

    /// The memory the estimator occupies: one byte per register, and it does
    /// not move however long the stream runs.
    pub fn bytes(&self) -> usize {
        self.registers.len()
    }

    /// Registers never touched — the input to the small-cardinality
    /// correction, and a useful diagnostic on its own: an estimator with no
    /// empty register has saturated its range.
    pub fn empty_registers(&self) -> usize {
        self.registers.iter().filter(|value| **value == 0).count()
    }

    pub fn add(&mut self, key: &str) {
        self.add_bytes(key.as_bytes());
    }

    pub fn add_bytes(&mut self, key: &[u8]) {
        // FNV-1a avalanched through splitmix64. FNV-1a alone would not do:
        // its top bits move too little with the key, and this estimator reads
        // the *leading zeros* of the hash, so a digest whose high bits are
        // sticky produces a rank distribution nothing like the geometric one
        // the estimator is derived under.
        let digest = splitmix64(fnv1a(key));
        let bits = u32::from(self.precision.bits());
        // usize from the top `bits` of the digest: below `registers()` by
        // construction, so the index cannot leave the vector.
        let index = (digest >> (64 - bits)) as usize;
        let remainder = digest << bits;
        // The tail has `64 - bits` meaningful bits; a tail of all zeros is
        // ranked at the maximum rather than at 65, which no register may hold.
        let rank = remainder
            .leading_zeros()
            .min(64 - bits)
            .saturating_add(1)
            .min(u32::from(self.precision.max_rank()));
        // u32 → u8: bounded by `max_rank`, at most 61.
        let rank = rank as u8;
        if self.registers[index] < rank {
            self.registers[index] = rank;
        }
    }

    /// The estimated number of distinct keys.
    ///
    /// An `f64` and not a `u64`, deliberately: this is a statistic with a
    /// declared relative standard error, and rounding it to an integer at the
    /// boundary invites a reader to treat it as a count somebody took.
    pub fn estimate(&self) -> f64 {
        let m = self.registers.len() as f64;
        let mut harmonic = 0.0_f64;
        for register in &self.registers {
            harmonic += 2.0_f64.powi(-i32::from(*register));
        }
        let raw = self.precision.alpha() * m * m / harmonic;
        let empty = self.empty_registers();
        // Linear counting below the raw estimator's usable range. Without it
        // a stream of a dozen keys reads as several times that, because the
        // harmonic mean of mostly-empty registers is dominated by the empties
        // rather than by the keys.
        if raw <= 2.5 * m && empty > 0 {
            m * (m / empty as f64).ln()
        } else {
            raw
        }
    }

    /// Absorb another estimator's registers.
    ///
    /// Refuses a different precision rather than resampling: the registers of
    /// two estimators at different precisions index different hash prefixes,
    /// and merging them produces a number that is not an estimate of anything.
    /// This is what lets a per-node estimator be combined at the centre
    /// without shipping the stream.
    pub fn merge(&mut self, other: &Self) -> Result<()> {
        if self.precision != other.precision {
            return Err(Error::invalid(format!(
                "a HyperLogLog at precision {} cannot absorb one at precision {}; their \
                 registers index different prefixes of the hash, so the union would be a number \
                 that estimates no cardinality at all",
                self.precision.bits(),
                other.precision.bits()
            )));
        }
        for (mine, theirs) in self.registers.iter_mut().zip(other.registers.iter()) {
            *mine = (*mine).max(*theirs);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Weighted reservoir
// ---------------------------------------------------------------------------

/// The most rows a reservoir may hold.
///
/// §21.1 sizes the representative sample at "10,000 rows, not 10 billion", so
/// the ceiling sits comfortably above the figure the blueprint names while
/// still refusing a capacity that would make the reservoir the archive it
/// exists to replace. The refusal is at construction, so the allocation below
/// is bounded by the type.
pub const RESERVOIR_MAX_ROWS: usize = 65_536;

/// One retained row and the key it was retained on.
#[derive(Clone, Debug, PartialEq)]
struct Row<T> {
    /// The A-Res key `u^(1/w)`. Larger keys are retained.
    key: f64,
    /// Arrival order, which breaks key ties so two replays of one stream
    /// produce one sample in one order.
    sequence: u64,
    item: T,
}

/// A fixed-size weighted sample of a stream.
///
/// Efraimidis and Spirakis' A-Res: each item is given the key `u^(1/w)` for a
/// uniform `u` and its weight `w`, and the `k` largest keys are kept. An
/// item's chance of being in the sample rises with its weight, which is what
/// §21.1's "recency- and regime-weighted sample" needs — the caller supplies
/// the weight, so recency, regime, or anything else it can justify is
/// expressible without this type knowing about any of them.
///
/// The randomness is a seeded splitmix64 counter, never the operating
/// system's. A sample that fed a fit has to be reproducible from the log, and
/// one drawn from an entropy source nobody recorded is not.
///
/// Holds no `serde` derive. A reservoir is a working set and the event log
/// holds the rows it sampled; a second durable copy of a derived fact is a
/// second source of truth for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Reservoir<T> {
    capacity: usize,
    seed: u64,
    draws: u64,
    seen: u64,
    /// Ascending by `(key, sequence)`, so the smallest key — the next to be
    /// displaced — is always at the front, and the iteration order that
    /// reaches a caller is total.
    rows: Vec<Row<T>>,
}

impl<T> Reservoir<T> {
    /// A reservoir holding at most `capacity` rows, drawing from `seed`.
    ///
    /// Refuses a capacity of zero — a sample of nothing is not a sample, and
    /// every consumer of it would divide by the row count — and refuses one
    /// above [`RESERVOIR_MAX_ROWS`] rather than allocating it.
    pub fn new(capacity: usize, seed: u64) -> Result<Self> {
        if capacity == 0 || capacity > RESERVOIR_MAX_ROWS {
            return Err(Error::invalid(format!(
                "a reservoir's capacity must lie between 1 and {RESERVOIR_MAX_ROWS}, not \
                 {capacity}; a capacity of zero is a sample every consumer divides by and a \
                 capacity above the ceiling makes the reservoir the archive it exists to replace"
            )));
        }
        Ok(Self {
            capacity,
            seed,
            draws: 0,
            seen: 0,
            rows: Vec::with_capacity(capacity),
        })
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Items offered, whether retained or not.
    pub const fn seen(&self) -> u64 {
        self.seen
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The standard error of a proportion estimated from a full reservoir,
    /// at its widest: `0.5/√k`.
    ///
    /// The declared bound, and the counterpart of
    /// [`Compression::rank_error`]. `√(p(1−p)/k)` is maximised at `p = 0.5`,
    /// and stating the maximum rather than a value that depends on the answer
    /// means a consumer can decide whether to rely on the sample before
    /// drawing it.
    ///
    /// Stated against the capacity and not the current length on purpose: a
    /// reservoir that has seen fewer items than it holds is an exact sample
    /// of everything, and a bound that narrowed as the sample filled would
    /// tell a consumer the estimate was *improving* as the stream got longer,
    /// which is backwards.
    pub fn sampling_error(&self) -> f64 {
        // usize → f64 at the statistics boundary: bounded by
        // `RESERVOIR_MAX_ROWS`, so exact.
        0.5 / (self.capacity as f64).sqrt()
    }

    /// Whether the whole stream fitted, so the sample is the population.
    ///
    /// Worth asking before a drift comparison: two exact samples that
    /// disagree disagree about the data, and two sampled ones may merely have
    /// been drawn differently.
    pub fn is_exhaustive(&self) -> bool {
        self.seen <= self.capacity as u64
    }

    /// One line for a manifest.
    pub fn describe(&self) -> String {
        format!(
            "weighted reservoir ({} row(s) of at most {}, {} item(s) seen): a proportion read \
             from it carries a standard error of at most {:.4}",
            self.rows.len(),
            self.capacity,
            self.seen,
            self.sampling_error()
        )
    }

    /// Offer one item at `weight`. `Ok(true)` if it was retained.
    ///
    /// Refuses a non-finite or non-positive weight rather than clamping it: a
    /// weight of zero asks for an item that can never be sampled, which is a
    /// caller that meant to filter and did not, and a negative weight makes
    /// `u^(1/w)` a number above one that displaces every honest key in the
    /// reservoir.
    pub fn push(&mut self, item: T, weight: f64) -> Result<bool> {
        if !weight.is_finite() || weight <= 0.0 {
            return Err(Error::invalid(format!(
                "a reservoir weight must be finite and above zero, not {weight}; zero asks for \
                 an item that can never be sampled and a negative weight produces a key above \
                 one, which displaces every honestly drawn row in the reservoir"
            )));
        }
        self.seen += 1;
        let sequence = self.seen;
        let uniform = self.next_uniform();
        // A-Res: `u^(1/w)`. Larger weights push the key toward one.
        let key = uniform.powf(1.0 / weight);
        if self.rows.len() < self.capacity {
            self.insert(Row {
                key,
                sequence,
                item,
            });
            return Ok(true);
        }
        let Some(weakest) = self.rows.first() else {
            // Unreachable while `capacity` is at least one, which
            // `Reservoir::new` guarantees; written as a branch rather than an
            // index so it cannot become a panic in a `Result`-returning
            // function if that ever changes.
            return Ok(false);
        };
        if key <= weakest.key {
            return Ok(false);
        }
        self.rows.remove(0);
        self.insert(Row {
            key,
            sequence,
            item,
        });
        Ok(true)
    }

    /// The retained rows, weakest key first — a total order, so two replays
    /// of one stream produce one sample in one order.
    pub fn samples(&self) -> impl Iterator<Item = &T> {
        self.rows.iter().map(|row| &row.item)
    }

    fn insert(&mut self, row: Row<T>) {
        let at = self
            .rows
            .partition_point(|held| (held.key, held.sequence) < (row.key, row.sequence));
        self.rows.insert(at, row);
    }

    /// The next uniform in the open interval `(0, 1)`.
    ///
    /// Open rather than half-open at both ends: a `u` of exactly zero makes
    /// every weight produce the key zero, and a `u` of exactly one makes a
    /// row that can never be displaced. The half-step added to the mantissa
    /// is what excludes both.
    fn next_uniform(&mut self) -> f64 {
        self.draws += 1;
        let bits = splitmix64(self.seed ^ self.draws.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        // 53 bits is an f64's mantissa, so every value below is exact.
        ((bits >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0
    }
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the estimator it is exercising still
// has to assert, and the abort is the reporting mechanism rather than a
// defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// A deterministic shuffle, so a test's stream is not sorted and is the
    /// same stream on every machine. Not a general-purpose generator and not
    /// used outside tests.
    fn shuffled(count: usize) -> Vec<f64> {
        let mut values: Vec<f64> = (0..count).map(|i| i as f64 / count as f64).collect();
        let mut state = 0x243F_6A88_85A3_08D3_u64;
        for i in (1..values.len()).rev() {
            state = splitmix64(state);
            let j = (state % (i as u64 + 1)) as usize;
            values.swap(i, j);
        }
        values
    }

    // -- t-digest ----------------------------------------------------------

    /// The accuracy claim, against a distribution whose every quantile is
    /// known exactly: a hundred thousand values spread uniformly over the unit
    /// interval, delivered shuffled. The value returned for `q` must sit
    /// within the declared rank error of `q`, and on this distribution rank
    /// and value coincide, so the assertion is directly on
    /// [`Compression::rank_error`].
    ///
    /// The tolerances are the digest's own declared bound and nothing looser:
    /// at compression 200 the median may be off by π/200 ≈ 0.0157 in rank,
    /// the first and 99th percentiles by 2π√(0.0099)/200 ≈ 0.0031. A constant
    /// estimator fails every one of them.
    ///
    /// **This test alone does not prove the scale function**, and the comment
    /// says so because the first version of it claimed to and did not.
    /// Replacing `k_scale` with a linear ramp — the layout that spends the
    /// centroid budget evenly rather than on the tails — left every assertion
    /// here passing, because on a uniform population the interpolation between
    /// two centroid means is exact wherever the centroids happen to fall. The
    /// distribution has to be one where rank and value do not coincide, which
    /// is what `a_digest_places_a_tail_value_at_its_true_rank` is for. A
    /// well-shaped test on the wrong distribution is the quietest kind of
    /// unverified test there is.
    ///
    /// Mutated by replacing `let target = q * self.total_weight` with
    /// `0.5 * self.total_weight` — the estimator replaced by a constant, the
    /// median — and confirmed the first assertion then fails by 0.49 against
    /// a declared error of 0.0031, then restored byte-for-byte. Mutating the
    /// interpolation's position to the centroid midpoint instead did *not*
    /// fire, and that is not a gap: the declared bound is one centroid's rank
    /// width and midpoint snapping errs by at most half of one, so an
    /// implementation doing that is inside the bound it declares. A test that
    /// failed on it would be asserting something the module does not promise.
    #[test]
    fn a_digest_returns_a_quantile_within_the_rank_error_its_compression_declares() -> Result<()> {
        let compression = Compression::new(200.0)?;
        let mut digest = TDigest::new(compression);
        let values = shuffled(100_000);
        // Premise: the stream is genuinely unsorted, so the digest is not
        // being handed an already-ordered population.
        assert!(
            values.windows(2).any(|pair| pair[0] > pair[1]),
            "premise: the stream must not arrive sorted"
        );
        for value in &values {
            digest.add(*value)?;
        }
        assert_eq!(digest.count(), 100_000);

        for q in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99] {
            let allowed = compression.rank_error(q)?;
            let estimate = digest.quantile(q)?;
            assert!(
                (estimate - q).abs() <= allowed,
                "quantile {q} estimated at {estimate}, off by {} against a declared rank error \
                 of {allowed}",
                (estimate - q).abs()
            );
        }
        Ok(())
    }

    /// The scale function's own claim, on a distribution where rank and value
    /// come apart: a hundred thousand points laid on the unit exponential at
    /// exactly `−ln(1 − (i + ½)/n)`, so the empirical rank of any value is its
    /// own `1 − e^{−x}` to within half a point in a hundred thousand and no
    /// sampling noise stands between the assertion and the estimator.
    ///
    /// [`TDigest::rank_of`] rather than [`TDigest::quantile`], deliberately.
    /// A rank read off the centroids snaps to a centroid boundary, so its
    /// error *is* the width of the centroid it lands in, which is exactly what
    /// [`Compression::rank_error`] declares; a quantile interpolates between
    /// two centroid means and so smooths that width away wherever the
    /// distribution is locally straight. The interpolation is what a caller
    /// wants and the snapping is what the bound is about, and testing the
    /// bound through the interpolation is how the uniform-stream test above
    /// came to pass under a scale function that was wrong.
    ///
    /// Eleven ranks, weighted toward both tails, where the k1 scale function
    /// makes a centroid narrow and a linear one does not. That is the whole
    /// difference between "a t-digest" and "two hundred equal buckets", and it
    /// is the reason a risk figure may be read from the 99.8th percentile of
    /// one and not of the other.
    ///
    /// Mutated by replacing `k_scale`'s body with the linear ramp
    /// `delta * q / 2.0`, which has the same range and so the same memory but
    /// spends it evenly — confirmed the assertions at 0.002, 0.005, 0.995 and
    /// 0.998 then fail (0.0020 against 0.0014, 0.0028 against 0.0022, 0.0041
    /// against 0.0022, 0.0020 against 0.0014) while the median stays
    /// comfortably inside — then restored byte-for-byte. Under the real scale
    /// function the same eleven ranks land between two and sixty times inside
    /// the bound, so the bound is neither vacuous nor met by luck.
    #[test]
    fn a_digest_places_a_tail_value_at_its_true_rank() -> Result<()> {
        let compression = Compression::new(200.0)?;
        let mut digest = TDigest::new(compression);
        let count = 100_000_usize;
        let mut values: Vec<f64> = (0..count)
            .map(|i| {
                // usize → f64: an index below 100,000 converts exactly.
                let q = (i as f64 + 0.5) / count as f64;
                -(1.0 - q).ln()
            })
            .collect();
        // Shuffled by the same deterministic walk `shuffled` uses, so the
        // digest is not handed a sorted stream.
        let mut state = 0x243F_6A88_85A3_08D3_u64;
        for i in (1..values.len()).rev() {
            state = splitmix64(state);
            let j = (state % (i as u64 + 1)) as usize;
            values.swap(i, j);
        }
        // Premise: the population is genuinely skewed, so rank and value do
        // not coincide and the layout of the centroids can be wrong.
        assert!(
            values.iter().copied().fold(f64::MIN, f64::max) > 8.0,
            "premise: the exponential tail must reach far beyond its median"
        );
        for value in &values {
            digest.add(*value)?;
        }

        for q in [
            0.002, 0.005, 0.01, 0.02, 0.05, 0.5, 0.95, 0.98, 0.99, 0.995, 0.998,
        ] {
            let value = -(1.0_f64 - q).ln();
            let allowed = compression.rank_error(q)?;
            let rank = digest.rank_of(value)?;
            assert!(
                (rank - q).abs() <= allowed,
                "the value at true rank {q} was placed at rank {rank}, off by {} against a \
                 declared rank error of {allowed}",
                (rank - q).abs()
            );
        }
        Ok(())
    }

    /// The memory claim: a digest's footprint is a function of its
    /// compression and of nothing else. A million observations must not move
    /// it, and the centroid count must stay inside the ceiling the
    /// compression declares.
    ///
    /// This is the property that makes it a sketch rather than a histogram,
    /// and the one an implementation loses silently — a digest that never
    /// compresses answers every quantile perfectly and grows without limit.
    ///
    /// Mutated by removing the `self.compress()` call from `add_weighted`'s
    /// full-buffer branch — confirmed the byte assertion then fails as the
    /// buffer grows past its capacity, then restored.
    #[test]
    fn a_digest_s_memory_does_not_grow_with_its_stream() -> Result<()> {
        let compression = Compression::new(100.0)?;
        let mut digest = TDigest::new(compression);
        let ceiling = compression.centroid_ceiling();
        assert_eq!(ceiling, 201, "premise: the ceiling is 2 x compression + 1");
        let before = digest.bytes();
        assert!(before > 0, "premise: an empty digest has allocated already");

        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..1_000_000 {
            state = splitmix64(state);
            // A heavy-tailed stream: the adversarial shape for a structure
            // that spends centroids on the tails.
            let value = ((state >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0;
            digest.add(-(1.0 - value).ln())?;
        }
        digest.compress();
        assert_eq!(digest.count(), 1_000_000);
        assert_eq!(
            digest.bytes(),
            before,
            "a million observations moved the digest's memory"
        );
        // Inside the ceiling, and inside *half* of it — which is the stronger
        // statement, and the one that says the guard in `compress` is a guard
        // rather than the working path. A merging digest settles between
        // `delta/2` and `delta` centroids; if this ever approached the ceiling
        // the guard would be coalescing the tail on every merge and the rank
        // error the compression declares would be fiction, which is exactly
        // what the module's first scale function did.
        assert!(
            digest.centroids() * 2 <= ceiling,
            "the digest holds {} centroids against a ceiling of {ceiling}; the scale function's \
             own bound is half that, so the memory guard is firing and the rank error the \
             compression declares is no longer the error the digest makes",
            digest.centroids()
        );
        // And it still answers: the median of an exponential with rate one is
        // ln 2, and the digest must find it.
        let median = digest.quantile(0.5)?;
        assert!(
            (median - std::f64::consts::LN_2).abs() < 0.01,
            "the median of a unit exponential is ln 2; the digest said {median}"
        );
        Ok(())
    }

    /// Every refusal on the quantile path fires: a compression outside the
    /// range in either direction and on the wire, a non-finite observation, a
    /// zero weight, a quantile outside `[0, 1]`, and a quantile asked of an
    /// empty digest.
    ///
    /// Mutated by neutralising `Compression::new`'s range check — confirmed
    /// it then reports `0 was accepted as a compression: Compression { delta:
    /// 0.0 }`, then restored byte-for-byte. Mutated again by neutralising
    /// `quantile`'s own range check so an out-of-range `q` is answered rather
    /// than refused — confirmed it then reports `a quantile of -0.001 was
    /// answered rather than refused`, then restored byte-for-byte.
    #[test]
    fn a_digest_refuses_a_compression_a_value_and_a_quantile_it_cannot_answer_for() -> Result<()> {
        // Premise: the range admits a sensible compression with room either
        // side, so the refusals below are about the edges and not about
        // everything.
        let usable = Compression::new(100.0)?;
        assert!((usable.value() - 100.0).abs() < f64::EPSILON);

        for delta in [
            0.0,
            -1.0,
            TDIGEST_MIN_COMPRESSION - 0.5,
            TDIGEST_MAX_COMPRESSION + 0.5,
            f64::NAN,
            f64::INFINITY,
        ] {
            let error = Compression::new(delta)
                .expect_err(&format!("{delta} was accepted as a compression"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
        }
        // The wire is gated the same way, because deserialisation goes
        // through `new`.
        assert!(
            serde_json::from_str::<Compression>(r#"{"delta":0.0}"#).is_err(),
            "a compression the constructor refuses was accepted off the wire"
        );
        let round_trip: Compression = serde_json::from_str(&serde_json::to_string(&usable)?)?;
        assert_eq!(round_trip, usable);

        let mut digest = TDigest::new(usable);
        assert!(
            digest.quantile(0.5).is_err(),
            "an empty digest answered a median; a zero returned here reads downstream as a \
             figure somebody computed"
        );
        assert!(
            digest.rank_of(1.0).is_err(),
            "an empty digest ranked a value"
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                digest.add(value).is_err(),
                "{value} was absorbed; one non-finite centroid makes every later quantile \
                 non-finite"
            );
        }
        for weight in [0.0, -1.0, f64::NAN] {
            assert!(
                digest.add_weighted(1.0, weight).is_err(),
                "a weight of {weight} was accepted"
            );
        }
        digest.add(1.0)?;
        for q in [-0.001, 1.001, f64::NAN] {
            assert!(
                digest.quantile(q).is_err(),
                "a quantile of {q} was answered rather than refused"
            );
            assert!(
                usable.rank_error(q).is_err(),
                "a rank error was stated for {q}"
            );
            assert!(!usable.tolerable_for(q, 1.0), "{q} was declared tolerable");
        }
        // A digest holding one observation answers both ends with it.
        assert!((digest.quantile(0.0)? - 1.0).abs() < 1e-12);
        assert!((digest.quantile(1.0)? - 1.0).abs() < 1e-12);
        Ok(())
    }

    /// A digest off the wire is held to its own compression's geometry: more
    /// centroids than the ceiling, a non-finite mean, a zero weight and a
    /// total that disagrees with the weights are each refused, and an honest
    /// round trip survives with its quantiles intact.
    ///
    /// Mutated by deleting the total-weight comparison in `try_from` —
    /// confirmed the disagreeing-total half then deserialises and this fails,
    /// then restored.
    #[test]
    fn a_digest_off_the_wire_is_held_to_the_geometry_its_compression_implies() -> Result<()> {
        let compression = Compression::new(20.0)?;
        let mut digest = TDigest::new(compression);
        for value in shuffled(500) {
            digest.add(value)?;
        }
        digest.compress();
        let honest: serde_json::Value = serde_json::to_value(&digest)?;
        // Premise: the honest form round-trips and still answers.
        let mut back: TDigest = serde_json::from_value(honest.clone())?;
        assert_eq!(back, digest);
        assert!((back.quantile(0.5)? - digest.quantile(0.5)?).abs() < 1e-12);

        let ceiling = compression.centroid_ceiling();
        let mut too_many = honest.clone();
        too_many["centroids"] = serde_json::Value::Array(
            (0..=ceiling)
                .map(|i| serde_json::json!({"mean": i as f64, "weight": 1.0}))
                .collect(),
        );
        assert!(
            serde_json::from_value::<TDigest>(too_many).is_err(),
            "a digest of {} centroids was accepted against a ceiling of {ceiling}",
            ceiling + 1
        );

        let mut zero_weight = honest.clone();
        zero_weight["centroids"] = serde_json::json!([{"mean": 1.0, "weight": 0.0}]);
        assert!(
            serde_json::from_value::<TDigest>(zero_weight).is_err(),
            "a zero-weight centroid was accepted; the interpolation divides by its span"
        );

        let mut disagreeing = honest;
        disagreeing["total_weight"] = serde_json::json!(1.0);
        assert!(
            serde_json::from_value::<TDigest>(disagreeing).is_err(),
            "a digest whose declared total disagrees with its centroids was accepted; every \
             quantile it answers is for a rank it never held"
        );
        Ok(())
    }

    /// The consumer's refusal: one compression is usable for a coarse rank
    /// question and refused for a fine one, and the line between them is the
    /// declared error at the quantile actually being asked about.
    ///
    /// Mutated by making `tolerable_for` return `true` unconditionally —
    /// confirmed the refused half then fails, then restored.
    #[test]
    fn a_consumer_refuses_a_digest_whose_declared_rank_error_exceeds_its_tolerance() -> Result<()> {
        let compression = Compression::new(50.0)?;
        // At the median the declared rank error is pi/50, about 0.0628.
        assert!(
            (compression.rank_error(0.5)? - std::f64::consts::PI / 50.0).abs() < 1e-12,
            "the declared error at the median is {}",
            compression.rank_error(0.5)?
        );
        assert!(
            compression.tolerable_for(0.5, 0.07),
            "a consumer that can absorb seven rank points in a hundred may rely on this digest"
        );
        assert!(
            !compression.tolerable_for(0.5, 0.05),
            "a consumer that must place the median within five rank points in a hundred cannot"
        );
        // And the tails are tighter, so the same digest serves a consumer at
        // the 99th percentile that it refuses at the median — the property the
        // scale function exists for, and the reason the bound is a function of
        // `q` rather than one number for the whole distribution.
        assert!(
            compression.tolerable_for(0.99, 0.05),
            "the 99th percentile's declared error is five times tighter than the median's"
        );
        assert!(!compression.tolerable_for(0.5, f64::NAN));
        assert!(!compression.tolerable_for(0.5, -1.0));
        Ok(())
    }

    // -- HyperLogLog -------------------------------------------------------

    /// The accuracy claim, against streams of known cardinality: a hundred
    /// thousand distinct keys, and separately a hundred, and separately ten
    /// keys repeated a hundred thousand times. Each estimate must sit inside
    /// the declared 95% band, and the last is the one that says the estimator
    /// counts *distinct* keys rather than observations.
    ///
    /// The band is `1.96 × 1.04/√m` — at precision 14 that is 1.6% — and a
    /// constant estimator fails all three, as does one that counts
    /// observations.
    ///
    /// Mutated by replacing the splitmix64 avalanche with the raw FNV-1a
    /// digest — confirmed the hundred-thousand assertion then fails by a wide
    /// margin, because FNV-1a's top bits carry too little of the key for the
    /// leading-zero count to be geometric, then restored. Mutated again by
    /// deleting the linear-counting branch — confirmed the hundred-key
    /// assertion then fails, then restored.
    #[test]
    fn a_hyperloglog_estimates_distinct_keys_inside_its_declared_band() -> Result<()> {
        let precision = Precision::new(14)?;
        // Premise: the declared band is tight enough for the assertions below
        // to mean something — under two percent, not under half.
        assert!(
            precision.relative_standard_error() < 0.01,
            "the declared standard error is {}",
            precision.relative_standard_error()
        );

        for truth in [100_u64, 100_000] {
            let mut estimator = HyperLogLog::new(precision);
            for key in 0..truth {
                estimator.add(&format!("subject-{key:08}"));
            }
            // f64 at the statistics boundary: a cardinality of at most
            // 100,000 converts exactly.
            let expected = truth as f64;
            let allowed = precision.absolute_error(expected);
            let estimate = estimator.estimate();
            assert!(
                (estimate - expected).abs() <= allowed,
                "{truth} distinct keys estimated at {estimate}, outside the declared band of \
                 +/-{allowed}"
            );
        }

        // Ten keys, a hundred thousand times. An estimator counting
        // observations would say a hundred thousand.
        let mut repeated = HyperLogLog::new(precision);
        for round in 0..10_000 {
            for key in 0..10 {
                repeated.add(&format!("subject-{key}"));
            }
            let _ = round;
        }
        let estimate = repeated.estimate();
        assert!(
            (estimate - 10.0).abs() <= 1.0,
            "ten distinct keys seen a hundred thousand times estimated at {estimate}"
        );
        assert_eq!(
            repeated.bytes(),
            precision.registers(),
            "the memory must be the precision's and nothing else's"
        );
        Ok(())
    }

    /// The memory claim, and the refusals around it: a precision outside the
    /// range in either direction and on the wire, a register vector whose
    /// length disagrees with the precision, a register above any rank a hash
    /// can produce, and a merge across precisions.
    ///
    /// Mutated by deleting the `registers.len()` comparison in `try_from` —
    /// confirmed the short-register half then deserialises and this fails,
    /// then restored. Mutated again by making `merge` ignore the precision
    /// mismatch — confirmed the merge assertion then fails, then restored.
    #[test]
    fn a_hyperloglog_refuses_a_precision_a_register_and_a_merge_it_cannot_honour() -> Result<()> {
        let precision = Precision::new(12)?;
        assert_eq!(precision.registers(), 4096);
        assert_eq!(precision.bits(), 12);

        for bits in [0_u8, HLL_MIN_PRECISION - 1, HLL_MAX_PRECISION + 1, 255] {
            let error =
                Precision::new(bits).expect_err(&format!("{bits} was accepted as a precision"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
            assert!(
                error.message().contains(&HLL_MAX_PRECISION.to_string()),
                "the refusal does not name the ceiling: {error}"
            );
        }
        assert!(
            serde_json::from_str::<Precision>(r#"{"bits":30}"#).is_err(),
            "a precision the constructor refuses was accepted off the wire"
        );

        let mut estimator = HyperLogLog::new(precision);
        estimator.add("subject");
        let honest: serde_json::Value = serde_json::to_value(&estimator)?;
        let back: HyperLogLog = serde_json::from_value(honest.clone())?;
        assert_eq!(back, estimator);

        let mut short = honest.clone();
        short["registers"] = serde_json::json!([0, 0, 0]);
        assert!(
            serde_json::from_value::<HyperLogLog>(short).is_err(),
            "three registers were accepted for a precision implying 4096; the first add past \
             index three would leave the vector"
        );

        let mut rogue = honest;
        let mut registers = vec![0_u8; precision.registers()];
        registers[0] = 64;
        rogue["registers"] = serde_json::to_value(&registers)?;
        assert!(
            serde_json::from_value::<HyperLogLog>(rogue).is_err(),
            "a register of 64 was accepted at precision 12, where no hash can produce a rank \
             above {}",
            64 - 12 + 1
        );

        let mut coarser = HyperLogLog::new(Precision::new(10)?);
        let error = coarser
            .merge(&estimator)
            .expect_err("a merge across precisions was accepted");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        // And a merge at the same precision is the union, not the sum.
        let mut left = HyperLogLog::new(precision);
        let mut right = HyperLogLog::new(precision);
        for key in 0..5_000 {
            left.add(&format!("subject-{key:08}"));
            // Half the keys are in both, so the union is 7,500 and the sum
            // would be 10,000.
            right.add(&format!("subject-{:08}", key + 2_500));
        }
        left.merge(&right)?;
        let union = left.estimate();
        let allowed = precision.absolute_error(7_500.0);
        assert!(
            (union - 7_500.0).abs() <= allowed,
            "the union of two overlapping five-thousand-key streams estimated at {union}, \
             outside the declared band of +/-{allowed} around 7500"
        );
        Ok(())
    }

    // -- weighted reservoir ------------------------------------------------

    /// The weighting claim, against a population whose composition is known:
    /// two thousand heavy items at weight ten among eighteen thousand light
    /// ones at weight one. The heavy items carry 20,000 of the 38,000 total
    /// weight, so a weighted sample is 52.6% heavy where an unweighted one
    /// would be 10%.
    ///
    /// Eight reservoirs at eight seeds, pooled. One is not enough and saying
    /// why matters more than the number: a single two-hundred-row sample of a
    /// 52.6% population has a standard error of 3.5 points and its seed-to-
    /// seed spread across twenty draws of this exact stream runs from 0.435 to
    /// 0.590, so a tolerance tight enough to be worth asserting would fail on
    /// some seeds and one loose enough to pass on all of them would admit
    /// almost anything. Pooling sixteen hundred rows brings the spread to
    /// about a point, and the four-point tolerance below is roughly three
    /// standard errors of the pooled figure — while the unweighted answer,
    /// 0.10, is more than thirty away.
    ///
    /// The capacity is two hundred rather than the thousand this test first
    /// used, and that is a fact about A-Res rather than a convenience: it
    /// samples *without replacement*, so drawing a thousand rows from a
    /// population holding only two thousand heavy items depletes the heavy
    /// pool and pulls the answer down to 0.496. At two hundred the depletion
    /// is a twentieth of the pool and the figure sits on the weight share.
    ///
    /// Mutated by inverting the retention comparison in `push` — declining the
    /// row when `key >= weakest.key` rather than when it falls at or below it
    /// — confirmed the pooled fraction is then exactly 0.1000, the population
    /// share, and this fails, then restored byte-for-byte. Mutated again by
    /// replacing `uniform.powf(1.0 / weight)` with `uniform`, which is
    /// unweighted reservoir sampling — confirmed the pooled fraction is then
    /// 0.1150 and this fails, then restored byte-for-byte.
    #[test]
    fn a_weighted_reservoir_over_samples_heavy_items_in_proportion_to_their_weight() -> Result<()> {
        let mut heavy_kept = 0_usize;
        let mut rows_kept = 0_usize;
        let mut heavy_offered = 0_usize;
        let mut offered = 0_usize;
        for seed in 0..8_u64 {
            let mut reservoir: Reservoir<bool> = Reservoir::new(200, seed)?;
            for index in 0..20_000_u64 {
                let heavy = index % 10 == 0;
                heavy_offered += usize::from(heavy);
                offered += 1;
                reservoir.push(heavy, if heavy { 10.0 } else { 1.0 })?;
            }
            assert_eq!(reservoir.len(), 200, "the reservoir did not fill");
            assert_eq!(reservoir.seen(), 20_000);
            assert!(!reservoir.is_exhaustive());
            heavy_kept += reservoir.samples().filter(|held| **held).count();
            rows_kept += reservoir.len();
        }
        // Premise: the population really is one in ten heavy, so an unweighted
        // sample would be ten percent and the assertion below is about the
        // weighting rather than about the population.
        assert_eq!(heavy_offered * 10, offered);
        assert_eq!(rows_kept, 1_600);

        // usize → f64 at the statistics boundary: a ratio of counts.
        let fraction = heavy_kept as f64 / rows_kept as f64;
        let expected = 20_000.0 / 38_000.0;
        assert!(
            (fraction - expected).abs() < 0.04,
            "the pooled sample is {fraction:.4} heavy against a weight share of {expected:.4}; \
             an unweighted reservoir would be 0.1000"
        );
        Ok(())
    }

    /// The memory claim and the refusals: capacity zero and capacity above
    /// the ceiling are refused at construction, a non-finite or non-positive
    /// weight is refused at the push, and a million items leave the row count
    /// exactly at the capacity.
    ///
    /// Mutated by neutralising the capacity check in `new` — confirmed it then
    /// reports `0 was accepted as a reservoir capacity`, then restored
    /// byte-for-byte. Mutated again by widening the fill branch to
    /// `self.rows.len() < self.capacity * 2` — confirmed it then reports `a
    /// million items left the reservoir holding 512 rows`, then restored
    /// byte-for-byte.
    ///
    /// The obvious mutation — letting `push` insert unconditionally, with no
    /// capacity branch at all — is **not** reported as fired, because it does
    /// not terminate: a million ordered insertions into a vector that never
    /// evicts is quadratic in memory movement and the test was still running
    /// after two minutes. That is evidence of a kind, and it is not the kind a
    /// mutation report may claim, so the bounded variant above is what was
    /// actually run.
    #[test]
    fn a_reservoir_refuses_an_unbounded_capacity_and_holds_its_row_count_against_a_long_stream()
    -> Result<()> {
        // Premise: a capacity inside the range is admitted, so the refusals
        // below are about the edges.
        let usable: Reservoir<u64> = Reservoir::new(10_000, 7)?;
        assert_eq!(usable.capacity(), 10_000);

        for capacity in [0, RESERVOIR_MAX_ROWS + 1, usize::MAX] {
            let error = Reservoir::<u64>::new(capacity, 7)
                .err()
                .unwrap_or_else(|| panic!("{capacity} was accepted as a reservoir capacity"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
            assert!(
                error.message().contains(&RESERVOIR_MAX_ROWS.to_string()),
                "the refusal does not name the ceiling: {error}"
            );
        }

        let mut reservoir: Reservoir<u64> = Reservoir::new(256, 11)?;
        for weight in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                reservoir.push(1, weight).is_err(),
                "a weight of {weight} was accepted"
            );
        }
        assert!(reservoir.is_empty(), "a refused push retained a row");

        for index in 0..1_000_000_u64 {
            reservoir.push(index, 1.0)?;
        }
        assert_eq!(
            reservoir.len(),
            256,
            "a million items left the reservoir holding {} rows",
            reservoir.len()
        );
        assert_eq!(reservoir.seen(), 1_000_000);
        // The declared bound is stated against the capacity, so it does not
        // narrow as the stream lengthens.
        assert!((reservoir.sampling_error() - 0.5 / 16.0).abs() < 1e-12);
        Ok(())
    }

    /// Two reservoirs given the same stream and the same seed hold the same
    /// rows in the same order, and one given a different seed does not.
    ///
    /// The property that makes a sample which fed a fit reproducible from the
    /// log. The second half is the premise: without it the test would pass on
    /// an implementation that ignored the seed entirely and kept the first
    /// `k` rows.
    ///
    /// Mutated by replacing `self.seed ^ ...` with a constant in
    /// `next_uniform` — confirmed the differing-seed assertion then fails,
    /// then restored.
    #[test]
    fn two_reservoirs_with_one_seed_draw_one_sample_and_two_seeds_do_not() -> Result<()> {
        let draw = |seed: u64| -> Result<Vec<u64>> {
            let mut reservoir: Reservoir<u64> = Reservoir::new(64, seed)?;
            for index in 0..5_000_u64 {
                reservoir.push(index, 1.0 + (index % 7) as f64)?;
            }
            Ok(reservoir.samples().copied().collect())
        };
        let first = draw(42)?;
        let second = draw(42)?;
        let other = draw(43)?;
        assert_eq!(first.len(), 64, "premise: the reservoir filled");
        assert_eq!(first, second, "one seed produced two samples");
        assert_ne!(
            first, other,
            "two seeds produced one sample; the draw is not reading the seed"
        );
        Ok(())
    }
}
