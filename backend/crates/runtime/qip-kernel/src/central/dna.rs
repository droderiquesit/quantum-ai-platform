//! The sealed bundle the centre ships to a cell.
//!
//! A cell that has lost contact with the centre keeps trading, so everything
//! it needs to trade correctly has to have arrived before the partition: the
//! compiled program, the features it reads, which model and evidence it came
//! from, the bound it may commit inside, and the conditions under which it
//! stops itself. [`StrategyDna`] is that bundle, and a
//! [`qip_contracts::governance::Provenance`] over the whole of it is what makes
//! it something the receiving side can check rather than merely parse.
//!
//! Three properties are structural rather than checked-at-the-call-site:
//!
//! * **A DNA cannot exist below pilot.** [`StrategyDna::seal`] is the only
//!   constructor and refuses a stage for which
//!   [`qip_contracts::gate::GateStage::holds_capital`] is false. A shadow-stage
//!   strategy has no representation here, which is the point — shadow is the
//!   last rung where being wrong is free, and shipping a signed grant to a cell
//!   is where it stops being free.
//! * **A DNA cannot carry a grant nobody approved.** `seal` takes a
//!   [`qip_compliance::ApprovedCapital`], which has no public constructor, and
//!   refuses unless its envelope's
//!   [`qip_contracts::CapitalEnvelope::signing_payload`] is byte-identical to
//!   the one being shipped. The governance record and the cell's bound
//!   therefore cannot describe different amounts.
//! * **A DNA cannot be silently edited.** The provenance digest covers the
//!   whole payload, and the payload additionally carries a digest per section,
//!   so verification says *which* part was altered rather than only that
//!   something was.
//!
//! # What the signature is not
//!
//! Sealing and verifying both take a [`SigningKey`], which is HMAC-SHA-256 over
//! a shared secret. A cell that can verify a DNA can also mint one. That is the
//! same gap `qip_compliance::signing` and `qip_capital::envelope` both name,
//! and it travels with this code for the same reason: production needs
//! asymmetric signing with the private key in a KMS, so that verification does
//! not confer the ability to sign.

use super::factory::StrategyCandidate;
use qip_compliance::approval::ApprovedCapital;
use qip_compliance::signing::SigningKey;
use qip_contracts::CapitalEnvelope;
use qip_contracts::feature::FeatureKey;
use qip_contracts::gate::GateStage;
use qip_contracts::governance::Provenance;
use qip_contracts::signal::StrategyId;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_core::sha256_hex;
use qip_lifecycle::evidence::KillCondition;
use qip_strategy::compile::CompiledStrategy;
use qip_strategy::program::Program;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The sections a digest is kept for, in the order verification reports them.
///
/// Named constants rather than string literals at three call sites, because a
/// typo in one of them would produce a section whose digest is never compared
/// and whose tampering is therefore never named.
const SECTIONS: [&str; 7] = [
    "compiled",
    "program",
    "features",
    "models",
    "artifacts",
    "envelope",
    "kill_conditions",
];

/// Everything the centre hands a cell, before it is sealed.
///
/// Fields are public so a receiving cell can read them without an accessor for
/// each; `section_digests` is private, which is what stops anything but
/// [`StrategyDna::seal`] from producing a payload whose per-section digests
/// agree with its contents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DnaPayload {
    pub strategy: StrategyId,
    /// The rung the strategy stood on when this was sealed. Always one that
    /// holds capital — see [`StrategyDna::seal`].
    pub stage: GateStage,
    pub cell: String,
    pub compiled: CompiledStrategy,
    /// The arena the compiled form indexes into. Shipped whole rather than by
    /// reference: a cell that cannot reach the centre cannot fetch it later.
    pub program: Program,
    /// Every feature the strategy reads. The cell's feature DAG must be able
    /// to supply all of them or the strategy emits nothing.
    pub features: Vec<FeatureKey>,
    /// The models distilled into the program, as `name@version`.
    pub model_references: Vec<String>,
    /// Digests of the evidence this promotion rested on.
    pub artifact_references: Vec<String>,
    /// The bound the cell trades inside. Signed and expiring by construction.
    pub envelope: CapitalEnvelope,
    /// What stops the strategy without anyone being reachable.
    pub kill_conditions: Vec<KillCondition>,
    pub issued_at: Timestamp,
    section_digests: BTreeMap<String, String>,
}

impl DnaPayload {
    /// The digest recorded for one section at sealing time.
    pub fn section_digest(&self, section: &str) -> Option<&str> {
        self.section_digests.get(section).map(String::as_str)
    }

    /// Recompute every section digest from the payload as it stands now.
    fn recompute_sections(&self) -> Result<BTreeMap<String, String>> {
        let mut digests = BTreeMap::new();
        digests.insert(SECTIONS[0].to_string(), digest_of(&self.compiled)?);
        digests.insert(SECTIONS[1].to_string(), digest_of(&self.program)?);
        digests.insert(SECTIONS[2].to_string(), digest_of(&self.features)?);
        digests.insert(SECTIONS[3].to_string(), digest_of(&self.model_references)?);
        digests.insert(
            SECTIONS[4].to_string(),
            digest_of(&self.artifact_references)?,
        );
        digests.insert(SECTIONS[5].to_string(), digest_of(&self.envelope)?);
        digests.insert(SECTIONS[6].to_string(), digest_of(&self.kill_conditions)?);
        Ok(digests)
    }
}

/// A payload with a signed provenance over the whole of it.
///
/// Deserialises, because a cell receives one over a wire and has to be able to
/// read the bytes before it can check them. That is not a way around
/// [`Self::seal`]: a value that arrives from anywhere else fails
/// [`Self::verify`], and every check `seal` makes is re-made there — including
/// the stage — so a deserialised DNA claiming shadow, or claiming a grant
/// nobody signed, is refused on arrival rather than admitted for having
/// parsed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyDna {
    payload: DnaPayload,
    provenance: Provenance,
}

impl StrategyDna {
    /// Seal a candidate's program and its grant into a shippable bundle.
    ///
    /// The only constructor. Every refusal below removes a way a cell could end
    /// up trading something nobody authorised, so each one names exactly what
    /// was wrong rather than reporting a generic denial.
    pub fn seal(
        candidate: &StrategyCandidate,
        stage: GateStage,
        approved: &ApprovedCapital,
        envelope: &CapitalEnvelope,
        key: &SigningKey,
        signer: impl Into<String>,
        now: Timestamp,
    ) -> Result<Self> {
        // The rule the whole type exists for. Shadow is the last rung where
        // being wrong is free; a signed, venue-scoped grant in a cell's hands
        // is where that stops.
        if !stage.holds_capital() {
            return Err(Error::denied(format!(
                "{} is at {}, which holds no capital; there is no DNA to ship until it has \
                 passed the pilot gate",
                candidate.strategy(),
                stage.as_str()
            )));
        }
        if envelope.strategy() != candidate.strategy() {
            return Err(Error::denied(format!(
                "the envelope grants capital to {} but the DNA is for {}",
                envelope.strategy(),
                candidate.strategy()
            )));
        }
        if envelope.cell() != candidate.cell() {
            return Err(Error::denied(format!(
                "the envelope is good at cell {} but the DNA would run at {}",
                envelope.cell(),
                candidate.cell()
            )));
        }
        // The governance record and the cell's bound must describe the same
        // grant. The signing payload covers every field that bounds what the
        // cell may do, so comparing it is comparing the whole authority.
        if envelope.signing_payload() != approved.envelope().signing_payload() {
            return Err(Error::denied(format!(
                "the grant being shipped to {} is not the one {} approved; the approved terms \
                 are `{}` and the shipped terms are `{}`",
                envelope.cell(),
                approved.approvers().join(" and "),
                approved.envelope().signing_payload(),
                envelope.signing_payload()
            )));
        }
        if !envelope.is_live(now) {
            return Err(Error::denied(format!(
                "the envelope for {} expired at {}; shipping it would hand a cell a grant that \
                 admits nothing",
                envelope.strategy(),
                envelope.expires_at().to_rfc3339()
            )));
        }
        if candidate.kill_conditions().is_empty() {
            return Err(Error::denied(format!(
                "{} states no kill conditions; a cell that cannot be reached must be able to \
                 stop itself, and nothing here tells it when",
                candidate.strategy()
            )));
        }
        if candidate.evidence_artifacts().is_empty() {
            return Err(Error::denied(format!(
                "{} names no evidence artifacts, so a provenance over this DNA would have no \
                 inputs and the chain from a live order back to the data it was fitted on \
                 would stop here",
                candidate.strategy()
            )));
        }

        let mut payload = DnaPayload {
            strategy: candidate.strategy().clone(),
            stage,
            cell: candidate.cell().to_string(),
            compiled: candidate.compiled().clone(),
            program: candidate.program().clone(),
            features: candidate.compiled().inputs().to_vec(),
            model_references: candidate
                .model_reference()
                .map(|reference| vec![reference.to_string()])
                .unwrap_or_default(),
            artifact_references: candidate.evidence_artifacts().to_vec(),
            envelope: envelope.clone(),
            kill_conditions: candidate.kill_conditions().to_vec(),
            issued_at: now,
            section_digests: BTreeMap::new(),
        };
        payload.section_digests = payload.recompute_sections()?;

        let canonical = canonical_form(&payload)?;
        let signature = key.sign(&canonical);
        // The inputs are the evidence, not the payload's own parts: the chain
        // this provenance belongs to walks backwards through what the strategy
        // was built from, and the program is the output of that chain rather
        // than a step in it.
        let provenance = Provenance::sign(
            canonical.as_bytes(),
            signer,
            signature,
            now,
            payload.artifact_references.clone(),
        )?;

        Ok(Self {
            payload,
            provenance,
        })
    }

    pub fn payload(&self) -> &DnaPayload {
        &self.payload
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.payload.strategy
    }

    pub fn stage(&self) -> GateStage {
        self.payload.stage
    }

    pub fn cell(&self) -> &str {
        &self.payload.cell
    }

    pub fn envelope(&self) -> &CapitalEnvelope {
        &self.payload.envelope
    }

    pub fn compiled(&self) -> &CompiledStrategy {
        &self.payload.compiled
    }

    pub fn program(&self) -> &Program {
        &self.payload.program
    }

    pub fn features(&self) -> &[FeatureKey] {
        &self.payload.features
    }

    pub fn kill_conditions(&self) -> &[KillCondition] {
        &self.payload.kill_conditions
    }

    /// The exact bytes the provenance digest and the signature were taken over.
    pub fn canonical(&self) -> Result<String> {
        canonical_form(&self.payload)
    }

    /// Check a bundle that arrived from somewhere else. The receiving side's
    /// entry point.
    ///
    /// The checks run cheapest-and-most-specific first so that the common
    /// tamper — editing one section and leaving its digest — is reported by
    /// name. A tamperer who also updates the section digest is caught by the
    /// whole-payload digest a step later, with a less specific message, which
    /// is the honest trade: naming the field costs a digest per section, and
    /// naming it under an adversary who updates both would cost shipping the
    /// original alongside the copy.
    pub fn verify(&self, key: &SigningKey, now: Timestamp) -> Result<()> {
        let what = format!(
            "the strategy DNA for {} at {}",
            self.payload.strategy, self.payload.cell
        );

        if !self.payload.stage.holds_capital() {
            return Err(Error::denied(format!(
                "{what} claims stage {}, which holds no capital; a DNA below pilot cannot have \
                 been sealed and must not be run",
                self.payload.stage.as_str()
            )));
        }

        let recomputed = self.payload.recompute_sections()?;
        for section in SECTIONS {
            let sealed = self.payload.section_digests.get(section);
            let actual = recomputed.get(section);
            if sealed != actual {
                return Err(Error::denied(format!(
                    "{what} does not verify: the `{section}` section was sealed as {} and is now \
                     {}",
                    sealed.map_or("absent", String::as_str),
                    actual.map_or("absent", String::as_str)
                )));
            }
        }

        let canonical = canonical_form(&self.payload)?;
        if !self.provenance.matches(canonical.as_bytes()) {
            return Err(Error::denied(format!(
                "{what} does not verify: the payload hashes to {} but the provenance records {}",
                sha256_hex(canonical.as_bytes()),
                self.provenance.digest()
            )));
        }
        key.require(&what, &canonical, self.provenance.signature())?;

        if self.provenance.inputs().is_empty() {
            return Err(Error::denied(format!(
                "{what} names no inputs, so nothing connects it to the evidence it was promoted \
                 on"
            )));
        }
        if self.payload.envelope.strategy() != &self.payload.strategy {
            return Err(Error::denied(format!(
                "{what} carries an envelope granting capital to {}",
                self.payload.envelope.strategy()
            )));
        }
        if self.payload.envelope.cell() != self.payload.cell {
            return Err(Error::denied(format!(
                "{what} carries an envelope good at cell {}",
                self.payload.envelope.cell()
            )));
        }
        if self.payload.features != self.payload.compiled.inputs() {
            return Err(Error::denied(format!(
                "{what} declares {} feature requirement(s) but its compiled form reads {}",
                self.payload.features.len(),
                self.payload.compiled.inputs().len()
            )));
        }
        if !self.payload.envelope.is_live(now) {
            return Err(Error::denied(format!(
                "{what} carries a grant that expired at {}; the cell stops on its own clock and \
                 this one has already stopped",
                self.payload.envelope.expires_at().to_rfc3339()
            )));
        }
        Ok(())
    }

    /// Whether a bundle verifies, without the reason.
    pub fn is_valid(&self, key: &SigningKey, now: Timestamp) -> bool {
        self.verify(key, now).is_ok()
    }
}

/// The bytes a digest and a signature are taken over.
///
/// JSON rather than a hand-rolled format: the field order is the declaration
/// order and the maps are ordered, so the same value produces the same string
/// on any machine, and a reviewer can read what was signed.
fn canonical_form(payload: &DnaPayload) -> Result<String> {
    serde_json::to_string(payload).map_err(|error| {
        Error::schema(format!(
            "the DNA payload for {} could not be rendered canonically: {error}",
            payload.strategy
        ))
    })
}

fn digest_of<T: Serialize>(value: &T) -> Result<String> {
    let rendered = serde_json::to_vec(value)
        .map_err(|error| Error::schema(format!("a DNA section could not be digested: {error}")))?;
    Ok(sha256_hex(&rendered))
}
