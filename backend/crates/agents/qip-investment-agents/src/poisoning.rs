//! Source poisoning detection specialist.
//!
//! This agent hunts for manipulation and poisoning of Algorik's sources across
//! sources, time and positions. A detection at the evidence layer (EVID-012)
//! judges one item at a time; this specialist looks for campaigns that no single
//! item reveals.
//!
//! Until 2026-10-11 `analyse` returned no data unconditionally, and the three
//! tests below it asserted on `PoisoningFinding`s they had written by hand —
//! a test named "a coordinated campaign is detected" that would have passed
//! with the detector deleted, because there was none. The detection now lives
//! in `qip_world_model::poisoning`, beside the evidence fabric it reads, and
//! this agent runs it over the fabric the world model holds and reports what
//! it found. The classes and the finding type are that module's, re-exported,
//! so there is one definition of what a poisoning finding is.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use std::sync::Arc;

pub use qip_world_model::poisoning::{
    PoisoningClass, PoisoningFinding, PoisoningThresholds, detect_poisoning,
};

/// Hunts for manipulation and source poisoning across the evidence fabric.
#[derive(Debug)]
pub struct PoisoningDetector {
    manifest: AgentManifest,
    desk: Arc<Desk>,
    thresholds: PoisoningThresholds,
}

impl PoisoningDetector {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self {
            manifest,
            desk,
            thresholds: PoisoningThresholds::default(),
        }
    }

    /// Judge against `thresholds` rather than the defaults.
    pub fn with_thresholds(mut self, thresholds: PoisoningThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl Agent for PoisoningDetector {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let world = self.desk.world.get(ctx)?;
        let fabric = world.evidence();
        if fabric.is_empty() {
            // An empty fabric is not a clean one: nothing was checked, and a
            // finding of "no poisoning" would read as a source given a pass.
            return Ok(no_data(
                ctx,
                brief.as_of,
                "the evidence fabric holds no claims, so there is nothing to scan for poisoning",
            ));
        }
        let findings = detect_poisoning(fabric, brief.as_of, &self.thresholds);
        let implicated: std::collections::BTreeSet<&str> = findings
            .iter()
            .flat_map(|finding| finding.sources.iter().map(String::as_str))
            .collect();
        let (direction, conviction, claim) = if findings.is_empty() {
            (
                Direction::Neutral,
                0.0,
                format!(
                    "{} claim(s) from {} source(s) scanned; no poisoning pattern found",
                    fabric.len(),
                    fabric.sources().len()
                ),
            )
        } else {
            (
                Direction::Negative,
                1.0,
                format!(
                    "{} poisoning finding(s) implicating {} source(s): {}",
                    findings.len(),
                    implicated.len(),
                    findings
                        .iter()
                        .map(|finding| finding.class.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        };
        let evidence = findings
            .iter()
            .map(|finding| {
                format!(
                    "poisoning:{}:{}: {}",
                    finding.class.as_str(),
                    finding
                        .sources
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("+"),
                    finding.evidence
                )
            })
            .collect::<Vec<_>>();
        let mut builder = FindingBuilder::new(ctx, brief.as_of, claim)
            .direction(direction, conviction)
            .fact(computed(
                ctx,
                "claims_scanned",
                fabric.len() as f64,
                "count",
                &["evidence_fabric"],
            ))
            .fact(computed(
                ctx,
                "poisoning_findings",
                findings.len() as f64,
                "count",
                &["evidence_fabric"],
            ))
            .caveats(vec![
                "sees only claims the evidence fabric holds, bounded by its capacity".to_string(),
            ]);
        if !evidence.is_empty() {
            builder = builder.evidence(evidence).falsifiers(vec![
                "the implicated sources' later claims win the contests they enter".to_string(),
            ]);
        }
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk::Desk;
    use crate::manifests;
    use qip_agents::finding::FindingStatus;
    use qip_agents::runtime::AgentHost;
    use qip_core::{AgentRunId, CorrelationId, Duration, Lineage, Timestamp};
    use qip_world_model::evidence::{META_ATTRIBUTE, META_SUBJECT, META_VALUE};
    use qip_world_model::{EvidenceRecord, WorldModel};

    fn now() -> Timestamp {
        Timestamp::from_secs(1_800_000_000)
    }

    fn claim(id: &str, source: &str, statement: &str) -> EvidenceRecord {
        EvidenceRecord::new(
            id.into(),
            source.into(),
            now(),
            now(),
            "test".into(),
            "commercial".into(),
            statement.into(),
            0.9,
        )
        .unwrap()
        .with_metadata(META_SUBJECT.into(), "ACME".into())
        .with_metadata(META_ATTRIBUTE.into(), "event".into())
        .with_metadata(META_VALUE.into(), "x".into())
    }

    fn run(world: WorldModel) -> AgentFinding {
        let mut desk = Desk::empty(now());
        desk.world = qip_agents::runtime::Gated::new(
            world,
            qip_agents::capability::Capability::ReadWorldModel,
            "world-model",
        );
        let agent = PoisoningDetector::new(manifests::poisoning_detector(now()), Arc::new(desk));
        let brief = AgentBrief::new(
            "are our sources being poisoned",
            now(),
            Duration::from_days(1),
        );
        let record = AgentHost::new(1).run(
            &agent,
            &brief,
            now(),
            Lineage::root(CorrelationId::from_string("cor-1"), "test"),
            AgentRunId::from_string("run-1"),
        );
        record.finding.expect("the detector produced a finding")
    }

    /// The test that used to assert on two hand-built findings. Now the
    /// agent runs the detector over a world model holding a real campaign.
    #[test]
    fn a_coordinated_manipulation_campaign_across_sources_is_detected() {
        let mut world = WorldModel::new();
        let hashes = ["a".repeat(64), "b".repeat(64), "c".repeat(64)];
        for (i, hash) in hashes.iter().enumerate() {
            world
                .absorb_evidence(
                    claim(
                        &format!("k{i}"),
                        &format!("https://site{i}.example"),
                        "Acme CEO arrested",
                    ),
                    Some(hash),
                    now(),
                )
                .unwrap();
        }
        assert_eq!(world.evidence().len(), 3, "premise: the campaign is held");
        let finding = run(world);
        assert_eq!(finding.direction, Direction::Negative, "{finding:?}");
        assert_eq!(finding.evidence.len(), 1, "{:?}", finding.evidence);
        assert!(
            finding.evidence[0].starts_with("poisoning:coordinated_manipulation:"),
            "{:?}",
            finding.evidence
        );
    }

    #[test]
    fn a_clean_episode_with_no_poisoning_injected_produces_no_findings() {
        let mut world = WorldModel::new();
        for (i, text) in ["rate 1.10", "euro at one ten", "fixing 1.10"]
            .iter()
            .enumerate()
        {
            world
                .absorb_evidence(
                    claim(&format!("ok{i}"), &format!("https://s{i}.example"), text),
                    None,
                    now(),
                )
                .unwrap();
        }
        assert_eq!(
            world.evidence().len(),
            3,
            "premise: the fabric is not empty"
        );
        let finding = run(world);
        assert_eq!(finding.direction, Direction::Neutral, "{finding:?}");
        assert!(finding.evidence.is_empty(), "{:?}", finding.evidence);
        assert_ne!(finding.status, FindingStatus::NoView);
    }

    #[test]
    fn an_empty_fabric_is_reported_as_no_data_rather_than_as_clean() {
        let finding = run(WorldModel::new());
        assert_eq!(finding.status, FindingStatus::NoView, "{finding:?}");
    }
}
