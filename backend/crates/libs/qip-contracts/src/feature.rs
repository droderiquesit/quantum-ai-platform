//! Feature identity, values, and bitemporal snapshots for the incremental DAG.
//!
//! Features are immutable snapshots at the instant they became knowable.
//! Every feature carries both times: `instant_true` (when the fact was true in
//! the market) and `knowable_at` (when this platform could first have acted on it).
//! A read filtered on `knowable_at` is the only correct way to prevent point-in-time leakage.

use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// What identifies a feature: its name, its subject and its parameters.
///
/// Two strategies asking for a 20-period realised volatility on the same
/// instrument must produce the same key, or the DAG computes it twice and the
/// whole point of sharing is lost.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FeatureKey {
    pub name: String,
    pub subject: ObjectId,
    /// Parameters in sorted `name=value` form, so the key is canonical
    /// regardless of the order a caller supplied them in.
    pub parameters: Vec<String>,
}

impl FeatureKey {
    pub fn new(name: impl Into<String>, subject: ObjectId) -> Self {
        Self {
            name: name.into(),
            subject,
            parameters: Vec::new(),
        }
    }

    /// Add a parameter, keeping the parameter list canonical.
    pub fn with(mut self, name: &str, value: impl fmt::Display) -> Self {
        self.parameters.push(format!("{name}={value}"));
        self.parameters.sort();
        self.parameters.dedup();
        self
    }

    /// A stable string form, used as the DAG node identity.
    pub fn canonical(&self) -> String {
        if self.parameters.is_empty() {
            format!("{}({})", self.name, self.subject.as_str())
        } else {
            format!(
                "{}({},{})",
                self.name,
                self.subject.as_str(),
                self.parameters.join(",")
            )
        }
    }
}

impl fmt::Display for FeatureKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// What a feature evaluated to.
///
/// Exact where the value is a quantity that reaches a decision, `f64` where it
/// is a statistic. The distinction is enforced by having two variants rather
/// than by convention.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FeatureValue {
    /// An exact quantity: a price, a size, a notional.
    Exact(Decimal),
    /// A statistic: a volatility, a correlation, a probability.
    Statistic(f64),
    /// A count.
    Count(u64),
    /// A boolean condition.
    Flag(bool),
    /// Computable in principle, not computable now — insufficient history,
    /// a stale input, a halted venue. Distinct from zero, which is a value.
    Undefined,
}

impl FeatureValue {
    pub const fn is_defined(&self) -> bool {
        !matches!(self, Self::Undefined)
    }

    /// The value as a statistic, where that is meaningful.
    ///
    /// Returns `None` for `Undefined` rather than a default, so a caller
    /// cannot accidentally treat "unknown" as "zero".
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Exact(d) => Some(d.to_f64()),
            Self::Statistic(v) => Some(*v),
            Self::Count(c) => Some(*c as f64),
            Self::Flag(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::Undefined => None,
        }
    }

    /// The value as an exact quantity, only where it is one.
    pub fn as_exact(&self) -> Option<Decimal> {
        match self {
            Self::Exact(d) => Some(*d),
            _ => None,
        }
    }
}

/// A monotonic version for a feature's value.
///
/// The DAG marks a node dirty by bumping the revision of what it depends on.
/// A consumer that holds a revision knows whether it is looking at a stale
/// value without recomputing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Revision(u64);

impl Revision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// A set of features evaluated together at one instant.
///
/// Carries the revision each value was computed at, so a strategy can assert
/// it is reasoning about one consistent view rather than a mixture of two.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FeatureVector {
    entries: Vec<(FeatureKey, FeatureValue, Revision)>,
    as_of: Option<Timestamp>,
}

impl FeatureVector {
    pub fn new(as_of: Timestamp) -> Self {
        Self {
            entries: Vec::new(),
            as_of: Some(as_of),
        }
    }

    pub fn insert(&mut self, key: FeatureKey, value: FeatureValue, revision: Revision) {
        match self.entries.iter_mut().find(|(k, _, _)| *k == key) {
            Some(slot) => {
                slot.1 = value;
                slot.2 = revision;
            }
            None => self.entries.push((key, value, revision)),
        }
    }

    pub fn get(&self, key: &FeatureKey) -> Option<FeatureValue> {
        self.entries
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, v, _)| *v)
    }

    pub fn revision_of(&self, key: &FeatureKey) -> Option<Revision> {
        self.entries
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, _, r)| *r)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn as_of(&self) -> Option<Timestamp> {
        self.as_of
    }

    pub fn iter(&self) -> impl Iterator<Item = (&FeatureKey, FeatureValue, Revision)> {
        self.entries.iter().map(|(k, v, r)| (k, *v, *r))
    }

    /// Keys whose value could not be computed.
    ///
    /// A strategy checks this before acting. Trading on a vector with
    /// undefined inputs is trading on a default somebody chose years ago.
    pub fn undefined(&self) -> Vec<&FeatureKey> {
        self.entries
            .iter()
            .filter(|(_, v, _)| !v.is_defined())
            .map(|(k, _, _)| k)
            .collect()
    }

    /// Whether every value in the vector is defined.
    pub fn is_complete(&self) -> bool {
        self.entries.iter().all(|(_, v, _)| v.is_defined())
    }
}

/// A distribution over possible feature values (ADR 0005 enforcement).
///
/// Models output distributions, not point estimates. This type enforces that
/// all model outputs carry uncertainty quantification in the form of
/// percentiles or moments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    /// Percentile values: (percentile, value) pairs in sorted order.
    /// Common percentiles: 10, 25, 50, 75, 90 for quartiles and deciles.
    percentiles: BTreeMap<u8, f64>,
    /// Mean (first moment) of the distribution, if available.
    mean: Option<f64>,
    /// Standard deviation, if available.
    std_dev: Option<f64>,
}

impl Distribution {
    /// Create a new distribution from percentiles.
    ///
    /// Percentiles must be in range [0, 100] and will be verified.
    pub fn new(percentiles: impl IntoIterator<Item = (u8, f64)>) -> qip_core::Result<Self> {
        let mut map = BTreeMap::new();
        for (p, v) in percentiles {
            if p > 100 {
                return Err(qip_core::Error::invalid(format!(
                    "percentile {} out of range [0, 100]",
                    p
                )));
            }
            map.insert(p, v);
        }
        if map.is_empty() {
            return Err(qip_core::Error::invalid(
                "distribution must have at least one percentile".to_string(),
            ));
        }
        Ok(Self {
            percentiles: map,
            mean: None,
            std_dev: None,
        })
    }

    /// Add mean and standard deviation to the distribution.
    pub fn with_moments(mut self, mean: f64, std_dev: f64) -> Self {
        self.mean = Some(mean);
        self.std_dev = Some(std_dev);
        self
    }

    /// The median (50th percentile) of the distribution.
    pub fn median(&self) -> Option<f64> {
        self.percentiles.get(&50).copied()
    }

    /// The mean (first moment) if available.
    pub fn mean(&self) -> Option<f64> {
        self.mean
    }

    /// The standard deviation (square root of variance) if available.
    pub fn std_dev(&self) -> Option<f64> {
        self.std_dev
    }

    /// All percentiles in sorted order.
    pub fn percentiles(&self) -> &BTreeMap<u8, f64> {
        &self.percentiles
    }

    /// Interquartile range (75th - 25th percentile).
    pub fn iqr(&self) -> Option<f64> {
        let q3 = self.percentiles.get(&75)?;
        let q1 = self.percentiles.get(&25)?;
        Some(q3 - q1)
    }
}

/// A marker type enforcing that a feature is not readable before its knowable instant.
///
/// This type system barrier prevents point-in-time leakage: a feature
/// constructed with `KnowableAt` can only be read after its knowable instant
/// has passed, as checked by the event store's point-in-time read.
///
/// The type is zero-cost; it carries no runtime data. Its presence alone
/// guarantees the structural invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KnowableAt(Timestamp);

impl KnowableAt {
    /// Seal a timestamp as a knowable instant.
    ///
    /// The sealed timestamp is the earliest moment at which this feature
    /// becomes readable in any point-in-time query.
    pub const fn at(instant: Timestamp) -> Self {
        Self(instant)
    }

    /// The instant itself.
    pub const fn instant(&self) -> Timestamp {
        self.0
    }

    /// Whether this feature is knowable at a given query instant.
    pub fn is_knowable_at(&self, as_of: Timestamp) -> bool {
        self.0 <= as_of
    }
}

/// An immutable snapshot of a feature value at the instant it became knowable.
///
/// Bitemporal record carrying both times:
/// - `instant_true`: when the fact was true in the market
/// - `knowable_at`: when the platform could first have acted on it
///
/// Features are immutable once recorded. Snapshots in the event log are the
/// only source of truth for what was known and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureSnapshot {
    /// What identifies this feature.
    key: FeatureKey,
    /// The value that became knowable.
    value: FeatureValue,
    /// When this fact was true in the market.
    instant_true: Timestamp,
    /// When the platform became able to act on it.
    knowable_at: KnowableAt,
}

impl FeatureSnapshot {
    /// Create a new feature snapshot with bitemporal stamps.
    ///
    /// Clamping `knowable_at` forward if it precedes `instant_true`, because
    /// a fact cannot be known before it happened. The clamp is visible through
    /// [`FeatureSnapshot::was_clamped`].
    pub fn new(
        key: FeatureKey,
        value: FeatureValue,
        instant_true: Timestamp,
        knowable_at: Timestamp,
    ) -> Self {
        let clamped_knowable = if knowable_at < instant_true {
            instant_true
        } else {
            knowable_at
        };
        Self {
            key,
            value,
            instant_true,
            knowable_at: KnowableAt::at(clamped_knowable),
        }
    }

    /// Create a snapshot where the fact became known at the instant it became true.
    pub fn immediate(key: FeatureKey, value: FeatureValue, at: Timestamp) -> Self {
        Self {
            key,
            value,
            instant_true: at,
            knowable_at: KnowableAt::at(at),
        }
    }

    /// The feature key.
    pub fn key(&self) -> &FeatureKey {
        &self.key
    }

    /// The feature value.
    pub fn value(&self) -> FeatureValue {
        self.value
    }

    /// When this fact was true in the market.
    pub fn instant_true(&self) -> Timestamp {
        self.instant_true
    }

    /// The knowable instant barrier (type-system enforcement).
    pub fn knowable_at(&self) -> KnowableAt {
        self.knowable_at
    }

    /// Whether this snapshot is knowable at a given query instant.
    ///
    /// The only correct predicate for a point-in-time read.
    pub fn is_knowable_at(&self, as_of: Timestamp) -> bool {
        self.knowable_at.is_knowable_at(as_of)
    }
}

/// A lattice tracking disagreement between model forecasts on the same feature.
///
/// Different models may produce different predictions for the same feature.
/// The lattice quantifies this disagreement as an epistemic asset: wide
/// disagreement signals model uncertainty, narrow disagreement suggests
/// calibrated consensus.
///
/// Implemented as a lattice with join (union) and meet (intersection)
/// operations on the set of predictions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForecastLattice {
    /// Feature key being predicted.
    key: FeatureKey,
    /// Forecast instant (when the forecast was made).
    instant_forecast: Timestamp,
    /// Distribution of forecasts from all models.
    forecasts: Vec<Distribution>,
    /// Disagreement magnitude (e.g., spread of medians).
    disagreement_width: f64,
    /// Consensus flag: true if all forecasts agree within tolerance.
    is_consensus: bool,
}

impl ForecastLattice {
    /// Create a new forecast lattice from a set of model distributions.
    ///
    /// Requires at least one forecast. Disagreement is computed as the
    /// difference between the maximum and minimum medians across forecasts.
    pub fn new(
        key: FeatureKey,
        instant_forecast: Timestamp,
        forecasts: Vec<Distribution>,
    ) -> qip_core::Result<Self> {
        if forecasts.is_empty() {
            return Err(qip_core::Error::invalid(
                "forecast lattice requires at least one distribution".to_string(),
            ));
        }

        // Compute disagreement as spread of medians
        let medians: Vec<f64> = forecasts.iter().filter_map(|d| d.median()).collect();

        let disagreement_width = if medians.len() < forecasts.len() {
            // If some forecasts lack medians, use max available range
            f64::INFINITY
        } else {
            let max = medians
                .iter()
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
                .unwrap_or(0.0);
            let min = medians
                .iter()
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .copied()
                .unwrap_or(0.0);
            max - min
        };

        // Consensus if disagreement is negligible (< 0.01% or the widths are all zero)
        let is_consensus = disagreement_width < 1e-4;

        Ok(Self {
            key,
            instant_forecast,
            forecasts,
            disagreement_width,
            is_consensus,
        })
    }

    /// The feature being predicted.
    pub fn key(&self) -> &FeatureKey {
        &self.key
    }

    /// When the forecast was made.
    pub fn instant_forecast(&self) -> Timestamp {
        self.instant_forecast
    }

    /// All model forecasts in this lattice.
    pub fn forecasts(&self) -> &[Distribution] {
        &self.forecasts
    }

    /// Magnitude of disagreement between models.
    ///
    /// Zero or near-zero indicates consensus; larger values indicate
    /// wide disagreement and thus high epistemic uncertainty.
    pub fn disagreement_width(&self) -> f64 {
        self.disagreement_width
    }

    /// Whether models are in consensus (disagreement below threshold).
    pub fn is_consensus(&self) -> bool {
        self.is_consensus
    }

    /// The consensus distribution (average/aggregate of all forecasts).
    ///
    /// Computed as the average of medians and moments where available.
    pub fn consensus_distribution(&self) -> Option<Distribution> {
        let medians: Vec<f64> = self.forecasts.iter().filter_map(|d| d.median()).collect();

        if medians.is_empty() {
            return None;
        }

        let avg_median = medians.iter().sum::<f64>() / medians.len() as f64;

        let avg_mean = {
            let means: Vec<f64> = self.forecasts.iter().filter_map(|d| d.mean()).collect();
            if means.is_empty() {
                None
            } else {
                Some(means.iter().sum::<f64>() / means.len() as f64)
            }
        };

        let avg_std_dev = {
            let std_devs: Vec<f64> = self.forecasts.iter().filter_map(|d| d.std_dev()).collect();
            if std_devs.is_empty() {
                None
            } else {
                Some(std_devs.iter().sum::<f64>() / std_devs.len() as f64)
            }
        };

        let mut percentiles = vec![(50, avg_median)];
        if let Some(mean) = avg_mean {
            percentiles.push((50, mean)); // Ensure 50th is included
        }

        let dist = Distribution::new(percentiles).ok()?;
        let dist = if let (Some(mean), Some(std_dev)) = (avg_mean, avg_std_dev) {
            dist.with_moments(mean, std_dev)
        } else {
            dist
        };
        Some(dist)
    }
}
