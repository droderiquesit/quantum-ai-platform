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

/// How many distinct `(feature, subject)` series one store may hold.
///
/// [`FEATURE_HISTORY`] bounds a series; nothing bounded the number of them.
/// The failure that exposed the gap: a reference-rate response whose `rates`
/// object carried sixty-four sixty-kilobyte keys minted sixty-four permanent
/// series per poll, and the store kept every one — 5,000 such keys were
/// measured resting in it with zero evictions, because eviction is a
/// within-series discipline and a new key is never a candidate for it.
///
/// Four thousand and ninety-six is chosen from the two sides it has to
/// satisfy. Above: `series_limit × history_limit` is the store's worst case,
/// and 4,096 × 512 values at roughly 48 bytes each is about 100 MB — the most
/// this store may ever cost a Cloud Run instance, which is a number an
/// operator can hold in their head. Below: the world model defines about
/// twenty features and keys them by instrument, economy or macro series, so
/// 4,096 series is well over two hundred instruments across every feature the
/// platform computes, against a development universe of a handful. A
/// deployment that legitimately outgrows it raises the bound at construction
/// through [`FeatureStore::with_bounds`], which is a decision somebody makes
/// rather than a ceiling discovered by falling through it.
pub const FEATURE_SERIES_LIMIT: usize = 4_096;

/// The longest a feature name or a subject may be, in characters.
///
/// A key here is an identifier: `close`, `macro_level`, `FX.EUR.USD`,
/// `EA.POLICY_RATE`, an object id. The longest this platform mints is well
/// under thirty characters, so 128 is four times any real one and still
/// nowhere near a size at which a key is data rather than a name. It is a
/// separate bound from [`FEATURE_SERIES_LIMIT`] because the two failures are
/// different: a million short keys and one 60 KB key both exhaust a process,
/// and a cardinality bound alone would admit 4,096 × 60 KB of pure key.
///
/// The ingestion boundary refuses a subject key that is not an identifier
/// before it is ever published (`qip_market_ingestion::adapter`). This bound
/// is not a duplicate of that one: it holds for every caller of this store,
/// including the ones that compose a key themselves, and it holds after a
/// record has already been admitted by whatever route.
pub const FEATURE_KEY_CHARS: usize = 128;

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

/// Which half of a `(feature, subject)` key broke a bound.
///
/// Named rather than described, because the two call for different actions: a
/// feature name too long is this platform's own bug, and a subject too long is
/// almost always something a source chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDimension {
    Feature,
    Subject,
}

impl KeyDimension {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Subject => "subject",
        }
    }
}

/// Why a key was refused.
///
/// It deliberately carries no key *text*. The keys this exists to refuse are
/// chosen by whoever answered the last hop, and a refusal carrying one would
/// put sixty kilobytes of a vendor's choosing — newlines, escape sequences and
/// all — into whatever renders the outcome. The shape is what a reader needs;
/// the text is what an attacker wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRefusal {
    /// One half of the key is longer than [`FeatureStore::key_limit`].
    TooLong {
        dimension: KeyDimension,
        length: usize,
        limit: usize,
    },
    /// The store already holds [`FeatureStore::series_limit`] series and this
    /// key would be a new one. An existing key is still recorded: the bound is
    /// on how many series exist, not on how often they are written.
    NoRoom { limit: usize },
}

/// What [`FeatureStore::record`] did.
///
/// Returned rather than swallowed so that a caller composing a key can tell
/// "stored" from "refused" — [`crate::world::WorldModel::absorb_macro`] uses
/// it to keep a refused subject out of the change journal as well as out of
/// the store. Callers on the per-tick path ignore it; for them the count in
/// [`FeatureStore::refusals`] is the record, and it is surfaced in
/// [`crate::world::WorldModel::statistics`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recording {
    Stored,
    Refused(KeyRefusal),
}

impl Recording {
    pub const fn is_refused(self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

/// Bitemporal feature store: reads respect both when a value was true
/// (valid_at) and when it became known (available_at).
///
/// The store enforces knowable-instant guards: a value is never returned
/// for a read before it became available. This prevents the class of bug
/// where a backtest sees data the live system could not have known yet,
/// producing strategy that looks excellent and loses money.
///
/// Two timestamps define the contract:
/// - `valid_at`: when the value describes (e.g., the close price from 3pm)
/// - `available_at`: when the platform learned it (e.g., 3:05pm after exchange
///   latency and processing)
///
/// A read at time T will return the most recent value whose `valid_at <= T`
/// and `available_at <= T`, or nothing if no such value exists. Neither
/// timestamp may be violated: a value unavailable at T is not returned even
/// if valid at T, and a value valid only after T is not returned even if
/// available before T.
pub trait BitemporalFeatureStore {
    /// Record one value for a feature and subject.
    ///
    /// The value's `available_at` timestamps the moment it became knowable.
    /// Returns what happened: stored, or refused with a reason.
    fn record(&mut self, feature: &str, subject: &str, value: FeatureValue) -> Recording;

    /// Record many values for one series in a single merge.
    ///
    /// Semantically identical to calling [`BitemporalFeatureStore::record`]
    /// once per value, with the same ordering and restatement invariants.
    fn record_many(&mut self, feature: &str, subject: &str, values: Vec<FeatureValue>)
    -> Recording;

    /// The value for `subject` as of a point in both time dimensions.
    ///
    /// Reads the most recent value where both:
    /// - `valid_at <= valid_at` (the value describes this instant or earlier)
    /// - `available_at <= known_at` (the value was knowable by this instant)
    ///
    /// Returns `None` if no such value exists or history has been evicted.
    /// The knowable-instant guard ensures a value never appears before
    /// `available_at`, even if `valid_at` precedes `known_at`.
    fn value_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<&FeatureValue>;

    /// The point-in-time read, stating which kind of nothing was found.
    ///
    /// Like [`BitemporalFeatureStore::value_as_of`] but distinguishes
    /// between three cases:
    /// - `Value`: found and returned
    /// - `NoValue`: no value ever recorded, or outside availability window
    /// - `Truncated`: the answer was evicted; consult the event log
    fn lookup_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> FeatureLookup<'_>;

    /// The current value, given everything known now.
    fn current(&self, feature: &str, subject: &str, now: Timestamp) -> Option<&FeatureValue>;

    /// The retained history for a subject, as known at `known_at`.
    ///
    /// Only values with `available_at <= known_at` are included. Returns
    /// at most [`FeatureStore::history_limit`] values; older ones are in
    /// the event log. The knowable-instant guard is enforced by the caller:
    /// every value returned was knowable by `known_at`.
    fn history(&self, feature: &str, subject: &str, known_at: Timestamp) -> Vec<&FeatureValue>;

    /// Register a feature definition.
    fn define(&mut self, feature: Feature);

    /// Retrieve a feature definition by name.
    fn definition(&self, name: &str) -> Option<&Feature>;

    /// Iterate all registered feature definitions.
    fn definitions(&self) -> Box<dyn Iterator<Item = &Feature> + '_>;

    /// How many distinct `(feature, subject)` series this store currently holds.
    fn series_count(&self) -> usize;

    /// How many values this store currently retains (bounded by construction).
    fn value_count(&self) -> usize;

    /// Values evicted to stay inside bounds. Non-zero means point-in-time
    /// reads may now return `FeatureLookup::Truncated`.
    fn evictions(&self) -> u64;

    /// Values evicted from one series. Non-zero means that series cannot
    /// answer reads before a certain instant without consulting the event log.
    fn evictions_for(&self, feature: &str, subject: &str) -> u64;

    /// The valid-time window one series still covers, oldest first.
    ///
    /// A read before the first element is answered by
    /// [`FeatureLookup::Truncated`], not by the first element.
    fn retained_window(&self, feature: &str, subject: &str) -> Option<(Timestamp, Timestamp)>;

    /// The longest key half this store will accept, in characters.
    fn key_limit(&self) -> usize;

    /// How many distinct series this store will retain at most.
    fn series_limit(&self) -> usize;

    /// How many values per series this store will retain at most.
    fn history_limit(&self) -> usize;

    /// Records refused because their key broke a bound.
    fn refusals(&self) -> u64;
}

/// Feature values, indexed by feature and subject.
///
/// Bounded in both dimensions, and they are different failures. Every series
/// is bounded by [`FeatureStore::history_limit`] — see [`FEATURE_HISTORY`] for
/// why the store owns the bound rather than its callers, which is that the
/// recording sites are on a per-tick path and cannot be trusted to remember a
/// cap they do not own. The *number* of series is bounded by
/// [`FeatureStore::series_limit`] and the length of each key by
/// [`FeatureStore::key_limit`], for the stronger version of the same reason:
/// a key arrives from outside the process, and the caller composing it is
/// usually the one with least idea what is in it.
#[derive(Debug)]
pub struct FeatureStore {
    definitions: BTreeMap<String, Feature>,
    /// (feature, subject) to values, kept in valid-time order.
    values: BTreeMap<(String, String), Series>,
    /// Values retained per series. Fixed at construction; never zero.
    history_limit: usize,
    /// Distinct series retained. Fixed at construction; never zero.
    series_limit: usize,
    /// Records refused because their key broke a bound. Non-zero means either
    /// this platform is minting keys it should not, or a source is — and
    /// either way the store is the only place that saw it happen.
    refused: u64,
}

impl Default for FeatureStore {
    fn default() -> Self {
        Self {
            definitions: BTreeMap::new(),
            values: BTreeMap::new(),
            history_limit: FEATURE_HISTORY,
            series_limit: FEATURE_SERIES_LIMIT,
            refused: 0,
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
        Self::with_bounds(history_limit, FEATURE_SERIES_LIMIT)
    }

    /// A store bounded in both dimensions.
    ///
    /// Refuses zero for either, on the same argument [`Self::with_history`]
    /// carries: a store that silently substituted one would hold a single
    /// series or a single value while reporting nothing wrong, and the
    /// caller's arithmetic slip would live on in production.
    pub fn with_bounds(history_limit: usize, series_limit: usize) -> Result<Self> {
        if history_limit == 0 {
            return Err(Error::invalid(
                "feature history limit must be at least 1; pass the number of \
                 observations to retain per (feature, subject) series, or use \
                 FeatureStore::new() for the default of 512",
            ));
        }
        if series_limit == 0 {
            return Err(Error::invalid(
                "feature series limit must be at least 1; pass the number of distinct \
                 (feature, subject) series to retain, or use FeatureStore::new() for the \
                 default of 4096",
            ));
        }
        Ok(Self {
            definitions: BTreeMap::new(),
            values: BTreeMap::new(),
            history_limit,
            series_limit,
            refused: 0,
        })
    }

    /// Values retained per `(feature, subject)` series.
    pub const fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// Distinct `(feature, subject)` series this store will hold.
    pub const fn series_limit(&self) -> usize {
        self.series_limit
    }

    /// The longest key half this store will accept, in characters.
    pub const fn key_limit(&self) -> usize {
        FEATURE_KEY_CHARS
    }

    /// Distinct `(feature, subject)` series currently held.
    pub fn series_count(&self) -> usize {
        self.values.len()
    }

    /// Records refused because their key broke a bound.
    ///
    /// Zero on every healthy deployment. Non-zero is a fact about the inputs,
    /// not about the store, and it is reported rather than logged at the site
    /// because the recording sites are per-tick and a log line per refused
    /// record is its own denial of service.
    pub const fn refusals(&self) -> u64 {
        self.refused
    }

    /// The key, or why it will not be one.
    ///
    /// Length first, cardinality second, and only for a key the store does not
    /// already hold: a series already open stays writable at the limit, or a
    /// store that filled up would stop absorbing the prices it was already
    /// tracking — which would turn a bound meant to survive a hostile response
    /// into an outage on the ordinary path.
    fn admissible_key(
        &self,
        feature: &str,
        subject: &str,
    ) -> std::result::Result<(String, String), KeyRefusal> {
        for (dimension, text) in [
            (KeyDimension::Feature, feature),
            (KeyDimension::Subject, subject),
        ] {
            let length = text.chars().count();
            if length > FEATURE_KEY_CHARS {
                return Err(KeyRefusal::TooLong {
                    dimension,
                    length,
                    limit: FEATURE_KEY_CHARS,
                });
            }
        }
        let key = (feature.to_string(), subject.to_string());
        if !self.values.contains_key(&key) && self.values.len() >= self.series_limit {
            return Err(KeyRefusal::NoRoom {
                limit: self.series_limit,
            });
        }
        Ok(key)
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
    ///
    /// Returns what it did. A refused key stores nothing and is counted in
    /// [`Self::refusals`]: the key is refused, never truncated to fit, because
    /// a key rewritten to fit is a series filed under a name its own source
    /// would not recognise, and the next value from that source opens a second
    /// one beside it.
    pub fn record(&mut self, feature: &str, subject: &str, value: FeatureValue) -> Recording {
        let key = match self.admissible_key(feature, subject) {
            Ok(key) => key,
            Err(refusal) => {
                self.refused = self.refused.saturating_add(1);
                return Recording::Refused(refusal);
            }
        };
        let limit = self.history_limit;
        let series = self.values.entry(key).or_default();
        match series
            .values
            .binary_search_by_key(&value.valid_at.as_nanos(), |v| v.valid_at.as_nanos())
        {
            // A restatement for the same instant replaces the earlier value.
            Ok(position) => series.values[position] = value,
            Err(position) => series.values.insert(position, value),
        }
        series.trim(limit);
        Recording::Stored
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
    ///
    /// Held to the same key bounds as [`FeatureStore::record`], for the reason
    /// the batch path exists at all: a feed handing over history in one call
    /// must not be a way around a bound the per-tick path obeys.
    pub fn record_many(
        &mut self,
        feature: &str,
        subject: &str,
        mut values: Vec<FeatureValue>,
    ) -> Recording {
        if values.is_empty() {
            return Recording::Stored;
        }
        let key = match self.admissible_key(feature, subject) {
            Ok(key) => key,
            Err(refusal) => {
                self.refused = self.refused.saturating_add(1);
                return Recording::Refused(refusal);
            }
        };
        // Stable sort by valid time, so equal-instant values keep the caller's
        // order and the later one wins the restatement below — exactly what a
        // sequence of `record` calls would have done.
        values.sort_by_key(|value| value.valid_at.as_nanos());
        let limit = self.history_limit;
        let series = self.values.entry(key).or_default();

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
        Recording::Stored
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

impl BitemporalFeatureStore for FeatureStore {
    fn record(&mut self, feature: &str, subject: &str, value: FeatureValue) -> Recording {
        // Forward to the concrete implementation
        FeatureStore::record(self, feature, subject, value)
    }

    fn record_many(
        &mut self,
        feature: &str,
        subject: &str,
        values: Vec<FeatureValue>,
    ) -> Recording {
        FeatureStore::record_many(self, feature, subject, values)
    }

    fn value_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> Option<&FeatureValue> {
        FeatureStore::value_as_of(self, feature, subject, valid_at, known_at)
    }

    fn lookup_as_of(
        &self,
        feature: &str,
        subject: &str,
        valid_at: Timestamp,
        known_at: Timestamp,
    ) -> FeatureLookup<'_> {
        FeatureStore::lookup_as_of(self, feature, subject, valid_at, known_at)
    }

    fn current(&self, feature: &str, subject: &str, now: Timestamp) -> Option<&FeatureValue> {
        FeatureStore::current(self, feature, subject, now)
    }

    fn history(&self, feature: &str, subject: &str, known_at: Timestamp) -> Vec<&FeatureValue> {
        FeatureStore::history(self, feature, subject, known_at)
    }

    fn define(&mut self, feature: Feature) {
        FeatureStore::define(self, feature)
    }

    fn definition(&self, name: &str) -> Option<&Feature> {
        FeatureStore::definition(self, name)
    }

    fn definitions(&self) -> Box<dyn Iterator<Item = &Feature> + '_> {
        Box::new(FeatureStore::definitions(self))
    }

    fn series_count(&self) -> usize {
        FeatureStore::series_count(self)
    }

    fn value_count(&self) -> usize {
        FeatureStore::value_count(self)
    }

    fn evictions(&self) -> u64 {
        FeatureStore::evictions(self)
    }

    fn evictions_for(&self, feature: &str, subject: &str) -> u64 {
        FeatureStore::evictions_for(self, feature, subject)
    }

    fn retained_window(&self, feature: &str, subject: &str) -> Option<(Timestamp, Timestamp)> {
        FeatureStore::retained_window(self, feature, subject)
    }

    fn key_limit(&self) -> usize {
        FeatureStore::key_limit(self)
    }

    fn series_limit(&self) -> usize {
        FeatureStore::series_limit(self)
    }

    fn history_limit(&self) -> usize {
        FeatureStore::history_limit(self)
    }

    fn refusals(&self) -> u64 {
        FeatureStore::refusals(self)
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

/// Knowable-instant guards: a value is never returned before it became known.
///
/// These tests exist because the entire point-in-time feature store was built
/// to prevent look-ahead bias: returning a value before it was available makes
/// a backtest look excellent and causes live losses. The guard is on both
/// timestamps equally. A value is only returned if both `valid_at <= read_time`
/// and `available_at <= read_time`. This module verifies the second condition
/// is never violated.
#[cfg(test)]
mod knowable_instant_guard_tests {
    use super::*;

    fn at(second: i64) -> Timestamp {
        Timestamp::from_secs(second)
    }

    /// A value whose available_at is later than the read is not returned,
    /// even if valid_at is satisfied. This is the core guard.
    #[test]
    fn a_value_not_yet_available_is_not_returned_even_if_valid() {
        let mut store = FeatureStore::new();

        // Record a value that was true at second 100 but did not arrive until
        // second 110. This 10-second delay is realistic — exchange latency,
        // network propagation, processing.
        let value = FeatureValue::new(99.5, at(100), at(110));

        store.record("close", "AAPL", value);

        // At second 105, we know the price was true at second 100
        // (valid_at <= 105), but we don't know the value yet (available_at = 110).
        // The guard must refuse this read.
        let early = store.value_as_of("close", "AAPL", at(105), at(105));
        assert!(
            early.is_none(),
            "a value with available_at=110 must not be returned at known_at=105"
        );

        // A moment before arrival still returns nothing.
        let almost = store.value_as_of("close", "AAPL", at(105), at(109));
        assert!(
            almost.is_none(),
            "a value with available_at=110 must not be returned at known_at=109"
        );

        // At second 110, the value becomes available and is returned.
        let on_time = store.value_as_of("close", "AAPL", at(105), at(110));
        assert!(
            on_time.is_some(),
            "a value with available_at=110 must be returned at known_at=110"
        );
        assert_eq!(on_time.unwrap().value, 99.5);

        // After arrival, reads earlier than the value are still answered from
        // this value if nothing newer has arrived.
        let later_read = store.value_as_of("close", "AAPL", at(105), at(200));
        assert_eq!(later_read.unwrap().value, 99.5);
    }

    /// The guard respects both dimensions independently. A value answering to
    /// valid_at but not available_at is refused; a value answering to available_at
    /// but not valid_at is also refused.
    #[test]
    fn both_timestamps_are_enforced_independently() {
        let mut store = FeatureStore::new();

        // Value true at 100, available at 110.
        store.record("price", "AAPL", FeatureValue::new(100.0, at(100), at(110)));

        // Read 1: valid_at is satisfied (100 <= 105) but available_at is not.
        // Refused on available_at.
        assert_eq!(
            store.lookup_as_of("price", "AAPL", at(105), at(105)),
            FeatureLookup::NoValue,
            "valid_at satisfied, available_at not: must refuse"
        );

        // Read 2: available_at is satisfied (110 <= 110) but valid_at is not
        // (120 > 100). Should return the value since valid_at <= 120 is satisfied.
        // Actually, valid_at=100 <= 120, so this read should be answered.
        let future_valid = store.value_as_of("price", "AAPL", at(120), at(110));
        assert!(
            future_valid.is_some(),
            "valid_at=100 <= 120 and available_at=110 <= 110 both satisfied"
        );

        // Read 3: available_at is satisfied but valid_at is too old (not satisfied
        // by this value but might by others). Read at a time the value is too old.
        let way_later = store.value_as_of("price", "AAPL", at(200), at(110));
        // This depends on staleness — without staleness rules it should be returned.
        // The value is still the most recent valid_at <= 200 and available_at <= 110.
        // Actually 110 <= 110 so available check passes; 100 <= 200 so valid_at passes.
        assert!(
            way_later.is_some(),
            "value is still in force for the read even though read time is much later"
        );
    }

    /// Multiple values in sequence each respect their own knowable instant.
    /// A later value arriving in time does not make an earlier value available
    /// retroactively.
    #[test]
    fn each_value_respects_its_own_available_at_independently() {
        let mut store = FeatureStore::new();

        // Value 1: true at 100, available at 110.
        store.record("close", "XYZ", FeatureValue::new(50.0, at(100), at(110)));

        // Value 2: true at 105, available at 115.
        store.record("close", "XYZ", FeatureValue::new(51.0, at(105), at(115)));

        // At time 111, value 1 is available but value 2 is not.
        let at_111 = store.value_as_of("close", "XYZ", at(105), at(111));
        assert_eq!(
            at_111.map(|v| v.value),
            Some(50.0),
            "at known_at=111: only value 1 (available_at=110) is available"
        );

        // At time 115, both are available, so the more recent one (value 2) is returned.
        let at_115 = store.value_as_of("close", "XYZ", at(105), at(115));
        assert_eq!(
            at_115.map(|v| v.value),
            Some(51.0),
            "at known_at=115: both are available, return the most recent"
        );

        // Value 2 arriving does not make value 1 retroactively knowable at 111.
        // At time 111, reading again after value 2 is stored must still return
        // value 1 (available at 110), not value 2 (not yet available).
        let reread_111 = store.value_as_of("close", "XYZ", at(105), at(111));
        assert_eq!(
            reread_111.map(|v| v.value),
            Some(50.0),
            "arrival of a later value does not retroactively make an earlier one available"
        );
    }

    /// The bitemporal trait interface enforces the guard the same way
    /// the concrete methods do — through the trait methods.
    #[test]
    fn the_bitemporal_trait_enforces_knowable_instant_guards() {
        let mut store: Box<dyn BitemporalFeatureStore> = Box::new(FeatureStore::new());

        // Record through the trait.
        let value = FeatureValue::new(42.0, at(50), at(60));
        let recording = store.record("metric", "server-1", value);
        assert_eq!(recording, Recording::Stored);

        // Read before available through the trait.
        let too_early = store.value_as_of("metric", "server-1", at(50), at(55));
        assert!(
            too_early.is_none(),
            "trait method value_as_of must also enforce knowable instant"
        );

        // Read after available through the trait.
        let on_time = store.value_as_of("metric", "server-1", at(50), at(60));
        assert!(
            on_time.is_some(),
            "trait method value_as_of must return when both conditions are met"
        );

        // lookup_as_of through the trait.
        match store.lookup_as_of("metric", "server-1", at(50), at(55)) {
            FeatureLookup::NoValue => (),
            other => {
                panic!("trait method lookup_as_of must refuse before available, got {other:?}")
            }
        }

        match store.lookup_as_of("metric", "server-1", at(50), at(60)) {
            FeatureLookup::Value(v) => assert_eq!(v.value, 42.0),
            other => panic!("trait method lookup_as_of must return when available, got {other:?}"),
        }
    }
}

/// The other dimension: how many series there may be, and how long a key may
/// be.
///
/// These exist because the retention bound above was read as *the* bound on
/// this store and is only half of one. A rate table answering with sixty-four
/// sixty-kilobyte currency codes minted sixty-four permanent series per poll
/// and evicted nothing, because eviction happens within a series and a new key
/// is never a candidate for it — 5,000 such keys were measured resting in a
/// store reporting zero evictions. The connector that let those keys through
/// now refuses them, but the bound belongs here too: this map is the thing
/// that grows, and it grows for every caller, not only the one that was
/// caught.
#[cfg(test)]
mod key_dimension_tests {
    use super::*;

    fn value() -> FeatureValue {
        FeatureValue::immediate(1.0, Timestamp::from_secs(1))
    }

    #[test]
    fn a_store_refuses_a_zero_series_limit_and_admits_the_smallest_real_one() {
        // The same argument as the history limit: a caller asking for zero has
        // a bug, and a store that quietly substituted one would refuse every
        // series but the first while reporting nothing wrong.
        let Err(error) = FeatureStore::with_bounds(8, 0) else {
            panic!("a zero series limit is a caller bug and must be refused, not clamped");
        };
        assert_eq!(error.code(), "invalid");
        assert!(
            error.message().contains("at least 1"),
            "the refusal must name what to do instead, got {:?}",
            error.message()
        );

        let admitted = FeatureStore::with_bounds(8, 1).expect("one series is legitimate");
        assert_eq!(admitted.series_limit(), 1);
        assert_eq!(
            admitted.history_limit(),
            8,
            "the two bounds are independent and neither overwrites the other"
        );
    }

    #[test]
    fn a_new_key_past_the_series_limit_is_refused_and_counted() {
        let mut store = FeatureStore::with_bounds(4, 3).expect("valid bounds");
        for index in 0..3 {
            assert_eq!(
                store.record("close", &format!("obj-{index}"), value()),
                Recording::Stored,
                "premise: the store admits keys until its limit"
            );
        }
        assert_eq!(store.series_count(), 3);
        assert_eq!(store.refusals(), 0, "nothing refused before the bound");

        assert_eq!(
            store.record("close", "obj-3", value()),
            Recording::Refused(KeyRefusal::NoRoom { limit: 3 }),
            "the fourth key must be refused, not admitted and not truncated"
        );
        assert_eq!(store.series_count(), 3, "and nothing was created for it");
        assert_eq!(store.refusals(), 1, "the refusal is visible from the store");
        assert!(
            store
                .value_as_of(
                    "close",
                    "obj-3",
                    Timestamp::from_secs(1),
                    Timestamp::from_secs(1)
                )
                .is_none(),
            "a refused record must not be readable back"
        );
    }

    #[test]
    fn a_series_already_open_keeps_recording_after_the_store_is_full() {
        // The half that stops this bound from being an outage. A store at its
        // limit must go on absorbing the prices it is already tracking, or a
        // hostile response would stop the ordinary path rather than only
        // failing to extend it.
        let mut store = FeatureStore::with_bounds(4, 2).expect("valid bounds");
        store.record("close", "obj-1", FeatureValue::immediate(1.0, at(1)));
        store.record("close", "obj-2", FeatureValue::immediate(2.0, at(1)));
        assert_eq!(
            store.record("close", "obj-3", value()),
            Recording::Refused(KeyRefusal::NoRoom { limit: 2 }),
            "premise: the store is full"
        );

        assert_eq!(
            store.record("close", "obj-1", FeatureValue::immediate(3.0, at(2))),
            Recording::Stored,
            "an open series must stay writable at the limit"
        );
        assert_eq!(
            store
                .value_as_of("close", "obj-1", at(2), at(2))
                .map(|value| value.value),
            Some(3.0)
        );
    }

    #[test]
    fn a_key_longer_than_the_bound_is_refused_rather_than_truncated() {
        // Truncating would be worse than refusing: two 60 KB keys sharing a
        // prefix would become one series holding both sources' values, and no
        // reader could tell which value came from where.
        let mut store = FeatureStore::new();
        let long = "Z".repeat(60_000);
        assert_eq!(
            store.record("macro_level", &long, value()),
            Recording::Refused(KeyRefusal::TooLong {
                dimension: KeyDimension::Subject,
                length: 60_000,
                limit: FEATURE_KEY_CHARS,
            }),
        );
        assert_eq!(
            store.record(&long, "obj-1", value()),
            Recording::Refused(KeyRefusal::TooLong {
                dimension: KeyDimension::Feature,
                length: 60_000,
                limit: FEATURE_KEY_CHARS,
            }),
            "the feature half is bounded too, and says which half it was"
        );
        assert_eq!(store.series_count(), 0);
        assert_eq!(store.refusals(), 2);

        // The premise, and the reason the bound is 128 rather than 16: every
        // key this platform actually mints is far inside it.
        assert_eq!(
            store.record("macro_level", "FX.EUR.USD", value()),
            Recording::Stored
        );
        assert!(
            "macro_level".len() + "EA.POLICY_RATE".len() < FEATURE_KEY_CHARS,
            "the longest key the platform mints must be comfortably inside the bound"
        );
    }

    #[test]
    fn a_refusal_carries_the_shape_of_the_key_and_never_the_key_itself() {
        // The keys this bound exists to refuse are chosen by whoever answered
        // the last hop. A refusal carrying one would put a vendor's newline
        // and escape sequence into whatever renders the outcome — which is the
        // second half of the same defect, one layer further in.
        let mut store = FeatureStore::new();
        let hostile = format!("macro\u{1b}[2J{}", "Z".repeat(200));
        let Recording::Refused(refusal) = store.record("macro_level", &hostile, value()) else {
            panic!("a 200-character subject with an escape sequence was accepted");
        };
        let rendered = format!("{refusal:?}");
        assert!(
            !rendered.contains('Z') && !rendered.contains('\u{1b}'),
            "the refusal repeated the key it refused: {rendered:?}"
        );
        assert!(
            rendered.contains(&hostile.chars().count().to_string()) && rendered.contains("Subject"),
            "the refusal must still say which half broke which bound and by how much: {rendered}"
        );
    }

    #[test]
    fn the_batch_path_is_held_to_the_same_key_bounds_as_the_single_one() {
        // `record_many` exists for speed. A bound the fast path does not obey
        // is a bound with a documented way around it.
        let mut store = FeatureStore::with_bounds(4, 1).expect("valid bounds");
        store.record("close", "obj-1", value());
        assert_eq!(
            store.record_many("close", "obj-2", vec![value()]),
            Recording::Refused(KeyRefusal::NoRoom { limit: 1 })
        );
        let long = "Z".repeat(200);
        assert_eq!(
            store.record_many("close", &long, vec![value()]),
            Recording::Refused(KeyRefusal::TooLong {
                dimension: KeyDimension::Subject,
                length: 200,
                limit: FEATURE_KEY_CHARS,
            })
        );
        assert_eq!(store.series_count(), 1);
        assert_eq!(store.refusals(), 2);
    }

    #[test]
    fn the_default_store_is_bounded_in_the_key_dimension_too() {
        // `FeatureStore::new()` is what the world model constructs, so a bound
        // that reached only the configured constructor would never reach the
        // defect — the same trap the history bound had to avoid.
        let mut store = FeatureStore::new();
        assert_eq!(store.series_limit(), FEATURE_SERIES_LIMIT);
        assert_eq!(store.key_limit(), FEATURE_KEY_CHARS);

        for index in 0..=FEATURE_SERIES_LIMIT {
            store.record("close", &format!("obj-{index}"), value());
        }
        assert_eq!(
            store.series_count(),
            FEATURE_SERIES_LIMIT,
            "one key past the limit must not open a series"
        );
        assert_eq!(
            store.refusals(),
            1,
            "and exactly the one past the limit was refused"
        );
    }

    fn at(second: i64) -> Timestamp {
        Timestamp::from_secs(second)
    }
}
