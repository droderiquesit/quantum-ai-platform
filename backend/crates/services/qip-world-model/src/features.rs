//! The point-in-time feature store.
//!
//! A feature value has two timestamps for the same reason a graph fact does:
//! when it was true, and when it became computable. A backtest that reads a
//! feature by valid time alone will read values assembled from data that had
//! not arrived yet, and the resulting strategy will look excellent and lose
//! money. [`FeatureStore::value_as_of`] takes both and will not return
//! otherwise.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many observations one `(feature, subject)` series retains.
///
/// The failure this prevents is not hypothetical and not slow: the absorption
/// path records `close` into this store on every trade and every tick, so with
/// a live feed attached the series grew monotonically for the life of the
/// process, in the one store written most often per cycle. Raw market data is
/// pass-through — the event log is the record and a working set is a window,
/// not an archive.
///
/// Five hundred and twelve matches the kernel's `SERIES_HISTORY`, and matching
/// it is the argument: `price_history`, `volume_history` and the spread series
/// are filled from the same absorption function on the same tick, so a feature
/// series that retained a different depth would answer a question about the
/// same instant that its neighbours could not, and the mismatch would show up
/// as an inconsistency nobody could locate. No reader of this store states a
/// lookback anywhere near it — the deepest is the regime detector's 250 — so
/// what the bound costs is only how far back a bitemporal query may reach, and
/// a read that reaches further is refused rather than answered from the stump
/// (see [`FeatureLookup::Truncated`]).
pub const FEATURE_HISTORY: usize = 512;

/// One observation of a feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureValue {
    pub value: f64,
    /// The instant the value describes.
    pub valid_at: Timestamp,
    /// When the value became computable from arrived data.
    pub available_at: Timestamp,
    /// Confidence in the value, in `[0, 1]`.
    pub confidence: f64,
    /// True when the value was imputed rather than observed.
    pub imputed: bool,
}

impl FeatureValue {
    pub fn new(value: f64, valid_at: Timestamp, available_at: Timestamp) -> Self {
        Self {
            value,
            valid_at,
            available_at,
            confidence: 1.0,
            imputed: false,
        }
    }

    /// A value available at the instant it describes — only correct for data
    /// computed from already-arrived observations.
    pub fn immediate(value: f64, at: Timestamp) -> Self {
        Self::new(value, at, at)
    }

    pub fn imputed(mut self) -> Self {
        self.imputed = true;
        self.confidence *= 0.7;
        self
    }

    /// Delay between the value being true and being usable.
    pub fn availability_lag(&self) -> Duration {
        self.available_at.since(self.valid_at)
    }
}

/// The definition of a feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub description: String,
    /// Subject the feature is computed for: an object id, an entity id.
    pub subject_kind: String,
    /// Typical delay before a value becomes available.
    pub expected_lag: Duration,
    /// How stale a value may be before it should not be used.
    pub max_staleness: Duration,
    /// Model or computation that produces it, for lineage.
    pub producer: String,
}

impl Feature {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        producer: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            subject_kind: "object".into(),
            expected_lag: Duration::ZERO,
            max_staleness: Duration::from_days(1),
            producer: producer.into(),
        }
    }

    pub fn with_lag(mut self, lag: Duration) -> Self {
        self.expected_lag = lag;
        self
    }

    pub fn with_staleness(mut self, staleness: Duration) -> Self {
        self.max_staleness = staleness;
        self
    }

    /// What the series is keyed by, from the vocabulary rather than a string
    /// a caller spelled.
    pub fn with_subject_kind(mut self, kind: crate::vocabulary::SubjectKind) -> Self {
        self.subject_kind = kind.as_str().into();
        self
    }
}

/// One `(feature, subject)` series: a bounded window, plus what it discarded.
///
/// The discard counts are not diagnostics for their own sake. Without them a
/// truncated read is indistinguishable from a read of a series that never held
/// the instant asked for, and those two answers call for opposite actions —
/// widen the window, or go and find the data.
#[derive(Debug, Default)]
struct Series {
    /// Kept in valid-time order, oldest first, at most `history_limit` long.
    values: Vec<FeatureValue>,
    /// Values dropped to stay inside the bound. Non-zero is how a deployment
    /// learns its window is too short from the store rather than from a
    /// query that quietly reached past the end of it.
    evicted: u64,
    /// Valid time of the oldest value this series ever held. Set once, at the
    /// first eviction: eviction is oldest-first, so the first value dropped is
    /// the earliest instant the series ever covered, and any read at or after
    /// it is a read into territory this series used to be able to answer.
    oldest_ever_held: Option<Timestamp>,
}

impl Series {
    /// Drop the oldest values until the series fits, counting what went.
    ///
    /// Applied at the insert, so nothing between inserts ever observes an
    /// over-long series, and one drain rather than a remove per element so a
    /// series that arrived long by any route converges immediately instead of
    /// paying the over-budget cost once per observation until it catches up.
    fn trim(&mut self, limit: usize) {
        if self.values.len() <= limit {
            return;
        }
        let excess = self.values.len() - limit;
        if self.oldest_ever_held.is_none() {
            self.oldest_ever_held = self.values.first().map(|value| value.valid_at);
        }
        self.values.drain(..excess);
        self.evicted = self
            .evicted
            .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
    }
}

/// The outcome of a point-in-time read.
///
/// [`FeatureStore::value_as_of`] returns an `Option` because most callers only
/// need the value. This type exists for the one distinction an `Option` cannot
/// carry: *we discarded that* is not *we never had that*. A bitemporal store
/// that answers a truncated read as though the window were the whole history
/// is worse than one that refuses, because the refusal is visible and the
/// wrong answer is not.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureLookup<'a> {
    /// The value in force at the instant asked for, known by the instant asked.
    Value(&'a FeatureValue),
    /// Nothing satisfies the read: no value was ever recorded for this feature
    /// and subject, none had arrived by `known_at`, or the nearest one is past
    /// the feature's staleness bound. The store is not hiding anything.
    NoValue,
    /// The read reaches into history this series has evicted. The answer is
    /// unavailable *here*; it is in the event log, which is the record.
    Truncated {
        /// Earliest valid time still retained. A read at or after this instant
        /// that still yields nothing failed on the `known_at` dimension.
        earliest_retained: Timestamp,
        /// Earliest valid time the series ever covered.
        oldest_ever_held: Timestamp,
        /// How many values this series has discarded.
        evicted: u64,
    },
}

impl<'a> FeatureLookup<'a> {
    /// The value, if the read produced one. A truncated read is not a value.
    pub fn value(self) -> Option<&'a FeatureValue> {
        match self {
            Self::Value(value) => Some(value),
            Self::NoValue | Self::Truncated { .. } => None,
        }
    }

    /// Whether the read fell outside the retained window.
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}

/// Feature values, indexed by feature and subject.
///
/// Every series is bounded by [`FeatureStore::history_limit`]; see
/// [`FEATURE_HISTORY`] for why the store owns the bound rather than its
/// callers. The recording sites are on a per-tick path and cannot be trusted
/// to remember a cap they do not own.
#[derive(Debug)]
pub struct FeatureStore {
    definitions: BTreeMap<String, Feature>,
    /// (feature, subject) to values, kept in valid-time order.
    values: BTreeMap<(String, String), Series>,
    /// Values retained per series. Fixed at construction; never zero.
    history_limit: usize,
}

impl Default for FeatureStore {
    fn default() -> Self {
        Self {
            definitions: BTreeMap::new(),
            values: BTreeMap::new(),
            history_limit: FEATURE_HISTORY,
        }
    }
}

impl FeatureStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store retaining `history_limit` values per series.
    ///
    /// Refuses zero rather than clamping it to one. A caller asking for a
    /// zero-length history has a bug — a configuration read from the wrong
    /// key, an arithmetic slip — and a store that silently substitutes one
    /// would answer every point-in-time read from a single value while
    /// reporting nothing wrong. Correcting the input hides the caller's defect
    /// and keeps it in production; refusing it stops the process where the
    /// mistake is still legible.
    pub fn with_history(history_limit: usize) -> Result<Self> {
        if history_limit == 0 {
            return Err(Error::invalid(
                "feature history limit must be at least 1; pass the number of \
                 observations to retain per (feature, subject) series, or use \
                 FeatureStore::new() for the default of 512",
            ));
        }
        Ok(Self {
            definitions: BTreeMap::new(),
            values: BTreeMap::new(),
            history_limit,
        })
    }

    /// Values retained per `(feature, subject)` series.
    pub const fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// Values discarded across every series to stay inside the bound.
    ///
    /// Non-zero means some point-in-time question can no longer be answered
    /// from this store. That is intended — but it should be known, not
    /// discovered from an answer that came back wrong.
    pub fn evictions(&self) -> u64 {
        self.values.values().map(|series| series.evicted).sum()
    }

    /// Values discarded from one series.
    pub fn evictions_for(&self, feature: &str, subject: &str) -> u64 {
        self.values
            .get(&(feature.to_string(), subject.to_string()))
            .map_or(0, |series| series.evicted)
    }

    /// The valid-time window one series still covers, oldest first.
    ///
    /// A read before the first element is answered by
    /// [`FeatureLookup::Truncated`], not by the first element.
    pub fn retained_window(&self, feature: &str, subject: &str) -> Option<(Timestamp, Timestamp)> {
        let series = self
            .values
            .get(&(feature.to_string(), subject.to_string()))?;
        match (series.values.first(), series.values.last()) {
            (Some(first), Some(last)) => Some((first.valid_at, last.valid_at)),
            _ => None,
        }
    }

    pub fn define(&mut self, feature: Feature) {
        self.definitions.insert(feature.name.clone(), feature);
    }

    pub fn definition(&self, name: &str) -> Option<&Feature> {
        self.definitions.get(name)
    }

    pub fn definitions(&self) -> impl Iterator<Item = &Feature> {
        self.definitions.values()
    }

    pub fn feature_count(&self) -> usize {
        self.definitions.len()
    }

    /// Values currently retained. Bounded by construction; not a count of
    /// everything ever recorded — [`FeatureStore::evictions`] holds the rest.
    pub fn value_count(&self) -> usize {
        self.values.values().map(|series| series.values.len()).sum()
    }

    /// Record a value, keeping the series ordered by valid time and bounded.
    pub fn record(&mut self, feature: &str, subject: &str, value: FeatureValue) {
        let limit = self.history_limit;
        let series = self
            .values
            .entry((feature.to_string(), subject.to_string()))
            .or_default();
        match series
            .values
            .binary_search_by_key(&value.valid_at.as_nanos(), |v| v.valid_at.as_nanos())
        {
            // A restatement for the same instant replaces the earlier value.
            Ok(position) => series.values[position] = value,
            Err(position) => series.values.insert(position, value),
        }
        series.trim(limit);
    }

    /// Record many values for one series in a single merge.
    ///
    /// Semantically identical to calling [`FeatureStore::record`] once per
    /// value — same ordering invariant, same restatement rule (a later value
    /// for the same instant replaces the earlier one, within this batch as
    /// against the stored series). It exists because the loop is quadratic
    /// where this is linear: a feed handing over history typically arrives
    /// newest-first, and each of *n* front-inserts into a sorted series moves
    /// the whole series, which at feed rates is the difference between a
    /// second and a minute.
    pub fn record_many(&mut self, feature: &str, subject: &str, mut values: Vec<FeatureValue>) {
        if values.is_empty() {
            return;
        }
        // Stable sort by valid time, so equal-instant values keep the caller's
        // order and the later one wins the restatement below — exactly what a
        // sequence of `record` calls would have done.
        values.sort_by_key(|value| value.valid_at.as_nanos());
        let limit = self.history_limit;
        let series = self
            .values
            .entry((feature.to_string(), subject.to_string()))
            .or_default();

        let existing = std::mem::take(&mut series.values);
        let mut merged: Vec<FeatureValue> = Vec::with_capacity(existing.len() + values.len());
        // A restatement replaces rather than duplicates: two values for one
        // instant would make "the value as of t" ambiguous.
        fn push_replacing(series: &mut Vec<FeatureValue>, value: FeatureValue) {
            match series.last_mut() {
                Some(last) if last.valid_at == value.valid_at => *last = value,
                _ => series.push(value),
            }
        }
        let mut old_iter = existing.into_iter().peekable();
        let mut new_iter = values.into_iter().peekable();
        loop {
            let ordering = match (old_iter.peek(), new_iter.peek()) {
                (None, None) => break,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(old), Some(new)) => old.valid_at.as_nanos().cmp(&new.valid_at.as_nanos()),
            };
            match ordering {
                std::cmp::Ordering::Less => {
                    if let Some(value) = old_iter.next() {
                        push_replacing(&mut merged, value);
                    }
                }
                std::cmp::Ordering::Greater => {
                    if let Some(value) = new_iter.next() {
                        push_replacing(&mut merged, value);
                    }
                }
                // The stored value and a new one describe the same instant:
                // the new one is the restatement and the old one is dropped.
                std::cmp::Ordering::Equal => {
                    old_iter.next();
                    if let Some(value) = new_iter.next() {
                        push_replacing(&mut merged, value);
                    }
                }
            }
        }
        series.values = merged;
        // The same bound as `record`, applied to the batch as a whole: a feed
        // handing over more history than the window holds keeps the newest of
        // it, and the count says how much it handed over that we did not keep.
        series.trim(limit);
    }

    /// The value for `subject` as of a point in both time dimensions.
    ///
    /// Returns the most recent value that both describes an instant at or
    /// before `valid_at` and had arrived by `known_at`. `None` covers both
    /// "no such value" and "that history has been evicted"; use
    /// [`FeatureStore::lookup_as_of`] where the difference matters. What it
    /// never does is return the oldest surviving value in place of an evicted
    /// one — retention narrows what can be answered, it does not change an
    /// answer.
    pub fn value_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<&FeatureValue> {
        self.lookup_as_of(feature, subject, valid_at, known_at)
            .value()
    }

    /// The point-in-time read, saying which kind of nothing it found.
    ///
    /// Eviction is oldest-first and the series is in valid-time order, so a
    /// value the window still holds is still the correct answer: everything
    /// discarded is older than everything kept, and an older value never
    /// outranks a newer one for the same read. The case retention does change
    /// is a read whose answer was discarded, and that one is reported as
    /// [`FeatureLookup::Truncated`] rather than answered from the stump.
    pub fn lookup_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> FeatureLookup<'_> {
        let Some(series) = self.values.get(&(feature.to_string(), subject.to_string())) else {
            return FeatureLookup::NoValue;
        };
        let candidate = series
            .values
            .iter()
            .rfind(|v| v.valid_at <= valid_at && v.available_at <= known_at);

        let Some(candidate) = candidate else {
            // Nothing retained answers the read. If the read reaches at or
            // before the earliest instant this series ever covered, the answer
            // may have been evicted, and saying so is the difference between
            // "widen the window" and "go and find the data".
            return match (series.oldest_ever_held, series.values.first()) {
                (Some(oldest_ever_held), Some(first)) if valid_at >= oldest_ever_held => {
                    FeatureLookup::Truncated {
                        earliest_retained: first.valid_at,
                        oldest_ever_held,
                        evicted: series.evicted,
                    }
                }
                _ => FeatureLookup::NoValue,
            };
        };

        // Beyond its staleness window a feature is not a value, it is a memory.
        if let Some(definition) = self.definitions.get(feature) {
            let age = valid_at.since(candidate.valid_at);
            if age > definition.max_staleness {
                return FeatureLookup::NoValue;
            }
        }
        FeatureLookup::Value(candidate)
    }

    /// The current value, given everything known now.
    pub fn current(&self, feature: &str, subject: &str, now: Timestamp) -> Option<&FeatureValue> {
        self.value_as_of(feature, subject, now, now)
    }

    /// The retained history for a subject, as known at `known_at`.
    ///
    /// The retained history, not the full one: at most
    /// [`FeatureStore::history_limit`] values, and
    /// [`FeatureStore::evictions_for`] says how many older ones are only in
    /// the event log now.
    pub fn history(&self, feature: &str, subject: &str, known_at: Timestamp) -> Vec<&FeatureValue> {
        self.values
            .get(&(feature.to_string(), subject.to_string()))
            .map(|series| {
                series
                    .values
                    .iter()
                    .filter(|v| v.available_at <= known_at)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A feature vector for one subject, using only what was known.
    ///
    /// Missing features are reported rather than defaulted: a zero in a feature
    /// vector is a value, and substituting one for "unknown" is how a model
    /// ends up trained on a fact that was never true.
    pub fn vector_as_of(
        &self,
        features: &[String],
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> (Vec<f64>, Vec<String>) {
        let mut values = Vec::with_capacity(features.len());
        let mut missing = Vec::new();
        for feature in features {
            match self.value_as_of(feature, subject, valid_at, known_at) {
                Some(value) => values.push(value.value),
                None => {
                    values.push(f64::NAN);
                    missing.push(feature.clone());
                }
            }
        }
        (values, missing)
    }

    /// Subjects with a value for a feature at a point in time.
    pub fn subjects_with(
        &self,
        feature: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<String> {
        self.values
            .keys()
            .filter(|(name, _)| name == feature)
            .map(|(_, subject)| subject.clone())
            .filter(|subject| {
                self.value_as_of(feature, subject, valid_at, known_at)
                    .is_some()
            })
            .collect()
    }

    /// A cross-section of one feature across subjects, for ranking.
    pub fn cross_section(
        &self,
        feature: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = self
            .subjects_with(feature, valid_at, known_at)
            .into_iter()
            .filter_map(|subject| {
                self.value_as_of(feature, &subject, valid_at, known_at)
                    .map(|v| (subject, v.value))
            })
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

/// Retention: what the store keeps, what it drops, and what it refuses to
/// answer once it has dropped something.
///
/// These tests exist because the absorption path records into this store once
/// per trade and once per tick, and until the bound below it kept every one of
/// them for the life of the process. The neighbouring series in the kernel
/// were bounded and tested; this store was left out of both.
#[cfg(test)]
mod retention_tests {
    use super::*;

    /// One observation per second from the epoch, so a value's index and its
    /// valid time are the same number and an assertion can name either.
    fn tick(index: i64) -> FeatureValue {
        FeatureValue::immediate(f64::from(i32::try_from(index).unwrap()), at(index))
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::from_secs(second)
    }

    #[test]
    fn a_store_refuses_a_zero_history_limit_and_admits_the_smallest_real_one() {
        // The refusal is only half the property. A gate that rejects every
        // value reads as a working gate and is not one, so assert the smallest
        // legitimate limit is admitted in the same breath.
        let refused = FeatureStore::with_history(0);
        let Err(error) = refused else {
            panic!("a zero-length history is a caller bug and must be refused, not clamped to 1");
        };
        assert_eq!(error.code(), "invalid");
        assert!(
            error.message().contains("at least 1"),
            "the refusal must name what to do instead, got {:?}",
            error.message()
        );

        let admitted = FeatureStore::with_history(1).expect("one value per series is legitimate");
        assert_eq!(admitted.history_limit(), 1);
    }

    #[test]
    fn a_series_driven_past_its_limit_retains_exactly_the_limit() {
        let mut store = FeatureStore::with_history(8).expect("8 is a valid limit");
        // Premise: fill to the bound without crossing it, and confirm nothing
        // was dropped. A retention test that never reaches capacity proves
        // nothing about retention.
        for second in 0..8 {
            store.record("close", "obj-1", tick(second));
        }
        assert_eq!(
            store.value_count(),
            8,
            "the window should be full, not over"
        );
        assert_eq!(store.evictions(), 0, "nothing evicted before the bound");

        for second in 8..40 {
            store.record("close", "obj-1", tick(second));
        }
        assert_eq!(
            store.value_count(),
            8,
            "the series must stay at its limit however long the feed runs"
        );
    }

    #[test]
    fn overflow_evicts_the_oldest_observation_first() {
        let mut store = FeatureStore::with_history(4).expect("4 is a valid limit");
        for second in 0..4 {
            store.record("close", "obj-1", tick(second));
        }
        // Premise: before the overflow the window starts at the first tick.
        assert_eq!(
            store.retained_window("close", "obj-1"),
            Some((at(0), at(3))),
            "the full window before any eviction"
        );

        for second in 4..7 {
            store.record("close", "obj-1", tick(second));
        }
        assert_eq!(
            store.retained_window("close", "obj-1"),
            Some((at(3), at(6))),
            "the three oldest go, the three newest arrive; newest is never dropped"
        );
        let retained: Vec<f64> = store
            .history("close", "obj-1", at(1_000))
            .iter()
            .map(|value| value.value)
            .collect();
        assert_eq!(
            retained,
            vec![3.0, 4.0, 5.0, 6.0],
            "oldest-first eviction, in valid-time order, matching push_bounded"
        );
    }

    #[test]
    fn evictions_are_counted_so_a_short_window_is_visible_from_the_store() {
        let mut store = FeatureStore::with_history(3).expect("3 is a valid limit");
        for second in 0..3 {
            store.record("close", "obj-1", tick(second));
        }
        // Premise: the counter is zero while the window suffices, so a
        // non-zero count below is caused by the overflow and not by existing.
        assert_eq!(store.evictions(), 0);
        assert_eq!(store.evictions_for("close", "obj-1"), 0);

        for second in 3..13 {
            store.record("close", "obj-1", tick(second));
        }
        assert_eq!(
            store.evictions_for("close", "obj-1"),
            10,
            "ten ticks past a three-deep window is ten values discarded"
        );
        assert_eq!(store.evictions(), 10, "and the store total agrees");
        assert_eq!(
            store.evictions_for("close", "obj-2"),
            0,
            "eviction is per series, not per store"
        );
    }

    #[test]
    fn a_read_behind_the_retained_window_reports_truncation_rather_than_the_oldest_survivor() {
        let mut store = FeatureStore::with_history(4).expect("4 is a valid limit");
        for second in 0..4 {
            store.record("close", "obj-1", tick(second));
        }
        // Premise: the read is answerable, and answered with the value that
        // was in force at that instant — not the one that will survive.
        let before = store
            .value_as_of("close", "obj-1", at(1), at(1))
            .expect("second 1 is inside the window while it is retained");
        assert!(
            (before.value - 1.0).abs() < f64::EPSILON,
            "the read answers with the tick from second 1"
        );

        for second in 4..10 {
            store.record("close", "obj-1", tick(second));
        }

        // The same read, after the answer has been evicted. Returning the
        // oldest survivor (second 6) would be a wrong answer wearing the shape
        // of a right one, and a backtest would never notice.
        assert!(
            store.value_as_of("close", "obj-1", at(1), at(1)).is_none(),
            "a truncated read has no value, least of all the oldest survivor"
        );
        match store.lookup_as_of("close", "obj-1", at(1), at(1)) {
            FeatureLookup::Truncated {
                earliest_retained,
                oldest_ever_held,
                evicted,
            } => {
                assert_eq!(
                    earliest_retained,
                    at(6),
                    "the window now starts at second 6"
                );
                assert_eq!(oldest_ever_held, at(0), "it once started at second 0");
                assert_eq!(evicted, 6);
            }
            other => panic!("a read into evicted history must say so, got {other:?}"),
        }
    }

    #[test]
    fn a_read_of_an_instant_the_series_never_covered_is_not_reported_as_truncation() {
        // The distinction the Truncated arm exists for cuts both ways: "we
        // discarded that" must not be claimed for data that never existed,
        // or the operator widens a window that was never the problem.
        let mut store = FeatureStore::with_history(2).expect("2 is a valid limit");
        for second in 10..20 {
            store.record("close", "obj-1", tick(second));
        }
        // Premise: this series has evicted, so a Truncated answer is available
        // to be given wrongly.
        assert_eq!(store.evictions_for("close", "obj-1"), 8);
        assert_eq!(
            store.lookup_as_of("close", "obj-1", at(5), at(100)),
            FeatureLookup::NoValue,
            "second 5 precedes everything the series ever held"
        );
        assert_eq!(
            store.lookup_as_of("close", "obj-2", at(15), at(100)),
            FeatureLookup::NoValue,
            "and an unknown subject was never truncated either"
        );
    }

    #[test]
    fn a_batch_longer_than_the_window_keeps_the_newest_of_it() {
        // A feed handing over history in one call must not be a way around the
        // bound that per-tick recording obeys.
        let mut store = FeatureStore::with_history(3).expect("3 is a valid limit");
        let batch: Vec<FeatureValue> = (0..9).map(tick).collect();
        assert_eq!(
            batch.len(),
            9,
            "premise: the batch is longer than the window"
        );
        store.record_many("close", "obj-1", batch);

        assert_eq!(store.value_count(), 3);
        assert_eq!(store.evictions_for("close", "obj-1"), 6);
        assert_eq!(
            store.retained_window("close", "obj-1"),
            Some((at(6), at(8))),
            "the newest three of the batch survive"
        );
    }

    #[test]
    fn the_default_store_is_bounded_at_the_kernel_series_depth() {
        // `FeatureStore::new()` is what the world model constructs and what
        // the per-tick absorption path therefore writes into. If the bound
        // reached only the explicitly configured constructor it would never
        // reach the defect.
        let mut store = FeatureStore::new();
        assert_eq!(store.history_limit(), FEATURE_HISTORY);
        assert_eq!(FEATURE_HISTORY, 512, "matching the kernel's SERIES_HISTORY");

        let overshoot = i64::try_from(FEATURE_HISTORY).expect("512 fits") + 100;
        for second in 0..overshoot {
            store.record("close", "obj-1", tick(second));
        }
        assert_eq!(
            store.value_count(),
            FEATURE_HISTORY,
            "612 ticks into a default store retain 512"
        );
        assert_eq!(store.evictions(), 100);
    }
}
