//! The strategy SDK.
//!
//! A strategy is a pure function from state to target weights. It does not
//! place orders, does not know about the broker, and cannot bypass risk: the
//! portfolio engine takes its output as a *proposal*, which the risk engine may
//! veto. Charter section 23 requires exactly that separation, and making the
//! trait unable to express an order is how it is enforced rather than asked for.

use qip_core::error::Result;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::universe::Universe;
use qip_portfolio::portfolio::Portfolio;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::signal::SignalSet;

/// What a strategy sees.
#[derive(Debug)]
pub struct StrategyContext<'a> {
    pub as_of: Timestamp,
    pub universe: &'a Universe,
    pub portfolio: &'a Portfolio,
    /// Latest prices, keyed by object id.
    pub prices: &'a BTreeMap<String, Decimal>,
    pub signals: &'a SignalSet,
    /// Point-in-time feature values, keyed by `feature/object`.
    pub features: &'a BTreeMap<String, f64>,
}

impl StrategyContext<'_> {
    pub fn price_of(&self, object_id: &ObjectId) -> Option<Decimal> {
        self.prices.get(object_id.as_str()).copied()
    }

    pub fn feature(&self, feature: &str, object_id: &ObjectId) -> Option<f64> {
        self.features
            .get(&format!("{feature}/{}", object_id.as_str()))
            .copied()
    }

    /// Instruments that are priced and tradable at this instant.
    pub fn tradable(&self) -> Vec<&qip_financial::object::FinancialObject> {
        self.universe
            .iter()
            .filter(|object| self.prices.contains_key(object.object_id.as_str()))
            .filter(|object| !object.regulatory.is_restricted())
            .filter(|object| !object.is_expired(self.as_of))
            .collect()
    }
}

/// Target portfolio weights as fractions of equity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetWeights {
    pub weights: BTreeMap<String, f64>,
}

impl TargetWeights {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_map(weights: BTreeMap<String, f64>) -> Self {
        Self { weights }
    }

    pub fn set(&mut self, object_id: &ObjectId, weight: f64) {
        self.weights.insert(object_id.as_str().to_string(), weight);
    }

    pub fn get(&self, object_id: &ObjectId) -> f64 {
        self.weights.get(object_id.as_str()).copied().unwrap_or(0.0)
    }

    pub fn len(&self) -> usize {
        self.weights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    /// Sum of weights: the invested fraction, which may exceed one with
    /// leverage or fall below it with cash.
    pub fn net(&self) -> f64 {
        self.weights.values().sum()
    }

    /// Sum of absolute weights: gross exposure per unit of equity.
    pub fn gross(&self) -> f64 {
        self.weights.values().map(|w| w.abs()).sum()
    }

    /// Scale so gross exposure equals `target`.
    pub fn scaled_to_gross(&self, target: f64) -> TargetWeights {
        let gross = self.gross();
        if gross <= 1e-12 {
            return self.clone();
        }
        let factor = target / gross;
        TargetWeights {
            weights: self
                .weights
                .iter()
                .map(|(k, w)| (k.clone(), w * factor))
                .collect(),
        }
    }

    /// Scale to `target_gross` while holding every weight at or below `cap`.
    ///
    /// Capping and then scaling would flatten the ranking: every name above the
    /// cap collapses to the same weight and the strongest signal is
    /// indistinguishable from the merely adequate one. Instead the excess above
    /// the cap is redistributed to the names still below it, repeatedly, so the
    /// ordering survives wherever the cap does not bind.
    ///
    /// When `cap * n` is below `target_gross` the cap wins and the result is
    /// under-invested. That is the correct outcome: a concentration limit is
    /// not negotiable against an exposure target.
    pub fn capped_to_gross(&self, target_gross: f64, cap: f64) -> TargetWeights {
        if self.weights.is_empty() || cap <= 0.0 {
            return TargetWeights::new();
        }
        let count = self.weights.len() as f64;
        let achievable = target_gross.min(cap * count);

        let mut scaled = self.scaled_to_gross(achievable).weights;
        // At most one pass per name is needed: each pass freezes at least one.
        for _ in 0..self.weights.len() {
            let over: Vec<String> = scaled
                .iter()
                .filter(|(_, w)| w.abs() > cap + 1e-12)
                .map(|(k, _)| k.clone())
                .collect();
            if over.is_empty() {
                break;
            }
            let mut excess = 0.0;
            for key in &over {
                if let Some(weight) = scaled.get_mut(key) {
                    excess += weight.abs() - cap;
                    *weight = cap * weight.signum();
                }
            }
            let free: Vec<String> = scaled
                .iter()
                .filter(|(k, w)| !over.contains(k) && w.abs() < cap - 1e-12)
                .map(|(k, _)| k.clone())
                .collect();
            if free.is_empty() {
                break;
            }
            let free_gross: f64 = free
                .iter()
                .filter_map(|k| scaled.get(k))
                .map(|w| w.abs())
                .sum();
            if free_gross <= 1e-12 {
                break;
            }
            for key in &free {
                if let Some(weight) = scaled.get_mut(key) {
                    let share = weight.abs() / free_gross;
                    *weight += excess * share * weight.signum();
                }
            }
        }
        TargetWeights { weights: scaled }
    }

    /// Drop positions below a minimum size.
    ///
    /// A 0.1% target weight costs the same in operational overhead as a 5% one
    /// and contributes nothing; clearing them out is not cosmetic.
    pub fn pruned(&self, minimum: f64) -> TargetWeights {
        TargetWeights {
            weights: self
                .weights
                .iter()
                .filter(|(_, w)| w.abs() >= minimum)
                .map(|(k, w)| (k.clone(), *w))
                .collect(),
        }
    }

    /// Turnover against a current set of weights: half the sum of absolute
    /// changes, which is the fraction of the book that must trade.
    pub fn turnover_from(&self, current: &BTreeMap<String, f64>) -> f64 {
        let mut keys: Vec<&String> = self.weights.keys().chain(current.keys()).collect();
        keys.sort();
        keys.dedup();
        let total: f64 = keys
            .into_iter()
            .map(|key| {
                let target = self.weights.get(key).copied().unwrap_or(0.0);
                let held = current.get(key).copied().unwrap_or(0.0);
                (target - held).abs()
            })
            .sum();
        total / 2.0
    }
}

/// A strategy's output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyDecision {
    pub strategy: String,
    pub as_of: Timestamp,
    pub targets: TargetWeights,
    /// Why these weights, in words. Recorded on the proposal for review.
    pub rationale: String,
    /// Confidence in the decision, in `[0, 1]`.
    pub confidence: f64,
    /// Factors the strategy is deliberately exposed to.
    pub intended_factors: Vec<String>,
}

/// A strategy: state in, target weights out.
///
/// Note what the trait cannot express. There is no method that places an order,
/// moves cash, or touches the broker. A strategy proposes; the portfolio and
/// risk engines dispose.
pub trait Strategy: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;

    /// The horizon the strategy trades over.
    fn horizon(&self) -> crate::signal::Horizon;

    /// Produce target weights from the current state.
    fn decide(&self, context: &StrategyContext<'_>) -> Result<StrategyDecision>;

    /// Factors the strategy is knowingly exposed to, so the risk review can
    /// challenge it on the right grounds.
    fn intended_factors(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A long-only strategy that weights by composite signal strength.
///
/// Deliberately simple: it exists to exercise the pipeline end to end and to
/// serve as the reference implementation of the trait, not to be a good
/// strategy.
#[derive(Debug, Clone)]
pub struct SignalWeightedStrategy {
    pub name: String,
    pub horizon: crate::signal::Horizon,
    /// Gross exposure to target.
    pub target_gross: f64,
    /// Maximum weight in any one name.
    pub max_weight: f64,
    /// Minimum composite score to hold anything at all.
    pub score_threshold: f64,
    /// Whether short positions are permitted.
    pub allow_shorts: bool,
}

impl Default for SignalWeightedStrategy {
    fn default() -> Self {
        Self {
            name: "signal-weighted".into(),
            horizon: crate::signal::Horizon::MediumTerm,
            target_gross: 0.9,
            max_weight: 0.10,
            score_threshold: 0.25,
            allow_shorts: false,
        }
    }
}

impl Strategy for SignalWeightedStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn horizon(&self) -> crate::signal::Horizon {
        self.horizon
    }

    fn decide(&self, context: &StrategyContext<'_>) -> Result<StrategyDecision> {
        let composite = context.signals.composite(self.horizon);
        let tradable: std::collections::BTreeSet<String> = context
            .tradable()
            .into_iter()
            .map(|o| o.object_id.as_str().to_string())
            .collect();

        let mut selected: Vec<(String, f64)> = composite
            .into_iter()
            .filter(|(object, _)| tradable.contains(object))
            .filter(|(_, score)| {
                if self.allow_shorts {
                    score.abs() >= self.score_threshold
                } else {
                    *score >= self.score_threshold
                }
            })
            .collect();

        if selected.is_empty() {
            return Ok(StrategyDecision {
                strategy: self.name.clone(),
                as_of: context.as_of,
                targets: TargetWeights::new(),
                rationale: "no instrument cleared the score threshold; holding cash".into(),
                confidence: 0.5,
                intended_factors: self.intended_factors(),
            });
        }

        // Weight proportionally to score magnitude, then scale to the exposure
        // target while respecting the per-name cap.
        selected.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let total: f64 = selected.iter().map(|(_, s)| s.abs()).sum();
        let mut weights = BTreeMap::new();
        for (object, score) in &selected {
            weights.insert(object.clone(), score / total);
        }

        let targets = TargetWeights::from_map(weights)
            .capped_to_gross(self.target_gross, self.max_weight)
            .pruned(0.005);

        let count = targets.len();
        let cap_binds = targets.gross() < self.target_gross - 1e-6;
        Ok(StrategyDecision {
            strategy: self.name.clone(),
            as_of: context.as_of,
            targets,
            rationale: if cap_binds {
                format!(
                    "{count} instruments cleared a composite score of {:.2} at the {} horizon; \
                     the {:.0}% per-name cap binds, so the book is deliberately under-invested \
                     rather than concentrated",
                    self.score_threshold,
                    self.horizon.as_str(),
                    self.max_weight * 100.0
                )
            } else {
                format!(
                    "{count} instruments cleared a composite score of {:.2} at the {} horizon; \
                     weighted by score magnitude within a {:.0}% per-name cap",
                    self.score_threshold,
                    self.horizon.as_str(),
                    self.max_weight * 100.0
                )
            },
            confidence: 0.6,
            intended_factors: self.intended_factors(),
        })
    }

    fn intended_factors(&self) -> Vec<String> {
        vec!["momentum".into(), "sentiment".into()]
    }
}
