//! Where a connector's checked evidence reaches the world model
//! (R12-EVID-2031..2039).
//!
//! Every connector runtime builds an `EvidenceRecord` per admitted event and
//! checks it against a fabric — but until this seam that fabric was the
//! runtime's own, private to one source. `ConnectorFeed::drain_evidence` and
//! `WorldModel::absorb_evidence` both existed and neither had a caller, so:
//!
//! * the world model's fabric — the one the poisoning detector reads — was
//!   empty in every binary, and `detect_poisoning` reported nothing because
//!   it had been given nothing, which reads exactly like a clean feed;
//! * two connectors disagreeing about one print were each consistent with
//!   themselves, so no contest, corroboration or common-origin collapse ever
//!   spanned two sources in production.
//!
//! [`Platform::absorb_evidence`] closes both. A composition root drains each
//! feed after its poll and hands the batch here; each claim is re-admitted
//! through the **world** fabric, which runs the temporal, geographic,
//! authenticity and contradiction checks again against everything every
//! source has said, and links it into the one provenance graph.
//!
//! # What it refuses and what it counts
//!
//! A claim the world fabric already holds is not re-admitted — the fabric
//! refuses a duplicate id because a claim admitted twice would corroborate
//! itself — and is counted as `duplicate`. A claim the fabric refuses for
//! incomplete provenance is counted as `refused` and its reason kept on the
//! report: one bad record from one source must not stop the poll that
//! delivered a hundred good ones, and must not vanish either. Admission
//! outcomes are counted on `qip_evidence_absorbed_total{outcome}`.
//!
//! # What it does not do
//!
//! It does not journal the verdicts or the links. `EvidenceAttached` carries
//! the symbolic stage's derivations, and a second body on one topic is the
//! decode failure `references.rs` records having shipped once, so each
//! admitted claim goes to its own topic, `evidence.admitted` (ADR 0152 §2):
//! the record, its document hash and the instant of admission, which are
//! the whole input of `EvidenceFabric::admit`. Links, contests and
//! resolutions are derived: [`Platform::journaled_evidence`] reads the claims
//! back in log order and `EvidenceFabric::rebuild` re-admits them. The fabric
//! is still in-memory and bounded; what a restart would rebuild is the window
//! the journal retains (`DerivedState`), and no composition root performs
//! that rebuild at start-up yet.

use crate::platform::Platform;
use qip_contracts::knowledge_delta::{
    ChangeOp, KnowledgeDelta, KnowledgeDeltaFields, KnowledgeTarget, MergePolicy,
};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::{EventBody, Topic};
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::metrics::labels;
use qip_world_model::fabric::{ClaimKey, JournaledClaim, KeyOutcome};
pub use qip_world_model::poisoning::{
    PoisoningClass, PoisoningFinding, PoisoningThresholds, detect_poisoning,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Claims a connector forwarded into the world fabric, by `outcome`:
/// `admitted`, `contested` (admitted and opened or joined a contest),
/// `inadmissible` (held, on an impossible timeline), `duplicate`, `refused`.
pub const EVIDENCE_ABSORBED: &str = "qip_evidence_absorbed_total";

/// Poisoning findings over the world fabric, by `class`
/// ([`PoisoningClass::as_str`]): a bounded enum, never a source name.
pub const EVIDENCE_POISONING_FINDINGS: &str = "qip_evidence_poisoning_findings_total";

/// Refusal reasons kept on one report. The counter carries the total; the
/// first few reasons are what an operator reads on the cycle line.
pub const REFUSALS_RETAINED: usize = 8;

/// The producer on every knowledge-delta record, so a replay picks them out
/// of the `world_model.updated` topic they share with the NOW, lattice and
/// federation reviews the way those are told apart: by origin and by
/// idempotency prefix.
pub const KNOWLEDGE_DELTA_ORIGIN: &str = "kernel/knowledge-delta";

const KNOWLEDGE_DELTA_PREFIX: &str = "knowledge-delta/";

/// The brain and region a kernel-produced delta names as its origin: the
/// kernel absorbs the evidence and the home region is where it runs.
const DELTA_BRAIN: &str = "qip-kernel";
const DELTA_REGION: &str = "home";

const EVIDENCE_ADMITTED_PREFIX: &str = "evidence-admitted/";

/// Origin of an `evidence.admitted` record in the log.
pub const EVIDENCE_ADMITTED_ORIGIN: &str = "kernel/evidence-admitted";

/// One claim the world fabric admitted, as the log holds it (ADR 0152 §2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceAdmittedRecord {
    pub claim: JournaledClaim,
}

impl EventBody for EvidenceAdmittedRecord {
    const TOPIC: Topic = Topic::EvidenceAdmitted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{EVIDENCE_ADMITTED_PREFIX}{}",
            self.claim.record.id
        ))
    }
}

/// One world-model change as the log holds it (R12-EXPAND-6039): the
/// contract's [`KnowledgeDeltaFields`], which a receiver re-admits through
/// `KnowledgeDelta::new` and so cannot read as anything the contract refuses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeDeltaRecord {
    pub delta: KnowledgeDeltaFields,
}

impl EventBody for KnowledgeDeltaRecord {
    const TOPIC: Topic = Topic::WorldModelUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{KNOWLEDGE_DELTA_PREFIX}{}", self.delta.delta_id))
    }
}

/// What the fabric said about one admitted claim, as a delta needs it.
struct AdmittedClaim<'a> {
    id: &'a str,
    key: &'a ClaimKey,
    value: &'a str,
    document_hash: &'a str,
    weight: f64,
    contradicts: &'a [String],
    /// The fabric's version before and after the claim; one apart.
    versions: (u64, u64),
}

/// Build the contract delta for one admitted claim, or say why the claim
/// cannot be one. A claim whose weight is outside `[0, 1]` is refused rather
/// than clamped into a confidence nobody computed.
fn knowledge_delta_for(claim: &AdmittedClaim<'_>, now: Timestamp) -> Result<KnowledgeDelta> {
    if !(0.0..=1.0).contains(&claim.weight) {
        return Err(Error::invalid(format!(
            "claim `{}` weighs {}, outside [0, 1]; a delta states a confidence, and one nobody              computed is not stated",
            claim.id, claim.weight
        )));
    }
    let contested = !claim.contradicts.is_empty();
    KnowledgeDelta::new(KnowledgeDeltaFields {
        schema_version: 1,
        delta_id: format!("kd-{}", claim.id),
        target: KnowledgeTarget::WorldModel,
        base_version: claim.versions.0,
        new_version: claim.versions.1,
        changes: vec![ChangeOp::Assert {
            key: format!(
                "{}/{}@{}",
                claim.key.subject,
                claim.key.attribute,
                claim.key.instant.as_nanos()
            ),
            value: claim.value.to_string(),
        }],
        provenance: vec![claim.id.to_string(), format!("doc:{}", claim.document_hash)],
        // Rounded from a value just proven to be in [0, 1], so the cast
        // cannot leave [0, 10000].
        confidence_bp: (claim.weight * 10_000.0).round() as u32,
        conflicts: claim
            .contradicts
            .iter()
            .map(|other| format!("kd-{other}"))
            .collect(),
        originating_brain: DELTA_BRAIN.to_string(),
        originating_region: DELTA_REGION.to_string(),
        merge_policy: if contested {
            MergePolicy::ReviewRequired
        } else {
            MergePolicy::FastForwardOnly
        },
        created_at: now,
    })
}

/// What one forwarded batch did to the world fabric.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvidenceIntake {
    /// Admitted and weighed.
    pub admitted: usize,
    /// Of `admitted`, claims that opened or entered a contest against
    /// another hypothesis — the cross-source disagreement a private fabric
    /// could never see.
    pub contested: usize,
    /// Of `admitted`, claims on an impossible timeline: held, so the
    /// poisoning detector sees the pattern, but weighing nothing.
    pub inadmissible: usize,
    /// Already held under the same id.
    pub duplicate: usize,
    /// Refused by the fabric.
    pub refused: usize,
    /// Of `admitted`, claims that changed what the world fabric believes and
    /// left one validated, journalled `KnowledgeDelta` each.
    pub knowledge_deltas: usize,
    /// Admitted claims whose delta the contract or the log refused. Counted,
    /// never fatal: the claim is already in the fabric.
    pub deltas_refused: usize,
    /// Of `admitted`, claims journaled to `evidence.admitted`.
    pub claims_journaled: usize,
    /// Admitted claims the log refused. Counted, never fatal: the claim is
    /// already in the fabric, but a restart would not rebuild it.
    pub journal_refused: usize,
    /// The first [`REFUSALS_RETAINED`] refusal reasons.
    pub refusals: Vec<String>,
    /// Ids of the claims in this batch the world fabric would not stand
    /// behind: refused, held inadmissible, or judged spoofed (R12-EVID-1200).
    /// Bounded by the batch, which the feed bounds. [`Self::release`] drops
    /// the records these name, so a claim the world refused is not observed.
    pub withheld: BTreeSet<String>,
}

impl EvidenceIntake {
    /// The items whose claim the world fabric did not withhold, in order.
    ///
    /// The kernel-level half of the truth loop: a record reaches `observe`
    /// only after the claim it rests on was admitted by the fabric every
    /// source shares, not merely by the one connector's private check. An
    /// id this intake never saw (a duplicate, already held) is released:
    /// the fabric holds it and has not judged it again.
    pub fn release<T>(&self, items: Vec<(T, String)>) -> Vec<T> {
        items
            .into_iter()
            .filter(|(_, id)| !self.withheld.contains(id))
            .map(|(item, _)| item)
            .collect()
    }

    /// Whether the batch carried anything.
    pub const fn is_empty(&self) -> bool {
        self.admitted + self.duplicate + self.refused == 0
    }

    pub fn describe(&self) -> String {
        format!(
            "evidence: {} admitted to the world fabric ({} contested, {} inadmissible), {} \
             duplicate, {} refused",
            self.admitted, self.contested, self.inadmissible, self.duplicate, self.refused
        )
    }
}

/// What [`Platform::admit_world_records`] let through.
#[derive(Debug)]
pub struct Released<T> {
    /// The records whose claim the world fabric stood behind, and every
    /// record that rests on no claim, in the order offered.
    pub records: Vec<T>,
    /// Records offered whose claim the fabric withheld.
    pub withheld: usize,
    /// What the poll's evidence did to the fabric.
    pub intake: EvidenceIntake,
}

impl Platform {
    /// Absorb one poll's checked evidence and hand back only the records the
    /// world fabric stood behind (R12-EVID-1200): the one seam every
    /// composition root's connector arm goes through, so the order evidence
    /// then observation is the kernel's and not three roots' copies of it.
    ///
    /// Each item is a record and the claim it rests on: the connector's
    /// claim id, or `None` for a record no source claimed (a synthetic,
    /// replayed or tape record), which has nothing to withhold. Until
    /// 2026-10-24 each root absorbed and released on its own, and two of the
    /// three observed every record whatever the fabric said; a root now has
    /// no way to reach the records but through the fabric's verdict.
    pub fn admit_world_records<T>(
        &mut self,
        evidence: Vec<CheckedEvidence>,
        items: Vec<(T, Option<String>)>,
        now: Timestamp,
    ) -> Released<T> {
        let intake = self.absorb_evidence(evidence, now);
        let offered = items.len();
        let records: Vec<T> = items
            .into_iter()
            .flat_map(|(record, claim)| match claim {
                Some(id) => intake.release(vec![(record, id)]),
                None => vec![record],
            })
            .collect();
        Released {
            withheld: offered - records.len(),
            records,
            intake,
        }
    }

    /// Forward one connector poll's checked evidence into the world model's
    /// fabric. See the module doc for what is counted and why nothing here
    /// stops the poll: infallible by design, every per-claim refusal is on
    /// the report rather than an error that would discard the batch's good
    /// claims with its bad one.
    pub fn absorb_evidence(
        &mut self,
        batch: Vec<CheckedEvidence>,
        now: Timestamp,
    ) -> EvidenceIntake {
        let mut intake = EvidenceIntake::default();
        if batch.is_empty() {
            return intake;
        }
        let metrics = self.telemetry.metrics.clone();
        let mut deltas: Vec<KnowledgeDelta> = Vec::new();
        let mut admitted_claims: Vec<JournaledClaim> = Vec::new();
        self.world.update(|world| {
            for checked in batch {
                if world.evidence().get(&checked.record.id).is_some() {
                    intake.duplicate += 1;
                    metrics.count(EVIDENCE_ABSORBED, labels([("outcome", "duplicate")]));
                    continue;
                }
                let id = checked.record.id.clone();
                let claim = ClaimKey::of(&checked.record);
                let before = world.evidence().version();
                let mut journaled = JournaledClaim {
                    record: checked.record.clone(),
                    document_hash: Some(checked.document_hash.clone()),
                    admitted_at: now,
                };
                match world.absorb_evidence(checked.record, Some(&checked.document_hash), now) {
                    Ok((verdict, outcome)) => {
                        intake.admitted += 1;
                        // The world fabric's own temporal verdict, checked
                        // against every source's claims, is what the journal
                        // keeps (R12-EVID-2036). Until 2026-10-25 the record
                        // journaled was the pre-admission clone, so the log
                        // said whatever the connector's private check had
                        // said, or nothing, and a claim the world found
                        // dated before its declared cause was journaled as
                        // if no one had looked.
                        journaled.record.temporal_consistency = Some(verdict.temporal.clone());
                        admitted_claims.push(journaled);
                        // Only an admissible claim with a checkable key moves
                        // what the fabric believes; an inadmissible one is
                        // held so the detector sees it and weighs nothing.
                        if let (true, Some((key, value))) = (verdict.admissible, claim) {
                            let admitted = AdmittedClaim {
                                id: &id,
                                key: &key,
                                value: &value,
                                document_hash: &checked.document_hash,
                                weight: verdict.belief_weight,
                                contradicts: &verdict.contradicts,
                                versions: (before, world.evidence().version()),
                            };
                            match knowledge_delta_for(&admitted, now) {
                                Ok(delta) => deltas.push(delta),
                                Err(_) => intake.deltas_refused += 1,
                            }
                        }
                        if !verdict.admissible || verdict.authenticity.spoofed {
                            intake.withheld.insert(id.clone());
                        }
                        let label = if !verdict.admissible {
                            intake.inadmissible += 1;
                            "inadmissible"
                        } else if matches!(outcome, KeyOutcome::BecameContested)
                            || !verdict.contradicts.is_empty()
                        {
                            intake.contested += 1;
                            "contested"
                        } else {
                            "admitted"
                        };
                        metrics.count(EVIDENCE_ABSORBED, labels([("outcome", label)]));
                    }
                    Err(error) => {
                        intake.refused += 1;
                        intake.withheld.insert(id.clone());
                        if intake.refusals.len() < REFUSALS_RETAINED {
                            intake.refusals.push(format!("`{id}`: {}", error.message()));
                        }
                        metrics.count(EVIDENCE_ABSORBED, labels([("outcome", "refused")]));
                    }
                }
            }
        });
        for claim in admitted_claims {
            match self.journal_record(
                EvidenceAdmittedRecord { claim },
                EVIDENCE_ADMITTED_ORIGIN,
                now,
            ) {
                Ok(()) => intake.claims_journaled += 1,
                Err(_) => intake.journal_refused += 1,
            }
        }
        for delta in deltas {
            let record = KnowledgeDeltaRecord {
                delta: delta.into_fields(),
            };
            match self.journal_record(record, KNOWLEDGE_DELTA_ORIGIN, now) {
                Ok(()) => intake.knowledge_deltas += 1,
                Err(_) => intake.deltas_refused += 1,
            }
        }
        intake
    }

    /// The claims the log holds as admitted, in the order they were admitted:
    /// the input `EvidenceFabric::rebuild` needs to derive the fabric again.
    /// A record that does not decode is an error, not a skipped claim — a
    /// rebuild that silently dropped one would hold a different belief from
    /// the fabric that wrote the journal.
    pub fn journaled_evidence(&self) -> Result<Vec<JournaledClaim>> {
        Self::journaled_evidence_in(self.event_log())
    }

    /// [`Self::journaled_evidence`] over a log the platform does not yet
    /// own: what assembly reads to rebuild the fabric before the first cycle.
    pub(crate) fn journaled_evidence_in(log: &qip_events::EventLog) -> Result<Vec<JournaledClaim>> {
        log.by_topic(Topic::EvidenceAdmitted)
            .into_iter()
            .map(|frame| {
                let envelope = qip_streaming::envelope::StreamEnvelope::from_frame(frame)?;
                Ok(envelope.decode::<EvidenceAdmittedRecord>()?.body.claim)
            })
            .collect()
    }

    /// Scan the world fabric for coordinated, poisoned, spoofed, synthetic
    /// and spamming sources as of `now`, and count each finding by class.
    ///
    /// Read-only over the fabric. What a finding *does* is the caller's: the
    /// deep brain's research loop raises each as a data gap, so a poisoned
    /// source becomes research into replacing it rather than a line nobody
    /// reads.
    pub fn evidence_poisoning(
        &self,
        now: Timestamp,
        thresholds: &PoisoningThresholds,
    ) -> Vec<PoisoningFinding> {
        let findings = detect_poisoning(self.world.read().evidence(), now, thresholds);
        for finding in &findings {
            self.telemetry.metrics.count(
                EVIDENCE_POISONING_FINDINGS,
                labels([("class", finding.class.as_str())]),
            );
        }
        findings
    }

    /// Claims the world fabric holds, for a caller that must prove the
    /// fabric was fed rather than read a clean scan of nothing.
    pub fn evidence_held(&self) -> usize {
        self.world.read().evidence().len()
    }
}
