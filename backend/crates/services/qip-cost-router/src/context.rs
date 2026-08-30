//! What is being decided, and under what conditions.
//!
//! The router is given a decision, not a question. It never sees the prompt,
//! the features or the order — only what the answer is worth, how long it may
//! take, how sure it has to be, whether it is allowed to be an estimate at all,
//! and the market conditions it is being made under. That is deliberate: a
//! router that could read the question would eventually route on the question,
//! and the reasoning behind a routing decision has to be reproducible from what
//! is recorded next to it.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_financial::asset_class::AssetClass;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Whether the answer is allowed to be an estimate.
///
/// The distinction the whole router is built around. [`Determinism::Required`]
/// is not a preference for reproducibility — it says this decision has no
/// acceptable failure mode that a model could be trusted with. Pre-trade risk
/// checks, limit arithmetic, kill switches and the order path are in that
/// class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    /// The answer must be a function of its inputs. Nothing above the
    /// deterministic rung may see this decision — see [`crate::Routing`],
    /// where that is a property of the types rather than of a check.
    Required,
    /// The answer may be an estimate, provided it carries the confidence the
    /// decision asks for.
    NotRequired,
}

/// A market or jurisdiction a decision is being made in.
///
/// A newtype rather than a string so a region cannot be silently compared
/// against a venue, an asset class or a free-text label. Reputation is scoped
/// by region, and a mis-keyed score reads as a model that has never been tried
/// here — which is the safe direction, but only if the key is a type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Region(String);

impl Region {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the market is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketRegime {
    /// Prices are going somewhere and continuing to.
    Trending,
    /// Prices come back. Most of the time, until they do not.
    MeanReverting,
    /// Correlations go to one and liquidity goes to nothing.
    Crisis,
    /// The book is thin enough that the price is an opinion.
    Illiquid,
    /// Nothing is happening, which is its own regime and not the absence of
    /// one: a model calibrated in quiet markets is a model with no experience
    /// of the day that matters.
    Quiet,
}

impl MarketRegime {
    pub const ALL: [Self; 5] = [
        Self::Trending,
        Self::MeanReverting,
        Self::Crisis,
        Self::Illiquid,
        Self::Quiet,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Trending => "trending",
            Self::MeanReverting => "mean_reverting",
            Self::Crisis => "crisis",
            Self::Illiquid => "illiquid",
            Self::Quiet => "quiet",
        }
    }
}

/// How violently prices are moving, independently of direction.
///
/// Separate from [`MarketRegime`] because they come apart: a quiet market can
/// be quietly volatile, and a trending one can trend calmly. A model that is
/// good at trends is not thereby good at trends in a high-volatility tape, and
/// scoring it on one axis hides exactly that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolatilityRegime {
    Low,
    Normal,
    High,
    Extreme,
}

impl VolatilityRegime {
    pub const ALL: [Self; 4] = [Self::Low, Self::Normal, Self::High, Self::Extreme];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Extreme => "extreme",
        }
    }
}

/// How far ahead the decision reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Horizon {
    /// Inside the round trip. Nothing above deterministic code arrives in time.
    Microsecond,
    Intraday,
    Daily,
    Weekly,
    /// Months. The horizon where a wrong answer is discovered slowly and
    /// costs the most.
    Strategic,
}

impl Horizon {
    pub const ALL: [Self; 5] = [
        Self::Microsecond,
        Self::Intraday,
        Self::Daily,
        Self::Weekly,
        Self::Strategic,
    ];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Microsecond => "microsecond",
            Self::Intraday => "intraday",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Strategic => "strategic",
        }
    }
}

/// The conditions a decision is being made under.
///
/// Doubles as the key a model's reputation is scored against, and that is not
/// an economy: a reputation keyed on anything other than the conditions the
/// decision is actually being made in is a reputation for a different
/// decision. Keeping one type means the routing record and the score it was
/// looked up with cannot drift apart.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Conditions {
    pub asset_class: AssetClass,
    pub region: Region,
    pub regime: MarketRegime,
    pub volatility: VolatilityRegime,
    pub horizon: Horizon,
}

impl Conditions {
    pub fn new(
        asset_class: AssetClass,
        region: Region,
        regime: MarketRegime,
        volatility: VolatilityRegime,
        horizon: Horizon,
    ) -> Self {
        Self {
            asset_class,
            region,
            regime,
            volatility,
            horizon,
        }
    }

    /// A stable, readable key for a routing record.
    pub fn label(&self) -> String {
        format!(
            "{:?}/{}/{}/{}/{}",
            self.asset_class,
            self.region,
            self.regime.as_str(),
            self.volatility.as_str(),
            self.horizon.as_str()
        )
    }
}

/// One decision the router is asked to place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionContext {
    /// What is being decided, for the record that follows the decision.
    pub subject: String,
    /// What getting this right is worth, in the currency the tier costs are
    /// quoted in.
    ///
    /// Not the notional. The value at stake is the difference between the good
    /// answer and the bad one — quoting the notional here makes every rung look
    /// affordable, which is the failure the affordability rule exists to catch.
    pub value_at_stake: Decimal,
    /// How long an answer may take before it is worthless.
    pub latency_budget: Duration,
    /// The confidence an answer must carry to be acted on, in `(0, 1]`.
    pub required_confidence_f64: f64,
    /// Whether an estimate is acceptable at all.
    pub determinism: Determinism,
    /// The market conditions, and the key a model's record is read under.
    pub conditions: Conditions,
}

impl DecisionContext {
    pub fn new(
        subject: impl Into<String>,
        value_at_stake: Decimal,
        latency_budget: Duration,
        required_confidence_f64: f64,
        determinism: Determinism,
        conditions: Conditions,
    ) -> Self {
        Self {
            subject: subject.into(),
            value_at_stake,
            latency_budget,
            required_confidence_f64,
            determinism,
            conditions,
        }
    }

    /// Refuse a context the router cannot reason about.
    ///
    /// Validated rather than clamped. A decision that arrives claiming to be
    /// worth nothing, or needing an answer in no time, is a caller bug, and
    /// clamping it to something plausible would route it as though it were a
    /// real decision.
    pub fn validate(&self) -> Result<()> {
        if self.subject.trim().is_empty() {
            return Err(Error::invalid(
                "a decision with no subject cannot be recorded, and an unrecorded routing decision is not auditable",
            ));
        }
        if self.value_at_stake <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "the decision '{}' claims a value at stake of {}; nothing is worth spending on a decision worth nothing",
                self.subject, self.value_at_stake
            )));
        }
        if self.latency_budget.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "the decision '{}' allows no time for an answer",
                self.subject
            )));
        }
        if !self.required_confidence_f64.is_finite()
            || self.required_confidence_f64 <= 0.0
            || self.required_confidence_f64 > 1.0
        {
            return Err(Error::invalid(format!(
                "the decision '{}' requires a confidence of {}, which is not a probability",
                self.subject, self.required_confidence_f64
            )));
        }
        Ok(())
    }
}
