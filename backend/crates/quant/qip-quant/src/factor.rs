//! Factor definitions and scoring.
//!
//! A factor is a named, cross-sectionally comparable characteristic. The
//! library records what each is, how it is computed and — importantly — what is
//! known to go wrong with it, so a strategy leaning on one carries that
//! knowledge to the risk review rather than discovering it in a drawdown.

use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The definition of one factor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactorDefinition {
    pub name: String,
    pub description: String,
    /// Features the factor is computed from.
    pub inputs: Vec<String>,
    /// Whether a higher raw value is expected to be better.
    pub higher_is_better: bool,
    /// Known failure modes, surfaced wherever the factor is used.
    pub known_weaknesses: Vec<String>,
    /// Typical horizon over which the factor is expected to pay.
    pub horizon_days: f64,
}

impl FactorDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            inputs: Vec::new(),
            higher_is_better: true,
            known_weaknesses: Vec::new(),
            horizon_days: 60.0,
        }
    }

    pub fn with_inputs(mut self, inputs: Vec<String>) -> Self {
        self.inputs = inputs;
        self
    }

    pub fn with_weakness(mut self, weakness: impl Into<String>) -> Self {
        self.known_weaknesses.push(weakness.into());
        self
    }

    pub fn inverted(mut self) -> Self {
        self.higher_is_better = false;
        self
    }

    pub fn over_days(mut self, days: f64) -> Self {
        self.horizon_days = days;
        self
    }
}

/// One instrument's standing on one factor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactorScore {
    pub factor: String,
    pub object_id: String,
    /// Raw value before standardisation.
    pub raw: f64,
    /// Cross-sectional z-score, sign-adjusted so positive is always better.
    pub score: f64,
    /// Percentile within the cross-section, in `[0, 1]`.
    pub percentile: f64,
}

/// The factors the platform ships with.
#[derive(Clone, Debug, Default)]
pub struct FactorLibrary {
    definitions: BTreeMap<String, FactorDefinition>,
}

impl FactorLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// The standard equity and cross-asset factors.
    ///
    /// Each records what is known to go wrong with it. These are not
    /// footnotes: momentum's crash risk and value's decade-long droughts are
    /// the reason a portfolio built on them needs a risk overlay.
    pub fn standard() -> Self {
        let mut library = Self::new();
        for definition in [
            FactorDefinition::new("momentum", "trailing risk-adjusted return")
                .with_inputs(vec!["close".into()])
                .over_days(180.0)
                .with_weakness("crashes sharply at regime turns, when the trend reverses fastest")
                .with_weakness("crowded: many participants hold the same exposure simultaneously"),
            FactorDefinition::new("value", "price relative to fundamentals")
                .with_inputs(vec!["close".into(), "revenue".into()])
                .over_days(365.0)
                .inverted()
                .with_weakness("can underperform for many years at a time")
                .with_weakness("a cheap price is sometimes correct rather than an opportunity"),
            FactorDefinition::new("quality", "profitability and balance-sheet strength")
                .with_inputs(vec!["revenue".into()])
                .over_days(250.0)
                .with_weakness("definitions vary widely, so exposure is not comparable across providers"),
            FactorDefinition::new("low_volatility", "inverse of realised volatility")
                .with_inputs(vec!["realised_volatility_20d".into()])
                .over_days(120.0)
                .inverted()
                .with_weakness("becomes an unintended rates bet when defensive names are bid up"),
            FactorDefinition::new("growth", "rate of change of fundamentals")
                .with_inputs(vec!["revenue".into()])
                .over_days(250.0)
                .with_weakness("extrapolating recent growth is the single most common error in equity research"),
            FactorDefinition::new("sentiment", "aggregate tone of news coverage")
                .with_inputs(vec!["sentiment".into()])
                .over_days(20.0)
                .with_weakness("reflects what is already priced more often than what is not"),
            FactorDefinition::new("carry", "yield pickup per unit of risk")
                .with_inputs(vec!["yield".into(), "realised_volatility_20d".into()])
                .over_days(90.0)
                .with_weakness("earns steadily and loses violently; the payoff is short a tail"),
        ] {
            library.define(definition);
        }
        library
    }

    pub fn define(&mut self, definition: FactorDefinition) {
        self.definitions.insert(definition.name.clone(), definition);
    }

    pub fn get(&self, name: &str) -> Option<&FactorDefinition> {
        self.definitions.get(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.definitions.keys().map(String::as_str).collect()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Score a cross-section of raw values for one factor.
    ///
    /// Uses robust z-scores, so one extreme value does not compress every other
    /// score toward zero, and flips the sign where lower is better.
    pub fn score_cross_section(&self, factor: &str, values: &[(String, f64)]) -> Vec<FactorScore> {
        if values.is_empty() {
            return Vec::new();
        }
        let higher_is_better = self
            .definitions
            .get(factor)
            .is_none_or(|d| d.higher_is_better);
        let raw: Vec<f64> = values.iter().map(|(_, v)| *v).collect();
        let z = stats::robust_z_scores(&raw);
        let ranks = stats::ranks(&raw);
        let count = raw.len() as f64;

        values
            .iter()
            .zip(z)
            .zip(ranks)
            .map(|(((object, value), score), rank)| {
                let sign = if higher_is_better { 1.0 } else { -1.0 };
                let percentile = if count > 1.0 {
                    (rank - 1.0) / (count - 1.0)
                } else {
                    0.5
                };
                FactorScore {
                    factor: factor.to_string(),
                    object_id: object.clone(),
                    raw: *value,
                    score: (sign * score).clamp(-3.0, 3.0),
                    percentile: if higher_is_better {
                        percentile
                    } else {
                        1.0 - percentile
                    },
                }
            })
            .collect()
    }

    /// Every recorded weakness across the factors a strategy uses.
    ///
    /// Handed to the adversarial reviewer, which is the point: a strategy built
    /// on momentum and carry should be challenged on crash risk and tail risk
    /// specifically, not generically.
    pub fn weaknesses_of(&self, factors: &[String]) -> Vec<(String, String)> {
        factors
            .iter()
            .filter_map(|name| self.definitions.get(name))
            .flat_map(|definition| {
                definition
                    .known_weaknesses
                    .iter()
                    .map(move |weakness| (definition.name.clone(), weakness.clone()))
            })
            .collect()
    }
}
