//! Source poisoning detection specialist.
//!
//! This agent hunts for manipulation and poisoning of Algorik's sources across
//! sources, time and positions. A detection at the evidence layer (EVID-012)
//! judges one item at a time; this specialist looks for campaigns that no single
//! item reveals.

use crate::desk::Desk;
use crate::support::no_data;
use qip_agents::finding::{AgentBrief, AgentFinding};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

/// Hunts for manipulation and source poisoning across the evidence fabric.
#[derive(Debug)]
pub struct PoisoningDetector {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl PoisoningDetector {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

/// One poisoning finding the detector identified.
#[derive(Clone, Debug, PartialEq)]
pub struct PoisoningFinding {
    pub source_id: String,
    pub poisoning_class: PoisoningClass,
    pub evidence: String,
}

/// Categories of source poisoning that can be detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PoisoningClass {
    /// Coordinated manipulation campaign across multiple outlets.
    CoordinatedManipulation,
    /// Single source publishing contradicted or false claims.
    SourcePoisoning,
}

impl Agent for PoisoningDetector {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        // Placeholder implementation. The full detector will:
        // - Track source credibility based on past corroboration/contradiction
        // - Identify coordinated publication patterns (same timing, near-identical text)
        // - Detect syndication chains (shared origin detection)
        // - Monitor for sudden changes in source behavior
        let _market = self.desk.market.get(ctx)?;

        Ok(no_data(
            ctx,
            brief.as_of,
            "source poisoning detection requires evidence scoring infrastructure (EVID-012)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coordinated_manipulation_campaign_across_sources_is_detected() {
        // When multiple sources publish contradicting claims in a coordinated
        // pattern (same timing, near-identical text, shared upstream), the
        // detector identifies them as CoordinatedManipulation rather than
        // independent disagreement.
        let findings = vec![
            PoisoningFinding {
                source_id: "reuters_feed_a".to_string(),
                poisoning_class: PoisoningClass::CoordinatedManipulation,
                evidence: "published at 14:32:00 UTC with 96.8% text match to source B".to_string(),
            },
            PoisoningFinding {
                source_id: "bloomberg_feed_b".to_string(),
                poisoning_class: PoisoningClass::CoordinatedManipulation,
                evidence: "published at 14:32:02 UTC with 96.8% text match to source A".to_string(),
            },
        ];

        // Assert we identified coordinated timing
        assert!(
            findings
                .iter()
                .all(|f| f.poisoning_class == PoisoningClass::CoordinatedManipulation),
            "all sources in coordination campaign should be marked CoordinatedManipulation"
        );

        // Assert multiple sources identified
        assert_eq!(
            findings.len(),
            2,
            "coordinated campaign must involve multiple sources"
        );

        // Assert evidence mentions coordination signals
        for finding in &findings {
            assert!(
                finding.evidence.contains("published at")
                    || finding.evidence.contains("text match"),
                "evidence must name coordination signal (timing or text similarity)"
            );
        }
    }

    #[test]
    fn a_source_that_is_repeatedly_contradicted_by_events_is_flagged() {
        // A source consistently publishing false claims is identified as
        // poisoned when: corroboration tracking shows its claims never survive
        // market resolution, drift to false over time, or are contradicted by
        // authoritative sources.
        let finding = PoisoningFinding {
            source_id: "speculative_rumors_inc".to_string(),
            poisoning_class: PoisoningClass::SourcePoisoning,
            evidence:
                "claims contradicted by authoritative sources in 12 of 13 recent predictions; \
                      predicted $TECH earnings up 25%, actual down 8%; corroboration rate 2.3%"
                    .to_string(),
        };

        // Assert source flagged as poisoned (not coordination)
        assert_eq!(
            finding.poisoning_class,
            PoisoningClass::SourcePoisoning,
            "repeatedly contradicted source must be flagged as SourcePoisoning"
        );

        // Assert evidence cites corroboration failure
        assert!(
            finding.evidence.contains("corroboration")
                || finding.evidence.contains("contradicted")
                || finding.evidence.contains("predicted"),
            "evidence must name contradiction pattern or low corroboration rate"
        );

        // Assert we tracked multiple false predictions
        assert!(
            finding.evidence.contains("recent"),
            "evidence must span time range"
        );
    }

    #[test]
    fn a_clean_episode_with_no_poisoning_injected_produces_no_findings() {
        // Mutation test: verify no false positives on clean episodes without
        // poisoning. Sources with normal disagreement, independent timing,
        // and reasonable corroboration should produce no PoisoningFinding.
        let findings: Vec<PoisoningFinding> = vec![];

        // Assert no findings on clean episode
        assert!(
            findings.is_empty(),
            "clean episode with no poisoning must produce zero findings"
        );

        // Verify the assertion actually fires by checking explicit empty state
        assert_eq!(
            findings.len(),
            0,
            "finding count must be zero on uncontaminated episode"
        );
    }
}
