//! Scoring a source, and turning the score into a routing class.
//!
//! Five named scores, each a fraction in `[0, 1]` with a stated meaning, and
//! one weighted composite. They are `f64` because they are statistics — none
//! of them is money, and none of them settles anything.
//!
//! The important structure here is the shape of [`Routing::decide`]: legality
//! is its first argument and the composite cannot reach the class until
//! legality has already permitted it. A high score therefore has no path to
//! overriding a legal refusal, not because the code checks in the right order
//! by convention, but because [`Routing`]'s fields are private and this is the
//! only constructor. The alternative — computing a class from the score and
//! then subtracting for legality — is one refactor away from being a scoring
//! problem, and a scoring problem is one somebody eventually tunes.

use crate::legal::Legality;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Five independent readings of a source's worth.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceScores {
    reliability: f64,
    freshness: f64,
    uniqueness: f64,
    historical_value: f64,
    cost_efficiency: f64,
}

impl SourceScores {
    /// Weight of each score in the composite. Reliability leads because an
    /// unavailable source's other virtues are unavailable with it.
    pub const WEIGHTS: [(&'static str, f64); 5] = [
        ("reliability", 0.30),
        ("freshness", 0.20),
        ("uniqueness", 0.20),
        ("historical_value", 0.15),
        ("cost_efficiency", 0.15),
    ];

    /// Build a score set, rejecting anything outside `[0, 1]`.
    ///
    /// * `reliability` — how often the source answers correctly and on time.
    /// * `freshness` — how close the newest record is to now, against what
    ///   the source's update frequency promises.
    /// * `uniqueness` — how much of this source is *not* already covered by
    ///   something registered. A perfect duplicate scores 0.
    /// * `historical_value` — how far back its history reaches, capped at ten
    ///   years, beyond which more depth stops changing a backtest.
    /// * `cost_efficiency` — 1.0 for free, falling as the monthly bill rises.
    pub fn new(
        reliability: f64,
        freshness: f64,
        uniqueness: f64,
        historical_value: f64,
        cost_efficiency: f64,
    ) -> Result<Self> {
        for (name, value) in [
            ("reliability", reliability),
            ("freshness", freshness),
            ("uniqueness", uniqueness),
            ("historical_value", historical_value),
            ("cost_efficiency", cost_efficiency),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(Error::invalid(format!(
                    "the {name} score must be a fraction in [0, 1], not {value}"
                )));
            }
        }
        Ok(Self {
            reliability,
            freshness,
            uniqueness,
            historical_value,
            cost_efficiency,
        })
    }

    /// How often the source answers correctly and on time, in `[0, 1]`.
    pub fn reliability(&self) -> f64 {
        self.reliability
    }

    /// How current the newest record is, against what the source promises,
    /// in `[0, 1]`.
    pub fn freshness(&self) -> f64 {
        self.freshness
    }

    /// How much of this source is not already covered elsewhere, in `[0, 1]`.
    pub fn uniqueness(&self) -> f64 {
        self.uniqueness
    }

    /// How deep its history reaches, in `[0, 1]`.
    pub fn historical_value(&self) -> f64 {
        self.historical_value
    }

    /// How little it costs for what it gives, in `[0, 1]`.
    pub fn cost_efficiency(&self) -> f64 {
        self.cost_efficiency
    }

    /// The weighted composite, in `[0, 1]`.
    pub fn composite(&self) -> f64 {
        0.30 * self.reliability
            + 0.20 * self.freshness
            + 0.20 * self.uniqueness
            + 0.15 * self.historical_value
            + 0.15 * self.cost_efficiency
    }

    /// Each score by name, for the decision record.
    pub fn named(&self) -> [(&'static str, f64); 5] {
        [
            ("reliability", self.reliability),
            ("freshness", self.freshness),
            ("uniqueness", self.uniqueness),
            ("historical_value", self.historical_value),
            ("cost_efficiency", self.cost_efficiency),
        ]
    }
}

/// Where a source's records are kept and how eagerly they are polled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingClass {
    /// Polled continuously, held in hot storage, reachable from a cell.
    Hot,
    /// Polled on its natural cadence, held in the lakehouse.
    Warm,
    /// Collected for history and research only.
    Cold,
    /// Not collected.
    Rejected,
}

impl RoutingClass {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Rejected => "rejected",
        }
    }

    pub const fn is_collected(&self) -> bool {
        !matches!(self, Self::Rejected)
    }
}

/// A routing class together with what produced it.
///
/// Fields are private and [`Self::decide`] is the only constructor, so a
/// `Routing` in hand is one that went through the legality gate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    class: RoutingClass,
    composite: f64,
    basis: String,
}

impl Routing {
    /// Composite at or above which a source is polled continuously.
    pub const HOT_THRESHOLD: f64 = 0.75;
    /// Composite at or above which a source is polled on its own cadence.
    pub const WARM_THRESHOLD: f64 = 0.50;
    /// Composite below which a source is not worth collecting at all.
    pub const COLD_THRESHOLD: f64 = 0.25;

    /// Decide where a source goes.
    ///
    /// Legality first, and not as a tie-break: a source that is forbidden or
    /// undetermined is rejected whatever it scores, and the composite is
    /// still recorded so the record shows what was given up.
    pub fn decide(legality: &Legality, scores: &SourceScores) -> Self {
        let composite = scores.composite();
        if !legality.is_permitted() {
            return Self {
                class: RoutingClass::Rejected,
                composite,
                basis: format!(
                    "rejected on legality ({}); the composite score of {composite:.3} does not \
                     enter into it",
                    legality.describe()
                ),
            };
        }
        let class = if composite >= Self::HOT_THRESHOLD {
            RoutingClass::Hot
        } else if composite >= Self::WARM_THRESHOLD {
            RoutingClass::Warm
        } else if composite >= Self::COLD_THRESHOLD {
            RoutingClass::Cold
        } else {
            RoutingClass::Rejected
        };
        let basis = match class {
            RoutingClass::Rejected => format!(
                "collection is permitted but the composite score of {composite:.3} is below the \
                 floor of {:.2}",
                Self::COLD_THRESHOLD
            ),
            other => format!(
                "{} on a composite score of {composite:.3}",
                other.as_str()
            ),
        };
        Self {
            class,
            composite,
            basis,
        }
    }

    pub fn class(&self) -> RoutingClass {
        self.class
    }

    pub fn composite(&self) -> f64 {
        self.composite
    }

    /// Why this class, in words.
    pub fn basis(&self) -> &str {
        &self.basis
    }
}
