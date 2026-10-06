//! Contradiction resolution tracking.
//!
//! When evidence contradicts itself, the contradiction is a fact that needs
//! recording and resolution. A ContradictionResolution captures when a
//! contradiction was identified, which evidence items were involved, how it was
//! resolved, and what evidence backs the resolution.
//!
//! The distinction this module holds: contradicting evidence is not deleted
//! (EVID-007), but it is also not left unresolved. Every contradiction tracked
//! here can be audited: which items disagreed, what was discovered, and why one
//! side or both sides changed standing.

use qip_core::ids::EvidenceId;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How a contradiction was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    /// One side was found to be factually incorrect.
    Correction,
    /// Both sides were found to describe different aspects of the same fact.
    Reconciliation,
    /// The contradiction reflects genuine temporal change, not a disagreement.
    TemporalSequence,
    /// Sources disagree on an assumption or measurement, both potentially valid.
    InterpretationDifference,
    /// Not yet resolved; recorded but investigation ongoing.
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

/// A recorded contradiction and how it was handled.
///
/// Every contradiction in the evidence set carries this record so that:
/// 1. The contradiction can be audited (which items disagreed)
/// 2. The resolution can be checked (how did we decide between them)
/// 3. Future contradictions can be evaluated in light of past resolutions
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContradictionResolution {
    /// When the contradiction was identified (becomes knowable).
    pub identified_at: Timestamp,
    /// IDs of the evidence items that contradict each other.
    pub contradicting_ids: BTreeSet<EvidenceId>,
    /// How the contradiction was resolved.
    pub method: ResolutionMethod,
    /// Optional evidence supporting the resolution itself.
    pub resolution_evidence_ids: BTreeSet<EvidenceId>,
    /// Timestamp when the resolution was completed.
    pub resolved_at: Timestamp,
    /// Human-readable summary of the resolution.
    pub summary: String,
}

impl ContradictionResolution {
    /// Create a new contradiction resolution record.
    ///
    /// Requires:
    /// - At least two contradicting evidence IDs
    /// - A non-empty summary
    pub fn new(
        identified_at: Timestamp,
        contradicting_ids: BTreeSet<EvidenceId>,
        method: ResolutionMethod,
        resolution_evidence_ids: BTreeSet<EvidenceId>,
        resolved_at: Timestamp,
        summary: impl Into<String>,
    ) -> Result<Self, String> {
        let summary = summary.into();

        if contradicting_ids.len() < 2 {
            return Err("contradiction requires at least two contradicting evidence items".into());
        }

        if summary.trim().is_empty() {
            return Err("contradiction resolution summary cannot be empty".into());
        }

        if identified_at > resolved_at {
            return Err(
                "contradiction identified after resolution is not physically possible".into(),
            );
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

    /// Whether this contradiction is still unresolved.
    pub fn is_unresolved(&self) -> bool {
        self.method == ResolutionMethod::Unresolved
    }

    /// Whether the resolution was a correction (one side wrong).
    pub fn was_correction(&self) -> bool {
        self.method == ResolutionMethod::Correction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contradiction_requires_at_least_two_items() {
        let now = Timestamp::from_secs(1_760_000_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));

        let result = ContradictionResolution::new(
            now,
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            now,
            "one side was wrong",
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("at least two contradicting evidence items")
        );
    }

    #[test]
    fn a_contradiction_requires_a_nonempty_summary() {
        let now = Timestamp::from_secs(1_760_000_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let result = ContradictionResolution::new(
            now,
            ids,
            ResolutionMethod::Correction,
            BTreeSet::new(),
            now,
            "   ",
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be empty"));
    }

    #[test]
    fn a_contradiction_identified_after_resolution_is_refused() {
        let now = Timestamp::from_secs(1_760_000_000);
        let later = Timestamp::from_secs(1_760_001_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let result = ContradictionResolution::new(
            later,
            ids,
            ResolutionMethod::Reconciliation,
            BTreeSet::new(),
            now,
            "both describe different aspects",
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("identified after resolution"));
    }

    #[test]
    fn a_valid_contradiction_resolution_carries_all_fields() {
        let now = Timestamp::from_secs(1_760_000_000);
        let later = Timestamp::from_secs(1_760_001_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let resolution = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Reconciliation,
            BTreeSet::new(),
            later,
            "both describe different time periods",
        );

        assert!(resolution.is_ok());
        let r = resolution.unwrap();
        assert_eq!(r.contradicting_ids, ids);
        assert_eq!(r.method, ResolutionMethod::Reconciliation);
        assert!(!r.is_unresolved());
        assert!(!r.was_correction());
    }

    #[test]
    fn method_as_str_discriminates_all_variants() {
        let methods = [
            ResolutionMethod::Correction,
            ResolutionMethod::Reconciliation,
            ResolutionMethod::TemporalSequence,
            ResolutionMethod::InterpretationDifference,
            ResolutionMethod::Unresolved,
        ];

        let strings: Vec<&str> = methods.iter().map(|m| m.as_str()).collect();

        assert_eq!(
            strings.len(),
            strings
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "as_str must discriminate all variants"
        );

        for s in strings {
            assert!(!s.is_empty(), "as_str must not be empty");
        }
    }

    #[test]
    fn is_unresolved_reflects_method() {
        let now = Timestamp::from_secs(1_760_000_000);
        let later = Timestamp::from_secs(1_760_001_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let unresolved = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Unresolved,
            BTreeSet::new(),
            later,
            "under investigation",
        )
        .unwrap();

        assert!(unresolved.is_unresolved());

        let resolved = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Correction,
            BTreeSet::new(),
            later,
            "one side was wrong",
        )
        .unwrap();

        assert!(!resolved.is_unresolved());
    }

    #[test]
    fn was_correction_reflects_method() {
        let now = Timestamp::from_secs(1_760_000_000);
        let later = Timestamp::from_secs(1_760_001_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let correction = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Correction,
            BTreeSet::new(),
            later,
            "one side was factually wrong",
        )
        .unwrap();

        assert!(correction.was_correction());

        let reconciliation = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Reconciliation,
            BTreeSet::new(),
            later,
            "both describe different aspects",
        )
        .unwrap();

        assert!(!reconciliation.was_correction());
    }

    #[test]
    fn resolution_evidence_can_be_empty_or_populated() {
        let now = Timestamp::from_secs(1_760_000_000);
        let later = Timestamp::from_secs(1_760_001_000);
        let mut ids = BTreeSet::new();
        ids.insert(EvidenceId::from_string("E1"));
        ids.insert(EvidenceId::from_string("E2"));

        let without_resolution_evidence = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Unresolved,
            BTreeSet::new(),
            later,
            "recorded but not yet investigated",
        )
        .unwrap();

        assert!(
            without_resolution_evidence
                .resolution_evidence_ids
                .is_empty()
        );

        let mut resolution_ids = BTreeSet::new();
        resolution_ids.insert(EvidenceId::from_string("E3"));

        let with_resolution_evidence = ContradictionResolution::new(
            now,
            ids.clone(),
            ResolutionMethod::Correction,
            resolution_ids.clone(),
            later,
            "third source confirmed E1 was right",
        )
        .unwrap();

        assert_eq!(
            with_resolution_evidence.resolution_evidence_ids,
            resolution_ids
        );
    }
}
