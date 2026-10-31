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
use qip_world_model::causal::CausalEdge;
use qip_world_model::fabric::{ClaimKey, JournaledClaim, KeyOutcome};
use qip_world_model::graph::Node;
pub use qip_world_model::poisoning::{
    PoisoningClass, PoisoningFinding, PoisoningThresholds, detect_poisoning,
};
use qip_world_model::relationship::Relationship;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// One relationship handed to [`Platform::absorb_knowledge`]: the typed edge,
/// the instant it became true, and the confidence it is asserted with.
#[derive(Clone, Debug, PartialEq)]
pub struct KnowledgeRelation {
    pub relationship: Relationship,
    pub valid_from: Timestamp,
    pub confidence: f64,
}

/// What one [`Platform::absorb_knowledge`] call wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KnowledgeIntake {
    pub nodes_added: usize,
    /// Nodes handed in that the graph already held, left as they were.
    pub nodes_held: usize,
    pub relations: usize,
}

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

/// The one spelling of a claim key a knowledge delta asserts under, written
/// by the producer and read back by [`Platform::contested_claims`]: a reader
/// that re-derived the key from the contest would join a delta to a contest
/// only while the two spellings happened to agree.
pub fn delta_key(key: &ClaimKey) -> String {
    format!(
        "{}/{}@{}",
        key.subject,
        key.attribute,
        key.instant.as_nanos()
    )
}

/// One claim key the world fabric holds two or more unresolved values for,
/// as the deep brain's sweep reads it (R12-EXPAND-6011, ADR 0151 d12): the
/// key, each value with the claims asserting it, and the knowledge deltas
/// whose `conflicts` recorded the disagreement (R12-EXPAND-6039). Bounded by
/// the fabric's capacity, which bounds the contests it can hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContestedClaim {
    pub key: ClaimKey,
    /// The key as the deltas assert it: [`delta_key`] of `key`.
    pub asserted_as: String,
    /// Each competing value and the claim ids asserting it.
    pub hypotheses: BTreeMap<String, BTreeSet<String>>,
    /// Ids of the journaled knowledge deltas that named this key's conflict,
    /// in log order; empty when the log retains none.
    pub deltas: Vec<String>,
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
            key: delta_key(claim.key),
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

    /// Every knowledge delta the log retains under
    /// [`KNOWLEDGE_DELTA_ORIGIN`], in log order (R12-EXPAND-6039). The
    /// `world_model.updated` topic carries other records too (lattice
    /// reviews, federation and NOW stages), so a frame is decoded only when
    /// its idempotency key carries the delta prefix. A delta that does not
    /// decode is an error, not a skipped record: a sweep that silently read
    /// past one would report fewer conflicts than the kernel journaled.
    pub fn knowledge_deltas(&self) -> Result<Vec<KnowledgeDeltaFields>> {
        self.event_log()
            .by_topic(Topic::WorldModelUpdated)
            .into_iter()
            .filter(|frame| {
                frame
                    .idempotency_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with(KNOWLEDGE_DELTA_PREFIX))
            })
            .map(|frame| {
                let envelope = qip_streaming::envelope::StreamEnvelope::from_frame(frame)?;
                Ok(envelope.decode::<KnowledgeDeltaRecord>()?.body.delta)
            })
            .collect()
    }

    /// The claim keys the world fabric holds unresolved contests on, each
    /// joined to the knowledge deltas that recorded a conflict under it
    /// (R12-EXPAND-6011, R12-EXPAND-6039). The contests come from the fabric,
    /// so a contest a later claim resolved is absent, and the list is bounded
    /// by the fabric's capacity rather than by the log. The deltas are the
    /// caller's reading of [`Self::knowledge_deltas`], handed in so one
    /// decode serves both this join and the caller's own count.
    pub fn contested_claims(&self, deltas: &[KnowledgeDeltaFields]) -> Vec<ContestedClaim> {
        let mut conflicts_by_key: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for delta in deltas.iter().filter(|d| !d.conflicts.is_empty()) {
            for change in &delta.changes {
                if let ChangeOp::Assert { key, .. } = change {
                    conflicts_by_key
                        .entry(key.as_str())
                        .or_default()
                        .push(delta.delta_id.clone());
                }
            }
        }
        self.world
            .read()
            .evidence()
            .contests()
            .iter()
            .filter(|(_, contest)| contest.is_contested())
            .map(|(key, contest)| {
                let asserted_as = delta_key(key);
                let deltas = conflicts_by_key
                    .get(asserted_as.as_str())
                    .cloned()
                    .unwrap_or_default();
                ContestedClaim {
                    key: key.clone(),
                    asserted_as,
                    hypotheses: contest.hypotheses.clone(),
                    deltas,
                }
            })
            .collect()
    }

    /// Of `subjects`, those the causal graph holds at least one live claim
    /// into as of `now` — the same `incoming` the federation's causal member
    /// reads, keyed by the instrument id the evidence fabric keys claims on.
    /// A contested subject absent from this set has disagreeing evidence and
    /// no causal link that would say which side is right (R12-EXPAND-6011).
    pub fn causally_explained<'a>(
        &self,
        subjects: impl IntoIterator<Item = &'a str>,
        now: Timestamp,
    ) -> BTreeSet<String> {
        let world = self.world.read();
        subjects
            .into_iter()
            .filter(|subject| !world.causal().incoming(subject, now).is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Record extracted knowledge in the knowledge graph: nodes the graph
    /// does not yet hold, then typed relationships between nodes it does,
    /// each holding from its own `valid_from` and known from `now`
    /// (R12-EXPAND-6031).
    ///
    /// The seam the cross-domain synthesis reads behind. `WorldModel::relate`
    /// existed and, outside `seed_demo_world` and tests, nothing in a deployed
    /// process called it, so the graph the synthesis walks holds the nodes the
    /// kernel adds for its own instruments and no entity and no edge. This is
    /// the one door for what a connector's or reference feed's extraction
    /// would hand in; it has no production caller yet, and the assessment row
    /// says so rather than this comment implying otherwise. A node already
    /// held is left as it was, because its `recorded_at` answers "when did the
    /// platform first hear of this" and must not be rewritten; a relationship
    /// the graph refuses — to a node it does not hold, or with a confidence
    /// outside the unit interval — stops the intake with the refusal, after
    /// the nodes and the relationships before it have been written, and the
    /// count on the error path is the caller's to read from the graph.
    pub fn absorb_knowledge(
        &mut self,
        nodes: Vec<Node>,
        relations: Vec<KnowledgeRelation>,
        now: Timestamp,
    ) -> Result<KnowledgeIntake> {
        self.world.update(|world| {
            let mut intake = KnowledgeIntake::default();
            for node in nodes {
                if world.graph().node(&node.id).is_some() {
                    intake.nodes_held += 1;
                    continue;
                }
                world.graph_mut().add_node(node)?;
                intake.nodes_added += 1;
            }
            for relation in relations {
                world.relate(
                    relation.relationship,
                    relation.valid_from,
                    now,
                    relation.confidence,
                )?;
                intake.relations += 1;
            }
            Ok(intake)
        })
    }

    /// Admit one causal claim to the world model, on the world model's own
    /// terms: `WorldModel::claim_causal` refuses a blank end and a quantum
    /// result offered as proof, and records the edge in the world model's
    /// own change journal, exactly as the cycle's re-estimation pass does
    /// through the same method. The edge reaches no event-log record from
    /// either caller, so this seam adds no second kind of unjournaled edge;
    /// it exists so a test can hold a causal claim against a contest
    /// without running a cycle, and it is not a knowledge-pack loader —
    /// one of those would need an event-log record of its own first.
    pub fn claim_causal(&mut self, edge: CausalEdge) -> Result<()> {
        self.world.update(|world| world.claim_causal(edge))
    }
}
