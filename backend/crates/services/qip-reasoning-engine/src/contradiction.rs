//! First-class contradiction resolution tracking.
//!
//! When two pieces of evidence contradict each other, the platform records
//! which one is believed and why, using bitemporal tracking to distinguish
//! when the contradiction became known from when it was resolved.

use qip_core::error::{Error, Result};
use qip_core::ids::EvidenceId;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How a contradiction was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionMethod {
    /// One piece of evidence was found to be factually incorrect.
    Correction,
    /// The contradicting pieces of evidence are both correct but describe
    /// different aspects or contexts; both remain valid.
    Reconciliation,
    /// The evidence refers to different points in time; ordering clarifies both.
    TemporalSequence,
    /// The contradiction arises from different interpretations of the same fact.
    InterpretationDifference,
    /// The contradiction remains unresolved; both sides maintain their position.
    Unresolved,
}

impl ResolutionMethod {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Correction => "correction",
            Self::Reconciliation => "reconciliation",
            Self::TemporalSequence => "temporal_sequence",
            Self::InterpretationDifference => "interpretation_difference",
            Self::Unresolved => "unresolved",
        }
    }
}

/// First-class record of a contradiction and how it was resolved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContradictionResolution {
    /// When the contradiction was identified.
    pub identified_at: Timestamp,
    /// The evidence items that contradicted each other.
    pub contradicting_ids: BTreeSet<EvidenceId>,
    /// How the contradiction was resolved.
    pub method: ResolutionMethod,
    /// Evidence that informed the resolution.
    pub resolution_evidence_ids: BTreeSet<EvidenceId>,
    /// When the contradiction was resolved.
    pub resolved_at: Timestamp,
    /// Summary of what the contradiction was and how it was handled.
    pub summary: String,
}

impl ContradictionResolution {
    /// Create a new contradiction resolution record.
    ///
    /// Refuses if fewer than 2 contradicting items, or if summary is empty,
    /// or if identified_at > resolved_at.
    pub fn new(
        identified_at: Timestamp,
        contradicting_ids: BTreeSet<EvidenceId>,
        method: ResolutionMethod,
        resolution_evidence_ids: BTreeSet<EvidenceId>,
        resolved_at: Timestamp,
        summary: impl Into<String>,
    ) -> Result<Self> {
        let summary = summary.into();

        if contradicting_ids.len() < 2 {
            return Err(Error::invalid(
                "a contradiction must have at least 2 contradicting items",
            ));
        }

        if summary.is_empty() {
            return Err(Error::invalid("summary must not be empty"));
        }

        if identified_at > resolved_at {
            return Err(Error::invalid(
                "identified_at must not be after resolved_at",
            ));
        }

        Ok(Self {
            identified_at,
            contradicting_ids,
            method,
            resolution_evidence_ids,
            resolved_at,
            summary,
        })
    }

    /// Validate the contradiction resolution is sound.
    ///
    /// This is called before storing, so a defect in the record is caught
    /// before it enters the audit log.
    pub fn validate(&self) -> Result<()> {
        if self.contradicting_ids.len() < 2 {
            return Err(Error::invalid(
                "contradiction must have at least 2 contradicting items",
            ));
        }

        if self.summary.is_empty() {
            return Err(Error::invalid("summary is empty"));
        }

        if self.identified_at > self.resolved_at {
            return Err(Error::invalid("identified_at cannot be after resolved_at"));
        }

        Ok(())
    }

    /// Short description of the resolution.
    pub fn describe(&self) -> String {
        format!(
            "Contradiction {} {} identified at {}, resolved by {} at {}",
            self.contradicting_ids.len(),
            self.method.as_str(),
            self.identified_at.to_rfc3339(),
            self.method.as_str(),
            self.resolved_at.to_rfc3339()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contradiction_requires_at_least_two_items() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1")]);
        let result = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "test",
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .message()
                .contains("at least 2 contradicting items")
        );
    }

    #[test]
    fn contradiction_requires_non_empty_summary() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let result = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("summary"));
    }

    #[test]
    fn contradiction_identified_before_resolved() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let result = ContradictionResolution::new(
            Timestamp::from_secs(1_760_002_000),
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "test",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("identified_at"));
    }

    #[test]
    fn correction_method() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "E1 was found to be factually incorrect",
        )
        .unwrap();

        assert_eq!(resolution.method, ResolutionMethod::Correction);
        assert_eq!(resolution.method.as_str(), "correction");
    }

    #[test]
    fn reconciliation_method() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Reconciliation,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "Both claims are correct in different contexts",
        )
        .unwrap();

        assert_eq!(resolution.method, ResolutionMethod::Reconciliation);
        assert_eq!(resolution.method.as_str(), "reconciliation");
    }

    #[test]
    fn temporal_sequence_method() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::TemporalSequence,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "Evidence refers to different time periods",
        )
        .unwrap();

        assert_eq!(resolution.method, ResolutionMethod::TemporalSequence);
        assert_eq!(resolution.method.as_str(), "temporal_sequence");
    }

    #[test]
    fn interpretation_difference_method() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::InterpretationDifference,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "Different interpretations of the same fact",
        )
        .unwrap();

        assert_eq!(
            resolution.method,
            ResolutionMethod::InterpretationDifference
        );
        assert_eq!(resolution.method.as_str(), "interpretation_difference");
    }

    #[test]
    fn unresolved_method() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Unresolved,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "Contradiction remains unresolved",
        )
        .unwrap();

        assert_eq!(resolution.method, ResolutionMethod::Unresolved);
        assert_eq!(resolution.method.as_str(), "unresolved");
    }

    #[test]
    fn mutation_verify_requires_two_contradicting_items() {
        let ids = BTreeSet::from([EvidenceId::from_string("E1"), EvidenceId::from_string("E2")]);
        let resolution = ContradictionResolution::new(
            Timestamp::from_secs(1_760_000_000),
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            Timestamp::from_secs(1_760_001_000),
            "test",
        )
        .unwrap();

        assert_eq!(resolution.contradicting_ids.len(), 2);
    }
}
