//! Feature identity and values for the incremental DAG.

use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};
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
