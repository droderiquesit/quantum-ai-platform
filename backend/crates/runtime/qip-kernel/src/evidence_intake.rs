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
//! It does not journal the verdicts to the event log. The only topic for
//! evidence, `EvidenceAttached`, already carries the symbolic stage's
//! derivations, and a second body on one topic is the decode failure
//! `references.rs` records having shipped once. The fabric is in-memory and
//! bounded; durability of its links (R12-EVID-2032) is not claimed here.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_market_ingestion::connector::evidence::CheckedEvidence;
use qip_observability::metrics::labels;
use qip_world_model::fabric::KeyOutcome;
pub use qip_world_model::poisoning::{
    PoisoningClass, PoisoningFinding, PoisoningThresholds, detect_poisoning,
};

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
    /// The first [`REFUSALS_RETAINED`] refusal reasons.
    pub refusals: Vec<String>,
}

impl EvidenceIntake {
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

impl Platform {
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
        self.world.update(|world| {
            for checked in batch {
                if world.evidence().get(&checked.record.id).is_some() {
                    intake.duplicate += 1;
                    metrics.count(EVIDENCE_ABSORBED, labels([("outcome", "duplicate")]));
                    continue;
                }
                let id = checked.record.id.clone();
                match world.absorb_evidence(checked.record, Some(&checked.document_hash), now) {
                    Ok((verdict, outcome)) => {
                        intake.admitted += 1;
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
                        if intake.refusals.len() < REFUSALS_RETAINED {
                            intake.refusals.push(format!("`{id}`: {}", error.message()));
                        }
                        metrics.count(EVIDENCE_ABSORBED, labels([("outcome", "refused")]));
                    }
                }
            }
        });
        intake
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
