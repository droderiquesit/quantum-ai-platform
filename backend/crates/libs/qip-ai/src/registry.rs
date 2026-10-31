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

use crate::serving::ModelArtifact;
use qip_core::error::{Error, Result};
use qip_core::{Duration, ModelId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The functional category of a model or reasoner.
///
/// Determines which lanes may use it and at what stage of the decision ladder.
/// The cost-router's Determinism::Required arm refuses to route any specialist
/// model or reasoner, so this enum's variants organize the intelligence that
/// routes to ModelTier rungs and nowhere else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelClass {
    #[default]
    /// A generic learned model or policy — the default case, routable to any tier.
    Generic,
    /// Specialist financial model for regime identification.
    SpecialistRegime,
    /// Specialist financial model for event probability.
    SpecialistEventProb,
    /// Specialist financial model for valuation.
    SpecialistValuation,
    /// Specialist financial model for liquidity.
    SpecialistLiquidity,
    /// Specialist financial model for slippage.
    SpecialistSlippage,
    /// Specialist financial model for default probability.
    SpecialistDefault,
    /// Specialist financial model for volatility.
    SpecialistVolatility,
    /// Specialist financial model for demand.
    SpecialistDemand,
    /// Specialist financial model for supply.
    SpecialistSupply,
    /// Symbolic reasoner — graph traversal and queries over explicit structure.
    ReasonerGraph,
    /// Symbolic reasoner — causal shock propagation.
    ReasonerCausal,
    /// A general-purpose pretrained model (a language or embedding model).
    /// Recorded so its use is attributable; never decision-eligible, because
    /// nothing in this registry measured it on the decision it would inform.
    Foundation,
    /// A learned policy that maps state to action. Decision-eligible only
    /// with a named benchmark, since a policy that beats nothing it was
    /// compared to has shown nothing (ADR 0006).
    Policy,
    /// A combination of member models. Decision-eligible only with a named
    /// benchmark, for the same reason: an ensemble is judged against the
    /// best of its members, or it is a more expensive member.
    Ensemble,
    /// A distilled student small enough for a reflex path. The one class
    /// besides `Generic` admitted to the hot lane, and decision-eligible only
    /// with a resource budget: a model admitted to a latency path with no
    /// stated ceiling has no ceiling anything can hold it to.
    Reflex,
}

impl ModelClass {
    pub const ALL: [Self; 16] = [
        Self::Generic,
        Self::SpecialistRegime,
        Self::SpecialistEventProb,
        Self::SpecialistValuation,
        Self::SpecialistLiquidity,
        Self::SpecialistSlippage,
        Self::SpecialistDefault,
        Self::SpecialistVolatility,
        Self::SpecialistDemand,
        Self::SpecialistSupply,
        Self::ReasonerGraph,
        Self::ReasonerCausal,
        Self::Foundation,
        Self::Policy,
        Self::Ensemble,
        Self::Reflex,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::SpecialistRegime => "specialist_regime",
            Self::SpecialistEventProb => "specialist_event_prob",
            Self::SpecialistValuation => "specialist_valuation",
            Self::SpecialistLiquidity => "specialist_liquidity",
            Self::SpecialistSlippage => "specialist_slippage",
            Self::SpecialistDefault => "specialist_default",
            Self::SpecialistVolatility => "specialist_volatility",
            Self::SpecialistDemand => "specialist_demand",
            Self::SpecialistSupply => "specialist_supply",
            Self::ReasonerGraph => "reasoner_graph",
            Self::ReasonerCausal => "reasoner_causal",
            Self::Foundation => "foundation",
            Self::Policy => "policy",
            Self::Ensemble => "ensemble",
            Self::Reflex => "reflex",
        }
    }

    /// Whether this model may be used at hot/reflex latency (lane 0).
    /// Specialist, reasoner, foundation, policy and ensemble models are too
    /// expensive for reflex paths; a reflex student is what they distil to.
    pub const fn allowed_in_hot_lane(self) -> bool {
        matches!(self, Self::Generic | Self::Reflex)
    }

    /// What this class needs on its card before it may inform a decision,
    /// beyond what every card needs. `Err` names the missing piece.
    fn class_eligibility(self, card: &ModelCard) -> std::result::Result<(), String> {
        match self {
            Self::Foundation => Err(
                "a foundation model is recorded for attribution and never decides: nothing \
                 here measured it on the decision it would inform"
                    .into(),
            ),
            Self::Policy | Self::Ensemble if card.benchmarks.is_empty() => Err(format!(
                "a {} model names no benchmark it was judged against; without one there is \
                 no evidence it beats anything",
                self.as_str()
            )),
            Self::Reflex if card.resource_budget.is_none() => Err(
                "a reflex model carries no resource budget; a model admitted to a latency path \
                 with no stated ceiling cannot be held to one"
                    .into(),
            ),
            _ => Ok(()),
        }
    }
}

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

/// The one alias this registry assigns: the version of a model name that may
/// inform a decision. It follows [`ModelStage::Production`] rather than being
/// a second pointer beside it, so the alias and the stage cannot disagree.
pub const PRODUCTION_ALIAS: &str = "production";

/// Whether a `market/volatility` regime label names a stressed regime
/// (ADR 0149 decision 10, R12-MODEL-6074): a market component of `crisis` or
/// `illiquid`, or a volatility component of `high` or `extreme`, as
/// `Platform::regime_context` writes them.
///
/// A label that is not two components separated by `/` is read as stressed.
/// The rule this feeds refuses a comparative claim, and a label nobody can
/// read is the case where refusing is the safe direction.
pub fn is_stressed_regime(regime: &str) -> bool {
    let Some((market, volatility)) = regime.split_once('/') else {
        return true;
    };
    if volatility.contains('/') {
        return true;
    }
    matches!(market, "crisis" | "illiquid") || matches!(volatility, "high" | "extreme")
}

/// How many model cards one registry holds before it refuses a new one.
///
/// The deep brain registers a card for every fit of every subject and never
/// removes one, so without a cap the registry grows for as long as the
/// process runs. 4,096 is far above any working set this platform has run
/// (a subject fitted every learning round for a year is a few hundred cards)
/// and small enough that the restore's full replay of registrations stays
/// cheap. Refused at the cap rather than evicted (R12-MODEL-9003): see
/// [`ModelRegistry::register`].
pub const MAX_CARDS: usize = 4_096;

/// How many alias moves a card retains, newest last. A model that flaps
/// between rollback and reactivation would otherwise grow its card without
/// bound; whoever journals the move holds the full history.
pub const ALIAS_MOVES_RETAINED: usize = 16;

/// One move of an alias on to or off a model version: who moved it, and on
/// what evidence (MODEL-057).
///
/// Until this record a promoted card said *when* it was deployed and nothing
/// about who decided or what they were looking at, so an alias found
/// pointing at the wrong version could not be traced to anyone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasMove {
    pub alias: String,
    /// Whether the move put the alias on this version (`true`) or took it
    /// off (`false`).
    pub assigned: bool,
    /// The desk or operator that moved it. Never blank.
    pub moved_by: String,
    /// What the move rested on, in words a reviewer can check against the
    /// card's evaluations. Never blank.
    pub evidence: String,
    pub at: Timestamp,
}

/// The record for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelCard {
    pub model_id: ModelId,
    pub name: String,
    pub version: String,
    /// What the model does and the decision it supports.
    pub purpose: String,
    /// Functional category determining which lanes and tiers may use it.
    #[serde(default)]
    pub class: ModelClass,
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
    /// The digest of the artifact this card was promoted with, once it has
    /// been — [`crate::serving::ModelArtifact::digest`], in-tree SHA-256 over
    /// the canonical payload. A digest and not a signature: it names the
    /// bytes a deployment must carry and says nothing about who produced
    /// them (ADR 0043). `None` for a card promoted without an artifact, which
    /// is the state every card was in before ADR 0083.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
    /// The named benchmarks and baselines this model is judged against,
    /// kept apart from `evaluations` (the measured results) so a reader can
    /// tell what the model was *meant* to beat from what it scored
    /// (EXPAND-038).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benchmarks: Vec<String>,
    /// The resource ceiling the model is admitted under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_budget: Option<ResourceBudget>,
    /// The reference this card displaced when it was promoted, recorded so a
    /// rollback has one unambiguous place to go back to. `None` for a card
    /// that displaced nothing, which therefore cannot be rolled back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_parent: Option<String>,
    /// Every recorded move of an alias on to or off this version, oldest
    /// first, bounded by [`ALIAS_MOVES_RETAINED`]. Written only by
    /// [`ModelRegistry::record_alias_move`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alias_moves: Vec<AliasMove>,
    /// The licensed market-data entitlements the model was trained on
    /// (R12-TICK-2091). Empty means the card records no licensed data —
    /// which is a claim, and [`ModelCard::authorise_location`] reads it as
    /// one, so a card trained on licensed data must say so here.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub data_entitlements: BTreeSet<String>,
    /// Where the artifact and its outputs may run or be used, as the
    /// intersection of what every entitlement above permits. A model trained
    /// on data licensed for one region and run in another breaches the
    /// licence invisibly; this is the field that makes it visible.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub permitted_locations: BTreeSet<String>,
    /// The market regimes (`Platform::regime_context` keys) this model's
    /// skill was measured in (R12-MODEL-4020). Empty means the card records
    /// no regime, which [`ModelCard::decision_eligibility_in`] reads as
    /// "not restricted" so a card that predates the field keeps working;
    /// every card a learning desk registers names the regime it was judged
    /// in, and a card that names one is restricted to exactly those.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub validated_regimes: BTreeSet<String>,
    /// The `(minimum, maximum)` each feature took in the data the model was
    /// fitted on (R12-RISK-4099). An input outside its range is an
    /// extrapolation the fit says nothing about, whether it came from a real
    /// market move or from someone shaping it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub training_bounds: BTreeMap<String, (f64, f64)>,
    /// Per feature, where its values came from (R12-MODEL-4016). Empty on a
    /// card that predates the field; [`ModelRegistry::promote`] refuses a
    /// card whose recorded origins show a look-ahead or an unaudited
    /// feature, and a learning desk refuses to promote a card missing any.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub feature_lineage: BTreeMap<String, FeatureOrigin>,
    /// The research campaign whose window the model was fitted on
    /// (R12-EXPAND-6076): the id of the campaign manifest the platform
    /// journaled, so the data behind the fit can be found from the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign_id: Option<String>,
    /// The build that fitted it, as the kernel names itself in a decision
    /// record (`qip-kernel <version> (<source commit>)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_version: Option<String>,
    /// The shadow period a self-fitted model ran through before it could be
    /// promoted (R12-EXPAND-6070). Written only by
    /// [`ModelRegistry::enter_shadow`] and
    /// [`ModelRegistry::record_shadow_round`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowRecord>,
    /// Whether the platform fitted this model for itself (the deep brain's
    /// learning desk). A self-fitted card reaches production only out of a
    /// completed shadow stage, whichever caller asks: the guard is here in
    /// [`ModelRegistry::promote`] rather than in a producer label a caller
    /// chooses (R12-EXPAND-6070).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub self_fitted: bool,
}

/// Shadow rounds a self-fitted model must win before it may be promoted.
/// Chosen, not measured: three windows of data that arrived after the fit is
/// the least that distinguishes a model from one lucky window.
pub const MIN_SHADOW_ROUNDS: u32 = 3;

/// A model's shadow period: it ran beside production, scored on data that
/// arrived after it was fitted, with its output acted on by nothing
/// ([`ModelStage::Shadow`] allows no decision).
///
/// The failure it prevents (R12-EXPAND-6070): a model the platform fitted
/// for itself went from its own holdout straight to production. A holdout
/// is the tail of the data the fit chose; it says how the model did on the
/// past it was handed, not how it does on a future nobody had yet seen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowRecord {
    pub since: Timestamp,
    /// Rounds in which the shadowed model beat production on fresh data.
    pub rounds_won: u32,
    pub last_round: Timestamp,
    /// One line per round won: what it was scored on, and against whom.
    pub evidence: Vec<String>,
}

/// Where one feature's values came from (R12-MODEL-4016): enough for a
/// reviewer to re-derive it and for a promotion to refuse a look-ahead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureOrigin {
    /// How the value is computed, in words a person can check against the code.
    pub derivation: String,
    /// Digest of the source records the column was computed from.
    pub source_digest: String,
    /// Digest of the transformation code that computed it.
    pub code_digest: String,
    /// The latest instant of source data any value of the feature reads.
    pub source_through: Timestamp,
    /// The instant the newest value claims to be known at.
    pub decision_at: Timestamp,
    /// How many rows the as-of audit recomputed from truncated and perturbed
    /// source. Zero is an audit that examined nothing, which is not a clean
    /// one.
    pub audited_rows: usize,
}

impl FeatureOrigin {
    /// Why this origin cannot stand behind a promotion, if it cannot.
    fn finding(&self, feature: &str) -> Option<String> {
        if self.source_through > self.decision_at {
            return Some(format!(
                "feature {feature} reads source through {}, after the {} it claims to be known at: \
                 look-ahead",
                self.source_through, self.decision_at
            ));
        }
        if self.audited_rows == 0 {
            return Some(format!(
                "feature {feature} was not audited as of its decision instant; an audit that \
                 examined nothing is not a clean one"
            ));
        }
        None
    }
}

/// A declared ceiling on what a model may consume. A ceiling set at
/// admission, not a measurement: enforcing it is the serving layer's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudget {
    /// Wall-clock microseconds one inference may take.
    pub max_inference_micros: u64,
    /// Bytes of memory the loaded model may hold.
    pub max_memory_bytes: u64,
}

/// A digest-named artifact, ready for a composition root to write.
///
/// The registry is a library and performs no I/O, so "promote writes the
/// artifact" is split at this type: the registry produces the name and the
/// bytes, and whoever composes it — a binary, today writing a file — puts
/// them somewhere. The name is the content's digest so that two promotions
/// of the same bytes land on one file and a file can be checked against its
/// own name by anyone holding it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedArtifact {
    /// `<digest>.json`.
    pub file_name: String,
    /// The artifact serialised, in the form
    /// [`crate::serving::ModelArtifact::from_json`] reads back.
    pub contents: String,
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
            class: ModelClass::Generic,
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
            artifact_digest: None,
            benchmarks: Vec::new(),
            resource_budget: None,
            rollback_parent: None,
            alias_moves: Vec::new(),
            data_entitlements: BTreeSet::new(),
            permitted_locations: BTreeSet::new(),
            validated_regimes: BTreeSet::new(),
            training_bounds: BTreeMap::new(),
            feature_lineage: BTreeMap::new(),
            campaign_id: None,
            code_version: None,
            shadow: None,
            self_fitted: false,
        }
    }

    /// What is missing to trace this card back to the campaign and the code
    /// that fitted it, if anything.
    pub fn provenance_gap(&self) -> Option<String> {
        let blank = |field: &Option<String>| field.as_deref().is_none_or(|v| v.trim().is_empty());
        match (blank(&self.campaign_id), blank(&self.code_version)) {
            (false, false) => None,
            (true, false) => Some("it records no research campaign".to_string()),
            (false, true) => Some("it records no code version".to_string()),
            (true, true) => {
                Some("it records neither a research campaign nor a code version".to_string())
            }
        }
    }

    /// Every way the recorded feature lineage cannot stand behind a
    /// promotion: a feature reading past its decision instant, or one never
    /// audited. Empty for a card recording no lineage — see
    /// [`Self::features_without_lineage`] for that half.
    pub fn lineage_findings(&self) -> Vec<String> {
        self.feature_lineage
            .iter()
            .filter_map(|(feature, origin)| origin.finding(feature))
            .collect()
    }

    /// The features this card consumes that its lineage does not account for.
    pub fn features_without_lineage(&self) -> Vec<&str> {
        self.features
            .iter()
            .filter(|feature| !self.feature_lineage.contains_key(*feature))
            .map(String::as_str)
            .collect()
    }

    /// Record a regime the model's skill was measured in.
    pub fn with_validated_regime(mut self, regime: impl Into<String>) -> Self {
        self.validated_regimes.insert(regime.into());
        self
    }

    /// Record the range each feature took in the fitting data. Refuses a
    /// non-finite or inverted range, and a name the card does not consume:
    /// a bound on a feature the model never reads would pass every input.
    pub fn with_training_bounds(mut self, bounds: BTreeMap<String, (f64, f64)>) -> Result<Self> {
        for (name, (low, high)) in &bounds {
            if !low.is_finite() || !high.is_finite() || low > high {
                return Err(Error::invalid(format!(
                    "{} bounds feature {name} to [{low}, {high}], which is not a finite range",
                    self.reference()
                )));
            }
            if !self.features.contains(name) {
                return Err(Error::invalid(format!(
                    "{} bounds feature {name}, which it does not consume; record the features \
                     before their bounds",
                    self.reference()
                )));
            }
        }
        self.training_bounds = bounds;
        Ok(self)
    }

    /// Record the entitlements the model was trained under and the locations
    /// the artifact may run in. Refuses entitlements with no permitted
    /// location: a model nobody may run anywhere is a licence misread, not a
    /// model, and recording it would hide that.
    pub fn with_data_entitlements(
        mut self,
        entitlements: BTreeSet<String>,
        permitted_locations: BTreeSet<String>,
    ) -> Result<Self> {
        if entitlements.iter().any(|e| e.trim().is_empty())
            || permitted_locations.iter().any(|l| l.trim().is_empty())
        {
            return Err(Error::invalid(
                "a model card's entitlements and permitted locations are named, never blank",
            ));
        }
        if !entitlements.is_empty() && permitted_locations.is_empty() {
            return Err(Error::invalid(format!(
                "{} was trained on licensed data ({}) whose terms permit no location; resolve \
                 the licence before registering the model",
                self.reference(),
                entitlements.iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        self.data_entitlements = entitlements;
        self.permitted_locations = permitted_locations;
        Ok(self)
    }

    /// Refuse to deploy, ship or use this model's outputs at `location`
    /// unless its training entitlements permit it there. A card recording
    /// no licensed data is not restricted by licence; every other card is
    /// restricted to exactly its permitted locations.
    pub fn authorise_location(&self, location: &str) -> Result<()> {
        if self.data_entitlements.is_empty() || self.permitted_locations.contains(location) {
            return Ok(());
        }
        Err(Error::denied(format!(
            "{} was trained on {} and may run only in {}; {location} is not among them, and \
             running it there would breach the data licence",
            self.reference(),
            self.data_entitlements
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            self.permitted_locations
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    pub fn with_benchmark(mut self, benchmark: impl Into<String>) -> Self {
        self.benchmarks.push(benchmark.into());
        self
    }

    pub const fn with_resource_budget(mut self, budget: ResourceBudget) -> Self {
        self.resource_budget = Some(budget);
        self
    }

    pub fn with_purpose(mut self, purpose: impl Into<String>) -> Self {
        self.purpose = purpose.into();
        self
    }

    pub fn with_class(mut self, class: ModelClass) -> Self {
        self.class = class;
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
        self.class.class_eligibility(self)
    }

    /// [`Self::decision_eligibility`] for a decision taken in `regime`, with
    /// no inputs to check (R12-RISK-4099): a card that names the regimes it
    /// was validated in is refused in another. For a caller that ranks
    /// models before any input exists, such as the cost router's reputation
    /// book, so a model validated only in a calm market is not ranked for a
    /// crisis decision. [`Self::decision_eligibility_in`] adds the input
    /// checks once the inputs are known.
    pub fn regime_eligibility(
        &self,
        now: Timestamp,
        regime: &str,
    ) -> std::result::Result<(), String> {
        self.decision_eligibility(now)?;
        match self.regime_gap(regime) {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }

    /// [`Self::decision_eligibility`] for a decision taken in `regime` on
    /// `inputs` (R12-MODEL-4020, R12-RISK-4099).
    ///
    /// Two further refusals. A card that names the regimes it was validated in
    /// is not accepted in another: skill measured in a calm market says
    /// nothing about a stressed one, and the scoreboard that chose the model
    /// only ever scored it where it ran. And an input outside the range the
    /// fit saw, or not a finite number, is refused: the model would be
    /// extrapolating, and an adversarial or corrupted value is exactly an
    /// input chosen to land there. A bounded feature the caller did not supply
    /// is refused too, since there is nothing to check it against.
    pub fn decision_eligibility_in(
        &self,
        now: Timestamp,
        regime: &str,
        inputs: &BTreeMap<String, f64>,
    ) -> std::result::Result<(), String> {
        self.decision_eligibility(now)?;
        if let Some(reason) = self.regime_gap(regime) {
            return Err(reason);
        }
        for (name, (low, high)) in &self.training_bounds {
            let Some(value) = inputs.get(name) else {
                return Err(format!(
                    "feature {name} has a recorded training range and no input to check against it"
                ));
            };
            if !value.is_finite() {
                return Err(format!("feature {name} is not a finite number"));
            }
            if value < low || value > high {
                return Err(format!(
                    "feature {name} is {value}, outside the [{low}, {high}] the model was fitted on"
                ));
            }
        }
        Ok(())
    }

    /// Why this card's validation does not cover `regime`, if it does not.
    fn regime_gap(&self, regime: &str) -> Option<String> {
        if self.validated_regimes.is_empty() || self.validated_regimes.contains(regime) {
            return None;
        }
        Some(format!(
            "validated only in {}, never scored in {regime}",
            self.validated_regimes
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Every model the platform knows about.
///
/// `Clone` so a caller that must journal a promotion *before* the registry
/// adopts it can promote a scratch copy first, write the record, and then
/// replace the live registry — the discipline the kernel's registration and
/// eligibility paths already keep. A registry is a map of cards measured in
/// kilobytes; the copy is cheaper than a log that names a promotion the
/// registry then refused.
#[derive(Clone, Debug, Default)]
pub struct ModelRegistry {
    cards: BTreeMap<String, ModelCard>,
    packages: BTreeMap<String, Package>,
}

/// The four kinds of package the registry holds (MODEL-068).
///
/// A closed set on purpose: the registry is what a deployment is checked
/// against, and a kind it does not know is a thing it cannot say anything
/// true about. [`PackageKind::parse`] refuses the rest by name rather than
/// filing them under a catch-all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Model,
    Policy,
    Feature,
    Risk,
}

impl PackageKind {
    pub const ALL: [Self; 4] = [Self::Model, Self::Policy, Self::Feature, Self::Risk];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Policy => "policy",
            Self::Feature => "feature",
            Self::Risk => "risk",
        }
    }

    pub fn parse(kind: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|known| known.as_str() == kind)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "`{kind}` is not a package kind the registry holds; the kinds are model, \
                     policy, feature and risk"
                ))
            })
    }
}

/// One registered package version and the digest of its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub kind: PackageKind,
    pub name: String,
    pub version: String,
    /// SHA-256 of the package's bytes, hex. Computed by the producer;
    /// the registry checks its shape and never overwrites it.
    pub digest: String,
    pub registered_at: Timestamp,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `card`, or replace the card already held under its reference.
    ///
    /// Refuses a new reference once the registry holds [`MAX_CARDS`] cards
    /// (R12-MODEL-9003). A replacement is always admitted: it does not grow
    /// the registry. Refused rather than evicted, because a card the event log
    /// still names and the registry silently dropped is a replay that cannot
    /// find it; the refusal is the visible control, and the caller counts and
    /// journals it.
    pub fn register(&mut self, card: ModelCard) -> Result<()> {
        let reference = card.reference();
        // ADR 0198: `promote` is the one door into production for a model the
        // platform fitted for itself, and this seam honours the same rule, so
        // a card built at `Production` cannot walk around the shadow stage
        // (the restore path registers journaled cards, which carry the shadow
        // they were promoted out of). A pipeline-trained card is governed by
        // its evaluation and CI attestation, not by shadow, and is not held
        // here.
        if card.self_fitted && card.stage == ModelStage::Production {
            let completed = card
                .shadow
                .as_ref()
                .is_some_and(|shadow| shadow.rounds_won >= MIN_SHADOW_ROUNDS);
            if !completed {
                return Err(Error::denied(format!(
                    "{reference} is not registered: a self-fitted card at Production must carry \
                     a completed shadow stage ({MIN_SHADOW_ROUNDS} round(s) won); promote it out \
                     of shadow instead of registering it there"
                )));
            }
        }
        if !self.cards.contains_key(&reference) && self.cards.len() >= MAX_CARDS {
            return Err(Error::denied(format!(
                "{reference} is not registered: the registry holds {MAX_CARDS} model cards, its \
                 cap; retire and archive cards from the event log before registering more"
            )));
        }
        self.cards.insert(reference, card);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    /// Register a package of one of the four kinds under `name@version`.
    ///
    /// Refuses an unknown kind, a digest that is not 64 hex characters (a
    /// digest nobody computed names nothing), and a second digest under a
    /// version already registered — the same rule
    /// [`Self::promote_artifact`] keeps for models, so a version names one
    /// artifact whatever kind of package it is. Re-registering the same
    /// digest is idempotent.
    pub fn register_package(
        &mut self,
        kind: &str,
        name: &str,
        version: &str,
        digest: &str,
        at: Timestamp,
    ) -> Result<()> {
        let kind = PackageKind::parse(kind)?;
        if name.trim().is_empty() || version.trim().is_empty() {
            return Err(Error::invalid(
                "a package needs a name and a version to be referenced by",
            ));
        }
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::invalid(format!(
                "{name}@{version} carries `{digest}`, which is not a SHA-256 digest; compute \
                 the digest of the package's bytes and register that"
            )));
        }
        let key = format!("{}:{name}@{version}", kind.as_str());
        if let Some(existing) = self.packages.get(&key) {
            if existing.digest == digest {
                return Ok(());
            }
            return Err(Error::denied(format!(
                "{} package {name}@{version} was registered at {} and these bytes digest to {digest}; \
                 a version names one artifact — register the new bytes under a new version",
                kind.as_str(),
                existing.digest
            )));
        }
        self.packages.insert(
            key,
            Package {
                kind,
                name: name.to_string(),
                version: version.to_string(),
                digest: digest.to_string(),
                registered_at: at,
            },
        );
        Ok(())
    }

    /// A registered package, by kind and `name@version`.
    pub fn package(&self, kind: PackageKind, name: &str, version: &str) -> Option<&Package> {
        self.packages
            .get(&format!("{}:{name}@{version}", kind.as_str()))
    }

    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
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

    /// [`Self::require_for_decision`] for a decision in `regime` on `inputs`:
    /// the card must also have been validated in that regime and the inputs
    /// must lie inside the range it was fitted on.
    pub fn require_for_decision_in(
        &self,
        reference: &str,
        regime: &str,
        inputs: &BTreeMap<String, f64>,
        now: Timestamp,
    ) -> Result<&ModelCard> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.decision_eligibility_in(now, regime, inputs)
            .map_err(|reason| {
                Error::denied(format!("{reference} may not drive a decision: {reason}"))
            })?;
        Ok(card)
    }

    /// Put a model into shadow: it runs beside production and nothing acts
    /// on it (R12-EXPAND-6070). Refused, before the card changes, for a
    /// model without a passing evaluation, one whose lineage shows a
    /// look-ahead, and one already in production, shadow or retirement —
    /// a shadow period restarted on a model that has one would erase the
    /// rounds it lost.
    pub fn enter_shadow(&mut self, reference: &str, at: Timestamp) -> Result<()> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if matches!(
            card.stage,
            ModelStage::Production | ModelStage::Shadow | ModelStage::Retired
        ) {
            return Err(Error::denied(format!(
                "{reference} is in {:?}; only a model not yet run may enter shadow",
                card.stage
            )));
        }
        if card.latest_evaluation().is_none_or(|e| !e.passed) {
            return Err(Error::denied(format!(
                "{reference} cannot enter shadow without a passing evaluation"
            )));
        }
        let leaks = card.lineage_findings();
        if !leaks.is_empty() {
            return Err(Error::denied(format!(
                "{reference} cannot enter shadow: {}",
                leaks.join("; ")
            )));
        }
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.stage = ModelStage::Shadow;
        card.shadow = Some(ShadowRecord {
            since: at,
            rounds_won: 0,
            last_round: at,
            evidence: Vec::new(),
        });
        Ok(())
    }

    /// Record one shadow round the model won, with what it was scored on.
    /// Returns the rounds won so far. Refused for a model not in shadow,
    /// blank evidence, and a round not strictly after the last one (or after
    /// entering shadow, for the first): a round is new data, and one round
    /// counted twice is one round.
    pub fn record_shadow_round(
        &mut self,
        reference: &str,
        evidence: &str,
        at: Timestamp,
    ) -> Result<u32> {
        if evidence.trim().is_empty() {
            return Err(Error::invalid(
                "a shadow round names what the model was scored on and against whom",
            ));
        }
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        let (ModelStage::Shadow, Some(shadow)) = (card.stage, card.shadow.as_mut()) else {
            return Err(Error::denied(format!(
                "{reference} is not in shadow; a round is recorded only for a shadowed model"
            )));
        };
        if at <= shadow.last_round {
            return Err(Error::denied(format!(
                "{reference}'s last shadow round was at {}; a round at {} is not a new one",
                shadow.last_round.as_secs(),
                at.as_secs()
            )));
        }
        shadow.rounds_won = shadow.rounds_won.saturating_add(1);
        shadow.last_round = at;
        shadow.evidence.push(evidence.trim().to_string());
        Ok(shadow.rounds_won)
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
        let leaks = card.lineage_findings();
        if !leaks.is_empty() {
            return Err(Error::denied(format!(
                "{reference} cannot be promoted: {}; a model fitted on the future backtests \
                 beautifully and cannot trade",
                leaks.join("; ")
            )));
        }
        if card.self_fitted {
            let completed = card.stage == ModelStage::Shadow
                && card
                    .shadow
                    .as_ref()
                    .is_some_and(|shadow| shadow.rounds_won >= MIN_SHADOW_ROUNDS);
            if !completed {
                return Err(Error::denied(format!(
                    "{reference} cannot be promoted: a self-fitted model reaches production only \
                     out of a completed shadow stage ({MIN_SHADOW_ROUNDS} round(s) won), and this \
                     one is in {:?}",
                    card.stage
                )));
            }
        }
        card.stage = ModelStage::Production;
        card.deployed_at = Some(at);
        Ok(())
    }

    /// Promote a model together with the artifact a deployment will carry
    /// (ADR 0083's promote stage), and return the artifact named by its
    /// digest for the composition root to write.
    ///
    /// Three refusals, each before the card changes: an artifact whose digest
    /// is not the digest of its payload (the bytes are not the bytes), a
    /// card that would not promote on its own terms — no passing evaluation
    /// — and a card already promoted with a *different* artifact. The last
    /// is the one a reader might expect to be allowed: re-promoting a
    /// version with new bytes is a different model wearing the same name,
    /// and every decision that referenced the old digest would then cite a
    /// model that no longer exists under that reference. Bump the version.
    pub fn promote_artifact(
        &mut self,
        artifact: &ModelArtifact,
        at: Timestamp,
    ) -> Result<PublishedArtifact> {
        artifact.verify_digest()?;
        let existing = self
            .get(&artifact.reference)
            .ok_or_else(|| {
                Error::not_found(format!("no model registered as {}", artifact.reference))
            })?
            .artifact_digest
            .clone();
        if let Some(existing) = existing
            && existing != artifact.digest
        {
            return Err(Error::denied(format!(
                "{} was promoted with artifact {existing} and this artifact digests to {}; a \
                 version whose bytes changed is a different model — register it under a new \
                 version rather than replacing what decisions already cite",
                artifact.reference, artifact.digest
            )));
        }
        self.promote(&artifact.reference, at)?;
        let card = self.get_mut(&artifact.reference).ok_or_else(|| {
            Error::not_found(format!("no model registered as {}", artifact.reference))
        })?;
        card.artifact_digest = Some(artifact.digest.clone());
        let contents = serde_json::to_string_pretty(artifact).map_err(|error| {
            Error::invalid(format!(
                "artifact {} could not be serialised: {error}",
                artifact.reference
            ))
        })?;
        Ok(PublishedArtifact {
            file_name: format!("{}.json", artifact.digest),
            contents,
        })
    }

    /// Record which of the references a promotion displaced is the one to
    /// return to. With several displaced, the most recently deployed is the
    /// parent: it is what production was running immediately beforehand.
    /// A no-op for an empty `displaced`, because a first promotion has no
    /// parent and must not be given one.
    pub fn record_rollback_parent(&mut self, reference: &str, displaced: &[String]) -> Result<()> {
        let mut parent: Option<(&String, i64)> = None;
        for candidate in displaced {
            let card = self.get(candidate).ok_or_else(|| {
                Error::not_found(format!("displaced model {candidate} is not registered"))
            })?;
            let at = card.deployed_at.unwrap_or(card.created_at).as_nanos();
            if parent.is_none_or(|(_, best)| at >= best) {
                parent = Some((candidate, at));
            }
        }
        let Some((parent, _)) = parent else {
            return Ok(());
        };
        let parent = parent.clone();
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.rollback_parent = Some(parent);
        Ok(())
    }

    /// Roll a production model back to the reference it displaced: the model
    /// is retired and its recorded parent returns to production under the
    /// artifact digest it was promoted with. Returns the parent.
    ///
    /// Refused, before anything changes, for a model that is not in
    /// production, one that recorded no parent (nothing to go back to, and
    /// guessing would be rolling back to a model nobody chose), or a parent
    /// that holds no artifact digest, because a rollback that cannot say
    /// which bytes it restores is not a rollback.
    pub fn rollback(&mut self, reference: &str, at: Timestamp) -> Result<&ModelCard> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if card.stage != ModelStage::Production {
            return Err(Error::denied(format!(
                "{reference} is in {:?}, not production; only a production model is rolled back",
                card.stage
            )));
        }
        let Some(parent) = card.rollback_parent.clone() else {
            return Err(Error::denied(format!(
                "{reference} recorded no parent when it was promoted, so there is nothing to \
                 roll back to; promote a known-good version instead"
            )));
        };
        let held = self.get(&parent).ok_or_else(|| {
            Error::not_found(format!("the recorded parent {parent} is not registered"))
        })?;
        if held.artifact_digest.is_none() {
            return Err(Error::denied(format!(
                "the recorded parent {parent} holds no artifact digest, so a rollback could \
                 not say which bytes it restores"
            )));
        }
        self.retire(reference, at)?;
        let restored = self
            .get_mut(&parent)
            .ok_or_else(|| Error::not_found(format!("the recorded parent {parent} vanished")))?;
        restored.stage = ModelStage::Production;
        restored.deployed_at = Some(at);
        restored.retired_at = None;
        Ok(restored)
    }

    /// Return a retired model to production: the rollback half of automatic
    /// retirement (MODEL-045).
    ///
    /// Refused unless the card is retired, carries the artifact digest it was
    /// promoted with (so what comes back is bytes that passed the gate, not a
    /// card that merely exists), last evaluated as passed, and has not drifted
    /// past its own threshold. A rollback to a model that is itself degraded
    /// replaces one failure with another and says it recovered.
    pub fn reactivate(&mut self, reference: &str, at: Timestamp) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if card.stage != ModelStage::Retired {
            return Err(Error::denied(format!(
                "{reference} is not retired, so there is nothing to reactivate"
            )));
        }
        if card.artifact_digest.is_none() {
            return Err(Error::denied(format!(
                "{reference} was never promoted with an artifact, so no known-good bytes exist \
                 to roll back to"
            )));
        }
        if card.latest_evaluation().is_none_or(|e| !e.passed) {
            return Err(Error::denied(format!(
                "{reference} cannot be reactivated without a passing evaluation"
            )));
        }
        if card.drift_score > card.drift_threshold {
            return Err(Error::denied(format!(
                "{reference} has itself drifted to {:.3}, past its threshold {:.3}; it is not a \
                 known-good model to roll back to",
                card.drift_score, card.drift_threshold
            )));
        }
        card.stage = ModelStage::Production;
        card.deployed_at = Some(at);
        card.retired_at = None;
        Ok(())
    }

    /// The aliases `reference` holds right now.
    ///
    /// [`PRODUCTION_ALIAS`] when the card is the production version of its
    /// name, and nothing otherwise — a retired, shadow or development card
    /// holds no alias, and of two production cards under one name only the
    /// one [`Self::production_version`] returns does. A list because the
    /// question is "which aliases", and one alias today is not a promise of
    /// one for ever.
    pub fn aliases(&self, reference: &str) -> Result<Vec<&'static str>> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        let holds = self
            .production_version(&card.name)
            .is_some_and(|production| production.reference() == reference);
        Ok(if holds {
            vec![PRODUCTION_ALIAS]
        } else {
            Vec::new()
        })
    }

    /// Record who moved the production alias on to or off `reference`, and
    /// on what evidence (MODEL-057).
    ///
    /// Called by whoever moved it, immediately after the move. Which way the
    /// alias went is read from the card's own stage rather than taken as an
    /// argument, so a caller cannot record an assignment the registry did
    /// not make. Refuses a blank mover or blank evidence — an anonymous move
    /// and an unexplained one are the two records this exists to rule out —
    /// and a reference the registry does not hold.
    pub fn record_alias_move(
        &mut self,
        reference: &str,
        moved_by: &str,
        evidence: &str,
        at: Timestamp,
    ) -> Result<()> {
        if moved_by.trim().is_empty() {
            return Err(Error::denied(format!(
                "the production alias move on {reference} names nobody; name the desk or \
                 operator that moved it"
            )));
        }
        if evidence.trim().is_empty() {
            return Err(Error::denied(format!(
                "the production alias move on {reference} states no evidence; say what the \
                 move rested on"
            )));
        }
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if card.alias_moves.len() >= ALIAS_MOVES_RETAINED {
            let excess = card.alias_moves.len() + 1 - ALIAS_MOVES_RETAINED;
            card.alias_moves.drain(..excess);
        }
        card.alias_moves.push(AliasMove {
            alias: PRODUCTION_ALIAS.to_string(),
            assigned: card.stage == ModelStage::Production,
            moved_by: moved_by.to_string(),
            evidence: evidence.to_string(),
            at,
        });
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

    /// [`Self::ineligible`] as it stands in `regime`: a production model whose
    /// skill was measured elsewhere is listed with that as its reason, where
    /// [`Self::ineligible`] would call it fine. `inputs_for` supplies the
    /// current inputs of the cards it can speak for, whose recorded training
    /// ranges are then checked against them too; a card it returns `None` for
    /// is judged on regime alone.
    pub fn ineligible_in_regime<'a>(
        &self,
        now: Timestamp,
        regime: &str,
        inputs_for: impl Fn(&ModelCard) -> Option<&'a BTreeMap<String, f64>>,
    ) -> Vec<(&ModelCard, String)> {
        self.cards
            .values()
            .filter(|c| c.stage == ModelStage::Production)
            .filter_map(|c| {
                let verdict = match inputs_for(c) {
                    Some(inputs) => c.decision_eligibility_in(now, regime, inputs),
                    None => c
                        .decision_eligibility(now)
                        .and_then(|()| c.regime_gap(regime).map_or(Ok(()), Err)),
                };
                verdict.err().map(|reason| (c, reason))
            })
            .collect()
    }

    /// Check whether a model is allowed in a hot-lane package.
    /// Specialist and reasoner models are refused for hot-lane use.
    pub fn allowed_in_hot_lane(&self, reference: &str) -> Result<()> {
        let card = self
            .get(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        if card.class.allowed_in_hot_lane() {
            Ok(())
        } else {
            Err(Error::denied(format!(
                "{} is {}, which is not allowed in hot-lane (lane 0) packages; only generic \
                 and reflex models are light enough for reflex paths",
                reference,
                card.class.as_str()
            )))
        }
    }
}
