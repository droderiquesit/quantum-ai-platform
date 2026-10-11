//! Steps 5 to 7 of the expansion loop: a reproducible campaign across the
//! four environments (EXPAND-050), one evaluation record on all nine
//! criteria (EXPAND-051), and the conversion of research that passed both
//! into a capability record (EXPAND-025).
//!
//! The failure prevented is research that closes onto nothing, and its
//! mirror image: research that becomes capability on a partial evaluation.
//! Calibration and walk-forward folds existed elsewhere in the tree, but
//! leakage, safety and legal eligibility were never part of the same record,
//! so a candidate could be "evaluated" with three of the nine questions
//! unasked and nothing would say which. Here a record missing any criterion
//! is refused by name, and [`promote`] is the only constructor of a
//! promoted capability.

use crate::plan::Environment;
use qip_contracts::expansion::{CapabilityEntry, CapabilityVerb, GapClass};
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where one campaign stage's inputs came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Digest of the dataset or tape the stage ran on.
    pub dataset_digest: String,
    /// The records the dataset was derived from.
    pub lineage: Vec<String>,
    /// The seed, so a simulated stage replays.
    pub seed: u64,
}

/// A research campaign spanning historical, simulated, paper and shadow
/// environments, in that order, each with its provenance (EXPAND-050).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchCampaign {
    question: String,
    stages: Vec<(Environment, Provenance)>,
}

impl ResearchCampaign {
    pub fn open(question: &str) -> Result<Self> {
        if question.trim().is_empty() {
            return Err(Error::invalid(
                "a campaign researches a question; state the question it answers",
            ));
        }
        Ok(Self {
            question: question.trim().to_string(),
            stages: Vec::new(),
        })
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub fn stages(&self) -> &[(Environment, Provenance)] {
        &self.stages
    }

    /// Record the next stage. Refuses a stage out of order — a paper result
    /// with no simulated stage behind it is a result nobody can say was
    /// tested before it met a book — and refuses a stage with no dataset
    /// digest or no lineage, because a stage that cannot name its inputs
    /// cannot be reproduced.
    pub fn record(&mut self, environment: Environment, provenance: Provenance) -> Result<()> {
        let expected = Environment::ORDER.get(self.stages.len()).copied();
        if expected != Some(environment) {
            return Err(Error::invalid(format!(
                "campaign `{}` expected {expected:?} next and was given {environment:?}; stages run historical, simulated, paper, shadow, in order",
                self.question
            )));
        }
        if provenance.dataset_digest.trim().is_empty()
            || provenance.lineage.is_empty()
            || provenance.lineage.iter().any(|l| l.trim().is_empty())
        {
            return Err(Error::invalid(format!(
                "the {environment:?} stage of `{}` names no dataset digest or lineage; a stage that cannot name its inputs cannot be reproduced",
                self.question
            )));
        }
        self.stages.push((environment, provenance));
        Ok(())
    }

    /// Whether all four environments ran.
    pub fn is_complete(&self) -> bool {
        self.stages.len() == Environment::ORDER.len()
    }
}

/// The nine questions an independent evaluation asks (§23.1 step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    Calibration,
    Robustness,
    Leakage,
    RegimeSensitivity,
    Cost,
    Latency,
    Safety,
    LegalEligibility,
    IncrementalValue,
}

impl Criterion {
    pub const ALL: [Self; 9] = [
        Self::Calibration,
        Self::Robustness,
        Self::Leakage,
        Self::RegimeSensitivity,
        Self::Cost,
        Self::Latency,
        Self::Safety,
        Self::LegalEligibility,
        Self::IncrementalValue,
    ];
}

/// What one criterion found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub passed: bool,
    /// The evaluation run or record the finding cites.
    pub evidence: String,
}

/// One evaluation covering all nine criteria (EXPAND-051). Built only by
/// [`EvaluationRecord::new`], which refuses a missing criterion or a finding
/// that cites nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    question: String,
    findings: BTreeMap<Criterion, Finding>,
}

impl EvaluationRecord {
    pub fn new(question: &str, findings: BTreeMap<Criterion, Finding>) -> Result<Self> {
        let missing: Vec<Criterion> = Criterion::ALL
            .iter()
            .copied()
            .filter(|c| !findings.contains_key(c))
            .collect();
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "the evaluation of `{question}` asks nothing about {missing:?}; all nine criteria are one record, and an unasked one is not a pass"
            )));
        }
        if let Some((c, _)) = findings.iter().find(|(_, f)| f.evidence.trim().is_empty()) {
            return Err(Error::invalid(format!(
                "the {c:?} finding for `{question}` cites no evidence; attach the evaluation run it came from"
            )));
        }
        Ok(Self {
            question: question.trim().to_string(),
            findings,
        })
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub fn findings(&self) -> &BTreeMap<Criterion, Finding> {
        &self.findings
    }

    /// The criteria that failed, in criterion order.
    pub fn failed(&self) -> Vec<Criterion> {
        self.findings
            .iter()
            .filter(|(_, f)| !f.passed)
            .map(|(c, _)| *c)
            .collect()
    }

    pub fn passed(&self) -> bool {
        self.failed().is_empty()
    }
}

/// The capability verb research of each class may become. None of them is
/// `Trade`, `Settle`, `Hedge`, `Transfer` or `Purchase`: research produces
/// sensing, reasoning, simulation and operations, and a capability that
/// moves money arrives through its own promotion, never through this one.
pub const fn verb_for(class: GapClass) -> CapabilityVerb {
    match class {
        GapClass::Data => CapabilityVerb::Sense,
        GapClass::Ontology
        | GapClass::Causal
        | GapClass::Memory
        | GapClass::Model
        | GapClass::Specialist => CapabilityVerb::Reason,
        GapClass::Execution | GapClass::ComputeQuantumResearch => CapabilityVerb::Simulate,
        GapClass::Tool => CapabilityVerb::Operate,
    }
}

/// Convert proven research into a capability record (EXPAND-025).
///
/// Refuses a campaign that did not run all four environments, an
/// evaluation of a different question, and an evaluation that failed any
/// criterion, naming which. The eligibility recorded is paper and shadow:
/// promotion to anything else is ADR 0108's, not this function's.
pub fn promote(
    class: GapClass,
    campaign: &ResearchCampaign,
    evaluation: &EvaluationRecord,
) -> Result<CapabilityEntry> {
    if !campaign.is_complete() {
        return Err(Error::denied(format!(
            "`{}` ran {} of 4 environments; a capability is promoted only after shadow",
            campaign.question(),
            campaign.stages().len()
        )));
    }
    if campaign.question() != evaluation.question() {
        return Err(Error::invalid(format!(
            "the evaluation is of `{}` and the campaign of `{}`; evaluate the research being promoted",
            evaluation.question(),
            campaign.question()
        )));
    }
    let failed = evaluation.failed();
    if !failed.is_empty() {
        return Err(Error::denied(format!(
            "`{}` failed {failed:?}; record the null and do not promote",
            campaign.question()
        )));
    }
    let entry = CapabilityEntry {
        verb: verb_for(class),
        implementations: vec![campaign.question().to_string()],
        confidence_level: 1.0,
        eligibility: "paper and shadow only".to_string(),
    };
    entry.validate()?;
    Ok(entry)
}
