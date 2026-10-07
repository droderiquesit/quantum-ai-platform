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

/// One of four required model acceptance validations (MODEL-033).
///
/// A model must pass all four types before staging. Each represents a distinct
/// validation perspective: out-of-time, cross-regime, adversarial stress, and
/// paper-trading simulation robustness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AcceptanceKind {
    /// Out-of-time validation via purged k-fold and deflated Sharpe.
    OutOfTime,
    /// Cross-regime scoring showing consistent performance across regimes.
    CrossRegime,
    /// Adversarial/scenario stress testing (portfolio-level resilience).
    AdversarialStress,
    /// Paper-trading simulation demonstrating robustness to real conditions.
    PaperTrading,
}

impl std::fmt::Display for AcceptanceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                AcceptanceKind::OutOfTime => "out-of-time",
                AcceptanceKind::CrossRegime => "cross-regime",
                AcceptanceKind::AdversarialStress => "adversarial-stress",
                AcceptanceKind::PaperTrading => "paper-trading",
            }
        )
    }
}

/// Unified acceptance record requiring all four validation types (MODEL-033).
///
/// Ties out-of-time, cross-regime, adversarial and paper-trading-simulation
/// results together. A model missing any one of the four kinds is refused
/// staging and promotion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelAcceptanceRecord {
    pub recorded_at: Timestamp,
    /// Which of the four acceptance kinds passed. A model with fewer than four
    /// distinct kinds is refused staging until the missing kinds have evidence.
    pub passed_kinds: BTreeMap<AcceptanceKind, bool>,
}

impl ModelAcceptanceRecord {
    /// Build an acceptance record with all four kinds set to passed or failed.
    pub fn new(
        recorded_at: Timestamp,
        out_of_time: bool,
        cross_regime: bool,
        adversarial_stress: bool,
        paper_trading: bool,
    ) -> Self {
        let passed_kinds = BTreeMap::from([
            (AcceptanceKind::OutOfTime, out_of_time),
            (AcceptanceKind::CrossRegime, cross_regime),
            (AcceptanceKind::AdversarialStress, adversarial_stress),
            (AcceptanceKind::PaperTrading, paper_trading),
        ]);
        Self {
            recorded_at,
            passed_kinds,
        }
    }

    /// Whether all four acceptance kinds have evidence and all passed.
    pub fn fully_accepted(&self) -> bool {
        self.passed_kinds.len() == 4 && self.passed_kinds.values().all(|&passed| passed)
    }

    /// Missing acceptance kinds, if any.
    pub fn missing_kinds(&self) -> Vec<AcceptanceKind> {
        vec![
            AcceptanceKind::OutOfTime,
            AcceptanceKind::CrossRegime,
            AcceptanceKind::AdversarialStress,
            AcceptanceKind::PaperTrading,
        ]
        .into_iter()
        .filter(|kind| !self.passed_kinds.contains_key(kind))
        .collect()
    }
}

/// The one alias this registry assigns: the version of a model name that may
/// inform a decision. It follows [`ModelStage::Production`] rather than being
/// a second pointer beside it, so the alias and the stage cannot disagree.
pub const PRODUCTION_ALIAS: &str = "production";

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
    /// Unified acceptance record requiring all four validation types (MODEL-033).
    /// `None` before acceptance is recorded; a model without this is refused staging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<ModelAcceptanceRecord>,
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
            acceptance: None,
        }
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

    pub fn register(&mut self, card: ModelCard) {
        self.cards.insert(card.reference(), card);
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
        if let Some(acceptance) = &card.acceptance {
            if !acceptance.fully_accepted() {
                let missing = acceptance
                    .missing_kinds()
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::denied(format!(
                    "{reference} cannot be promoted without all four acceptance kinds; missing: {missing}"
                )));
            }
            if !acceptance.passed_kinds.values().all(|&passed| passed) {
                let failed = acceptance
                    .passed_kinds
                    .iter()
                    .filter_map(|(kind, &passed)| {
                        if !passed {
                            Some(kind.to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::denied(format!(
                    "{reference} cannot be promoted with failed acceptance kinds: {failed}"
                )));
            }
        } else {
            return Err(Error::denied(format!(
                "{reference} cannot be promoted without a model acceptance record (MODEL-033)"
            )));
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

    /// Record unified model acceptance (MODEL-033).
    ///
    /// Ties out-of-time, cross-regime, adversarial and paper-trading validation
    /// results together. A model must have all four kinds passed before staging.
    pub fn record_acceptance(
        &mut self,
        reference: &str,
        record: ModelAcceptanceRecord,
    ) -> Result<()> {
        let card = self
            .get_mut(reference)
            .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
        card.acceptance = Some(record);
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
