//! Model governance.
//!
//! Charter section 22: every model is tracked, and every investment decision
//! references the models that produced it. The registry is the record — what a
//! model is, what it was trained on, how it evaluated, who owns it, and whether
//! it is still fit to be used.
//!
//! The consequential part is [`ModelRegistry::require_for_decision`]: a model
//! that has been retired, has drifted past its threshold, or has never been
//! evaluated cannot be used in a decision. The check is a hard error, not a
//! warning, because the alternative is a stale model quietly continuing to
//! trade.

use qip_core::error::{Error, Result};
use qip_core::{Duration, ModelId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a model is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStage {
    /// Being developed; not usable for anything that matters.
    Development,
    /// Under evaluation against held-out data.
    Validation,
    /// Approved for use in decisions.
    Production,
    /// Running alongside production for comparison, output not acted on.
    Shadow,
    /// Withdrawn. Any decision referencing it is invalid.
    Retired,
}

impl ModelStage {
    pub fn allows_decisions(&self) -> bool {
        matches!(self, Self::Production)
    }
}

/// Evaluation result for one model version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    pub evaluated_at: Timestamp,
    /// Dataset the evaluation ran on.
    pub dataset: String,
    /// Named metrics, e.g. `brier_score`, `auc`, `rmse`.
    pub metrics: BTreeMap<String, f64>,
    /// Whether the evaluation met the model's acceptance criteria.
    pub passed: bool,
}

/// The record for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCard {
    pub model_id: ModelId,
    pub name: String,
    pub version: String,
    /// What the model does and the decision it supports.
    pub purpose: String,
    pub stage: ModelStage,
    /// Datasets the model was fitted on, with their time ranges.
    pub training_datasets: Vec<String>,
    /// Feature names the model consumes.
    pub features: Vec<String>,
    /// Hyperparameters, recorded so a fit can be reproduced.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
    /// Team or individual accountable for the model.
    pub owner: String,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<Timestamp>,
    pub evaluations: Vec<EvaluationRecord>,
    /// Latest measured drift against the training distribution.
    #[serde(default)]
    pub drift_score: f64,
    /// Drift above which the model must be re-evaluated before further use.
    pub drift_threshold: f64,
    /// How long an evaluation stays valid.
    pub evaluation_validity: Duration,
    /// Known limitations, recorded so they are visible at the point of use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl ModelCard {
    pub fn new(
        model_id: ModelId,
        name: impl Into<String>,
        version: impl Into<String>,
        owner: impl Into<String>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            model_id,
            name: name.into(),
            version: version.into(),
            purpose: String::new(),
            stage: ModelStage::Development,
            training_datasets: Vec::new(),
            features: Vec::new(),
            parameters: BTreeMap::new(),
            owner: owner.into(),
            created_at,
            deployed_at: None,
            retired_at: None,
            evaluations: Vec::new(),
            drift_score: 0.0,
            drift_threshold: 0.2,
            evaluation_validity: Duration::from_days(90),
            limitations: Vec::new(),
        }
    }

    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = purpose.into();
        self
    }

    pub fn with_features(mut self, features: Vec<String>) -> Self {
        self.features = features;
        self
    }

    pub fn with_training_data(mut self, datasets: Vec<String>) -> Self {
        self.training_datasets = datasets;
        self
    }

    pub fn with_limitation(mut self, limitation: impl Into<String>) -> Self {
        self.limitations.push(limitation.into());
        self
    }

    /// Fully-qualified reference recorded on decisions: `name@version`.
    pub fn reference(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn latest_evaluation(&self) -> Option<&EvaluationRecord> {
        self.evaluations
            .iter()
            .max_by_key(|e| e.evaluated_at.as_nanos())
    }

    /// Whether the model may be used for a decision at `now`, with the reason
    /// when it may not.
    pub fn decision_eligibility(&self, now: Timestamp) -> std::result::Result<(), String> {
        if !self.stage.allows_decisions() {
            return Err(format!("model is in {:?}, not production", self.stage));
        }
        if self.retired_at.is_some() {
            return Err("model has been retired".into());
        }
        let Some(evaluation) = self.latest_evaluation() else {
            return Err("model has never been evaluated".into());
        };
        if !evaluation.passed {
            return Err(format!(
                "the latest evaluation on {} did not pass",
                evaluation.dataset
            ));
        }
        let age = now.since(evaluation.evaluated_at);
        if age > self.evaluation_validity {
            return Err(format!(
                "the last evaluation is {age:?} old, beyond the {:?} validity window",
                self.evaluation_validity
            ));
        }
        if self.drift_score > self.drift_threshold {
            return Err(format!(
                "drift {:.3} exceeds the threshold {:.3}",
                self.drift_score, self.drift_threshold
            ));
        }
        Ok(())
    }
}

/// Every model the platform knows about.
#[derive(Debug, Default)]
pub struct ModelRegistry {
    cards: BTreeMap<String, ModelCard>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, card: ModelCard) {
        self.cards.insert(card.reference(), card);
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Look up by `name@version`.
    pub fn get(&self, reference: &str) -> Option<&ModelCard> {
        self.cards.get(reference)
    }

    pub fn get_mut(&mut self, reference: &str) -> Option<&mut ModelCard> {
        self.cards.get_mut(reference)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ModelCard> {
        self.cards.values()
    }

    /// The production version of a model, if any.
    pub fn production_version(&self, name: &str) -> Option<&ModelCard> {
        self.cards
            .values()
            .filter(|c| c.name == name && c.stage == ModelStage::Production)
            .max_by_key(|c| c.deployed_at.unwrap_or(c.created_at).as_nanos())
    }

    /// Fetch a model for use in a decision, or explain why it cannot be used.
    ///
    /// The hard failure is the point: a retired or drifted model must stop
    /// influencing capital, and the only reliable way to ensure that is to make
    /// the call site unable to proceed.
    pub fn require_for_decision(&self, reference: &str, now: Timestamp) -> Result<&ModelCard> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.decision_eligibility(now).map_err(|reason| {
            Error::denied(format!("{reference} may not drive a decision: {reason}"))
        })?;
        Ok(card)
    }

    /// Promote a model to production, recording when.
    pub fn promote(&mut self, reference: &str, at: Timestamp) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if card.latest_evaluation().is_none_or(|e| !e.passed) {
            return Err(Error::denied(format!(
                "{reference} cannot be promoted without a passing evaluation"
            )));
        }
        card.stage = ModelStage::Production;
        card.deployed_at = Some(at);
        Ok(())
    }

    /// Retire a model. Anything referencing it afterwards is rejected.
    pub fn retire(&mut self, reference: &str, at: Timestamp) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.stage = ModelStage::Retired;
        card.retired_at = Some(at);
        Ok(())
    }

    /// Record an evaluation.
    pub fn record_evaluation(&mut self, reference: &str, record: EvaluationRecord) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.evaluations.push(record);
        Ok(())
    }

    /// Update a model's drift score.
    pub fn record_drift(&mut self, reference: &str, drift: f64) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.drift_score = drift;
        Ok(())
    }

    /// Production models that are no longer eligible, with the reason.
    ///
    /// Surfaced on the operator dashboard: a model silently ageing out of
    /// validity is a governance failure that should be visible before it
    /// blocks a decision.
    pub fn ineligible(&self, now: Timestamp) -> Vec<(&ModelCard, String)> {
        self.cards
            .values()
            .filter(|c| c.stage == ModelStage::Production)
            .filter_map(|c| c.decision_eligibility(now).err().map(|reason| (c, reason)))
            .collect()
    }
}
