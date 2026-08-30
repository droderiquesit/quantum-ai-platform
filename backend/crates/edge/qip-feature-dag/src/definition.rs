//! What a feature is, and what it is allowed to read.

use crate::state::{InstrumentState, MarketReads, MarketState};
use qip_contracts::{FeatureKey, FeatureValue};
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp};
use std::fmt;

/// The shape of a defined feature value, without the value.
///
/// The DAG checks each computed value against the kind its definition
/// declared. A feature that quietly changes from an exact quantity to a
/// statistic changes what every consumer downstream is allowed to do with it,
/// and the strategy compiler type-checks against the declaration rather than
/// against whatever the last evaluation happened to produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValueKind {
    /// An exact quantity: a price, a size, a notional.
    Exact,
    /// A statistic: a volatility, a correlation, a ratio.
    Statistic,
    /// A count.
    Count,
    /// A boolean condition.
    Flag,
}

impl ValueKind {
    /// The kind of a value, or `None` for [`FeatureValue::Undefined`], which
    /// is every kind's absence rather than a kind of its own.
    pub const fn of(value: &FeatureValue) -> Option<Self> {
        match value {
            FeatureValue::Exact(_) => Some(Self::Exact),
            FeatureValue::Statistic(_) => Some(Self::Statistic),
            FeatureValue::Count(_) => Some(Self::Count),
            FeatureValue::Flag(_) => Some(Self::Flag),
            FeatureValue::Undefined => None,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Statistic => "statistic",
            Self::Count => "count",
            Self::Flag => "flag",
        }
    }
}

/// Everything a definition may read while computing.
///
/// The context is the only way in. A definition that reached for a clock, a
/// global or the previous evaluation's output would not be replayable, and the
/// DAG's central claim — that an incremental evaluation equals a full one — is
/// only true of pure definitions.
#[derive(Debug)]
pub struct FeatureContext<'a> {
    as_of: Timestamp,
    state: &'a MarketState,
    dependencies: &'a [FeatureValue],
    max_staleness: Duration,
}

impl<'a> FeatureContext<'a> {
    pub fn new(
        as_of: Timestamp,
        state: &'a MarketState,
        dependencies: &'a [FeatureValue],
        max_staleness: Duration,
    ) -> Self {
        Self {
            as_of,
            state,
            dependencies,
            max_staleness,
        }
    }

    /// The instant the whole evaluation pass is reasoning about.
    pub const fn as_of(&self) -> Timestamp {
        self.as_of
    }

    /// The value of the `index`th declared dependency.
    ///
    /// Positional, matching the order [`FeatureDefinition::dependencies`]
    /// returned. Out of range reads as undefined rather than panicking: a
    /// definition and its declaration disagreeing is a bug, but it is not a
    /// reason to take the hot path down.
    pub fn dependency(&self, index: usize) -> FeatureValue {
        self.dependencies
            .get(index)
            .copied()
            .unwrap_or(FeatureValue::Undefined)
    }

    /// Every dependency value, in declaration order.
    pub const fn dependencies(&self) -> &'a [FeatureValue] {
        self.dependencies
    }

    /// An instrument's state whatever its age.
    ///
    /// For the features that measure staleness itself. Everything else wants
    /// [`FeatureContext::fresh`].
    pub fn instrument(&self, object_id: &ObjectId) -> Option<&'a InstrumentState> {
        self.state.instrument(object_id)
    }

    /// An instrument's state, or `None` when it has gone stale.
    ///
    /// A price from a feed that stopped an hour ago is not a price. Returning
    /// it would let a strategy quote against a market that has moved on.
    pub fn fresh(&self, object_id: &ObjectId) -> Option<&'a InstrumentState> {
        self.state
            .instrument(object_id)
            .filter(|state| !state.is_stale(self.as_of, self.max_staleness))
    }

    /// The grid mids are sampled onto, needed to annualise anything measured
    /// per sample.
    pub fn sample_interval(&self) -> Duration {
        self.state.sample_interval()
    }

    /// Samples per year on the sampling grid, the annualisation factor for a
    /// per-sample volatility.
    pub fn samples_per_year(&self) -> f64 {
        let years = self.sample_interval().as_years_f64();
        if years > 0.0 { 1.0 / years } else { 0.0 }
    }
}

/// A feature: a pure function from dependency values and market state to one
/// value.
///
/// Purity is the contract. The DAG reuses a value whenever nothing it declared
/// as an input has changed, so a definition that reads anything it did not
/// declare — or that returns a different answer for the same inputs — makes
/// the cached value and the recomputed value differ, and the cell then trades
/// on whichever one it happened to hold.
pub trait FeatureDefinition: fmt::Debug {
    /// What this feature is called and what it is computed on.
    fn key(&self) -> FeatureKey;

    /// The features whose current values this one is computed from.
    ///
    /// Order is significant: it is the order dependency values arrive in.
    fn dependencies(&self) -> Vec<FeatureKey> {
        Vec::new()
    }

    /// The instruments whose market state this feature reads.
    ///
    /// A message about an instrument that is not named here cannot dirty this
    /// node. Under-declaring is a correctness bug; over-declaring only costs
    /// recomputation.
    fn subjects(&self) -> Vec<ObjectId> {
        vec![self.key().subject]
    }

    /// Which parts of that state it reads.
    fn reads(&self) -> MarketReads {
        MarketReads::NONE
    }

    /// The kind of value this definition produces when it is defined at all.
    fn value_kind(&self) -> ValueKind;

    /// Whether the value moves with the evaluation instant even when no
    /// message has arrived.
    ///
    /// True only for features *of* the passage of time — a time since the last
    /// print goes stale on its own. Everything else answers the same way until
    /// something changes, which is what makes reuse safe.
    fn time_sensitive(&self) -> bool {
        false
    }

    /// Compute the value, or [`FeatureValue::Undefined`] when it cannot be
    /// computed from what is known.
    ///
    /// `Err` is for a definition that has been given something impossible, not
    /// for missing history. Insufficient history is an ordinary state of the
    /// world and is reported as undefined.
    fn compute(&self, ctx: &FeatureContext<'_>) -> Result<FeatureValue>;
}
