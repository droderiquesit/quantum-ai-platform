#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

//! R12-AMBIENT-9001: an agent reads evidence through the store port, behind
//! its own capability, and the read is on the run record.

use qip_agents::capability::{Capability, CapabilitySet};
use qip_agents::evidence::{
    EvidenceDesk, EvidenceQuery, EvidenceRow, EvidenceStore, MAX_EVIDENCE_ROWS, evidence_desk,
    read_evidence,
};
use qip_agents::finding::{AgentBrief, AgentFinding};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext, AgentHost, RunStatus};
use qip_core::error::Result;
use qip_core::ids::AgentRunId;
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::time::{Duration, Timestamp};
use std::sync::Arc;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

#[derive(Debug)]
struct Warehouse {
    rows: Vec<EvidenceRow>,
}

impl EvidenceStore for Warehouse {
    fn read(&self, query: &EvidenceQuery, _as_of: Timestamp) -> Result<Vec<EvidenceRow>> {
        Ok(self.rows.iter().take(query.limit()).cloned().collect())
    }
}

fn row(id: &str, at: Timestamp) -> EvidenceRow {
    EvidenceRow {
        id: id.to_string(),
        knowable_at: at,
        body: "spread widened".to_string(),
    }
}

#[derive(Debug)]
struct Reader {
    manifest: AgentManifest,
    desk: EvidenceDesk,
}

impl Agent for Reader {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }
    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let rows = read_evidence(
            &self.desk,
            ctx,
            &EvidenceQuery::new("credit_spreads", "XSIM", 10)?,
        )?;
        Ok(AgentFinding::new(
            ctx.run_id().clone(),
            "reader",
            ctx.now(),
            brief.as_of,
            format!("{} evidence rows", rows.len()),
        ))
    }
}

fn run(grants: CapabilitySet, rows: Vec<EvidenceRow>) -> qip_agents::AgentRunRecord {
    let manifest = AgentManifest::research("reader", "Reader", "reads evidence", now())
        .with_competencies(vec!["test fixture scope".to_string()])
        .with_capabilities(grants);
    let agent = Reader {
        manifest,
        desk: evidence_desk(Arc::new(Warehouse { rows })),
    };
    AgentHost::new(5).run(
        &agent,
        &AgentBrief::new("does credit lead equity", now(), Duration::from_days(30)),
        now(),
        Lineage::root(CorrelationId::from_string("cor-9"), "test"),
        AgentRunId::from_string("run-1"),
    )
}

/// Mutation: change `Capability::ReadEvidence` in `evidence_desk` to another
/// read capability and the denied run below succeeds.
#[test]
fn evidence_is_read_only_through_its_own_capability_and_the_read_is_on_the_record() {
    let granted = run(
        CapabilitySet::of([Capability::ReadEvidence]),
        vec![row("r-1", now())],
    );
    assert!(granted.status.is_success(), "{:?}", granted.status);
    assert_eq!(granted.capabilities_used(), vec![Capability::ReadEvidence]);

    // The same agent with every other read capability and not this one.
    let others: Vec<Capability> = Capability::all()
        .into_iter()
        .filter(|c| c.is_read_only() && *c != Capability::ReadEvidence)
        .collect();
    assert!(!others.is_empty(), "premise: other read capabilities exist");
    let denied = run(CapabilitySet::of(others), vec![row("r-1", now())]);
    assert!(matches!(denied.status, RunStatus::Failed { .. }));
    assert_eq!(denied.denied_accesses().len(), 1);
    assert_eq!(
        denied.denied_accesses()[0].capability,
        Capability::ReadEvidence
    );
}

/// Mutation: delete the `knowable_at > now` refusal in `read_evidence` and
/// the future row is returned.
#[test]
fn a_store_that_returns_a_row_from_the_future_is_refused() {
    let future = now().saturating_add(Duration::from_days(1));
    let record = run(
        CapabilitySet::of([Capability::ReadEvidence]),
        vec![row("r-1", future)],
    );
    assert!(
        matches!(record.status, RunStatus::Failed { .. }),
        "{:?}",
        record.status
    );
}

#[test]
fn a_query_that_is_unbounded_or_could_carry_a_statement_is_refused_not_clamped() {
    assert!(EvidenceQuery::new("credit_spreads", "XSIM", MAX_EVIDENCE_ROWS).is_ok());
    for limit in [0, MAX_EVIDENCE_ROWS + 1] {
        assert!(EvidenceQuery::new("credit_spreads", "XSIM", limit).is_err());
    }
    for dataset in ["", "a b", "t; DROP TABLE x", "a`b"] {
        assert!(EvidenceQuery::new(dataset, "XSIM", 1).is_err(), "{dataset}");
    }
    assert!(EvidenceQuery::new("credit_spreads", "  ", 1).is_err());
}
