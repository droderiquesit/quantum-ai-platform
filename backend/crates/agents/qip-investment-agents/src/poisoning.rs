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
    //! The detector these tests used to describe does not exist. `analyse` is
    //! a placeholder that reads the market and returns a no-view finding;
    //! nothing constructs a `PoisoningFinding`, and no manifest in
    //! `manifests.rs` or agent in the roster runs it. The three tests that
    //! stood here built `PoisoningFinding` values by hand and asserted the
    //! fields they had just set — they passed with `analyse` deleted. These
    //! drive `analyse` through `AgentHost`, the way the organisation would,
    //! and assert what it actually does. **Production gap**: coordinated-
    //! campaign and contradicted-source detection (EVID-012 follow-on).
    use super::*;
    use crate::manifests;
    use qip_agents::finding::{AgentBrief, FindingStatus};
    use qip_agents::runtime::{AgentHost, RunStatus};
    use qip_core::ids::AgentRunId;
    use qip_core::lineage::{CorrelationId, Lineage};
    use qip_core::time::{Duration, Timestamp};

    /// The exact reason `analyse` gives. Compared by equality, not by a
    /// substring, so a placeholder that changed its wording is noticed.
    const REASON: &str =
        "source poisoning detection requires evidence scoring infrastructure (EVID-012)";

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn run(detector: &PoisoningDetector) -> qip_agents::runtime::AgentRunRecord {
        let brief = AgentBrief::new(
            "is any source feeding the desk a coordinated campaign",
            now(),
            Duration::from_days(1),
        );
        AgentHost::new(1).run(
            detector,
            &brief,
            now(),
            Lineage::root(CorrelationId::from_string("cor-poison"), "test"),
            AgentRunId::from_string("run-poison"),
        )
    }

    /// GAP PIN. Run through the host with a manifest that may read the
    /// market, the detector succeeds and says it has no view, naming the
    /// missing layer, with zero conviction. It never claims to have looked
    /// for a campaign and found none.
    ///
    /// Mutation: replace `no_data(...)` in `analyse` with
    /// `crate::support::out_of_scope(...)` — the status becomes `Deferred`
    /// and the `NoView` assertion fires. Changing the reason text fails the
    /// claim equality. Implementing real detection that returns a view also
    /// fails it, which is intended: rewrite this test to the requirement.
    #[test]
    fn the_poisoning_detector_reports_no_view_naming_its_missing_evidence_layer() {
        let detector = PoisoningDetector::new(
            manifests::macro_analyst(now()),
            Arc::new(Desk::empty(now())),
        );
        let record = run(&detector);
        assert!(
            matches!(record.status, RunStatus::Succeeded),
            "premise: the run was admitted and completed: {:?}",
            record.status
        );
        let finding = record.finding.expect("a completed run carries a finding");
        assert_eq!(finding.status, FindingStatus::NoView);
        assert_eq!(finding.claim, REASON);
        assert!(qip_core::testing::is_exactly_zero(
            finding.effective_conviction()
        ));
    }

    /// The placeholder's one live line is the market read, which goes through
    /// the desk's capability gate. A manifest without `ReadMarketData` must
    /// therefore not complete — proving the detector is a governed agent and
    /// not a constant, and that the gate is not bypassed.
    ///
    /// Mutation: delete `let _market = self.desk.market.get(ctx)?;` from
    /// `analyse` — the ungranted run then succeeds and the assertion fires.
    #[test]
    fn the_poisoning_detector_cannot_run_without_the_market_read_grant() {
        let granted = manifests::macro_analyst(now());
        // Premise: the granted manifest really holds the capability, so the
        // ungranted one below differs in exactly that respect.
        assert!(
            granted
                .capabilities
                .contains(qip_agents::capability::Capability::ReadMarketData)
        );
        let ungranted =
            granted
                .clone()
                .with_capabilities(qip_agents::capability::CapabilitySet::of([
                    qip_agents::capability::Capability::PublishHypothesis,
                ]));
        let detector = PoisoningDetector::new(ungranted, Arc::new(Desk::empty(now())));
        let record = run(&detector);
        assert!(
            !matches!(record.status, RunStatus::Succeeded),
            "a detector without the market grant completed: {:?}",
            record.status
        );
    }
}
