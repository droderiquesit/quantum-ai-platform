//! Blueprint §21.1 and §22.2's streaming half, over the series the platform
//! already senses: what the node would keep if it kept no archive, and what
//! that summary says has changed.
//!
//! §22.2's table has seven rows. Two were reached before this module — the
//! count-min sketch for "frequency and cardinality", and a batch covariance
//! standing in for the exponentially weighted one. This module reaches three
//! more, over live data rather than in a test: **quantiles** from a t-digest,
//! **cardinality** from a HyperLogLog, and **representative samples** from a
//! weighted reservoir, one set per instrument the SENSE stage has priced. It
//! also gives [`qip_numerics::stats::RunningStats`] — Welford's accumulator,
//! in the tree since long before this and with no caller anywhere — its first.
//!
//! # What it produces, and what makes it a finding rather than a number
//!
//! Two things, and both fire on data a running platform actually holds.
//!
//! * **A distribution shift.** The retained return series is split in two: the
//!   older half is the reference, the recent half is the current, and
//!   [`qip_training::estimators::StreamingDrift`] reads the population
//!   stability index off two digests. The finding is raised only where the
//!   index exceeds [`qip_training::estimators::StreamingDrift::floor`] — the
//!   index the two digests' own declared rank errors could manufacture between
//!   two draws of *one* distribution. Without that floor the index is never
//!   zero and every instrument reads as drifting every cycle, which is a
//!   control that fires always and therefore says nothing.
//! * **A feed that has stopped being a distribution.** The cardinality
//!   estimator answers how many distinct values the series holds; a series
//!   reporting a handful across hundreds of observations is a stuck or
//!   quantised feed, and every quantile, moment and drift index computed from
//!   it describes the fault rather than the market.
//!
//! # The honest limit on the memory claim
//!
//! §21.1's argument is that a node keeps sufficient statistics instead of an
//! archive. This module does not yet let the platform stop keeping the series:
//! it *reads* `Platform::price_history`, which is already bounded at
//! `SERIES_HISTORY` and is read by the detectors, the simulate stage and the
//! covariance estimate. What it demonstrates is the other half — that the
//! summary is a few kilobytes, that it answers the questions the series is
//! kept for, and that it carries an error bound the series cannot. The
//! estimators are rebuilt each cycle from the bounded window rather than
//! maintained across cycles, because a `Platform` field is the one thing this
//! lane may not add; a set maintained incrementally would be strictly cheaper
//! and strictly more faithful to §21.1, and it is the next step rather than
//! this one.
//!
//! Say that plainly rather than reporting "constant memory achieved": the
//! measured figure this module produces is the size of the summary, and the
//! claim it supports is that the summary is small enough to replace the
//! window — not that anything has yet replaced it.

use qip_core::error::Result;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_training::estimators::{DRIFT_BUCKETS, FeatureEstimators, FeatureSummary, StreamingDrift};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Returns an instrument needs before its distribution is split and compared.
///
/// Two hundred, so each half of the comparison holds a hundred. Below that the
/// two digests are describing samples small enough that a single unusual day
/// moves a decile, and the index would report a shift in the data rather than
/// in the distribution behind it. The platform's own series bound is 512
/// prices, so this admits any instrument that has been priced for about two
/// fifths of a full window.
pub const FEATURE_STATISTICS_MIN_RETURNS: usize = 200;

/// The seed the per-instrument reservoirs draw from.
///
/// Fixed rather than taken from a clock, and that is the whole reason it is a
/// named constant: §21.1's representative sample has to be reproducible from
/// the log, and a sample drawn from an entropy source nobody recorded is a fit
/// nobody can re-run. Two replays of one cycle draw the same rows.
pub const FEATURE_STATISTICS_SEED: u64 = 0x5EED_F00D_1234_ABCD;

/// What one instrument's summary says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstrumentStatistics {
    pub object_id: String,
    /// Returns summarised — the whole retained window, both halves.
    pub returns: u64,
    pub mean: f64,
    pub standard_deviation: f64,
    /// The median, from the digest rather than from a sort.
    pub median: f64,
    /// The fifth and ninety-fifth percentiles, likewise.
    pub lower_tail: f64,
    pub upper_tail: f64,
    /// Distinct return values the cardinality estimator found.
    pub distinct_values: f64,
    /// Whether the series is varied enough to be a distribution at all.
    pub has_moved: bool,
    /// Rows in the recency-weighted sample.
    pub sample_rows: usize,
    /// The bytes the whole summary costs — the figure §22.2's "few KB per
    /// distribution" has to be, measured rather than asserted.
    pub bytes: usize,
}

/// One instrument whose recent returns no longer look like its older ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistributionShift {
    pub object_id: String,
    pub drift: StreamingDrift,
}

/// The measurement, once a cycle.
///
/// Carries the findings inside it rather than as a second record, because a
/// shift is a property of an instrument this record already lists and two
/// records would be two places to look for one fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureStatisticsReview {
    /// One entry per instrument summarised, in identifier order.
    pub instruments: Vec<InstrumentStatistics>,
    /// Instruments the estimators could not be built for, and why. Recorded
    /// rather than dropped: an instrument silently missing from a review reads
    /// exactly like one that was fine.
    pub unmeasured: Vec<(String, String)>,
    /// Instruments whose recent returns have shifted past what the estimators'
    /// own error explains, in identifier order.
    pub shifted: Vec<DistributionShift>,
    /// Instruments whose feed has stopped being a distribution.
    pub quantised: Vec<String>,
    /// What every summary cost together.
    pub bytes: usize,
    pub cycle: u64,
    pub at: Timestamp,
}

impl EventBody for FeatureStatisticsReview {
    const TOPIC: Topic = Topic::FeatureComputed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("feature-statistics:{}", self.cycle))
    }
}

impl FeatureStatisticsReview {
    /// One line for the cycle's detail.
    ///
    /// Counts rather than names in the ordinary case, and names only the
    /// findings. The platform's universe is as wide as the desk configures it,
    /// so a line naming every instrument would be unbounded; the instruments
    /// that shifted are bounded by the ones that did, which is the number an
    /// operator is reading the line for.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "{} instrument(s) summarised into {} byte(s) of streaming estimators",
            self.instruments.len(),
            self.bytes
        );
        if !self.unmeasured.is_empty() {
            line.push_str(&format!(", {} not measured", self.unmeasured.len()));
        }
        if self.shifted.is_empty() {
            line.push_str("; no distribution moved past what the estimators' error explains");
        } else {
            let named: Vec<String> = self
                .shifted
                .iter()
                .map(|shift| {
                    format!(
                        "{} (index {:.3} against a floor of {:.3})",
                        shift.object_id, shift.drift.population_stability_index, shift.drift.floor
                    )
                })
                .collect();
            line.push_str(&format!(
                "; {} distribution(s) moved: {}",
                self.shifted.len(),
                named.join(", ")
            ));
        }
        if !self.quantised.is_empty() {
            line.push_str(&format!(
                "; {} feed(s) report too few distinct values to be a distribution: {}",
                self.quantised.len(),
                self.quantised.join(", ")
            ));
        }
        line
    }

    /// Whether anything was found. The caller journals either way — see
    /// [`measure`] — but a summary line is worth a cycle's detail only when
    /// there is something in it.
    pub fn has_findings(&self) -> bool {
        !self.shifted.is_empty() || !self.quantised.is_empty()
    }
}

/// Summarise every instrument's return distribution and say which have moved.
///
/// The entry point. `history` is the platform's own bounded price series per
/// instrument; everything else is derived here and nothing is retained.
///
/// Instruments with fewer than [`FEATURE_STATISTICS_MIN_RETURNS`] returns are
/// listed in `unmeasured` with the reason rather than silently skipped, and so
/// is any instrument whose series the estimators refused — a non-finite return
/// from a zero or negative price, most likely, which is a data fault worth
/// seeing rather than a row to drop.
///
/// Returns a review even when nothing was found and even when nothing could be
/// measured. The empty review is the point, for the reason `family_review`
/// gives about its zero gauge: a cycle that produced no record reads exactly
/// like a cycle where this never ran, and those mean opposite things.
pub fn measure(
    history: &BTreeMap<String, Vec<f64>>,
    cycle: u64,
    at: Timestamp,
) -> FeatureStatisticsReview {
    let mut instruments = Vec::new();
    let mut unmeasured = Vec::new();
    let mut shifted = Vec::new();
    let mut quantised = Vec::new();
    let mut bytes = 0usize;

    for (object_id, prices) in history {
        match summarise(object_id, prices) {
            Ok(Some(outcome)) => {
                bytes += outcome.statistics.bytes;
                if !outcome.statistics.has_moved {
                    quantised.push(object_id.clone());
                }
                if outcome.drift.is_material() {
                    shifted.push(DistributionShift {
                        object_id: object_id.clone(),
                        drift: outcome.drift,
                    });
                }
                instruments.push(outcome.statistics);
            }
            Ok(None) => unmeasured.push((
                object_id.clone(),
                format!(
                    "{} price(s) held, which is fewer than the {} returns a split comparison \
                     needs",
                    prices.len(),
                    FEATURE_STATISTICS_MIN_RETURNS
                ),
            )),
            Err(error) => unmeasured.push((object_id.clone(), error.message().to_string())),
        }
    }

    FeatureStatisticsReview {
        instruments,
        unmeasured,
        shifted,
        quantised,
        bytes,
        cycle,
        at,
    }
}

/// One instrument's summary and the shift between its two halves.
struct Summarised {
    statistics: InstrumentStatistics,
    drift: StreamingDrift,
}

/// Build one instrument's estimators, or say there is not enough series.
///
/// The split is *in time*, older half against recent half, and never a random
/// one: a randomly chosen reference for a time series is one whose neighbours
/// the current half already contains, and the comparison would report that no
/// distribution ever moves. The same discipline
/// `qip_training::dataset::TrainingDataset::split_at_fraction` takes, for the
/// same reason.
fn summarise(object_id: &str, prices: &[f64]) -> Result<Option<Summarised>> {
    let returns = qip_numerics::stats::simple_returns(prices);
    if returns.len() < FEATURE_STATISTICS_MIN_RETURNS {
        return Ok(None);
    }
    let split = returns.len() / 2;
    let (older, recent) = returns.split_at(split);

    // Three sets rather than two: the whole window is what the summary
    // describes, and the halves are what the comparison is between. Reusing
    // the whole-window set as one side would compare a distribution with a
    // superset of itself, which understates every shift by construction.
    let mut whole = FeatureEstimators::standard(FEATURE_STATISTICS_SEED)?;
    let mut reference = FeatureEstimators::standard(FEATURE_STATISTICS_SEED)?;
    let mut current = FeatureEstimators::standard(FEATURE_STATISTICS_SEED)?;
    for value in &returns {
        whole.observe(object_id, *value)?;
    }
    for value in older {
        reference.observe(object_id, *value)?;
    }
    for value in recent {
        current.observe(object_id, *value)?;
    }

    let drift = current
        .drift_against(&mut reference, DRIFT_BUCKETS)?
        .remove(object_id)
        .ok_or_else(|| {
            qip_core::error::Error::invalid(format!(
                "no drift index was computed for {object_id} although both halves were \
                 summarised; the comparison silently skipping a feature is the failure that \
                 would make a review of nothing read as a review that found nothing"
            ))
        })?;

    let summary = whole.get_mut(object_id).ok_or_else(|| {
        qip_core::error::Error::invalid(format!(
            "{object_id} was observed and then absent from its own estimator set"
        ))
    })?;
    let statistics = InstrumentStatistics {
        object_id: object_id.to_string(),
        returns: summary.observations(),
        mean: summary.mean(),
        standard_deviation: summary.standard_deviation(),
        median: summary.quantile(0.5)?,
        lower_tail: summary.quantile(0.05)?,
        upper_tail: summary.quantile(0.95)?,
        distinct_values: summary.distinct_values(),
        has_moved: summary.has_moved(),
        sample_rows: FeatureSummary::sample_rows(summary),
        bytes: FeatureSummary::bytes(summary),
    };
    Ok(Some(Summarised { statistics, drift }))
}

// The workspace denies `panic_in_result_fn` for production code. A test that
// returns `Result` so it can use `?` still has to assert, and the abort is the
// reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// A deterministic price path whose returns are drawn around `drift` with
    /// scale `scale`. Not a general-purpose generator and not used outside
    /// tests.
    fn path(count: usize, drift: f64, scale: f64, seed: u64) -> Vec<f64> {
        let mut state = seed;
        let mut price = 100.0_f64;
        let mut prices = vec![price];
        for _ in 0..count {
            let mut sum = 0.0;
            for _ in 0..12 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                sum += ((state >> 11) as f64 + 0.5) / 9_007_199_254_740_992.0;
            }
            price *= 1.0 + drift + scale * (sum - 6.0);
            prices.push(price);
        }
        prices
    }

    /// The finding: an instrument whose recent returns are four times as
    /// volatile as its older ones is reported as shifted, and one whose two
    /// halves are independent draws of the same process is not.
    ///
    /// Both halves are load-bearing, and the second is the one that matters
    /// here. The index computed between two digests of one distribution is
    /// never exactly zero, so a review without
    /// `StreamingDrift::floor` would report every instrument as shifted every
    /// cycle — a control that fires always, which is the same defect as one
    /// that cannot fire and reads as protection just as convincingly.
    ///
    /// Mutated by replacing `outcome.drift.is_material()` in `measure` with
    /// `true` — confirmed the calm instrument is then reported as shifted and
    /// this fails, then restored byte-for-byte. Mutated again by replacing it
    /// with `false` — confirmed it then reports `the review named []; 2
    /// instrument(s) summarised into 14480 byte(s) of streaming estimators; no
    /// distribution moved past what the estimators' error explains`, then
    /// restored byte-for-byte.
    #[test]
    fn an_instrument_whose_recent_returns_changed_scale_is_reported_and_a_calm_one_is_not()
    -> Result<()> {
        let mut history: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        // Calm: both halves drawn from one process.
        history.insert("CALM".to_string(), path(400, 0.0, 0.01, 0xA11CE));
        // Shifted: the second half four times as volatile as the first.
        let mut shifted = path(200, 0.0, 0.005, 0xB0B);
        let tail = path(200, 0.0, 0.02, 0xCAFE);
        // The tail is rescaled onto the end of the first leg so the series is
        // a price path and not two paths concatenated at a jump — a single
        // discontinuity would itself be an outlier and the finding would be
        // about that one return rather than about the distribution.
        let last = shifted.last().copied().unwrap_or(100.0);
        let base = tail.first().copied().unwrap_or(100.0);
        shifted.extend(tail.iter().skip(1).map(|price| price / base * last));
        history.insert("SHIFTED".to_string(), shifted);

        let review = measure(&history, 7, Timestamp::from_secs(1_000));
        // Premise: both instruments were actually measured, so the assertions
        // below are about the detector and not about a series that was skipped.
        assert_eq!(review.instruments.len(), 2, "{:?}", review.unmeasured);
        assert!(review.unmeasured.is_empty(), "{:?}", review.unmeasured);
        assert!(
            review.instruments.iter().all(|row| row.returns >= 400),
            "premise: both series are long enough to split"
        );

        let named: Vec<&str> = review
            .shifted
            .iter()
            .map(|shift| shift.object_id.as_str())
            .collect();
        assert_eq!(
            named,
            vec!["SHIFTED"],
            "the review named {named:?}; {}",
            review.describe()
        );
        Ok(())
    }

    /// The other finding: a feed reporting the same handful of prices is not a
    /// distribution, and is named as such rather than summarised as if its
    /// quantiles meant something.
    ///
    /// Mutated by neutralising the `!outcome.statistics.has_moved` branch in
    /// `measure`, so nothing is ever named quantised — confirmed the list is
    /// then empty and this fails, then restored byte-for-byte. The mutation's
    /// output is worth quoting because of the *other* thing it showed: with
    /// the cardinality check gone the stuck feed still appears, but under
    /// `shifted`, at an index of 8.107 against a floor of 0.306. A three-value
    /// feed does register as a distribution shift, which is a true statement
    /// about a false subject, and an operator reading only the shift list
    /// would go looking for a market event that never happened. Naming the
    /// fault as a fault is what the cardinality estimator is for here.
    #[test]
    fn a_price_feed_repeating_three_values_is_named_rather_than_summarised_as_a_distribution()
    -> Result<()> {
        let mut history: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        // A feed stuck on a three-price cycle: every return is one of three
        // values, so the "distribution" has a cardinality of three.
        let stuck: Vec<f64> = (0..401).map(|index| 100.0 + (index % 3) as f64).collect();
        history.insert("STUCK".to_string(), stuck);
        history.insert("LIVE".to_string(), path(400, 0.0, 0.01, 0xD00D));

        let review = measure(&history, 3, Timestamp::from_secs(2_000));
        // Premise: both were measured and both hold the same number of
        // returns, so the difference below is cardinality and not length.
        assert_eq!(review.instruments.len(), 2, "{:?}", review.unmeasured);
        let lengths: Vec<u64> = review.instruments.iter().map(|row| row.returns).collect();
        assert_eq!(lengths, vec![400, 400], "premise: equal-length series");

        assert_eq!(
            review.quantised,
            vec!["STUCK".to_string()],
            "the review named {:?}; {}",
            review.quantised,
            review.describe()
        );
        assert!(
            review.describe().contains("STUCK"),
            "the summary line does not name the stuck feed: {}",
            review.describe()
        );
        Ok(())
    }

    /// A series too short to split is reported as unmeasured with the reason,
    /// and the review still comes back rather than being skipped.
    ///
    /// The empty review is the deliverable here. A measurement that returned
    /// nothing when it had nothing to say would leave a cycle on a young
    /// platform indistinguishable from a cycle where this module was never
    /// called — and this repository has already shipped three modules that
    /// went quiet when idle and so reached no surface at all.
    ///
    /// Mutated by replacing the `Ok(None)` arm's `unmeasured.push` with a
    /// `drop`, so a series below the bar vanishes instead of being named —
    /// confirmed the length assertion then fails with `left: 0, right: 1`,
    /// then restored byte-for-byte.
    #[test]
    fn a_series_too_short_to_split_is_named_with_its_reason_rather_than_dropped() -> Result<()> {
        let mut history: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        history.insert("YOUNG".to_string(), path(50, 0.0, 0.01, 1));
        // Premise: the series is genuinely below the bar, so the assertion is
        // about the reporting and not about the threshold. A `const` block
        // because both sides are constants and clippy is right that an
        // assertion on two constants belongs at compile time — the premise is
        // no weaker for being checked earlier.
        const { assert!(50 < FEATURE_STATISTICS_MIN_RETURNS) };

        let review = measure(&history, 1, Timestamp::from_secs(3_000));
        assert!(review.instruments.is_empty());
        assert_eq!(review.unmeasured.len(), 1);
        let (named, reason) = &review.unmeasured[0];
        assert_eq!(named, "YOUNG");
        assert!(
            reason.contains(&FEATURE_STATISTICS_MIN_RETURNS.to_string()),
            "the reason does not name the bar: {reason}"
        );
        assert!(!review.has_findings());
        assert!(
            review.describe().contains("not measured"),
            "the summary line hides the unmeasured instrument: {}",
            review.describe()
        );

        // And an empty history still produces a review.
        let empty = measure(&BTreeMap::new(), 2, Timestamp::from_secs(4_000));
        assert_eq!(empty.instruments.len(), 0);
        assert_eq!(empty.cycle, 2);
        Ok(())
    }

    /// The memory figure the review reports is the summary's and not the
    /// series', and it is inside §22.2's "few KB per distribution".
    ///
    /// Mutated by reporting `prices.len() * 8` as the instrument's bytes,
    /// which is what the series itself costs — confirmed the equality
    /// assertion then fails with `3208 against 320008`, then restored
    /// byte-for-byte.
    ///
    /// The hundredfold length difference is what makes that mutation fire.
    /// Comparing two series of similar length would leave a byte count
    /// proportional to the series looking exactly like one that is not, which
    /// is the shape of memory assertion that passes forever and guards
    /// nothing.
    #[test]
    fn the_reported_memory_is_the_summary_s_and_does_not_move_with_the_series() -> Result<()> {
        let mut short: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        short.insert("ONE".to_string(), path(400, 0.0, 0.01, 11));
        let mut long: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        long.insert("ONE".to_string(), path(40_000, 0.0, 0.01, 11));

        let short_review = measure(&short, 1, Timestamp::from_secs(1));
        let long_review = measure(&long, 2, Timestamp::from_secs(2));
        // Premise: the two series really do differ by two orders of magnitude,
        // so an equal byte count below is a fact about the summary.
        assert_eq!(short_review.instruments.len(), 1);
        assert_eq!(long_review.instruments.len(), 1);
        assert_eq!(short_review.instruments[0].returns, 400);
        assert_eq!(long_review.instruments[0].returns, 40_000);

        assert_eq!(
            short_review.bytes, long_review.bytes,
            "a hundredfold longer series changed the summary's size: {} against {}",
            short_review.bytes, long_review.bytes
        );
        assert!(
            long_review.bytes <= 8_192,
            "one instrument's summary cost {} bytes against §22.2's few-KB-per-distribution \
             budget",
            long_review.bytes
        );
        Ok(())
    }
}
