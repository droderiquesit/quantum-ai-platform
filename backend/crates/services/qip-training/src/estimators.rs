//! Sufficient statistics for a feature, held in memory that does not grow
//! with the stream (§21.1, §22.2).
//!
//! §21.1 is titled "Training Without an Archive" and its first row is the
//! whole argument: "Welford moments, exponentially weighted covariance,
//! t-digest quantiles, count-min, HyperLogLog. Maintained in the node in
//! constant memory", against terabytes a year of raw capture. This module is
//! where a feature's summary lives, and it holds no sample of the stream
//! except the fixed-size one §21.1 asks for by name.
//!
//! # Which budget this is measured against
//!
//! §21.1 puts "~320 KB for 200 features" beside that row and it is easy to
//! read as the budget for all five estimators together. It is not, and §22.2
//! says which one it is: the covariance row, "O(k²) — 200 features is ~320
//! KB", which is `200² × 8` bytes to the byte. Quantiles are budgeted on the
//! line below it, at "few KB per distribution", and cardinality at
//! "kilobytes". [`FeatureEstimators::standard`] costs about 7 KB a feature and
//! is measured against *those* lines. A first draft of this module was sized
//! to fit two hundred features inside 320 KB, which forced a compression so
//! coarse that [`StreamingDrift::floor`] exceeded every index it could
//! compute — a drift check that refuses every comparison, arrived at by
//! reading the wrong row of the blueprint.
//!
//! # Why this exists beside `qip_ai::evaluation::DriftReport`
//!
//! [`qip_ai::evaluation::DriftReport::compare`] computes the same population
//! stability index this module does, and it is not being replaced: it is the
//! right tool where both samples are in hand. What it cannot do is the thing
//! §21.1 is about — it takes `&[f64]` twice, so a caller measuring drift
//! against a training window has to *retain that window*, which is the
//! archive. `qip-deepbrain`'s learning round does exactly that today, keeping
//! a `BTreeMap<String, Vec<f64>>` of every feature column each registered
//! model was fitted on, for as long as the model is registered.
//!
//! [`FeatureEstimators`] is the same information in a few kilobytes, and
//! [`StreamingDrift::compare`] reads the index off two digests with no sample
//! behind either. It also carries something the batch version cannot:
//!
//! # The bound, and the refusal that makes it one
//!
//! Every share in the index comes from a [`TDigest::rank_of`] call, so every
//! share carries that digest's declared rank error. [`StreamingDrift::floor`]
//! is the index those errors could manufacture on two *identical*
//! distributions, and [`StreamingDrift::is_material`] refuses to call a shift
//! a finding unless it exceeds that. A drift number without such a floor is a
//! number that always looks like something, because two estimates of one
//! distribution never agree exactly; §21.1's "every estimator declares an
//! error bound, and one that drifts past it marks every model depending on it
//! as degraded" needs both halves, and the floor is the half that stops the
//! second one firing on estimator noise.
//!
//! Deterministic throughout, like everything it is built on: the same stream
//! in the same order produces the same summary and the same index on every
//! machine, so a degradation recorded against a model can be recomputed from
//! the log rather than merely believed.

use qip_core::error::{Error, Result};
use qip_numerics::stats::RunningStats;
use qip_numerics::streaming::{Compression, HyperLogLog, Precision, Reservoir, TDigest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most features one estimator set may summarise.
///
/// §21.1 sizes the node's whole budget at two hundred features; the ceiling
/// sits just above that so a caller feeding a wider frame is refused rather
/// than silently spending several times the memory the blueprint budgets. The
/// refusal is at the first observation of the feature that would be the
/// 257th, which is the only moment the set can tell.
pub const MAX_FEATURES: usize = 256;

/// The buckets a drift index is computed over.
///
/// Ten, because the reference shares are then a tenth each and a single
/// bucket emptying moves the index by about 0.23 — just under the 0.25 the
/// platform elsewhere treats as a material shift. Fewer buckets and a real
/// shift hides inside one; more and each bucket's share approaches the
/// digest's own rank error, which is when [`StreamingDrift::floor`] starts
/// refusing every comparison.
pub const DRIFT_BUCKETS: usize = 10;

/// How much of a feature's stream must be distinct before its distribution is
/// worth summarising at all: one value in fifty.
///
/// A price series that reports three values across five hundred observations
/// is a stuck feed, not a distribution, and every quantile, moment and drift
/// index computed from it is a statistic about a fault. One in fifty is
/// deliberately far below anything a live instrument produces — a genuinely
/// coarse tick grid still moves far more than that over a window — so this
/// fires on the fault and not on a quiet market.
pub const DISTINCT_RATIO_FLOOR: f64 = 0.02;

/// A feature's sufficient statistics, and nothing else.
///
/// Four estimators, each declaring its own error: Welford's moments,
/// quantiles, distinct values, and the fixed-size weighted sample §21.1 calls
/// a "representative sample". No field holds anything that grows with the
/// stream.
#[derive(Clone, Debug)]
pub struct FeatureSummary {
    moments: RunningStats,
    digest: TDigest,
    distinct: HyperLogLog,
    sample: Reservoir<f64>,
    observations: u64,
}

impl FeatureSummary {
    fn new(compression: Compression, precision: Precision, sample: Reservoir<f64>) -> Self {
        Self {
            moments: RunningStats::new(),
            digest: TDigest::new(compression),
            distinct: HyperLogLog::new(precision),
            sample,
            observations: 0,
        }
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    pub fn mean(&self) -> f64 {
        self.moments.mean()
    }

    pub fn standard_deviation(&self) -> f64 {
        self.moments.stddev()
    }

    pub fn minimum(&self) -> f64 {
        self.moments.min()
    }

    pub fn maximum(&self) -> f64 {
        self.moments.max()
    }

    /// The estimated number of distinct values seen, with the HyperLogLog's
    /// declared relative standard error.
    pub fn distinct_values(&self) -> f64 {
        self.distinct.estimate()
    }

    /// Whether the feature has moved enough to be a distribution.
    ///
    /// The one place the cardinality estimator decides something. A summary
    /// that answers `false` here is reporting a stuck or quantised feed, and
    /// its quantiles describe the fault rather than the market — see
    /// [`DISTINCT_RATIO_FLOOR`].
    ///
    /// `true` below thirty observations, because a short stream has not had
    /// the chance to be varied and calling it stuck would fire on every
    /// feature every time a process restarts.
    pub fn has_moved(&self) -> bool {
        if self.observations < 30 {
            return true;
        }
        // u64 → f64 at the statistics boundary: a ratio of counts, and the
        // cardinality estimate is already a statistic.
        self.distinct.estimate() / self.observations as f64 >= DISTINCT_RATIO_FLOOR
    }

    /// The fixed-size recency-weighted sample, ascending by retention key.
    pub fn sample(&self) -> impl Iterator<Item = &f64> {
        self.sample.samples()
    }

    pub fn sample_rows(&self) -> usize {
        self.sample.len()
    }

    /// The standard error a proportion read off the sample carries.
    pub fn sample_error(&self) -> f64 {
        self.sample.sampling_error()
    }

    /// The memory this summary occupies, which does not move with the stream.
    pub fn bytes(&self) -> usize {
        self.digest.bytes()
            + self.distinct.bytes()
            + self.sample.capacity() * std::mem::size_of::<f64>()
            + std::mem::size_of::<RunningStats>()
    }

    /// The value at rank `q`, within the digest's declared rank error.
    pub fn quantile(&mut self, q: f64) -> Result<f64> {
        self.digest.quantile(q)
    }

    /// One line for a journal: what the estimators say and how far each may
    /// be wrong.
    pub fn describe(&mut self, name: &str) -> String {
        let median = self
            .digest
            .quantile(0.5)
            .map_or_else(|_| "unknown".to_string(), |value| format!("{value:.6}"));
        format!(
            "{name}: {} observation(s), mean {:.6}, sd {:.6}, median {median}, ~{:.0} distinct \
             value(s), {} sampled row(s), {} byte(s)",
            self.observations,
            self.mean(),
            self.standard_deviation(),
            self.distinct_values(),
            self.sample_rows(),
            self.bytes()
        )
    }
}

/// Every feature's summary, in name order, under a declared memory ceiling.
///
/// Built from one compression, one precision and one sample size so that two
/// sets can be compared: [`StreamingDrift::compare`] reads both sides' rank
/// errors, and two sets with different compressions would be two answers to
/// "how wrong may this share be".
#[derive(Clone, Debug)]
pub struct FeatureEstimators {
    compression: Compression,
    precision: Precision,
    sample_rows: usize,
    seed: u64,
    features: BTreeMap<String, FeatureSummary>,
}

impl FeatureEstimators {
    /// A set whose estimators are built from `compression`, `precision` and a
    /// `sample_rows`-row reservoir seeded from `seed`.
    ///
    /// Fallible only through the reservoir, whose capacity is the one
    /// parameter that is not already a validated type.
    pub fn new(
        compression: Compression,
        precision: Precision,
        sample_rows: usize,
        seed: u64,
    ) -> Result<Self> {
        // Built and dropped so an impossible capacity is refused here rather
        // than at the first observation of the first feature, which could be
        // an hour into a run.
        let _probe: Reservoir<f64> = Reservoir::new(sample_rows, seed)?;
        Ok(Self {
            compression,
            precision,
            sample_rows,
            seed,
            features: BTreeMap::new(),
        })
    }

    /// The set §22.2's quantile and cardinality budgets describe: compression
    /// 100, precision 8, a 64-row sample.
    ///
    /// About 7.2 KB a feature — 6.4 KB of digest, 256 bytes of registers, 512
    /// bytes of sample — so two hundred features cost 1.45 MB against the two
    /// million observations' 16 MB of raw `f64`s, at a hundred observations
    /// each. §22.2 budgets "few KB per distribution" for quantiles and
    /// "kilobytes" for cardinality, and 7.2 KB is both.
    ///
    /// **The compression was chosen by the floor, not by the memory.** At
    /// compression 20 the set costs 2 KB a feature and
    /// [`StreamingDrift::floor`] comes out at 4.5 — above any index a
    /// ten-bucket comparison produces — so every drift comparison would be
    /// refused as unexplainable by the estimators. At 100 the floor is around
    /// 0.3, which a one-standard-deviation shift clears by an order of
    /// magnitude and an independent redraw of the same distribution does not.
    /// The other two figures follow the same rule: precision 8 states a 6.5%
    /// standard error on a cardinality, which is ample for the
    /// [`FeatureSummary::has_moved`] question and for nothing finer, and a
    /// 64-row sample reports a proportion to 6 points.
    ///
    /// These are summary statistics for deciding whether a distribution has
    /// moved, not figures anything sizes a position from, and
    /// [`FeatureEstimators::new`] is there for a caller that needs better and
    /// can afford it.
    pub fn standard(seed: u64) -> Result<Self> {
        Self::new(Compression::new(100.0)?, Precision::new(8)?, 64, seed)
    }

    /// Absorb one observation of `feature`.
    ///
    /// Refuses the observation that would create the `MAX_FEATURES + 1`-th
    /// feature, naming the ceiling: a set that silently grew past it would
    /// spend several times the memory §21.1 budgets, and the caller that fed
    /// a wider frame than the node is sized for is the thing to fix.
    ///
    /// Refuses a non-finite value, through the digest: one NaN centroid makes
    /// every quantile read from the feature afterwards a NaN and the summary
    /// keeps no record of which observation did it.
    ///
    /// The reservoir weight is the observation's own index, so the `k`-th
    /// arrival is `k` times as likely to be retained as the first. That is
    /// §21.1's "recency-weighted sample" as arithmetic rather than as a
    /// setting, and it is a polynomial weighting rather than an exponential
    /// one on purpose: `exp(age / half_life)` overflows to infinity on a
    /// stream that runs for a few thousand half-lives, and a weight of
    /// infinity makes every key exactly one and the sample the first `k` rows
    /// for ever.
    pub fn observe(&mut self, feature: &str, value: f64) -> Result<()> {
        if !self.features.contains_key(feature) && self.features.len() >= MAX_FEATURES {
            return Err(Error::invalid(format!(
                "a feature set already holds {MAX_FEATURES} feature(s) and {feature:?} would be \
                 another; §21.1 sizes a node's whole budget at two hundred features, so a set \
                 that grew past the ceiling would spend several times the memory the blueprint \
                 budgets and would do it without anybody choosing to"
            )));
        }
        if !value.is_finite() {
            return Err(Error::invalid(format!(
                "{feature:?} was observed at {value}; a non-finite observation makes every \
                 quantile read from the feature afterwards non-finite and the summary keeps no \
                 record of which observation did it"
            )));
        }
        let compression = self.compression;
        let precision = self.precision;
        let sample_rows = self.sample_rows;
        // The seed is mixed with the feature name so two features in one set
        // do not draw the same sequence, which would make their samples move
        // together and any comparison between them a comparison of one draw.
        let seed = self.seed ^ name_seed(feature);
        let summary = match self.features.get_mut(feature) {
            Some(held) => held,
            None => {
                let sample = Reservoir::new(sample_rows, seed)?;
                self.features
                    .entry(feature.to_string())
                    .or_insert_with(|| FeatureSummary::new(compression, precision, sample))
            }
        };
        summary.observations += 1;
        summary.moments.push(value);
        summary.digest.add(value)?;
        // The cardinality estimator counts the value's own bytes, so two
        // observations are distinct exactly when their `f64` bits differ.
        // Formatting would make 0.1 and 0.10000000000000001 one value, which
        // is the opposite of what a stuck-feed check is asking.
        summary.distinct.add_bytes(&value.to_bits().to_be_bytes());
        // u64 → f64 at the statistics boundary: the weight is a count, and a
        // stream long enough for the conversion to lose precision has long
        // since made the weighting irrelevant.
        summary.sample.push(value, summary.observations as f64)?;
        Ok(())
    }

    pub fn features(&self) -> impl Iterator<Item = (&String, &FeatureSummary)> {
        self.features.iter()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.features.keys()
    }

    pub fn get(&self, feature: &str) -> Option<&FeatureSummary> {
        self.features.get(feature)
    }

    pub fn get_mut(&mut self, feature: &str) -> Option<&mut FeatureSummary> {
        self.features.get_mut(feature)
    }

    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// The memory the whole set occupies — the number §21.1's "~320 KB for
    /// 200 features" has to be.
    pub fn bytes(&self) -> usize {
        self.features
            .values()
            .map(FeatureSummary::bytes)
            .sum::<usize>()
    }

    /// Features this set holds that `reference` does not, and the reverse.
    ///
    /// Reported rather than ignored: a model fitted on a feature the live
    /// stream has stopped producing is extrapolating into a column of
    /// nothing, and a comparison that silently skipped the missing feature
    /// would report calm.
    pub fn unmatched(&self, reference: &Self) -> BTreeSet<String> {
        self.features
            .keys()
            .filter(|name| !reference.features.contains_key(*name))
            .chain(
                reference
                    .features
                    .keys()
                    .filter(|name| !self.features.contains_key(*name)),
            )
            .cloned()
            .collect()
    }

    /// The drift of every feature in this set against the same feature in
    /// `reference`, in name order.
    ///
    /// Features present on only one side are absent from the result and are
    /// [`Self::unmatched`]'s business.
    pub fn drift_against(
        &mut self,
        reference: &mut Self,
        buckets: usize,
    ) -> Result<BTreeMap<String, StreamingDrift>> {
        let mut out = BTreeMap::new();
        let names: Vec<String> = self.features.keys().cloned().collect();
        for name in names {
            let (Some(current), Some(prior)) = (
                self.features.get_mut(&name),
                reference.features.get_mut(&name),
            ) else {
                continue;
            };
            let drift = StreamingDrift::compare(&mut prior.digest, &mut current.digest, buckets)?;
            out.insert(name, drift);
        }
        Ok(out)
    }
}

/// A per-feature seed derived from the name, so two features in one set draw
/// independent sequences.
///
/// FNV-1a over the bytes, written out rather than borrowed from
/// `qip_numerics::sketch`, whose copy is `pub(crate)` to that crate. Five
/// lines is the cheaper of the two prices; the alternative is widening a
/// hash's visibility across a crate boundary so a seed can be derived from a
/// string, which makes a sketch's internals part of another crate's contract.
fn name_seed(name: &str) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// How far one feature's distribution has moved, read off two digests.
///
/// The same population stability index
/// [`qip_ai::evaluation::DriftReport`] reports, computed without either
/// sample in hand — and carrying the floor the batch version has no way to
/// state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamingDrift {
    /// The index. Zero when the two distributions agree bucket for bucket.
    pub population_stability_index: f64,
    /// The most any one bucket's share may be wrong by, from the two digests'
    /// own declared rank errors.
    pub share_error: f64,
    /// The index those errors alone could manufacture between two identical
    /// distributions. A shift at or below this is not evidence of anything.
    pub floor: f64,
    pub buckets: usize,
    pub reference_observations: u64,
    pub current_observations: u64,
}

impl StreamingDrift {
    /// Compare `current` against `reference` over `buckets` equal-mass
    /// buckets of the reference distribution.
    ///
    /// Both digests are read, neither is changed except by the compression
    /// every read settles. Refuses fewer than two buckets — an index over one
    /// bucket is zero whatever the distributions are, which reads as "no
    /// drift" and is the most dangerous possible answer — and refuses a
    /// digest holding nothing, because a bucket edge taken from an empty
    /// reference is not a number.
    pub fn compare(reference: &mut TDigest, current: &mut TDigest, buckets: usize) -> Result<Self> {
        if buckets < 2 {
            return Err(Error::invalid(format!(
                "a drift index needs at least two buckets, not {buckets}; over one bucket every \
                 pair of distributions scores zero, which reads as 'nothing moved' rather than \
                 as 'nothing was measured'"
            )));
        }
        if reference.is_empty() || current.is_empty() {
            return Err(Error::invalid(format!(
                "a drift index needs observations on both sides and has {} against {}; a bucket \
                 edge taken from an empty digest is not a number and the index built on it would \
                 not be either",
                reference.count(),
                current.count()
            )));
        }
        // usize → f64 at the statistics boundary: a bucket count.
        let width = 1.0 / buckets as f64;
        // The reference's own equal-mass edges, so the comparison is against
        // where the reference data actually lived rather than an arbitrary
        // grid — the same discipline `DriftReport::compare` takes.
        let mut worst_reference_error = 0.0_f64;
        let mut worst_current_error = 0.0_f64;
        let mut previous_rank = 0.0_f64;
        let mut index = 0.0_f64;
        // A share of zero would make the logarithm infinite, so an empty
        // bucket is floored below any share a real bucket can hold.
        const FLOOR: f64 = 1e-6;
        for bucket in 1..=buckets {
            // usize → f64: a bucket ordinal.
            let upper = bucket as f64 * width;
            let edge = reference.quantile(upper.min(1.0))?;
            let rank = if bucket == buckets {
                1.0
            } else {
                current.rank_of(edge)?
            };
            let current_share = (rank - previous_rank).max(0.0);
            previous_rank = rank;
            let reference_share = width.max(FLOOR);
            let observed = current_share.max(FLOOR);
            index += (observed - reference_share) * (observed / reference_share).ln();
            worst_reference_error =
                worst_reference_error.max(reference.compression().rank_error(upper.min(1.0))?);
            worst_current_error =
                worst_current_error.max(current.compression().rank_error(upper.min(1.0))?);
        }
        // One error from each digest, and not two from each.
        //
        // A bucket's share is a difference of two edge ranks, so the arithmetic
        // invites four worst cases: two edges, two digests. Four is not the
        // bound to state, because the two edges' errors come from *one*
        // centroid layout and move together — a digest whose centroids sit
        // high at one edge sits high at the next, and the difference cancels
        // most of it. Taking one worst case from each side is the conservative
        // reading of a correlated pair, and stating four would put the floor
        // above the index on every configuration this platform can afford,
        // which is a control that refuses everything rather than one that
        // works. At compression 20 even this bound exceeds a tenth, which is
        // why `standard` is not built at compression 20 — the floor decided
        // the configuration rather than the other way round.
        let share_error = worst_reference_error + worst_current_error;
        Ok(Self {
            population_stability_index: index.max(0.0),
            share_error,
            floor: spurious_index(buckets, share_error),
            buckets,
            reference_observations: reference.count(),
            current_observations: current.count(),
        })
    }

    /// Whether the shift is larger than the estimators' own error can
    /// explain.
    ///
    /// The whole reason this type carries a floor. Two digests of one
    /// distribution never agree exactly, so the index is never zero and a
    /// consumer comparing it against a fixed threshold is comparing signal
    /// plus estimator noise against a number chosen without knowing how much
    /// noise there was. Strictly greater, so a shift landing exactly on the
    /// floor is refused: the floor is what the error alone produces, and
    /// matching it is not exceeding it.
    pub fn is_material(&self) -> bool {
        self.population_stability_index > self.floor
    }

    /// One line for a journal.
    pub fn describe(&self, feature: &str) -> String {
        format!(
            "{feature}: stability index {:.4} against a floor of {:.4} from the estimators' own \
             error ({} bucket(s), {} reference against {} current observation(s)) — {}",
            self.population_stability_index,
            self.floor,
            self.buckets,
            self.reference_observations,
            self.current_observations,
            if self.is_material() {
                "the shift is larger than the estimators can explain"
            } else {
                "within what the estimators' error alone produces"
            }
        )
    }
}

/// The largest index two identical distributions could show when every
/// bucket's share may be off by `share_error`.
///
/// Each bucket contributes `(c − r)·ln(c/r)` with `r = 1/buckets`. The
/// contribution is largest when `c` is as far from `r` as the error allows,
/// and the logarithm is larger on the high side than the low, so `r + e` is
/// the worst case and the sum over `buckets` of it is the bound. Not a
/// tight bound and not claimed to be one: it is an upper bound on what the
/// estimators alone can manufacture, and a bound that is loose in the safe
/// direction refuses some real shifts, where a tight one that is wrong admits
/// noise as evidence.
fn spurious_index(buckets: usize, share_error: f64) -> f64 {
    if share_error <= 0.0 {
        return 0.0;
    }
    // usize → f64 at the statistics boundary: a bucket count.
    let width = 1.0 / buckets as f64;
    let high = width + share_error;
    buckets as f64 * (high - width) * (high / width).ln()
}

/// Models that depend on a feature whose statistics have drifted, and which
/// of their features did.
///
/// §21.1's "one that drifts past it marks every model depending on it as
/// degraded", as a function over the dependency the platform already records:
/// `qip_ai::registry::ModelCard::features` is the list of feature names a
/// model consumes, and this is the join.
///
/// Takes the dependency as `(model reference, feature names)` pairs rather
/// than a `ModelCard` so this crate does not acquire a dependency on the
/// registry: `qip-training` fits models and measures them, and a crate that
/// both trains a model and decides its standing in the registry grades its
/// own homework. The caller holding the registry does the join's other half.
///
/// A model naming no drifted feature is absent from the result rather than
/// present with an empty set, so a caller iterating the map is iterating the
/// degraded models and not every model the platform holds.
pub fn degraded_models<'a>(
    drifted: &BTreeSet<String>,
    dependencies: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (reference, features) in dependencies {
        let hit: BTreeSet<String> = features
            .iter()
            .filter(|feature| drifted.contains(*feature))
            .cloned()
            .collect();
        if !hit.is_empty() {
            out.insert(reference.to_string(), hit);
        }
    }
    out
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` still has to assert, and the abort is the
// reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// A deterministic standard-normal-ish stream: the sum of twelve uniforms
    /// less six, which is the Irwin-Hall approximation. Not a general-purpose
    /// generator and not used outside tests.
    fn normals(count: usize, mean: f64, seed: u64) -> Vec<f64> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                let mut sum = 0.0;
                for _ in 0..12 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    sum += ((state >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0;
                }
                sum - 6.0 + mean
            })
            .collect()
    }

    /// The memory claim §21.1 rests on, as a number: two hundred features and
    /// ten thousand observations of each cost what two hundred *empty*
    /// summaries cost, and each is inside §22.2's "few KB per distribution".
    ///
    /// The two-million observations are the point. A `BTreeMap<String,
    /// Vec<f64>>` holding the same stream — which is what
    /// `qip_ai::evaluation::DriftReport::compare` requires of its caller, and
    /// what `qip-deepbrain` keeps per registered model — is sixteen megabytes,
    /// eleven times what this holds, and it grows with every further
    /// observation where this does not.
    ///
    /// The per-feature assertion is the one that matters and is stated
    /// separately from the total on purpose: a total that passed while one
    /// feature held everything would still be an archive.
    ///
    /// Mutated by adding `observations * 8` to `FeatureSummary::bytes()`,
    /// which is what a summary retaining its stream would cost — confirmed the
    /// per-feature assertion then fails at 87,240 bytes against the 8,192
    /// ceiling, then restored byte-for-byte.
    #[test]
    fn two_hundred_features_summarise_two_million_observations_in_the_blueprint_s_budget()
    -> Result<()> {
        let mut estimators = FeatureEstimators::standard(7)?;
        let values = normals(10_000, 0.0, 0x5EED);
        for feature in 0..200 {
            let name = format!("feature-{feature:03}");
            for value in &values {
                estimators.observe(&name, *value)?;
            }
        }
        // Premise: the stream really was two million observations, so the
        // assertion below is about the summary and not about a short run.
        assert_eq!(estimators.len(), 200);
        let total: u64 = estimators
            .features()
            .map(|(_, summary)| summary.observations())
            .sum();
        assert_eq!(total, 2_000_000);

        // "Few KB per distribution", per feature, and the total below it.
        for (name, summary) in estimators.features() {
            assert!(
                summary.bytes() <= 8_192,
                "{name} cost {} bytes against §22.2's few-KB-per-distribution budget",
                summary.bytes()
            );
        }
        let bytes = estimators.bytes();
        let raw = 2_000_000 * std::mem::size_of::<f64>();
        assert!(
            bytes < 2_000_000 && bytes * 10 < raw,
            "two hundred features cost {bytes} bytes against §22.2's few-KB-per-distribution \
             budget; the raw stream behind them is {raw} bytes"
        );
        // And it is genuinely constant: ten thousand more observations of one
        // feature move nothing.
        let before = estimators
            .get("feature-000")
            .map(FeatureSummary::bytes)
            .unwrap_or_default();
        assert!(before > 0, "premise: a summary occupies something");
        for value in &values {
            estimators.observe("feature-000", *value)?;
        }
        assert_eq!(
            estimators
                .get("feature-000")
                .map(FeatureSummary::bytes)
                .unwrap_or_default(),
            before,
            "ten thousand further observations moved the summary's memory"
        );
        Ok(())
    }

    /// The ceiling fires, and it fires on the feature that would exceed it
    /// rather than on the observation count.
    ///
    /// Mutated by removing the `features.len() >= MAX_FEATURES` check —
    /// confirmed the refusal assertion then fails, then restored.
    #[test]
    fn a_feature_set_refuses_the_feature_that_would_take_it_past_its_ceiling() -> Result<()> {
        let mut estimators = FeatureEstimators::standard(1)?;
        for feature in 0..MAX_FEATURES {
            estimators.observe(&format!("feature-{feature:04}"), 1.0)?;
        }
        // Premise: the set is exactly full, and a further observation of a
        // feature it already holds is still accepted — the ceiling is on
        // features and not on observations.
        assert_eq!(estimators.len(), MAX_FEATURES);
        estimators.observe("feature-0000", 2.0)?;

        let error = estimators
            .observe("one-too-many", 1.0)
            .expect_err("a set already holding its ceiling accepted another feature");
        assert_eq!(error.code(), "invalid", "got {error:?}");
        assert!(
            error.message().contains(&MAX_FEATURES.to_string()),
            "the refusal does not name the ceiling: {error}"
        );
        assert_eq!(
            estimators.len(),
            MAX_FEATURES,
            "the refused feature was created"
        );
        Ok(())
    }

    /// The finding: a feature whose distribution has moved by a whole
    /// standard deviation is material, and the same feature measured against
    /// an independent draw of its own distribution is not.
    ///
    /// Both halves are load-bearing. Without the second, a drift check that
    /// returned "material" unconditionally would pass — and that is not
    /// hypothetical, because an index computed from two estimates of one
    /// distribution is never exactly zero, so the naive version of this check
    /// fires on every feature every time.
    ///
    /// Mutated by comparing the index against zero instead of against the
    /// floor in `is_material`, which removes the floor — confirmed it then
    /// reports `an independent draw of the same distribution was called a
    /// finding: return_1: stability index 0.0147 against a floor of 0.3063`,
    /// then restored byte-for-byte. Mutated again by having `compare` set a
    /// zero index unconditionally — confirmed it then reports `a
    /// one-standard-deviation shift was not a finding: ... stability index
    /// 0.0000`, then restored byte-for-byte.
    #[test]
    fn a_feature_that_has_shifted_a_standard_deviation_is_material_and_a_redrawn_one_is_not()
    -> Result<()> {
        let mut reference = FeatureEstimators::standard(11)?;
        let mut redrawn = FeatureEstimators::standard(12)?;
        let mut shifted = FeatureEstimators::standard(13)?;
        for value in normals(5_000, 0.0, 0xA11CE) {
            reference.observe("return_1", value)?;
        }
        // An independent draw of the same distribution, and a draw of one
        // whose mean has moved by a standard deviation.
        for value in normals(5_000, 0.0, 0xB0B) {
            redrawn.observe("return_1", value)?;
        }
        for value in normals(5_000, 1.0, 0xB0B) {
            shifted.observe("return_1", value)?;
        }
        // Premise: the two current sets really do differ in mean by about one
        // standard deviation, so the assertions below are about the detector
        // and not about the data.
        let (stable_mean, moved_mean) = (
            redrawn
                .get("return_1")
                .map(FeatureSummary::mean)
                .unwrap_or_default(),
            shifted
                .get("return_1")
                .map(FeatureSummary::mean)
                .unwrap_or_default(),
        );
        assert!(
            (moved_mean - stable_mean - 1.0).abs() < 0.1,
            "premise: the shifted stream's mean is {moved_mean} against {stable_mean}"
        );

        let stable = redrawn.drift_against(&mut reference, DRIFT_BUCKETS)?;
        let moved = shifted.drift_against(&mut reference, DRIFT_BUCKETS)?;
        let stable = stable
            .get("return_1")
            .copied()
            .ok_or_else(|| Error::invalid("no drift computed for the stable draw".to_string()))?;
        let moved = moved
            .get("return_1")
            .copied()
            .ok_or_else(|| Error::invalid("no drift computed for the shifted draw".to_string()))?;

        assert!(
            !stable.is_material(),
            "an independent draw of the same distribution was called a finding: {}",
            stable.describe("return_1")
        );
        assert!(
            moved.is_material(),
            "a one-standard-deviation shift was not a finding: {}",
            moved.describe("return_1")
        );
        assert!(
            moved.population_stability_index > stable.population_stability_index,
            "the shifted draw scored {} against the stable draw's {}",
            moved.population_stability_index,
            stable.population_stability_index
        );
        assert!(stable.floor > 0.0, "the floor is not a floor at zero");
        Ok(())
    }

    /// The refusals on the comparison: one bucket, and a digest holding
    /// nothing on either side.
    ///
    /// Mutated by removing the `buckets < 2` check — confirmed the one-bucket
    /// assertion then fails and `compare` returns an index of zero, which is
    /// the reading the refusal exists to prevent, then restored.
    #[test]
    fn a_drift_comparison_refuses_one_bucket_and_an_empty_digest() -> Result<()> {
        let compression = Compression::new(50.0)?;
        let mut reference = TDigest::new(compression);
        let mut current = TDigest::new(compression);
        // Premise: with both sides populated and two buckets the comparison
        // succeeds, so the refusals below are about the edges.
        for value in normals(500, 0.0, 3) {
            reference.add(value)?;
        }
        for value in normals(500, 0.0, 4) {
            current.add(value)?;
        }
        assert!(StreamingDrift::compare(&mut reference, &mut current, 2).is_ok());

        for buckets in [0, 1] {
            let error = StreamingDrift::compare(&mut reference, &mut current, buckets)
                .expect_err(&format!("{buckets} bucket(s) were accepted"));
            assert_eq!(error.code(), "invalid", "got {error:?}");
        }
        let mut empty = TDigest::new(compression);
        assert!(
            StreamingDrift::compare(&mut empty, &mut current, DRIFT_BUCKETS).is_err(),
            "an empty reference was accepted; its bucket edges are not numbers"
        );
        assert!(
            StreamingDrift::compare(&mut reference, &mut empty, DRIFT_BUCKETS).is_err(),
            "an empty current side was accepted"
        );
        Ok(())
    }

    /// The cardinality estimator decides something: a stuck feed is not a
    /// distribution, and a live one is.
    ///
    /// Mutated by making `has_moved` return `true` unconditionally —
    /// confirmed the stuck half then fails, then restored.
    #[test]
    fn a_feed_reporting_three_values_across_five_hundred_observations_has_not_moved() -> Result<()>
    {
        let mut estimators = FeatureEstimators::standard(5)?;
        for index in 0..500 {
            // usize → f64: three repeating values.
            estimators.observe("stuck", (index % 3) as f64)?;
        }
        for value in normals(500, 0.0, 6) {
            estimators.observe("live", value)?;
        }
        let stuck = estimators
            .get("stuck")
            .ok_or_else(|| Error::invalid("the stuck feature is absent".to_string()))?;
        let live = estimators
            .get("live")
            .ok_or_else(|| Error::invalid("the live feature is absent".to_string()))?;
        // Premise: both sides saw the same number of observations, so the
        // difference below is cardinality and not sample size.
        assert_eq!(stuck.observations(), live.observations());
        assert!(
            (stuck.distinct_values() - 3.0).abs() < 1.0,
            "the stuck feed's cardinality estimate is {}",
            stuck.distinct_values()
        );
        assert!(
            !stuck.has_moved(),
            "a feed of three values across five hundred observations was called a distribution"
        );
        assert!(
            live.has_moved(),
            "a live feed of five hundred draws was called stuck; its cardinality estimate is {}",
            live.distinct_values()
        );
        Ok(())
    }

    /// The join §21.1 asks for: a model that names a drifted feature is
    /// degraded and one that does not is untouched.
    ///
    /// Mutated by having `degraded_models` insert every model rather than
    /// only those with a hit — confirmed the untouched-model assertion then
    /// fails, then restored.
    #[test]
    fn a_model_naming_a_drifted_feature_is_degraded_and_one_naming_none_is_not() {
        let drifted: BTreeSet<String> = ["volatility_10".to_string()].into_iter().collect();
        let regime: Vec<String> = vec!["return_1".to_string(), "volatility_10".to_string()];
        let momentum: Vec<String> = vec!["return_1".to_string(), "momentum_5".to_string()];
        // Premise: both models name features, and only one of them names the
        // drifted one — without this the test would pass on an empty input.
        assert!(regime.contains(&"volatility_10".to_string()));
        assert!(!momentum.contains(&"volatility_10".to_string()));

        let degraded = degraded_models(
            &drifted,
            [
                ("regime@1.0.0", regime.as_slice()),
                ("momentum@2.0.0", momentum.as_slice()),
            ],
        );
        assert_eq!(degraded.len(), 1, "got {degraded:?}");
        assert_eq!(
            degraded.get("regime@1.0.0").map(BTreeSet::len),
            Some(1),
            "the degraded model does not name the feature that degraded it: {degraded:?}"
        );
        assert!(
            !degraded.contains_key("momentum@2.0.0"),
            "a model naming no drifted feature was marked degraded"
        );
    }
}
