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
//! Two records ride on those seams rather than beside them.
//!
//! * **Who moved the alias, and on what evidence** (MODEL-057). A promotion
//!   or a rollback is refused until the composition root has named the
//!   process's model desk through [`Platform::name_model_desk`]; the name and
//!   the evidence — the promoted card's own latest evaluation, read off the
//!   registry and never taken from the caller — are written on the
//!   [`ModelPromotion`] record and on the registry card in the same act as
//!   the move. A promoted card used to say when it was deployed and nothing
//!   about who decided.
//! * **Which version of the promoted set** (MODEL-037). Every record that
//!   changes the set makes a [`ModelPack`] naming the pack it supersedes and
//!   what it adds, keeps and removes. Derived from the promotion records and
//!   not journalled a second time; [`pack_membership`] walks the predecessor
//!   pointers and rebuilds any pack's members from the deltas alone.
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
use qip_contracts::policy::{InlineModel, ModelManifest, Slot};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::log::EventLog;
use qip_events::{EventBody, Topic};
use qip_observability::metrics::{labels, names};
use qip_quantum::foundry::{FamilyKind, FamilyStatus, Foundry};
use qip_strategy::model::DistilledModel;
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The producer on every promotion record, so a replay picks them out of
/// the `model.evaluated` topic they share with the deep brain's evaluation
/// records the way the venue review's records are picked out by origin.
pub const MODEL_PROMOTION_ORIGIN: &str = "kernel/model-promotion";

/// The lineage producer of every [`ModelRegistered`] record, which shares the
/// promotion records' topic and is told apart from them by this.
pub const MODEL_REGISTRATION_ORIGIN: &str = "kernel/model-registration";

/// A registered model card as it stood when the registry handed it to the
/// log (R12-MODEL-9003): its version, every evaluation, its lineage, its
/// stage and alias history and its limitations, whether or not it was ever
/// promoted.
///
/// The registry is in memory in the deep brain, and the promotion records
/// only carry promoted versions, so a card registered and evaluated and then
/// not promoted was lost at every restart — and with it the evidence that a
/// fit failed, which is the record that keeps a search from retrying until
/// something passes unseen. A record is written when a card is new or has
/// changed since the last one; the newest record per reference is the card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRegistered {
    pub card: qip_ai::registry::ModelCard,
}

impl EventBody for ModelRegistered {
    /// The Learn group, which the log never evicts, like the promotions.
    const TOPIC: Topic = Topic::ModelEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// Whether two cards differ only in a drift reading that stays on the same
/// side of the card's threshold. A reading that crosses it changes whether
/// the model may inform a decision, so it is a record; one that does not is
/// monitoring the next round re-measures.
fn same_but_drift(a: &qip_ai::registry::ModelCard, b: &qip_ai::registry::ModelCard) -> bool {
    let drifted = |card: &qip_ai::registry::ModelCard| card.drift_score > card.drift_threshold;
    if drifted(a) != drifted(b) {
        return false;
    }
    let mut b = b.clone();
    b.drift_score = a.drift_score;
    *a == b
}

/// Every card the log's registration records hold, the newest per
/// reference, for a restarted process to rebuild from.
pub(crate) fn resume_model_registrations(
    log: &EventLog,
) -> Result<BTreeMap<String, qip_ai::registry::ModelCard>> {
    let mut cards = BTreeMap::new();
    for event in log.events() {
        if event.topic != ModelRegistered::TOPIC
            || event.lineage.producer != MODEL_REGISTRATION_ORIGIN
        {
            continue;
        }
        let card = StreamEnvelope::from_frame(event)
            .and_then(|envelope| envelope.decode::<ModelRegistered>())?
            .body
            .card;
        cards.insert(card.reference(), card);
    }
    Ok(cards)
}

/// The `outcome` label values of `qip_model_serves_total`: the provider served
/// the artifact, or refused it. Constants, so the label stays a fixed pair
/// however many sites record it.
pub const SERVE_SERVED: &str = "served";
pub const SERVE_REFUSED: &str = "refused";

/// Who produced a promotion candidate (blueprint section 10, MODEL-067).
///
/// Deploy candidates come from the training pipeline and nowhere else. The
/// quantum gateway's output is a control signal (a selection or search
/// result) that may inform a classical candidate, and that candidate arrives
/// here as [`Self::QuantumInformed`] carrying the reference of the classical
/// baseline it was compared against, and the workload family its quantum
/// input answered (R12-QUANT-4018). A quantum result presented as the
/// candidate itself is [`Self::QuantumGateway`], and is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProducer {
    TrainingPipeline,
    /// A model the platform fitted and proposed for itself: the deep
    /// brain's learning desk. Promoted only out of a shadow period of at
    /// least [`MIN_SHADOW_ROUNDS`] won rounds (R12-EXPAND-6070), because a
    /// self-proposed change must run beside production, acted on by
    /// nothing, before it may replace what production runs.
    SelfFitted,
    QuantumGateway,
    QuantumInformed {
        baseline: String,
        /// The `qip_quantum::foundry::FamilyKind` (by its variant name) the
        /// quantum input was produced for. Absent from records written
        /// before the field existed; a candidate naming none is refused,
        /// because a quantum benefit is claimed only on a workload family
        /// the foundry has built.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workload: Option<String>,
    },
}

impl ModelProducer {
    fn default_pipeline() -> Self {
        Self::TrainingPipeline
    }

    fn is_training_pipeline(&self) -> bool {
        matches!(self, Self::TrainingPipeline)
    }
}

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
    /// Who produced the candidate. Absent from records written before the
    /// field existed, all of which came from the training pipeline.
    #[serde(
        default = "ModelProducer::default_pipeline",
        skip_serializing_if = "ModelProducer::is_training_pipeline"
    )]
    pub producer: ModelProducer,
    /// The model desk that moved the production alias: the name the
    /// composition root gave through [`Platform::name_model_desk`]
    /// (MODEL-057). Absent only from records written before the field
    /// existed; no record is written without one since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_by: Option<String>,
    /// What the move rested on, as the kernel read it off the registry at
    /// that instant — the promoted card's own latest evaluation, and for a
    /// rollback the degraded model's drift reading beside it. Derived, never
    /// taken from the caller, so a desk cannot assert evidence it lacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Where the promoted card's training-data licence lets it run
    /// (`ModelCard::permitted_locations`), read off the registry at the
    /// promotion and never taken from the caller; `None` when the card
    /// carries no data entitlement and so may run anywhere. A distillate of a
    /// restricted card is named only in a manifest issued for a permitted
    /// location (R12-TICK-2091). Absent from records written before the field
    /// existed, which replay as unrestricted because the log does not say
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permitted_locations: Option<BTreeSet<String>>,
    /// Every stage the promotion passed, in order, as the kernel read each
    /// off the registry and the verifier at that instant (R12-EXPAND-6070).
    /// Absent from records written before the field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
    /// The central plane's signature over [`Self::signed_payload`]: the
    /// reference, the artifact digest and the stages. A stage list edited
    /// after the fact does not verify (`Platform::promotion_verifies`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub at: Timestamp,
}

impl ModelPromotion {
    /// What [`Self::signature`] covers.
    pub fn signed_payload(&self) -> String {
        format!(
            "model-promotion|{}|{}|{}",
            self.reference,
            self.artifact_digest,
            self.stages.join("\u{1f}")
        )
    }
}

/// Shadow rounds a self-fitted model must win before it may be promoted: the
/// registry's own figure, which `ModelRegistry::promote` enforces for every
/// self-fitted card whoever calls it.
pub use qip_ai::registry::MIN_SHADOW_ROUNDS;

/// The shadow stage a self-fitted candidate must have completed, as one
/// stage line, or the refusal (R12-EXPAND-6070).
fn require_completed_shadow(registry: &ModelRegistry, reference: &str) -> Result<String> {
    let card = registry
        .get(reference)
        .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
    match (&card.stage, &card.shadow) {
        (qip_ai::registry::ModelStage::Shadow, Some(shadow))
            if shadow.rounds_won >= MIN_SHADOW_ROUNDS =>
        {
            Ok(format!(
                "shadow since {}: {} round(s) won against production on data after the fit ({})",
                shadow.since.as_secs(),
                shadow.rounds_won,
                shadow.evidence.join("; ")
            ))
        }
        (qip_ai::registry::ModelStage::Shadow, Some(shadow)) => Err(Error::denied(format!(
            "{reference} is not promoted: a self-fitted model completes its shadow stage first, \
             and it has won {} of the {MIN_SHADOW_ROUNDS} shadow round(s) required",
            shadow.rounds_won
        ))),
        (stage, _) => Err(Error::denied(format!(
            "{reference} is not promoted: a self-fitted model that skipped its shadow stage is \
             refused production (it is in {stage:?}); run it beside production through \
             ModelRegistry::enter_shadow first"
        ))),
    }
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
#[derive(Clone, Debug, PartialEq)]
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
    /// The Model Pack this manifest was produced from (MODEL-037), so the
    /// shipping line names which version of the promoted set a cell was
    /// told about and which one it superseded.
    pack: Option<ModelPack>,
    /// Promoted distillates left out because no verified release pack names
    /// their artifact at the inline model the cells compute (ADR 0110),
    /// counted so the line says how many and why.
    /// Always zero while no release packs are admitted.
    unreleased: usize,
    /// Promoted distillates left out because their card's data licence does
    /// not permit the location the manifest was issued for (R12-TICK-2091).
    unlicensed: usize,
    /// Promoted distillates left out because the release's rollout record
    /// does not target the cell, or not now (ADR 0149 decision 4).
    untargeted: usize,
}

impl ModelManifestIssue {
    pub fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    pub fn slot(&self) -> Slot<ModelManifest> {
        Slot::produced(self.manifest.clone(), self.produced_at)
    }

    /// The Model Pack the manifest was produced from.
    pub fn pack(&self) -> Option<&ModelPack> {
        self.pack.as_ref()
    }

    /// Promoted distillates withheld for want of a verified release pack.
    pub const fn unreleased(&self) -> usize {
        self.unreleased
    }

    /// Promoted distillates withheld because their licence does not permit
    /// the manifest's location.
    pub const fn unlicensed(&self) -> usize {
        self.unlicensed
    }

    pub fn describe(&self) -> String {
        let line = format!(
            "trained models: shipped {} distillate(s) by digest, newest promoted {}; {} \
             promoted model(s) named to no cell because the fidelity policy admitted no \
             distillate of them; {}",
            self.manifest.models.len(),
            self.produced_at.to_rfc3339(),
            self.advisory_only,
            self.pack.as_ref().map_or_else(
                || "no model pack on record".to_string(),
                ModelPack::describe
            )
        );
        let line = if self.unreleased == 0 {
            line
        } else {
            format!(
                "{line}; {} promoted distillate(s) withheld because no verified release pack \
                 names their artifact (ADR 0110)",
                self.unreleased
            )
        };
        let line = if self.unlicensed == 0 {
            line
        } else {
            format!(
                "{line}; {} promoted distillate(s) withheld because their training-data licence \
                 does not permit this location",
                self.unlicensed
            )
        };
        if self.untargeted == 0 {
            line
        } else {
            format!(
                "{line}; {} promoted distillate(s) withheld because the release's rollout record \
                 does not target this cell now",
                self.untargeted
            )
        }
    }

    /// Distillates withheld because their announcement does not target the
    /// cell the manifest was issued for.
    pub fn untargeted(&self) -> usize {
        self.untargeted
    }
}

/// One version of the promoted set — a Model Pack — and its lineage
/// (blueprint section 10, MODEL-037).
///
/// A projection of the log's [`ModelPromotion`] records and nothing else:
/// every record that changes the promoted set yields one pack, naming the
/// pack it supersedes and the delta that record made. Not journalled as a
/// record of its own, because the promotion records already hold the fact
/// and a second journal of it could disagree with the first.
///
/// Membership is deliberately not a field. A pack that carried both its
/// members and its delta would hold two claims about one fact; instead the
/// id is a digest over the predecessor's id and the full membership, so
/// [`pack_membership`] can rebuild the members from the deltas alone and
/// prove the rebuild against the id.
///
/// This is the pack's *version and lineage* and nothing more. What a pack
/// must carry to be applied — signed artifacts, features, calibration,
/// allowed universes, a resource budget, an expiry (CONTRACT-011) — is a
/// different contract, unbuilt here, and this type does not stand in for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPack {
    /// SHA-256 over the predecessor's id and every member as
    /// `reference=artifact digest`, in reference order.
    pub id: String,
    /// The pack this one supersedes; `None` for the first pack only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor: Option<String>,
    /// Model versions this pack adds, by reference, at the artifact digest
    /// each was promoted under.
    pub added: BTreeMap<String, String>,
    /// Model versions carried over from the predecessor unchanged.
    pub kept: BTreeSet<String>,
    /// Model versions the predecessor held and this pack does not.
    pub removed: BTreeSet<String>,
    /// The instant of the promotion record that made this pack.
    pub at: Timestamp,
}

impl ModelPack {
    /// The id a pack with this predecessor and this membership carries.
    fn id_of(predecessor: Option<&str>, members: &BTreeMap<String, String>) -> String {
        let mut canonical = format!("model-pack\n{}\n", predecessor.unwrap_or("-"));
        for (reference, digest) in members {
            canonical.push_str(&format!("{reference}={digest}\n"));
        }
        qip_core::sha256_hex(canonical.as_bytes())
    }

    /// The pack `record` makes of `members`, which this also updates — or
    /// `None`, leaving `members` as they were, for a record that changes
    /// nothing (the same version re-promoted at the same bytes), because a
    /// version of the set that differs from its predecessor in nothing is
    /// not a version.
    fn succeed(
        predecessor: Option<&ModelPack>,
        members: &mut BTreeMap<String, String>,
        record: &ModelPromotion,
    ) -> Option<Self> {
        let mut removed = BTreeSet::new();
        for displaced in &record.displaced {
            if displaced != &record.reference && members.remove(displaced).is_some() {
                removed.insert(displaced.clone());
            }
        }
        let mut added = BTreeMap::new();
        if members.get(&record.reference) != Some(&record.artifact_digest) {
            members.insert(record.reference.clone(), record.artifact_digest.clone());
            added.insert(record.reference.clone(), record.artifact_digest.clone());
        }
        if added.is_empty() && removed.is_empty() {
            return None;
        }
        let kept = members
            .keys()
            .filter(|reference| !added.contains_key(*reference))
            .cloned()
            .collect();
        let predecessor = predecessor.map(|pack| pack.id.clone());
        Some(Self {
            id: Self::id_of(predecessor.as_deref(), members),
            predecessor,
            added,
            kept,
            removed,
            at: record.at,
        })
    }

    /// One clause naming the pack, what it superseded and the delta.
    pub fn describe(&self) -> String {
        format!(
            "model pack {} supersedes {} (+{} added, {} kept, -{} removed)",
            short(&self.id),
            self.predecessor.as_deref().map_or("nothing", short),
            self.added.len(),
            self.kept.len(),
            self.removed.len()
        )
    }
}

/// The first twelve characters of a digest, for a line a person reads.
/// A quantum-informed candidate is evaluated only on a workload family the
/// platform's foundry has built (blueprint §10, QUANT-004): a family recorded
/// as not built has no encoding, baseline or benchmark, so no quantum
/// advantage on it could have been measured. Refuses a missing family, a name
/// that is no family, and a family the foundry holds only as not built.
fn require_eligible_workload(reference: &str, workload: Option<&str>) -> Result<()> {
    let Some(name) = workload else {
        return Err(Error::denied(format!(
            "{reference} is not promoted: a quantum-informed candidate must name the workload              family its quantum input answered, and this one names none"
        )));
    };
    let Some(kind) = FamilyKind::ALL
        .into_iter()
        .find(|kind| format!("{kind:?}") == name)
    else {
        return Err(Error::denied(format!(
            "{reference} is not promoted: `{name}` is not a workload family the blueprint names"
        )));
    };
    match Foundry::platform()?.status(kind) {
        Some(FamilyStatus::Registered(_)) => Ok(()),
        Some(FamilyStatus::NotBuilt { reason }) => Err(Error::denied(format!(
            "{reference} is not promoted: the {name} family is not built ({reason}); a quantum              benefit is evaluated only on a family with an encoding, baseline and benchmark"
        ))),
        None => Err(Error::denied(format!(
            "{reference} is not promoted: the foundry does not account for the {name} family"
        ))),
    }
}

fn short(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}

/// Every pack the promotion records make, oldest first.
fn packs_of(records: &[ModelPromotion]) -> Vec<ModelPack> {
    let mut members = BTreeMap::new();
    let mut packs: Vec<ModelPack> = Vec::new();
    for record in records {
        if let Some(pack) = ModelPack::succeed(packs.last(), &mut members, record) {
            packs.push(pack);
        }
    }
    packs
}

/// Walk `lineage` from the pack named `id` back to the first through the
/// predecessor pointers, then rebuild that pack's membership — reference to
/// artifact digest — from the deltas alone.
///
/// Refuses rather than returning a membership nobody can vouch for: an id
/// the lineage does not hold, a predecessor pointer that leads nowhere, a
/// delta that removes or keeps a version its predecessor never held, and a
/// rebuilt membership whose digest is not the pack's id. Each is a lineage
/// edited after the fact or assembled from two logs, and a membership read
/// off it would name models nobody promoted.
pub fn pack_membership(lineage: &[ModelPack], id: &str) -> Result<BTreeMap<String, String>> {
    let by_id: BTreeMap<&str, &ModelPack> = lineage
        .iter()
        .map(|pack| (pack.id.as_str(), pack))
        .collect();
    let mut chain = Vec::new();
    let mut cursor = Some(id);
    while let Some(wanted) = cursor {
        let pack = by_id.get(wanted).ok_or_else(|| {
            Error::not_found(format!(
                "the lineage holds no model pack {wanted}; walk a lineage read from the log \
                 the pack was promoted in"
            ))
        })?;
        if chain.len() >= lineage.len() {
            return Err(Error::invalid(format!(
                "the lineage of model pack {id} loops through {wanted}; a pack cannot \
                 supersede itself — rebuild the lineage from the log"
            )));
        }
        chain.push(*pack);
        cursor = pack.predecessor.as_deref();
    }
    let mut members: BTreeMap<String, String> = BTreeMap::new();
    for pack in chain.iter().rev() {
        for reference in &pack.removed {
            if members.remove(reference).is_none() {
                return Err(Error::invalid(format!(
                    "model pack {} removes {reference}, which its predecessor did not hold; \
                     rebuild the lineage from the log",
                    pack.id
                )));
            }
        }
        let carried: BTreeSet<String> = members
            .keys()
            .filter(|reference| !pack.added.contains_key(*reference))
            .cloned()
            .collect();
        if carried != pack.kept {
            return Err(Error::invalid(format!(
                "model pack {} says it keeps {:?} and its predecessor leaves it {:?}; \
                 rebuild the lineage from the log",
                pack.id, pack.kept, carried
            )));
        }
        members.extend(pack.added.clone());
        if ModelPack::id_of(pack.predecessor.as_deref(), &members) != pack.id {
            return Err(Error::invalid(format!(
                "model pack {} does not digest to its own id once its lineage is walked; \
                 a delta or a predecessor pointer was changed after the pack was made — \
                 rebuild the lineage from the log",
                pack.id
            )));
        }
    }
    Ok(members)
}

/// The evidence a card's alias moves on: its latest evaluation, in words.
///
/// Read off the registry rather than supplied by the desk, for the reason
/// `register_fit` takes no `passed` argument: evidence a caller may state is
/// evidence a caller may misstate.
fn evaluation_evidence(registry: &ModelRegistry, reference: &str) -> Result<String> {
    let card = registry
        .get(reference)
        .ok_or_else(|| Error::not_found(format!("no model registered as {reference}")))?;
    let evaluation = card.latest_evaluation().ok_or_else(|| {
        Error::denied(format!(
            "{reference} holds no evaluation, so there is no evidence its alias could move \
             on; evaluate it first"
        ))
    })?;
    let metrics: Vec<String> = evaluation
        .metrics
        .iter()
        .map(|(name, value)| format!("{name}={value:.6}"))
        .collect();
    Ok(format!(
        "{reference} evaluated on {} at {}: {} ({})",
        evaluation.dataset,
        evaluation.evaluated_at.to_rfc3339(),
        if evaluation.passed {
            "passed"
        } else {
            "did not pass"
        },
        metrics.join(", ")
    ))
}

/// Rebuild the promoted set from the log alone, in log order, applying
/// each record exactly as [`Platform::promote_model`] applied it.
pub(crate) fn resume_model_promotions(log: &EventLog) -> Result<BTreeMap<String, ModelPromotion>> {
    let mut promoted = BTreeMap::new();
    for record in promotions_in(log)? {
        apply_promotion(&mut promoted, record);
    }
    Ok(promoted)
}

/// The newest Model Pack the log's promotion records make, for a restarted
/// process to hold beside the promoted set it rebuilt from the same records.
pub(crate) fn resume_model_pack(log: &EventLog) -> Result<Option<ModelPack>> {
    Ok(packs_of(&promotions_in(log)?).pop())
}

/// Every promotion the log holds, in log order, including those since
/// displaced.
fn promotions_in(log: &EventLog) -> Result<Vec<ModelPromotion>> {
    let mut records = Vec::new();
    for event in log.events() {
        if event.topic != ModelPromotion::TOPIC || event.lineage.producer != MODEL_PROMOTION_ORIGIN
        {
            continue;
        }
        records.push(
            StreamEnvelope::from_frame(event)
                .and_then(|envelope| envelope.decode::<ModelPromotion>())?
                .body,
        );
    }
    Ok(records)
}

/// One apply for the live path and the replay, so they cannot diverge.
fn apply_promotion(promoted: &mut BTreeMap<String, ModelPromotion>, record: ModelPromotion) {
    for displaced in &record.displaced {
        promoted.remove(displaced);
    }
    promoted.insert(record.reference.clone(), record);
}

impl Platform {
    /// Journal every card in `registry` that is new or has changed since this
    /// process last journaled it — a registration, an evaluation, a stage or
    /// alias move, a limitation — and return how many were written
    /// (R12-MODEL-9003). The deep brain calls it after each learning round,
    /// so the registry it rebuilds empty at start can be refilled from the
    /// log by [`Self::registered_model_cards`]. A card is written before the
    /// in-memory mirror moves, so a refused write is retried next time
    /// rather than forgotten.
    pub fn journal_model_registry(
        &mut self,
        registry: &ModelRegistry,
        now: Timestamp,
    ) -> Result<usize> {
        let mut written = 0;
        for card in registry.iter() {
            let reference = card.reference();
            // A drift reading alone is not a new record. The desk re-measures
            // every earlier fit on a subject each round, so journaling each
            // reading would grow the never-evicted Learn group with the
            // square of the rounds; drift is a monitoring figure the next
            // round re-measures, not part of what the registry must keep.
            if self
                .journaled_cards
                .get(&reference)
                .is_some_and(|journaled| same_but_drift(journaled, card))
            {
                continue;
            }
            self.journal_record(
                ModelRegistered { card: card.clone() },
                MODEL_REGISTRATION_ORIGIN,
                now,
            )?;
            self.journaled_cards.insert(reference, card.clone());
            written += 1;
        }
        Ok(written)
    }

    /// Every promotion the log holds, in log order, displaced ones included:
    /// what a restarted desk fetches incumbent and predecessor artifacts by
    /// (R12-MODEL-9003).
    pub fn model_promotion_history(&self) -> Result<Vec<ModelPromotion>> {
        promotions_in(self.event_log())
    }

    /// Journal the cards a committed promotion or rollback moved. The record
    /// is the commit, so a failure here must not report the move as refused:
    /// the in-memory mirror is advanced only on a successful write, so the
    /// caller's next `journal_model_registry` writes them again, and a
    /// restart in between rebuilds their stages and lifecycle from the
    /// promotion records (R12-MODEL-9003). The failure is counted where the
    /// alerts look rather than dropped.
    fn journal_moved_cards(&mut self, registry: &ModelRegistry, now: Timestamp) {
        if self.journal_model_registry(registry, now).is_err() {
            self.telemetry.metrics.count(
                names::MODEL_PACKAGES,
                labels([("outcome", "cards_unjournaled")]),
            );
        }
    }

    /// Every registered card the log holds, the newest record per reference,
    /// in reference order: what a restarted deep brain registers before its
    /// first round (R12-MODEL-9003).
    pub fn registered_model_cards(&self) -> impl Iterator<Item = &qip_ai::registry::ModelCard> {
        self.journaled_cards.values()
    }

    /// The provider the composition root handed in.
    pub fn model_provider(&self) -> &dyn ModelProvider {
        self.model_provider.as_ref()
    }

    /// Serve an artifact through the platform's provider, off the hot path.
    ///
    /// The one call every consumer of a served score goes through, so a
    /// process assembled without a provider refuses here with the null
    /// provider's own sentence rather than anywhere downstream.
    ///
    /// Counted on `qip_model_serves_total` by outcome, at the call and not at
    /// the consumer: a provider that refuses every artifact would otherwise
    /// leave the model-serving objective unmeasured, which reads as no
    /// problem. Promotion's own serve goes through the same count.
    pub fn serve_model(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        let served = self.model_provider.serve(artifact);
        self.telemetry.metrics.count(
            names::MODEL_SERVES,
            labels([(
                "outcome",
                if served.is_ok() {
                    SERVE_SERVED
                } else {
                    SERVE_REFUSED
                },
            )]),
        );
        served
    }

    /// Every promotion this process holds, by reference, in reference order.
    /// Whether a promotion record carries the central plane's signature
    /// over its reference, digest and stages (R12-EXPAND-6070). False for a
    /// record with no signature and for one edited after it was signed.
    pub fn promotion_verifies(&self, record: &ModelPromotion) -> bool {
        record.signature.as_deref().is_some_and(|signature| {
            self.central
                .verifies_record(&record.signed_payload(), signature)
        })
    }

    pub fn model_promotions(&self) -> &BTreeMap<String, ModelPromotion> {
        &self.model_promotions
    }

    /// Name the desk that moves the production alias in this process
    /// (MODEL-057).
    ///
    /// One process has one model desk, and every promotion and rollback it
    /// journals carries that name, so "who moved the alias" is answered by
    /// the record itself. Naming the same desk again changes nothing, so the
    /// desk may name itself each time it acts. Refuses a blank name — an
    /// unnamed desk is the anonymous move this exists to rule out — and a
    /// second, different name, because a process whose moves are attributed
    /// to whoever spoke last attributes nothing.
    pub fn name_model_desk(&mut self, desk: &str) -> Result<()> {
        if desk.trim().is_empty() {
            return Err(Error::invalid(
                "a model desk has a name; a blank one would journal every promotion as moved \
                 by nobody — pass the composition root's own desk name",
            ));
        }
        match &self.model_desk {
            Some(named) if named != desk => Err(Error::denied(format!(
                "this process's model desk is already named {named}; a second name ({desk}) \
                 would attribute one desk's promotions to another — one process, one desk"
            ))),
            Some(_) => Ok(()),
            None => {
                self.model_desk = Some(desk.to_string());
                Ok(())
            }
        }
    }

    /// The desk promotions and rollbacks in this process are attributed to.
    pub fn model_desk(&self) -> Option<&str> {
        self.model_desk.as_deref()
    }

    /// The named desk, or the refusal an unattributed move earns.
    fn require_model_desk(&self, reference: &str, act: &str) -> Result<String> {
        self.model_desk.clone().ok_or_else(|| {
            Error::denied(format!(
                "{reference} is not {act}: no model desk is named in this process, so the \
                 move of its production alias could be attributed to nobody; call \
                 Platform::name_model_desk with the desk's name first"
            ))
        })
    }

    /// The Model Pack in force: the newest version of the promoted set, with
    /// the pack it superseded and the delta that made it (MODEL-037).
    pub fn model_pack(&self) -> Option<&ModelPack> {
        self.model_pack.as_ref()
    }

    /// Every Model Pack this platform's log records, oldest first — read
    /// from the log rather than held, because the lineage grows with every
    /// promotion and the log is already its record.
    pub fn model_pack_lineage(&self) -> Result<Vec<ModelPack>> {
        Ok(packs_of(&promotions_in(self.event_log())?))
    }

    /// Advance the pack in force by one record, from the promoted set as it
    /// stands *before* that record is applied. The live path's half of what
    /// [`resume_model_pack`] does on a restart, through the same
    /// [`ModelPack::succeed`], so the two cannot diverge.
    fn succeed_model_pack(&mut self, record: &ModelPromotion) {
        let mut members = self
            .model_promotions
            .iter()
            .map(|(reference, held)| (reference.clone(), held.artifact_digest.clone()))
            .collect();
        if let Some(pack) = ModelPack::succeed(self.model_pack.as_ref(), &mut members, record) {
            self.model_pack = Some(pack);
        }
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
        self.promote_model_from(
            ModelProducer::TrainingPipeline,
            registry,
            artifact,
            distilled,
            displaced,
            now,
        )
    }

    /// [`Self::promote_model`] for a candidate whose producer is named.
    ///
    /// Refuses a candidate produced by the quantum gateway, and a
    /// quantum-informed one whose classical baseline is not on record with a
    /// fitted calibration (ADR 0006): a quantum-informed candidate is
    /// admitted only with its baseline comparison attached.
    pub fn promote_model_from(
        &mut self,
        producer: ModelProducer,
        registry: &mut ModelRegistry,
        artifact: &ModelArtifact,
        distilled: Option<&DistilledModel>,
        displaced: &[String],
        now: Timestamp,
    ) -> Result<PublishedArtifact> {
        let mut stages = Vec::new();
        match &producer {
            ModelProducer::TrainingPipeline => {}
            ModelProducer::SelfFitted => {
                stages.push(require_completed_shadow(registry, &artifact.reference)?);
            }
            ModelProducer::QuantumGateway => {
                return Err(Error::denied(format!(
                    "{} is not promoted: its producer is the quantum gateway, whose output is a \
                     control signal and never a deploy candidate; train a classical candidate \
                     informed by it and promote that",
                    artifact.reference
                )));
            }
            ModelProducer::QuantumInformed { baseline, workload } => {
                require_eligible_workload(&artifact.reference, workload.as_deref())?;
                crate::central::models::require_calibrated_baseline(registry, baseline).map_err(
                    |error| {
                        Error::denied(format!(
                            "{} is not promoted: a quantum-informed candidate needs its \
                             classical baseline comparison attached: {}",
                            artifact.reference,
                            error.message()
                        ))
                    },
                )?;
            }
        }
        // A version names one artifact for good (MODEL-034). The log is the
        // history, so the first digest a reference was ever promoted under is
        // read from it: a registry that forgot the card, a restarted desk
        // whose fit counter began again at the same version, and a retired
        // reference are all caught here, where the registry's own per-card
        // refusal cannot see them.
        if let Some(first) = promotions_in(self.event_log())?
            .into_iter()
            .find(|record| record.reference == artifact.reference)
            && first.artifact_digest != artifact.digest
        {
            return Err(Error::denied(format!(
                "{} is not promoted: the log records it first promoted at artifact {}, and this \
                 artifact is {}; a version names one artifact, so publish the new bytes under a \
                 new version",
                artifact.reference, first.artifact_digest, artifact.digest
            )));
        }
        // A version the log promoted and has since taken out of production
        // returns only through `rollback_model`, which checks the reading it
        // rolls back on and the predecessor's eligibility. Promoted again, it
        // skipped both, stood in production with its retirement still on the
        // card (so `decision_eligibility` refused it), and a restarted desk
        // replayed the record as the rollback it was not, restoring a history
        // that differed from the live one (R12-MODEL-9003, eighth review). A
        // version still in production may be published again under the same
        // bytes, as a restarted desk does.
        if !self.model_promotions().contains_key(&artifact.reference)
            && let Some(first) = promotions_in(self.event_log())?
                .into_iter()
                .find(|record| record.reference == artifact.reference)
        {
            return Err(Error::denied(format!(
                "{} is not promoted: the log promoted it at {} and has since taken it out of \
                 production; a version returns to production only through a rollback",
                artifact.reference, first.at
            )));
        }
        let served = self.serve_model(artifact).map_err(|error| {
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
        let desk = self.require_model_desk(&artifact.reference, "promoted")?;
        let mut scratch = registry.clone();
        let published = scratch.promote_artifact(artifact, now)?;
        // Blueprint §9.11 (R12-MODEL-7201): the verifier society attacks the
        // candidate before the alias moves. Placed after the registry's own
        // refusals so a card the registry refuses is refused for the
        // registry's reason; journaled whichever way it goes, so a promotion
        // the society let through is as visible as one it stopped. A
        // refutation refuses the promotion — authority not added, never a
        // path created. See `crate::model_tournament`.
        let verification = self.tournament.verify_model(
            card,
            self.quarantined_datasets(),
            self.latest_regime_crossing(),
            now,
        );
        let refuted = verification
            .verdict
            .is_refuted()
            .then(|| verification.verdict.summary());
        let verified = verification.verdict.summary();
        self.journal_record(
            crate::model_tournament::ModelVerified { verification },
            crate::model_tournament::VERIFIER_ORIGIN,
            now,
        )?;
        if let Some(summary) = refuted {
            return Err(Error::denied(format!(
                "{} is not promoted: {summary}",
                artifact.reference
            )));
        }
        for reference in displaced {
            scratch.retire(reference, now)?;
        }
        // MODEL-057: who moved the alias and on what evidence, on the same
        // scratch as the move, so the registry never holds a production
        // alias nobody can be asked about. The evidence is the promoted
        // card's own latest evaluation, which `promote_artifact` has just
        // required to have passed.
        let evidence = evaluation_evidence(&scratch, &artifact.reference)?;
        scratch.record_alias_move(&artifact.reference, &desk, &evidence, now)?;
        for reference in displaced {
            scratch.record_alias_move(
                reference,
                &desk,
                &format!("displaced by {}: {evidence}", artifact.reference),
                now,
            )?;
        }
        // EXPAND-038: the promotion records what it displaced as the rollback
        // parent, on the same scratch, so the record that says a model went
        // live and the record of where to go back to are one write or none.
        // Done after the retirements and from the pre-retirement deploy
        // times, which retire does not touch.
        scratch.record_rollback_parent(&artifact.reference, displaced)?;
        // R12-TICK-2091: the licence travels with the record, read off the
        // card here, because the manifest is issued long after the registry
        // the desk handed in has gone and a cell must never be named a model
        // its training data may not run beside.
        let card = scratch.get(&artifact.reference).ok_or_else(|| {
            Error::not_found(format!(
                "{} has no card in the registry it was promoted in; its licence cannot be read",
                artifact.reference
            ))
        })?;
        let permitted_locations =
            (!card.data_entitlements.is_empty()).then(|| card.permitted_locations.clone());
        // The stages in the order they ran: the shadow period (for a
        // self-fitted candidate, checked above), the registry's evaluation,
        // the verifier society's verdict.
        stages.push(format!("evaluation: {evidence}"));
        stages.push(format!("verifier: {verified}"));
        let mut record = ModelPromotion {
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
            producer,
            moved_by: Some(desk),
            evidence: Some(evidence),
            permitted_locations,
            stages,
            signature: None,
            at: now,
        };
        // Signed first, so every copy of the record on the log carries the
        // centre's signature (lane L4). Then every card as it stands *before*
        // the move, journaled before the record (R12-MODEL-9003), so the
        // candidate's card and its evaluation are on the log whatever happens
        // next, and nothing on the log claims a promotion the record does not
        // hold. Then the record, which is the commit; then the moved cards,
        // whose write may not unmake it.
        record.signature = Some(self.central.sign_record(&record.signed_payload()));
        self.journal_model_registry(registry, now)?;
        self.journal_record(record.clone(), MODEL_PROMOTION_ORIGIN, now)?;
        *registry = scratch;
        self.succeed_model_pack(&record);
        apply_promotion(&mut self.model_promotions, record);
        self.journal_moved_cards(registry, now);
        Ok(published)
    }

    /// Retire a degraded promoted model and reactivate the one it displaced:
    /// automatic retirement with rollback to the last known-good (MODEL-045).
    ///
    /// The predecessor is read from the log, not from the caller: the records
    /// this promotion displaced, newest first, each taken back at the digest
    /// it was first promoted under (which is the digest that passed the gate,
    /// and which [`Self::promote_model_from`] guarantees is the only one that
    /// reference ever carried). A predecessor the registry will not
    /// reactivate (drifted itself, never carried an artifact) is skipped for
    /// the one before it. Same discipline as a promotion: a scratch registry,
    /// the record journalled first, then adopted, so a refused rollback leaves
    /// the log and the registry as they were. Returns the reactivated
    /// reference.
    pub fn rollback_model(
        &mut self,
        registry: &mut ModelRegistry,
        degraded: &str,
        now: Timestamp,
    ) -> Result<String> {
        let active = self
            .model_promotions
            .get(degraded)
            .cloned()
            .ok_or_else(|| {
                Error::not_found(format!(
                    "{degraded} is not in the promoted set, so there is nothing to roll back from"
                ))
            })?;
        let desk = self.require_model_desk(degraded, "rolled back")?;
        let reading = registry.get(degraded).map_or_else(
            || format!("{degraded} holds no card to read a drift from"),
            |card| {
                format!(
                    "{degraded} read drift {:.3} against its threshold {:.3}",
                    card.drift_score, card.drift_threshold
                )
            },
        );
        let history = promotions_in(self.event_log())?;
        let mut refusals = Vec::new();
        for predecessor in active.displaced.iter().rev() {
            let Some(original) = history
                .iter()
                .find(|record| &record.reference == predecessor)
            else {
                refusals.push(format!("{predecessor} has no promotion record in the log"));
                continue;
            };
            let mut scratch = registry.clone();
            if let Err(error) = scratch.reactivate(predecessor, now) {
                refusals.push(error.message().to_string());
                continue;
            }
            // The card must carry the digest the log first recorded: what
            // comes back is the artifact that passed, not a card edited since.
            if scratch
                .get(predecessor)
                .and_then(|card| card.artifact_digest.as_deref())
                != Some(original.artifact_digest.as_str())
            {
                refusals.push(format!(
                    "{predecessor}'s card does not carry the digest the log first recorded for it"
                ));
                continue;
            }
            scratch.retire(degraded, now)?;
            // MODEL-057: a rollback moves the alias twice — off the degraded
            // model and back on to its predecessor — and both moves name the
            // desk and the reading it acted on.
            let evidence = format!("{reading}; {}", evaluation_evidence(&scratch, predecessor)?);
            scratch.record_alias_move(predecessor, &desk, &evidence, now)?;
            scratch.record_alias_move(degraded, &desk, &evidence, now)?;
            let record = ModelPromotion {
                displaced: vec![degraded.to_string()],
                moved_by: Some(desk),
                evidence: Some(evidence),
                at: now,
                ..original.clone()
            };
            // As in a promotion: the unmoved cards, the record, then the
            // moved cards, whose write may not unmake the rollback.
            self.journal_model_registry(registry, now)?;
            self.journal_record(record.clone(), MODEL_PROMOTION_ORIGIN, now)?;
            *registry = scratch;
            self.succeed_model_pack(&record);
            apply_promotion(&mut self.model_promotions, record);
            self.journal_moved_cards(registry, now);
            return Ok(predecessor.clone());
        }
        Err(Error::not_found(format!(
            "{degraded} has no known-good predecessor to roll back to: {}",
            if refusals.is_empty() {
                "it displaced nothing".to_string()
            } else {
                refusals.join("; ")
            }
        )))
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
        self.model_manifest_for(None)
    }

    /// The `trained_models` slot as issued for `location`: a distillate whose
    /// card's training-data licence names permitted locations is named only
    /// where `location` is one of them, with the same rule as
    /// `ModelCard::authorise_location` (R12-TICK-2091). `None` — a cell whose
    /// location the centre has not been told — withholds every restricted
    /// distillate: a licence that cannot be checked is not a licence granted.
    pub fn model_manifest_for(&self, location: Option<&str>) -> Result<ModelManifestIssue> {
        self.model_manifest_targeted(location, None)
    }

    /// [`Self::model_manifest_for`], for one cell: `target` is the cell's
    /// region, its node id and the instant the manifest is issued at. With
    /// control-pack announcements admitted (ADR 0149 decision 4), a
    /// distillate whose pack is announced is named only where its
    /// announcement targets that region and node inside its window, and
    /// withheld from every manifest issued with no target — a rollout record
    /// can only narrow what a cell is told. A pack nobody announced is
    /// unaffected.
    pub fn model_manifest_targeted(
        &self,
        location: Option<&str>,
        target: Option<(&str, &str, Timestamp)>,
    ) -> Result<ModelManifestIssue> {
        let mut untargeted = 0usize;
        let mut models = BTreeMap::new();
        let mut packs = BTreeMap::new();
        let mut produced_at: Option<Timestamp> = None;
        let mut advisory_only = 0usize;
        let mut unreleased = 0usize;
        let mut unlicensed = 0usize;
        for promotion in self.model_promotions.values() {
            let Some(entry) = &promotion.distilled else {
                advisory_only += 1;
                continue;
            };
            if let Some(permitted) = &promotion.permitted_locations
                && !location.is_some_and(|location| permitted.contains(location))
            {
                unlicensed += 1;
                continue;
            }
            // ADR 0110: with release packs admitted, a distillate is named to
            // a cell only when a verified pack names its artifact *and* the
            // inline model the cell will compute — name and digest — and
            // the pack rides beside it, because a bundle cell verifies that
            // pack itself rather than trusting this process's payload
            // signature (Amendment 1). A pack for the artifact that names a
            // different inline model, or none, would be refused by every
            // cell, so it is withheld and counted here instead.
            if let Some(released) = &self.release_packs {
                let inline = InlineModel {
                    name: entry.name.clone(),
                    digest: entry.digest.clone(),
                };
                match released.get(&promotion.artifact_digest) {
                    Some(pack) if pack.inline_model.as_ref() == Some(&inline) => {
                        let announced = self
                            .control_announcements
                            .as_ref()
                            .and_then(|held| held.get(&promotion.artifact_digest));
                        if let Some(announcement) = announced
                            && !target.is_some_and(|(region, node, now)| {
                                announcement.targets(region, node, now)
                            })
                        {
                            untargeted += 1;
                            continue;
                        }
                        packs.insert(entry.name.clone(), pack.clone());
                    }
                    _ => {
                        unreleased += 1;
                        continue;
                    }
                }
            }
            models.insert(entry.name.clone(), entry.digest.clone());
            produced_at = Some(produced_at.map_or(promotion.at, |at| at.max(promotion.at)));
        }
        match produced_at {
            Some(produced_at) => Ok(ModelManifestIssue {
                manifest: ModelManifest { models, packs },
                produced_at,
                advisory_only,
                pack: self.model_pack.clone(),
                unreleased,
                unlicensed,
                untargeted,
            }),
            None if untargeted > 0 => Err(Error::denied(format!(
                "{untargeted} promoted distillate(s) held and the release's rollout record \
                 targets none of them at {} inside its window; the trained_models slot ships \
                 unproduced for it",
                target.map_or_else(
                    || "a cell the manifest was not issued for".to_string(),
                    |(region, node, _)| format!("node {node} in {region}")
                )
            ))),
            None if unlicensed > 0 && unreleased == 0 => Err(Error::denied(format!(
                "{unlicensed} promoted distillate(s) held and the training-data licence of each \
                 refuses {}; the trained_models slot ships unproduced for it",
                location.map_or_else(
                    || "a cell whose location is not declared".to_string(),
                    |location| format!("location {location}")
                )
            ))),
            None if unreleased > 0 => Err(Error::not_found(format!(
                "{unreleased} promoted distillate(s) held and no verified release pack names any \
                 of their artifacts at the inline model the cells compute (ADR 0110); the trained_models slot ships unproduced and \
                 every cell refuses a plan carrying a model inline until a two-person release \
                 signs one under the model-pack key"
            ))),
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
