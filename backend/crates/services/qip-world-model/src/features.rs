//! The point-in-time feature store.
//!
//! A feature value has two timestamps for the same reason a graph fact does:
//! when it was true, and when it became computable. A backtest that reads a
//! feature by valid time alone will read values assembled from data that had
//! not arrived yet, and the resulting strategy will look excellent and lose
//! money. [`FeatureStore::value_as_of`] takes both and will not return
//! otherwise.

use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
}

/// Feature values, indexed by feature and subject.
#[derive(Debug, Default)]
pub struct FeatureStore {
    definitions: BTreeMap<String, Feature>,
    /// (feature, subject) to values, kept in valid-time order.
    values: BTreeMap<(String, String), Vec<FeatureValue>>,
}

impl FeatureStore {
    pub fn new() -> Self {
        Self::default()
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

    pub fn value_count(&self) -> usize {
        self.values.values().map(Vec::len).sum()
    }

    /// Record a value, keeping the series ordered by valid time.
    pub fn record(&mut self, feature: &str, subject: &str, value: FeatureValue) {
        let series = self
            .values
            .entry((feature.to_string(), subject.to_string()))
            .or_default();
        match series.binary_search_by_key(&value.valid_at.as_nanos(), |v| v.valid_at.as_nanos()) {
            // A restatement for the same instant replaces the earlier value.
            Ok(position) => series[position] = value,
            Err(position) => series.insert(position, value),
        }
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
        let series = self
            .values
            .entry((feature.to_string(), subject.to_string()))
            .or_default();

        let existing = std::mem::take(series);
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
        *series = merged;
    }

    /// The value for `subject` as of a point in both time dimensions.
    ///
    /// Returns the most recent value that both describes an instant at or
    /// before `valid_at` and had arrived by `known_at`.
    pub fn value_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<&FeatureValue> {
        let series = self
            .values
            .get(&(feature.to_string(), subject.to_string()))?;
        let candidate = series
            .iter()
            .rfind(|v| v.valid_at <= valid_at && v.available_at <= known_at)?;

        // Beyond its staleness window a feature is not a value, it is a memory.
        if let Some(definition) = self.definitions.get(feature) {
            let age = valid_at.since(candidate.valid_at);
            if age > definition.max_staleness {
                return None;
            }
        }
        Some(candidate)
    }

    /// The current value, given everything known now.
    pub fn current(&self, feature: &str, subject: &str, now: Timestamp) -> Option<&FeatureValue> {
        self.value_as_of(feature, subject, now, now)
    }

    /// The full history for a subject, as known at `known_at`.
    pub fn history(&self, feature: &str, subject: &str, known_at: Timestamp) -> Vec<&FeatureValue> {
        self.values
            .get(&(feature.to_string(), subject.to_string()))
            .map(|series| {
                series
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
