//! The market factor, computed from the platform's own price tape.
//!
//! # Why this exists
//!
//! [`crate::metrics::beta`] estimates a beta against a benchmark, and until
//! 2026-09-08 nothing outside tests called it. The consequence reached three
//! capabilities at once: `factor_betas` was built empty wherever the kernel
//! built a position period, so `RiskDecomposition::effective_bets` reported
//! zero over contributions nobody populated, `StressTester::apply` pushed every
//! position onto its `unmodelled` list, and the learning engine's factor
//! attribution read the same empty map. One missing wire, three capabilities
//! that executed and measured nothing.
//!
//! The wire needed a benchmark, and choosing one settles what every factor
//! number in the platform is measured against — so it is ADR 0052's decision
//! and not this module's. This module implements it and nothing more.
//!
//! # What the factor is
//!
//! The equal-weighted arithmetic mean of the per-period simple returns of the
//! instruments on the platform's own tape. Equal weight because the platform
//! holds no share counts and no free float, so a capitalisation weight would
//! have to be estimated from something it does not observe. Its own tape
//! because an index fetched from outside is a number a replay cannot check.
//!
//! # What it refuses to do
//!
//! Report a beta from too short an overlap. Below [`MINIMUM_OVERLAP`] an
//! instrument carries no beta at all, and every consumer treats a missing beta
//! as *unmodelled* rather than as zero. That distinction is the point: a
//! position nobody could model and a position genuinely insensitive to a shock
//! are different facts, and a stress report that conflates them understates the
//! book.

use crate::metrics;
use qip_numerics::stats;
use std::collections::BTreeMap;

/// The name the factor carries in `factor_betas` and `factor_returns`.
///
/// The platform's own vocabulary for the thing this module estimates: the
/// common movement of the instruments on its tape.
pub const MARKET_FACTOR: &str = "market";

/// The name the standard stress library gives the shock this factor answers.
///
/// Two names for one movement, and the second is not this crate's to choose:
/// `qip_simulation_engine::scenario::standard_library` calls its equity-style
/// shock `equity`, and a `FactorExposure` whose beta is filed under any other
/// key is a position the stress tester reports as *unmodelled*. ADR 0052 said
/// the two names already matched. They did not — the ADR is corrected, and the
/// mapping lives here, in one constant, rather than as a string literal at the
/// seam that consumes it.
///
/// An acceptance test asserts the standard library still shocks this name, so
/// a rename there fails a gate rather than silently emptying every stress
/// report.
pub const EQUITY_SHOCK: &str = "equity";

/// The fewest overlapping return observations a beta may be estimated from.
///
/// A covariance over four points is arithmetic, not evidence. Twenty is the
/// smallest window where the estimate stops swinging on a single observation,
/// and an instrument below it is reported with no beta rather than with a
/// number that would be read as one.
pub const MINIMUM_OVERLAP: usize = 20;

/// The variance below which the factor is treated as having none.
///
/// `metrics::beta` divides by the benchmark's variance and answers `0.0`
/// rather than dividing by zero. That answer is indistinguishable from a
/// genuine zero beta, so this module refuses to report any beta at all when
/// the factor is this flat: an unmeasurable market is unmodelled, not benign.
const VARIANCE_FLOOR: f64 = 1e-15;

/// Simple returns from a close series. One shorter than its input.
///
/// A non-positive close yields no return for that step rather than a division
/// by zero or a negative price ratio: the tape should not contain one, and a
/// silent `inf` propagating into a covariance is worse than a gap.
fn returns(closes: &[f64]) -> Vec<f64> {
    closes
        .windows(2)
        .filter_map(|pair| {
            let (previous, current) = (pair[0], pair[1]);
            (previous > 0.0).then_some((current - previous) / previous)
        })
        .collect()
}

/// The market factor and the betas of each instrument against it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketFactor {
    /// The factor's own return series, oldest first.
    returns: Vec<f64>,
    /// Beta per instrument, holding only those with enough overlap.
    betas: BTreeMap<String, f64>,
    /// Residual variance per instrument, over the same overlap as its beta.
    ///
    /// Kept beside the beta rather than recomputed by a caller: the two are
    /// the two halves of one single-factor model, and a caller that recomputed
    /// the residual over a different window would produce a decomposition
    /// whose parts do not sum to the whole it was measured from.
    specific: BTreeMap<String, f64>,
}

impl MarketFactor {
    /// Estimate the factor from a tape of closes per instrument.
    ///
    /// The factor's return in a period is the mean of the instrument returns
    /// available for that period. An instrument with a shorter history
    /// contributes to the periods it has and is absent from the rest, rather
    /// than being padded — a padded series is a claim the instrument was flat
    /// on days it was not observed.
    pub fn estimate(tape: &BTreeMap<String, Vec<f64>>) -> Self {
        let per_instrument: BTreeMap<String, Vec<f64>> = tape
            .iter()
            .map(|(instrument, closes)| (instrument.clone(), returns(closes)))
            .filter(|(_, series)| !series.is_empty())
            .collect();
        if per_instrument.is_empty() {
            return Self::default();
        }

        // Series are aligned on their *most recent* observation, not their
        // first: two instruments whose tapes started on different days share
        // today, and aligning on index zero would compare one's Monday with
        // the other's Thursday.
        let longest = per_instrument
            .values()
            .map(Vec::len)
            .max()
            .unwrap_or_default();
        let mut factor = Vec::with_capacity(longest);
        for step in (0..longest).rev() {
            let mut sum = 0.0;
            let mut count = 0usize;
            for series in per_instrument.values() {
                if let Some(value) = series
                    .len()
                    .checked_sub(step + 1)
                    .and_then(|index| series.get(index))
                {
                    sum += *value;
                    count += 1;
                }
            }
            if count > 0 {
                factor.push(sum / count as f64);
            }
        }

        // A factor with no variance carries no information, and
        // `metrics::beta` answers `0.0` for it — a number that reads
        // downstream as "immune to the market" when what happened is that the
        // market did not move. Every instrument stays unmodelled instead.
        // Found by a test whose fixture grew at a constant rate: every return
        // identical, variance nil, and every beta reported as a confident
        // zero.
        if stats::variance(&factor) <= VARIANCE_FLOOR {
            return Self {
                returns: factor,
                betas: BTreeMap::new(),
                specific: BTreeMap::new(),
            };
        }

        let mut betas = BTreeMap::new();
        let mut specific = BTreeMap::new();
        for (instrument, series) in &per_instrument {
            let overlap = series.len().min(factor.len());
            if overlap < MINIMUM_OVERLAP {
                continue;
            }
            let instrument_tail = &series[series.len() - overlap..];
            let factor_tail = &factor[factor.len() - overlap..];
            let beta = metrics::beta(instrument_tail, factor_tail);
            // The residual of the single-factor model: what the instrument's
            // variance is once the part the factor explains is removed.
            // Floored at zero because sampling noise can make the subtraction
            // negative by a rounding, and a negative variance is refused
            // downstream — `FactorRisk::new` rejects it — so the floor is the
            // difference between a model and an error return.
            let residual = (stats::variance(instrument_tail)
                - beta * beta * stats::variance(factor_tail))
            .max(0.0);
            betas.insert(instrument.clone(), beta);
            specific.insert(instrument.clone(), residual);
        }

        Self {
            returns: factor,
            betas,
            specific,
        }
    }

    /// The variance of the factor's own return series.
    pub fn variance(&self) -> f64 {
        stats::variance(&self.returns)
    }

    /// One instrument's residual variance, or `None` where it carries no beta.
    ///
    /// Present exactly where [`Self::beta_of`] is present: an instrument the
    /// factor could not be measured against has no residual either, because
    /// there is nothing to take the residual of.
    pub fn specific_variance_of(&self, instrument: &str) -> Option<f64> {
        self.specific.get(instrument).copied()
    }

    /// The factor's most recent return, or `None` where it has none.
    pub fn latest_return(&self) -> Option<f64> {
        self.returns.last().copied()
    }

    /// The factor's return series, oldest first.
    pub fn returns(&self) -> &[f64] {
        &self.returns
    }

    /// One instrument's beta, or `None` where the overlap was too short.
    ///
    /// `None` means *unmodelled* and never *zero*. A caller that substitutes
    /// zero here has turned "we could not measure this" into "this is immune",
    /// which is the failure this whole module is arranged to avoid.
    pub fn beta_of(&self, instrument: &str) -> Option<f64> {
        self.betas.get(instrument).copied()
    }

    /// Every instrument that carries a beta.
    pub fn modelled(&self) -> impl Iterator<Item = (&String, &f64)> {
        self.betas.iter()
    }

    /// How many instruments carry a beta.
    pub fn modelled_count(&self) -> usize {
        self.betas.len()
    }

    /// Whether the factor was estimated from anything at all.
    pub fn is_empty(&self) -> bool {
        self.returns.is_empty()
    }
}
