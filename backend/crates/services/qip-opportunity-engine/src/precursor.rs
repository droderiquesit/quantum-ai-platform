//! Opportunity sentinels: precursors to a dislocation, before the anomaly
//! detector fires (ADR 0434, R12-AMBIENT-3069).
//!
//! The detectors in [`crate::detector`] report a dislocation once it has
//! happened: a correlation that has broken, a spread that is already wide.
//! By then the cheapest moment to look has passed. A sentinel watches the
//! same statistics *below* their thresholds and reports a run of them rising
//! toward one — a pair's correlation gap widening sweep after sweep while
//! still inside the band the breakdown detector tolerates.
//!
//! Three rules keep a precursor distinct from an anomaly, and each is
//! load-bearing:
//!
//! * **Same statistic, same threshold.** Each family scores on the number
//!   the matching detector fires on, at that detector's threshold, so
//!   "below threshold" means "the detector has not fired" rather than a
//!   second opinion that can disagree with it.
//! * **A rise, not a level.** One elevated reading is noise; a precursor is
//!   [`PrecursorConfig::run`] consecutive sweeps each strictly higher than
//!   the last, the newest past [`PrecursorConfig::floor_share`] of the
//!   threshold.
//! * **Never a finished signal.** A subject that reached its threshold, or
//!   that any detector flagged this sweep, raises no precursor and its run
//!   restarts: the anomaly path owns it, and raising both would be two
//!   claims about one fact.
//!
//! The cross-asset family scores every priced pair. The other four need
//! reference data the context does not carry on its own — which two
//! listings are one product in two regions, which observations measure a
//! physical product, which legs form a structure — and are silent until a
//! deployment configures them. Silence there is a configuration, stated in
//! the config, not a detector that cannot fire: each family's test fires it.

use crate::detector::{DetectionContext, correlation_departure};
use qip_core::{Duration, Timestamp};
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The five dislocations the sentinel looks ahead of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecursorFamily {
    /// Two instruments whose correlation is drifting from its long-run value.
    CrossAsset,
    /// One product listed in two regions whose price gap is widening.
    CrossRegion,
    /// A subject moving after a knowable event, not yet by an anomaly's
    /// margin.
    Event,
    /// A physical-product reading (inventory, freight, throughput) drifting
    /// from its recent distribution.
    PhysicalProduct,
    /// A multi-leg structure whose weighted spread is leaving its range.
    MultiLeg,
}

impl PrecursorFamily {
    pub const ALL: [Self; 5] = [
        Self::CrossAsset,
        Self::CrossRegion,
        Self::Event,
        Self::PhysicalProduct,
        Self::MultiLeg,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CrossAsset => "cross_asset",
            Self::CrossRegion => "cross_region",
            Self::Event => "event",
            Self::PhysicalProduct => "physical_product",
            Self::MultiLeg => "multi_leg",
        }
    }
}

/// A structure whose legs move together: a calendar, a crack, a butterfly.
/// The spread is `Σ weight × ln(price)` over the legs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegSet {
    pub name: String,
    pub legs: Vec<(String, f64)>,
}

/// What the sentinel watches and how much of a rise it calls a precursor.
#[derive(Clone, Debug, PartialEq)]
pub struct PrecursorConfig {
    /// Consecutive strictly rising scores a precursor needs. At least two.
    pub run: usize,
    /// The share of the threshold the newest score must reach, in `(0, 1)`:
    /// a run rising from nothing to nearly nothing is not approaching
    /// anything.
    pub floor_share: f64,
    /// The cross-asset family's window and baseline — the breakdown
    /// detector's own defaults, so the threshold below is the detector's.
    pub recent_window: usize,
    pub minimum_baseline: f64,
    pub correlation_threshold: f64,
    /// The robust z (median and MAD, as the observation detector scores)
    /// past which a spread, a physical reading or a post-event return is
    /// an anomaly rather than a precursor.
    pub series_threshold: f64,
    /// Readings a series needs before its latest is scored.
    pub minimum_history: usize,
    /// Pairs of listings that are one product in two regions.
    pub regional_pairs: BTreeSet<(String, String)>,
    /// Observation names that measure a physical product.
    pub physical: BTreeSet<String>,
    /// Multi-leg structures to watch.
    pub leg_sets: Vec<LegSet>,
    /// How long after an event became knowable a move on its subject is
    /// watched as building toward it.
    pub event_window: Duration,
}

impl Default for PrecursorConfig {
    fn default() -> Self {
        let breakdown = crate::detector::CorrelationBreakdownDetector::default();
        Self {
            run: 3,
            floor_share: 0.5,
            recent_window: breakdown.recent_window,
            minimum_baseline: breakdown.minimum_baseline,
            correlation_threshold: breakdown.threshold,
            series_threshold: crate::detector::ObservationDetector::new(
                crate::detector::AnomalyKind::FundamentalSurprise,
            )
            .threshold,
            minimum_history: 12,
            regional_pairs: BTreeSet::new(),
            physical: BTreeSet::new(),
            leg_sets: Vec::new(),
            event_window: Duration::from_days(5),
        }
    }
}

impl PrecursorConfig {
    /// Refuse a configuration under which the sentinel could not tell a
    /// rise from a reading, rather than clamping it into one that can.
    pub fn validate(&self) -> qip_core::Result<()> {
        let refuse = |what: String| Err(qip_core::Error::invalid(what));
        if self.run < 2 {
            return refuse(format!(
                "a precursor run of {} cannot rise; state at least two sweeps",
                self.run
            ));
        }
        if !(self.floor_share > 0.0 && self.floor_share < 1.0) {
            return refuse(format!(
                "a precursor floor of {} of the threshold is not a share in (0, 1)",
                self.floor_share
            ));
        }
        for (name, threshold) in [
            ("correlation", self.correlation_threshold),
            ("series", self.series_threshold),
        ] {
            if !(threshold.is_finite() && threshold > 0.0) {
                return refuse(format!(
                    "the {name} precursor threshold {threshold} is not a positive number"
                ));
            }
        }
        for set in &self.leg_sets {
            if set.legs.len() < 2 || set.legs.iter().any(|(_, w)| !w.is_finite() || *w == 0.0) {
                return refuse(format!(
                    "leg set {} needs two or more legs, each with a finite non-zero weight",
                    set.name
                ));
            }
        }
        Ok(())
    }
}

/// One family's reading of one subject in one sweep.
#[derive(Clone, Debug, PartialEq)]
pub struct PrecursorScore {
    pub family: PrecursorFamily,
    pub subject: String,
    /// Magnitude, in the units of `threshold`.
    pub score: f64,
    /// Where the matching detector fires.
    pub threshold: f64,
}

/// A run of rising sub-threshold scores: a dislocation being approached.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Precursor {
    pub family: PrecursorFamily,
    pub subject: String,
    /// The run, oldest first, each strictly above the one before.
    pub scores: Vec<f64>,
    pub threshold: f64,
    pub detected_at: Timestamp,
}

impl Precursor {
    /// The newest score as a share of the threshold, in `[0, 1)`.
    pub fn approach(&self) -> f64 {
        self.scores.last().copied().unwrap_or(0.0) / self.threshold
    }
}

/// Score every family over one sweep's context.
pub fn score(context: &DetectionContext, config: &PrecursorConfig) -> Vec<PrecursorScore> {
    let mut out = Vec::new();
    let regional: BTreeSet<(&str, &str)> = config
        .regional_pairs
        .iter()
        .flat_map(|(a, b)| [(a.as_str(), b.as_str()), (b.as_str(), a.as_str())])
        .collect();

    // Cross-asset: every priced pair that is not one product twice.
    let subjects: Vec<&String> = context.prices.keys().collect();
    for (index, first) in subjects.iter().enumerate() {
        for second in subjects.iter().skip(index + 1) {
            if regional.contains(&(first.as_str(), second.as_str())) {
                continue;
            }
            if let Some(departure) = correlation_departure(
                &context.prices[*first],
                &context.prices[*second],
                config.recent_window,
                config.minimum_baseline,
            ) {
                out.push(PrecursorScore {
                    family: PrecursorFamily::CrossAsset,
                    subject: format!("{first}|{second}"),
                    score: departure.z.abs(),
                    threshold: config.correlation_threshold,
                });
            }
        }
    }

    // Cross-region: the log price gap between two listings of one product.
    for (first, second) in &config.regional_pairs {
        let (Some(a), Some(b)) = (context.prices.get(first), context.prices.get(second)) else {
            continue;
        };
        let n = a.len().min(b.len());
        let gap: Option<Vec<f64>> = (0..n)
            .map(|i| {
                let (pa, pb) = (a[a.len() - n + i], b[b.len() - n + i]);
                (pa > 0.0 && pb > 0.0).then(|| pa.ln() - pb.ln())
            })
            .collect();
        if let Some(z) = gap.and_then(|g| latest_z(&g, config.minimum_history)) {
            out.push(PrecursorScore {
                family: PrecursorFamily::CrossRegion,
                subject: format!("{first}|{second}"),
                score: z.abs(),
                threshold: config.series_threshold,
            });
        }
    }

    // Event: the latest return on a subject an event landed on recently.
    let since = context.as_of.saturating_sub(config.event_window);
    let touched: BTreeSet<&str> = context
        .events
        .iter()
        .filter(|e| e.known_at() >= since && e.known_at() <= context.as_of)
        .map(|e| e.subject.as_str())
        .collect();
    for subject in touched {
        let Some(prices) = context.prices.get(subject) else {
            continue;
        };
        if let Some(z) = latest_z(&stats::log_returns(prices), config.minimum_history) {
            out.push(PrecursorScore {
                family: PrecursorFamily::Event,
                subject: subject.to_string(),
                score: z.abs(),
                threshold: config.series_threshold,
            });
        }
    }

    // Physical product: the named readings.
    for name in &config.physical {
        if let Some(z) = context
            .observations
            .get(name)
            .and_then(|v| latest_z(v, config.minimum_history))
        {
            out.push(PrecursorScore {
                family: PrecursorFamily::PhysicalProduct,
                subject: name.clone(),
                score: z.abs(),
                threshold: config.series_threshold,
            });
        }
    }

    // Multi-leg: the weighted log spread, over the bars every leg has.
    for set in &config.leg_sets {
        let series: Option<Vec<&Vec<f64>>> = set
            .legs
            .iter()
            .map(|(leg, _)| context.prices.get(leg))
            .collect();
        let Some(series) = series else {
            continue;
        };
        let n = series.iter().map(|s| s.len()).min().unwrap_or(0);
        let spread: Option<Vec<f64>> = (0..n)
            .map(|i| {
                series
                    .iter()
                    .zip(&set.legs)
                    .map(|(s, (_, w))| {
                        let p = s[s.len() - n + i];
                        (p > 0.0).then(|| w * p.ln())
                    })
                    .sum()
            })
            .collect();
        if let Some(z) = spread.and_then(|s| latest_z(&s, config.minimum_history)) {
            out.push(PrecursorScore {
                family: PrecursorFamily::MultiLeg,
                subject: set.name.clone(),
                score: z.abs(),
                threshold: config.series_threshold,
            });
        }
    }
    out
}

/// The latest reading's distance from the median of those before it, in
/// MADs — the observation detector's statistic. `None` on a short history
/// or one with no spread, where any distance is infinite and means nothing.
fn latest_z(values: &[f64], minimum_history: usize) -> Option<f64> {
    if values.len() < minimum_history.max(2) {
        return None;
    }
    let history = &values[..values.len() - 1];
    let scale = stats::median_absolute_deviation(history);
    if scale <= 1e-12 {
        return None;
    }
    let z = (values[values.len() - 1] - stats::median(history)) / scale;
    z.is_finite().then_some(z)
}

/// The sentinel's memory: the last few scores per family and subject.
///
/// Bounded by the sweep: a subject absent from a sweep is forgotten, so the
/// working set is at most this sweep's scores times [`PrecursorConfig::run`].
#[derive(Debug, Default)]
pub struct PrecursorScanner {
    history: BTreeMap<(PrecursorFamily, String), VecDeque<f64>>,
}

impl PrecursorScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subjects and families currently being watched.
    pub fn watched(&self) -> usize {
        self.history.len()
    }

    /// Feed one sweep's scores. `flagged` holds every subject a detector
    /// reported this sweep: its run restarts and it raises nothing.
    pub fn observe(
        &mut self,
        scores: &[PrecursorScore],
        flagged: &BTreeSet<String>,
        config: &PrecursorConfig,
        now: Timestamp,
    ) -> Vec<Precursor> {
        let mut next: BTreeMap<(PrecursorFamily, String), VecDeque<f64>> = BTreeMap::new();
        let mut raised = Vec::new();
        for reading in scores {
            let key = (reading.family, reading.subject.clone());
            if !reading.score.is_finite()
                || reading.score >= reading.threshold
                || flagged.contains(&reading.subject)
            {
                // Finished, or not a number: the anomaly path owns it.
                continue;
            }
            let mut run = self.history.remove(&key).unwrap_or_default();
            run.push_back(reading.score);
            while run.len() > config.run {
                run.pop_front();
            }
            let rising = run.len() == config.run
                && run.iter().zip(run.iter().skip(1)).all(|(a, b)| b > a)
                && reading.score >= config.floor_share * reading.threshold;
            if rising {
                raised.push(Precursor {
                    family: reading.family,
                    subject: reading.subject.clone(),
                    scores: run.iter().copied().collect(),
                    threshold: reading.threshold,
                    detected_at: now,
                });
                // Raised once per run: the next precursor needs a new rise.
                continue;
            }
            next.insert(key, run);
        }
        self.history = next;
        raised
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(family: PrecursorFamily, subject: &str, score: f64) -> PrecursorScore {
        PrecursorScore {
            family,
            subject: subject.to_string(),
            score,
            threshold: 2.5,
        }
    }

    fn feed(
        scanner: &mut PrecursorScanner,
        family: PrecursorFamily,
        scores: &[f64],
        flagged: &BTreeSet<String>,
    ) -> Vec<Vec<Precursor>> {
        let config = PrecursorConfig::default();
        scores
            .iter()
            .map(|s| {
                scanner.observe(
                    &[reading(family, "s", *s)],
                    flagged,
                    &config,
                    Timestamp::from_secs(0),
                )
            })
            .collect()
    }

    #[test]
    fn three_rising_scores_below_the_threshold_raise_one_precursor_and_a_flat_run_none() {
        let mut scanner = PrecursorScanner::new();
        let rounds = feed(
            &mut scanner,
            PrecursorFamily::CrossAsset,
            &[0.9, 1.4, 2.0],
            &BTreeSet::new(),
        );
        assert!(rounds[0].is_empty() && rounds[1].is_empty());
        assert_eq!(rounds[2].len(), 1, "{rounds:?}");
        assert_eq!(rounds[2][0].scores, vec![0.9, 1.4, 2.0]);
        assert!((rounds[2][0].approach() - 0.8).abs() < 1e-12);

        let mut scanner = PrecursorScanner::new();
        let flat = feed(
            &mut scanner,
            PrecursorFamily::CrossAsset,
            &[2.0, 2.0, 2.0, 2.0],
            &BTreeSet::new(),
        );
        assert!(flat.iter().all(Vec::is_empty), "{flat:?}");
    }

    #[test]
    fn a_rise_that_reached_the_threshold_or_was_flagged_is_not_a_precursor() {
        let mut scanner = PrecursorScanner::new();
        let crossed = feed(
            &mut scanner,
            PrecursorFamily::MultiLeg,
            &[1.0, 1.8, 2.6],
            &BTreeSet::new(),
        );
        assert!(crossed.iter().all(Vec::is_empty), "{crossed:?}");

        let mut scanner = PrecursorScanner::new();
        let flagged = BTreeSet::from(["s".to_string()]);
        let owned = feed(
            &mut scanner,
            PrecursorFamily::MultiLeg,
            &[1.0, 1.8, 2.2],
            &flagged,
        );
        assert!(owned.iter().all(Vec::is_empty), "{owned:?}");
        assert_eq!(scanner.watched(), 0, "a flagged subject is not remembered");
    }

    #[test]
    fn a_run_rising_from_nothing_to_little_stays_below_the_floor() {
        let mut scanner = PrecursorScanner::new();
        // 1.2 is under half of 2.5.
        let low = feed(
            &mut scanner,
            PrecursorFamily::Event,
            &[0.2, 0.6, 1.2],
            &BTreeSet::new(),
        );
        assert!(low.iter().all(Vec::is_empty), "{low:?}");
    }

    #[test]
    fn a_configuration_that_cannot_tell_a_rise_from_a_reading_is_refused() {
        let ok = PrecursorConfig::default();
        assert!(ok.validate().is_ok());
        let short = PrecursorConfig {
            run: 1,
            ..ok.clone()
        };
        assert!(short.validate().is_err());
        let floor = PrecursorConfig {
            floor_share: 1.0,
            ..ok.clone()
        };
        assert!(floor.validate().is_err());
        let one_leg = PrecursorConfig {
            leg_sets: vec![LegSet {
                name: "x".into(),
                legs: vec![("a".into(), 1.0)],
            }],
            ..ok
        };
        assert!(one_leg.validate().is_err());
    }

    /// A series of `len` quiet readings alternating around `level` with a
    /// unit MAD, then `last`.
    fn quiet_then(len: usize, last: f64) -> Vec<f64> {
        let mut v: Vec<f64> = (0..len)
            .map(|i| if i % 2 == 0 { 101.0 } else { 99.0 })
            .collect();
        v.push(last);
        v
    }

    #[test]
    fn each_configured_family_scores_the_subject_it_names() {
        let at = Timestamp::from_secs(10_000_000);
        // Prices: a, b quiet; listing a@eu tracks a; legs c1, c2.
        let base: Vec<f64> = (0..100).map(|i| 100.0 + (i % 3) as f64).collect();
        // Out of phase with `base`, so a gap or spread against it has a
        // spread of its own to be scored against.
        let other: Vec<f64> = (0..100).map(|i| 100.0 + ((i + 1) % 3) as f64).collect();
        let mut drifted = base.clone();
        if let Some(last) = drifted.last_mut() {
            *last *= 1.02;
        }
        let event = crate::catalyst::MarketEvent::new(
            "e1",
            "a",
            "earnings",
            at.saturating_sub(Duration::from_days(1)),
            at.saturating_sub(Duration::from_days(1)),
        );
        let context = DetectionContext::new(at)
            .with_prices("a", drifted.clone())
            .with_prices("a@eu", other.clone())
            .with_prices("c1", drifted)
            .with_prices("c2", other)
            .with_observations("crude/inventory", quiet_then(20, 102.0))
            .with_events(vec![event]);
        let config = PrecursorConfig {
            regional_pairs: BTreeSet::from([("a".to_string(), "a@eu".to_string())]),
            physical: BTreeSet::from(["crude/inventory".to_string()]),
            leg_sets: vec![LegSet {
                name: "c1-c2".into(),
                legs: vec![("c1".into(), 1.0), ("c2".into(), -1.0)],
            }],
            ..PrecursorConfig::default()
        };
        let scores = score(&context, &config);
        let families: BTreeSet<(PrecursorFamily, &str)> = scores
            .iter()
            .map(|s| (s.family, s.subject.as_str()))
            .collect();
        for expected in [
            (PrecursorFamily::CrossRegion, "a|a@eu"),
            (PrecursorFamily::Event, "a"),
            (PrecursorFamily::PhysicalProduct, "crude/inventory"),
            (PrecursorFamily::MultiLeg, "c1-c2"),
        ] {
            assert!(families.contains(&expected), "{expected:?} in {scores:?}");
        }
        // Premise: the history is long enough for the cross-asset family to
        // score the pairs it does see, so its silence on the regional pair
        // below is the exclusion and not a short tape.
        assert!(
            families.contains(&(PrecursorFamily::CrossAsset, "a@eu|c1")),
            "{scores:?}"
        );
        // One product twice is a cross-region pair, never a cross-asset one.
        assert!(
            !families.contains(&(PrecursorFamily::CrossAsset, "a|a@eu")),
            "{scores:?}"
        );
        let physical = scores
            .iter()
            .find(|s| s.family == PrecursorFamily::PhysicalProduct)
            .map(|s| s.score);
        // Median 100, latest 102, against the observation detector's MAD.
        let history = &quiet_then(20, 102.0)[..20];
        let expected = 2.0 / stats::median_absolute_deviation(history);
        assert_eq!(physical, Some(expected));
    }
}
