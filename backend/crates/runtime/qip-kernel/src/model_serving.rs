//! Where a trained model is promoted, served and named to a cell (ADR 0083).
//!
//! Three seams on [`Platform`], and the reason each is here rather than in
//! `qip-deepbrain`, which trains the models, or `qip-api`, which ships the
//! policy:
//!
//! * [`Platform::promote_model`] is the promote stage of blueprint §21.2.
//!   It asks the platform's provider to serve the artifact first, so a
//!   model nobody in this process can evaluate is never promoted; promotes
//!   a scratch copy of the registry, so a registry that would refuse is
//!   never journalled as having accepted; journals a [`ModelPromotion`]
//!   before the live registry adopts it, so the log never lacks a
//!   promotion the platform is acting on; and hands back the digest-named
//!   artifact for the composition root to write, because a kernel performs
//!   no I/O it cannot replay.
//! * [`Platform::model_manifest`] is the deploy stage's producer: the
//!   `trained_models` slot names every promoted distillate by the digest a
//!   cell checks on install, `DistilledModel::digest()`, keyed by the name
//!   the compiled plan carries it under. **Not** the artifact's SHA-256 —
//!   `Cell::check_models_promoted` compares the inline model's own digest,
//!   and a manifest filled from `ModelCard::artifact_digest` would name
//!   every promoted model at a digest no cell recognises. Both digests are
//!   on the record so a reader can match either.
//! * [`Platform::serve_model`] is the port §39.1's advisory row and the
//!   deep brain's incumbent comparison score through. The provider is the
//!   one the composition root handed in; `Platform::new` holds
//!   [`qip_ai::serving::NoProvider`] and serves nothing, which is what every
//!   root did before this module.
//!
//! What is deliberately absent. No route promotes a model: the API holds no
//! registry and the promotion rule lives with the desk that fitted the
//! candidate and can rescore the incumbent on the same held-out rows. No
//! stage of the cycle reads a served score yet — the models this platform
//! trains predict a next-bar return per instrument, and §39.1 row 10 names
//! cost, dispersion and regime estimates; wiring a return prediction into
//! the regime filter would invent an observation, so the consumer waits on
//! a model of the kind the row names. And nothing here signs: a digest under
//! the mesh envelope's HMAC proves the bytes and the sender, not who trained
//! the model (ADR 0043).

use crate::platform::Platform;
use qip_ai::registry::{ModelRegistry, PublishedArtifact};
use qip_ai::serving::{ModelArtifact, ModelFormat, ModelProvider, ServedModel};
use qip_contracts::policy::{ModelManifest, Slot};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_strategy::model::DistilledModel;
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The producer on every promotion record, so a replay picks them out of
/// the `model.evaluated` topic they share with the deep brain's evaluation
/// records the way the venue review's records are picked out by origin.
pub const MODEL_PROMOTION_ORIGIN: &str = "kernel/model-promotion";

/// The distillate a promotion names to the cells: the name the compiled
/// plan carries the model under and the digest `Cell::check_models_promoted`
/// compares against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistilledManifestEntry {
    pub name: String,
    /// `DistilledModel::digest()` — over the name and the coefficients'
    /// bits, which is what the cell computes from the inline model.
    pub digest: String,
}

/// One promotion, as the log holds it and as a restarted process rebuilds
/// its promoted set from.
///
/// Journalled *before* the registry adopts the promotion, and written on
/// the outcome only: a refused promotion is not a record here, because the
/// registry's refusal is the fact and the desk's round line carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPromotion {
    /// `ModelCard::reference()` — name and version.
    pub reference: String,
    /// The card's name, under which the incumbent it displaced stood.
    pub name: String,
    pub format: ModelFormat,
    /// `ModelArtifact::digest` — SHA-256 over the canonical payload, the
    /// name of the file the composition root writes.
    pub artifact_digest: String,
    /// How many inputs the served model reads, as the provider served it.
    pub arity: usize,
    /// The distillate the cells may run inline, where the fidelity policy
    /// admitted one. `None` promotes the teacher for advisory use only and
    /// names nothing to a cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distilled: Option<DistilledManifestEntry>,
    /// References retired by this promotion — the incumbents it beat on the
    /// held-out rows. Removed from the promoted set on apply and on replay,
    /// so a cell holding a plan with a displaced distillate inline refuses
    /// it on the next manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displaced: Vec<String>,
    pub at: Timestamp,
}

impl EventBody for ModelPromotion {
    /// The Learn group, which the log never evicts, so a restarted process
    /// rebuilds its promoted set from every promotion it ever made. Told
    /// apart from the deep brain's evaluation records by
    /// [`MODEL_PROMOTION_ORIGIN`].
    const TOPIC: Topic = Topic::ModelEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// What the `trained_models` slot carries this cycle, or why it ships
/// unproduced — the same shape as the causal and episodic issues, so
/// `pending_policy` reads all three alike.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelManifestIssue {
    manifest: ModelManifest,
    /// The newest promotion the manifest names, which is the instant a cell
    /// reads the slot's age from: a manifest is as fresh as its last change,
    /// not as the cycle that shipped it.
    produced_at: Timestamp,
    /// Promotions that named no distillate — teachers promoted for advisory
    /// use — counted so the line says how many promoted models a cell is
    /// *not* being told about, and why.
    advisory_only: usize,
}

impl ModelManifestIssue {
    pub fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    pub fn slot(&self) -> Slot<ModelManifest> {
        Slot::produced(self.manifest.clone(), self.produced_at)
    }

    pub fn describe(&self) -> String {
        format!(
            "trained models: shipped {} distillate(s) by digest, newest promoted {}; {} \
             promoted model(s) named to no cell because the fidelity policy admitted no \
             distillate of them",
            self.manifest.models.len(),
            self.produced_at.to_rfc3339(),
            self.advisory_only
        )
    }
}

/// Rebuild the promoted set from the log alone, in log order, applying
/// each record exactly as [`Platform::promote_model`] applied it.
pub(crate) fn resume_model_promotions(log: &EventLog) -> Result<BTreeMap<String, ModelPromotion>> {
    let mut promoted = BTreeMap::new();
    for event in log.events() {
        if event.topic != ModelPromotion::TOPIC || event.lineage.producer != MODEL_PROMOTION_ORIGIN
        {
            continue;
        }
        let record = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<ModelPromotion>())?
            .body;
        apply_promotion(&mut promoted, record);
    }
    Ok(promoted)
}

/// One apply for the live path and the replay, so they cannot diverge.
fn apply_promotion(promoted: &mut BTreeMap<String, ModelPromotion>, record: ModelPromotion) {
    for displaced in &record.displaced {
        promoted.remove(displaced);
    }
    promoted.insert(record.reference.clone(), record);
}

impl Platform {
    /// The provider the composition root handed in.
    pub fn model_provider(&self) -> &dyn ModelProvider {
        self.model_provider.as_ref()
    }

    /// Serve an artifact through the platform's provider, off the hot path.
    ///
    /// The one call every consumer of a served score goes through, so a
    /// process assembled without a provider refuses here with the null
    /// provider's own sentence rather than anywhere downstream.
    pub fn serve_model(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        self.model_provider.serve(artifact)
    }

    /// Every promotion this process holds, by reference, in reference order.
    pub fn model_promotions(&self) -> &BTreeMap<String, ModelPromotion> {
        &self.model_promotions
    }

    /// Promote a model with its artifact: serve it first, promote a scratch
    /// registry, journal, then adopt (blueprint §21.2's promote stage).
    ///
    /// Four refusals, each before anything changes. The provider cannot
    /// serve the artifact — ADR 0083's own sentence, and the reason this is
    /// on the platform: a model the process could not evaluate is not one
    /// it may act on. The served model's arity disagrees with the card's
    /// feature list — an artifact of one shape under a card describing
    /// another is two claims about one function. The registry refuses the
    /// promotion — no passing evaluation, a digest that is not the
    /// payload's, a version whose bytes changed — with its own sentence.
    /// A displaced reference the registry does not hold.
    ///
    /// `displaced` is the caller's finding: the desk that fitted the
    /// candidate rescored the incumbent on the same held-out rows and is
    /// naming what it beat. Retired here, on the same scratch, so a
    /// promotion and its retirements are one record or none.
    pub fn promote_model(
        &mut self,
        registry: &mut ModelRegistry,
        artifact: &ModelArtifact,
        distilled: Option<&DistilledModel>,
        displaced: &[String],
        now: Timestamp,
    ) -> Result<PublishedArtifact> {
        let served = self.model_provider.serve(artifact).map_err(|error| {
            Error::denied(format!(
                "{} is not promoted: {}",
                artifact.reference,
                error.message()
            ))
        })?;
        let card = registry.get(&artifact.reference).ok_or_else(|| {
            Error::not_found(format!("no model registered as {}", artifact.reference))
        })?;
        if card.features.len() != served.arity() {
            return Err(Error::invalid(format!(
                "{} is not promoted: its card names {} feature(s) and the artifact the \
                 provider served reads {} input(s); the card and the artifact describe \
                 different functions — re-pack the artifact from the fit the card records",
                artifact.reference,
                card.features.len(),
                served.arity()
            )));
        }
        let name = card.name.clone();
        let mut scratch = registry.clone();
        let published = scratch.promote_artifact(artifact, now)?;
        for reference in displaced {
            scratch.retire(reference, now)?;
        }
        let record = ModelPromotion {
            reference: artifact.reference.clone(),
            name,
            format: artifact.format,
            artifact_digest: artifact.digest.clone(),
            arity: served.arity(),
            distilled: distilled.map(|model| DistilledManifestEntry {
                name: model.name().to_string(),
                digest: model.digest(),
            }),
            displaced: displaced.to_vec(),
            at: now,
        };
        self.journal_record(record.clone(), MODEL_PROMOTION_ORIGIN, now)?;
        *registry = scratch;
        apply_promotion(&mut self.model_promotions, record);
        Ok(published)
    }

    /// The `trained_models` slot: every promoted distillate by the digest a
    /// cell checks, or the reason the slot ships unproduced.
    ///
    /// Refuses, rather than producing an empty manifest, when no distillate
    /// has been promoted in this process. An empty produced manifest and no
    /// manifest narrow a cell identically — `check_models_promoted` refuses
    /// any inline model either way — but they are different facts with
    /// different owners: "the centre promoted nothing" is the deep brain's
    /// and "this process has not resumed a promotion" is the shipper's, and
    /// the line says which.
    pub fn model_manifest(&self) -> Result<ModelManifestIssue> {
        let mut models = BTreeMap::new();
        let mut produced_at: Option<Timestamp> = None;
        let mut advisory_only = 0usize;
        for promotion in self.model_promotions.values() {
            match &promotion.distilled {
                Some(entry) => {
                    models.insert(entry.name.clone(), entry.digest.clone());
                    produced_at = Some(produced_at.map_or(promotion.at, |at| at.max(promotion.at)));
                }
                None => advisory_only += 1,
            }
        }
        match produced_at {
            Some(produced_at) => Ok(ModelManifestIssue {
                manifest: ModelManifest { models },
                produced_at,
                advisory_only,
            }),
            None => Err(Error::not_found(format!(
                "no distilled model has been promoted in this process ({} promotion(s) held, \
                 {advisory_only} advisory-only); the trained_models slot ships unproduced and \
                 every cell refuses a plan carrying a model inline until the deep brain \
                 promotes a distillate and this process resumes it from the log",
                self.model_promotions.len()
            ))),
        }
    }
}
