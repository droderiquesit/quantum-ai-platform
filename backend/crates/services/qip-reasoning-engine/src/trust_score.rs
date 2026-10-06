//! Continuous source trust and evidence confidence rescoring.
//!
//! A source's trust score updates in the same processing pass as each evidence
//! outcome that corroborates or contradicts its claims. Trust is not rescored
//! in a periodic batch; every corroboration lifts it, every contradiction lowers
//! it, and each update records the evidence that caused it.
//!
//! Trust starts at the kind's reliability ceiling and moves within `[0.0, ceiling]`
//! based on outcomes. A source that claims supporting facts and those facts hold
//! gains trust up to its ceiling; a source that claims supporting facts and they
//! fail loses trust. Contradictions count double: they are both failures and
//! active disconfirmations.

use crate::Evidence;
use crate::contradiction::ContradictionResolution;
use qip_core::error::{Error, Result};
use qip_core::ids::EvidenceId;
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};

/// An update to a source's trust score.
///
/// Every change is attributed to the evidence that caused it, so the platform
/// can say why a source lost or gained confidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrustUpdate {
    /// When the update was applied.
    pub updated_at: Timestamp,
    /// The evidence whose outcome caused this update.
    pub evidence_id: EvidenceId,
    /// How much the score moved.
    pub delta: f64,
    /// Outcome that drove the update: corroboration or contradiction.
    pub outcome: TrustOutcome,
    /// The new score after this update.
    pub new_score: f64,
}

/// What kind of evidence outcome changed the trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustOutcome {
    /// The source's claim was corroborated by other evidence.
    Corroborated,
    /// The source's claim was contradicted by other evidence.
    Contradicted,
}

impl TrustOutcome {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Corroborated => "corroborated",
            Self::Contradicted => "contradicted",
        }
    }
}

/// Trust score for a source over time.
///
/// Trust starts at the ceiling of the source's evidence kind and moves within
/// `[0.0, ceiling]` as outcomes are observed. Each update is recorded with
/// the evidence that caused it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceTrust {
    /// The source identifier.
    pub source_name: String,
    /// The highest trust this source can reach, based on the kind of evidence
    /// it typically provides (e.g., filings are ceiling 0.98, rumours 0.25).
    pub ceiling: f64,
    /// Current trust score in [0.0, ceiling].
    pub current_score: f64,
    /// When the source's trust was first established.
    pub established_at: Timestamp,
    /// Number of times this source's evidence has been corroborated.
    pub corroborations: usize,
    /// Number of times this source's evidence has been contradicted.
    pub contradictions: usize,
    /// History of score changes, newest first.
    pub update_history: Vec<TrustUpdate>,
}

impl SourceTrust {
    /// Create a new source trust record starting at the ceiling.
    pub fn new(
        source_name: impl Into<String>,
        ceiling: f64,
        established_at: Timestamp,
    ) -> Result<Self> {
        let source_name = source_name.into();
        if source_name.is_empty() {
            return Err(Error::invalid("source_name must not be empty"));
        }
        if !(0.0..=1.0).contains(&ceiling) {
            return Err(Error::invalid("ceiling must be in [0.0, 1.0]"));
        }

        Ok(Self {
            source_name,
            ceiling,
            current_score: ceiling,
            established_at,
            corroborations: 0,
            contradictions: 0,
            update_history: Vec::new(),
        })
    }

    /// Record a corroboration of this source's evidence.
    ///
    /// The score moves up towards the ceiling by a fraction of the distance to it.
    /// A source already at ceiling moves nowhere. Returns the update that was recorded.
    pub fn record_corroboration(
        &mut self,
        evidence_id: EvidenceId,
        at: Timestamp,
    ) -> Result<TrustUpdate> {
        let gap_to_ceiling = self.ceiling - self.current_score;
        // Move 10% of the way towards the ceiling per corroboration.
        let delta = gap_to_ceiling * 0.10;
        let new_score = (self.current_score + delta).min(self.ceiling);

        let update = TrustUpdate {
            updated_at: at,
            evidence_id,
            delta,
            outcome: TrustOutcome::Corroborated,
            new_score,
        };

        self.current_score = new_score;
        self.corroborations += 1;
        self.update_history.insert(0, update.clone());

        Ok(update)
    }

    /// Record a contradiction of this source's evidence.
    ///
    /// The score moves down by a larger fraction than corroboration moves up
    /// (contradictions are weighted more heavily). A source at 0.0 stays there.
    /// Returns the update that was recorded.
    pub fn record_contradiction(
        &mut self,
        evidence_id: EvidenceId,
        at: Timestamp,
    ) -> Result<TrustUpdate> {
        // Move 20% of the distance to zero per contradiction (more weight than corroboration).
        let delta = -self.current_score * 0.20;
        let new_score = (self.current_score + delta).max(0.0);

        let update = TrustUpdate {
            updated_at: at,
            evidence_id,
            delta,
            outcome: TrustOutcome::Contradicted,
            new_score,
        };

        self.current_score = new_score;
        self.contradictions += 1;
        self.update_history.insert(0, update.clone());

        Ok(update)
    }

    /// Whether this source's trust is in good standing.
    pub fn is_trusted(&self) -> bool {
        self.current_score >= self.ceiling * 0.7
    }

    /// Whether this source has lost too much confidence to be used.
    pub fn is_discredited(&self) -> bool {
        self.current_score < self.ceiling * 0.2
    }

    /// Summary of this source's trust position.
    pub fn describe(&self) -> String {
        format!(
            "source '{}': trust {:.1}% (ceiling {:.1}%), {} corroborations, {} contradictions",
            self.source_name,
            self.current_score * 100.0,
            self.ceiling * 100.0,
            self.corroborations,
            self.contradictions
        )
    }
}

/// Tracks trust for all sources, updating continuously as evidence is evaluated.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrustRegistry {
    sources: std::collections::BTreeMap<String, SourceTrust>,
}

impl TrustRegistry {
    /// Create an empty trust registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new source or get an existing one.
    pub fn get_or_create(
        &mut self,
        source_name: impl Into<String>,
        ceiling: f64,
        at: Timestamp,
    ) -> Result<&mut SourceTrust> {
        let source_name = source_name.into();
        use std::collections::btree_map::Entry;
        match self.sources.entry(source_name.clone()) {
            Entry::Vacant(vacant) => {
                let trust = SourceTrust::new(source_name, ceiling, at)?;
                Ok(vacant.insert(trust))
            }
            Entry::Occupied(occupied) => Ok(occupied.into_mut()),
        }
    }

    /// Get a source's current trust without creating it.
    pub fn get(&self, source_name: &str) -> Option<&SourceTrust> {
        self.sources.get(source_name)
    }

    /// Get a mutable reference to a source's trust.
    pub fn get_mut(&mut self, source_name: &str) -> Option<&mut SourceTrust> {
        self.sources.get_mut(source_name)
    }

    /// List all registered sources and their current trust.
    pub fn all_sources(&self) -> Vec<&SourceTrust> {
        self.sources.values().collect()
    }

    /// Sources in good standing (trust >= 70% of ceiling).
    pub fn trusted_sources(&self) -> Vec<&SourceTrust> {
        self.sources.values().filter(|s| s.is_trusted()).collect()
    }

    /// Sources that have been discredited (trust < 20% of ceiling).
    pub fn discredited_sources(&self) -> Vec<&SourceTrust> {
        self.sources
            .values()
            .filter(|s| s.is_discredited())
            .collect()
    }

    /// Update trust based on evidence corroboration or contradiction.
    ///
    /// When evidence resolves (is corroborated or contradicted by other evidence),
    /// this records the outcome for every source that contributed to the resolved
    /// evidence. The update is immediate, not batched.
    pub fn update_from_resolution(
        &mut self,
        resolution: &ContradictionResolution,
        corroborating_evidence: &[Evidence],
        contradicting_evidence: &[Evidence],
    ) -> Result<Vec<TrustUpdate>> {
        let mut updates = Vec::new();

        // Contradictions count against sources more heavily.
        for evidence in contradicting_evidence {
            if let Some(source) = self.get_mut(&evidence.origin) {
                let update = source
                    .record_contradiction(evidence.evidence_id.clone(), resolution.resolved_at)?;
                updates.push(update);
            }
        }

        // Corroborations count for sources.
        for evidence in corroborating_evidence {
            if let Some(source) = self.get_mut(&evidence.origin) {
                let update = source
                    .record_corroboration(evidence.evidence_id.clone(), resolution.resolved_at)?;
                updates.push(update);
            }
        }

        Ok(updates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_trust_starts_at_ceiling() {
        let trust = SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        assert_eq!(trust.current_score, 0.9);
        assert_eq!(trust.corroborations, 0);
        assert_eq!(trust.contradictions, 0);
    }

    #[test]
    fn corroboration_moves_score_up() {
        let mut trust =
            SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        trust.current_score = 0.70; // Start below ceiling

        let evidence_id = EvidenceId::from_string("E1");
        let update = trust
            .record_corroboration(evidence_id.clone(), Timestamp::from_secs(1_760_001_000))
            .unwrap();

        // Should move 10% of gap to ceiling: (0.9 - 0.70) * 0.10 = 0.02
        assert_eq!(update.outcome, TrustOutcome::Corroborated);
        assert_eq!(update.new_score, 0.72);
        assert_eq!(trust.current_score, 0.72);
        assert_eq!(trust.corroborations, 1);
    }

    #[test]
    fn contradiction_moves_score_down() {
        let mut trust =
            SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        trust.current_score = 0.70;

        let evidence_id = EvidenceId::from_string("E2");
        let update = trust
            .record_contradiction(evidence_id.clone(), Timestamp::from_secs(1_760_001_000))
            .unwrap();

        // Should move 20% towards zero: 0.70 * 0.20 = 0.14 downward
        assert_eq!(update.outcome, TrustOutcome::Contradicted);
        assert!((update.new_score - 0.56).abs() < 1e-10);
        assert!((trust.current_score - 0.56).abs() < 1e-10);
        assert_eq!(trust.contradictions, 1);
    }

    #[test]
    fn contradiction_outweighs_corroboration() {
        let mut trust =
            SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        trust.current_score = 0.80;

        // One corroboration moves up by (0.9 - 0.80) * 0.10 = 0.01
        trust
            .record_corroboration(
                EvidenceId::from_string("E1"),
                Timestamp::from_secs(1_760_001_000),
            )
            .unwrap();
        assert_eq!(trust.current_score, 0.81);

        // One contradiction moves down by 0.81 * 0.20 = 0.162, net effect
        trust
            .record_contradiction(
                EvidenceId::from_string("E2"),
                Timestamp::from_secs(1_760_002_000),
            )
            .unwrap();
        assert!(trust.current_score < 0.81); // Score went down despite prior corroboration
    }

    #[test]
    fn is_trusted_checks_ceiling_ratio() {
        let trust = SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        assert!(trust.is_trusted()); // At ceiling

        let mut trust =
            SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        trust.current_score = 0.65; // 72% of ceiling
        assert!(trust.is_trusted());

        trust.current_score = 0.62; // 69% of ceiling, below threshold
        assert!(!trust.is_trusted());
    }

    #[test]
    fn is_discredited_checks_ceiling_ratio() {
        let mut trust =
            SourceTrust::new("reuters", 0.9, Timestamp::from_secs(1_760_000_000)).unwrap();
        trust.current_score = 0.20; // 22% of ceiling
        assert!(!trust.is_discredited());

        trust.current_score = 0.17; // 19% of ceiling, below threshold
        assert!(trust.is_discredited());
    }

    #[test]
    fn trust_registry_creates_sources() {
        let mut registry = TrustRegistry::new();
        let source = registry
            .get_or_create("reuters", 0.9, Timestamp::from_secs(1_760_000_000))
            .unwrap();
        assert_eq!(source.source_name, "reuters");

        let _source2 = registry
            .get_or_create("reuters", 0.9, Timestamp::from_secs(1_760_000_000))
            .unwrap();
        assert_eq!(registry.all_sources().len(), 1); // Not duplicated
    }

    #[test]
    fn trust_registry_filters_by_status() {
        let mut registry = TrustRegistry::new();
        let s1 = registry
            .get_or_create("trusted", 0.9, Timestamp::from_secs(1_760_000_000))
            .unwrap();
        s1.current_score = 0.75; // Trusted

        let s2 = registry
            .get_or_create("discredited", 0.9, Timestamp::from_secs(1_760_000_000))
            .unwrap();
        s2.current_score = 0.15; // Discredited

        assert_eq!(registry.trusted_sources().len(), 1);
        assert_eq!(registry.discredited_sources().len(), 1);
    }
}
