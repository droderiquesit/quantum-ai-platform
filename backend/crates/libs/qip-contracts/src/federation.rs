//! The World Model Federation link: contracts for exchanging states between
//! competing world models and the ambient mesh.
//!
//! The Proactive Ambient Model Mesh (AMBIENT-011) coordinates multiple world
//! models held in parallel, weighting each by its track record on surprise
//! prediction. Two failures must be structural:
//!
//! * A discovery nobody can replay — so each federation output is journaled with
//!   its originating model, timestamp and evidence;
//! * A feedback loop that reinforces a miscalibrated model — so the federation
//!   records every outcome that proves or disproves a hypothesis, not only the
//!   winning path.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One world model's hypothesis about a future state, priced with its
/// confidence in that forecast.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hypothesis {
    /// Unique identifier across the federation snapshot.
    pub id: String,
    /// The world model this came from, e.g., "regression-v3", "causal-dag-v1".
    pub model_id: String,
    /// What is predicted to change and how (e.g., "volatility increases",
    /// "correlation breaks down").
    pub statement: String,
    /// One of the entities the hypothesis affects; matches AmbientSignal subjects.
    pub subject: String,
    /// The confidence this model places in the hypothesis, in basis points
    /// (0 to 10000).
    pub confidence_bp: u32,
    /// When the hypothesis is valid: from this instant.
    pub horizon_start: Timestamp,
    /// How long the hypothesis is expected to hold; if negative or zero,
    /// the hypothesis has no bounded duration (stationary assumption).
    pub horizon_duration: Duration,
    /// Evidence identifiers supporting this hypothesis.
    pub evidence_ids: Vec<String>,
}

impl Hypothesis {
    /// Refuses an empty id, model_id or statement, an empty subject, a
    /// confidence above 10000 bp, or empty evidence list.
    pub fn new(
        id: impl Into<String>,
        model_id: impl Into<String>,
        statement: impl Into<String>,
        subject: impl Into<String>,
        confidence_bp: u32,
        horizon_start: Timestamp,
        horizon_duration: Duration,
        evidence_ids: Vec<String>,
    ) -> Result<Self> {
        let (id, model_id, statement, subject) =
            (id.into(), model_id.into(), statement.into(), subject.into());
        if id.is_empty() {
            return Err(Error::invalid(
                "a hypothesis needs an id; supply a unique identifier within the federation snapshot",
            ));
        }
        if model_id.is_empty() {
            return Err(Error::invalid(
                "a hypothesis needs a model_id; name which model produced it",
            ));
        }
        if statement.is_empty() {
            return Err(Error::invalid(
                "a hypothesis needs a statement; describe what is predicted to change",
            ));
        }
        if subject.is_empty() {
            return Err(Error::invalid(
                "a hypothesis needs a subject; name the entity it affects",
            ));
        }
        if confidence_bp > 10_000 {
            return Err(Error::invalid(format!(
                "confidence {confidence_bp} bp exceeds 10000; scale the model, it is not clamped"
            )));
        }
        if evidence_ids.is_empty() || evidence_ids.iter().any(String::is_empty) {
            return Err(Error::invalid(
                "a hypothesis must carry evidence lineage; a claim without sources cannot be replayed",
            ));
        }
        Ok(Self {
            id,
            model_id,
            statement,
            subject,
            confidence_bp,
            horizon_start,
            horizon_duration,
            evidence_ids,
        })
    }
}

/// A future state the federation simulates under a set of conditioning drivers
/// and a hypothesis. Scenarios are branches held between federation updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalScenario {
    /// Unique identifier within the federation snapshot.
    pub id: String,
    /// The hypothesis this scenario conditions on.
    pub hypothesis_id: String,
    /// The driver values and magnitudes this scenario assumes
    /// (e.g., {"rate_shock_bp": "250", "fx_move_pct": "-2.5"}).
    pub conditioning: BTreeMap<String, String>,
    /// Loss this scenario computes for the currently held book,
    /// in the same currency as the book's value.
    pub conditional_loss: Decimal,
    /// When this scenario was evaluated.
    pub evaluated_at: Timestamp,
}

impl ConditionalScenario {
    /// Refuses an empty id, hypothesis_id, an empty conditioning map, or
    /// a conditioning map with empty keys/values.
    pub fn new(
        id: impl Into<String>,
        hypothesis_id: impl Into<String>,
        conditioning: BTreeMap<String, String>,
        conditional_loss: Decimal,
        evaluated_at: Timestamp,
    ) -> Result<Self> {
        let (id, hypothesis_id) = (id.into(), hypothesis_id.into());
        if id.is_empty() {
            return Err(Error::invalid(
                "a scenario needs an id; supply a unique identifier within the federation snapshot",
            ));
        }
        if hypothesis_id.is_empty() {
            return Err(Error::invalid(
                "a scenario needs a hypothesis_id; name which hypothesis conditions it",
            ));
        }
        if conditioning.is_empty()
            || conditioning.keys().any(|k| k.is_empty())
            || conditioning.values().any(|v| v.is_empty())
        {
            return Err(Error::invalid(
                "a scenario must name the drivers and values it conditions on; supply at least one, none blank",
            ));
        }
        Ok(Self {
            id,
            hypothesis_id,
            conditioning,
            conditional_loss,
            evaluated_at,
        })
    }
}

/// An outcome recorded when a prior hypothesis or scenario is resolved by
/// observed fact. Used to calibrate model weights.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// Unique outcome identifier.
    pub id: String,
    /// The hypothesis this outcome resolves, if any.
    pub hypothesis_id: Option<String>,
    /// The scenario this outcome resolves, if any.
    pub scenario_id: Option<String>,
    /// What actually happened; matches the hypothesis/scenario statement scope.
    pub observed_state: String,
    /// Whether the hypothesis/scenario prediction matched the outcome.
    pub prediction_correct: bool,
    /// When the outcome became knowable.
    pub observed_at: Timestamp,
}

impl Outcome {
    /// Refuses an empty id, observed_state, or if both hypothesis_id and
    /// scenario_id are absent.
    pub fn new(
        id: impl Into<String>,
        hypothesis_id: Option<String>,
        scenario_id: Option<String>,
        observed_state: impl Into<String>,
        prediction_correct: bool,
        observed_at: Timestamp,
    ) -> Result<Self> {
        let (id, observed_state) = (id.into(), observed_state.into());
        if id.is_empty() {
            return Err(Error::invalid(
                "an outcome needs an id; supply a unique identifier",
            ));
        }
        if hypothesis_id.is_none() && scenario_id.is_none() {
            return Err(Error::invalid(
                "an outcome must resolve a hypothesis or scenario; supply at least one",
            ));
        }
        if observed_state.is_empty() {
            return Err(Error::invalid(
                "an outcome needs an observed_state; describe what actually happened",
            ));
        }
        Ok(Self {
            id,
            hypothesis_id,
            scenario_id,
            observed_state,
            prediction_correct,
            observed_at,
        })
    }
}

/// Surprise the federation detected, recorded with model attribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationSurprise {
    /// Unique identifier.
    pub id: String,
    /// Which model(s) in the federation detected this.
    pub model_ids: Vec<String>,
    /// The subject (instrument, location, etc.) surprised.
    pub subject: String,
    /// Observed value, expected value, and the surprise z-score.
    pub observed: Decimal,
    pub expected: Decimal,
    pub z_score: Decimal,
    /// When surprise was detected.
    pub detected_at: Timestamp,
    /// Evidence supporting the detection.
    pub evidence_ids: Vec<String>,
}

impl FederationSurprise {
    /// Refuses an empty id, model_ids, subject, empty evidence, an empty
    /// observed/expected/z_score field, or evidence with blanks.
    pub fn new(
        id: impl Into<String>,
        model_ids: Vec<String>,
        subject: impl Into<String>,
        observed: Decimal,
        expected: Decimal,
        z_score: Decimal,
        detected_at: Timestamp,
        evidence_ids: Vec<String>,
    ) -> Result<Self> {
        let (id, subject) = (id.into(), subject.into());
        if id.is_empty() {
            return Err(Error::invalid(
                "a surprise needs an id; supply a unique identifier",
            ));
        }
        if model_ids.is_empty() || model_ids.iter().any(String::is_empty) {
            return Err(Error::invalid(
                "a surprise must name which models detected it; supply at least one, none blank",
            ));
        }
        if subject.is_empty() {
            return Err(Error::invalid(
                "a surprise needs a subject; name the entity surprised",
            ));
        }
        if evidence_ids.is_empty() || evidence_ids.iter().any(String::is_empty) {
            return Err(Error::invalid(
                "a surprise must carry evidence lineage; a detection without sources cannot be replayed",
            ));
        }
        Ok(Self {
            id,
            model_ids,
            subject,
            observed,
            expected,
            z_score,
            detected_at,
            evidence_ids,
        })
    }
}

/// The federation's view at one snapshot: the active hypotheses, scenarios,
/// and surprise detections across all models. Held as a complete immutable
/// record, journaled after update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationSnapshot {
    pub hypotheses: Vec<Hypothesis>,
    pub scenarios: Vec<ConditionalScenario>,
    pub surprises: Vec<FederationSurprise>,
    pub outcomes: Vec<Outcome>,
    pub snapshot_at: Timestamp,
}

impl FederationSnapshot {
    pub fn new(
        hypotheses: Vec<Hypothesis>,
        scenarios: Vec<ConditionalScenario>,
        surprises: Vec<FederationSurprise>,
        outcomes: Vec<Outcome>,
        snapshot_at: Timestamp,
    ) -> Self {
        Self {
            hypotheses,
            scenarios,
            surprises,
            outcomes,
            snapshot_at,
        }
    }
}
