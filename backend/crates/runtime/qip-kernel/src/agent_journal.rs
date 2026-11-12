//! Every agent run the REASON stage dispatches, on the event log (ADR 0456,
//! R12-AMBIENT-9029).
//!
//! The failure this prevents: until this module, a panel run left the
//! process as two counters (`qip_agent_runs_total`,
//! `qip_permission_denials_total`) and a problem string. The organisation's
//! `AuditTrail` held the full [`AgentRunRecord`] — the manifest digest it ran
//! under, the brief, every access it attempted and whether it was granted,
//! and the finding it returned — but only in memory, so a restart erased the
//! one account of what each specialist said and what it reached for. A
//! hypothesis built from those findings could not be reproduced from the log,
//! and an agent that probed for a capability its manifest withholds left a
//! number on a dashboard rather than evidence anybody could read back.
//!
//! The record is journaled whole, not summarised: a summary is a second claim
//! about the run, and when it and the audit trail disagree the log should be
//! the one that is right.

use qip_agents::runtime::AgentRunRecord;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// Prefixes the idempotency key, so a reader of [`Topic::AgentRunCompleted`]
/// can tell an agent run from any other body that shares the topic.
pub const IDEMPOTENCY_PREFIX: &str = "agent-run:";

/// The origin the kernel journals agent runs under.
pub const ORIGIN: &str = "kernel/agent-run";

/// One agent run, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRunJournal {
    /// The kernel cycle that dispatched the brief.
    pub cycle: u64,
    /// The run, unabridged: status, spend, every access with its grant, the
    /// brief and the finding.
    pub record: AgentRunRecord,
}

impl EventBody for AgentRunJournal {
    const TOPIC: Topic = Topic::AgentRunCompleted;
    const SCHEMA_VERSION: u32 = 1;

    /// Keyed on the run id, which the organisation mints once per dispatch
    /// (`run-<agent>-<sequence>`), so a re-delivered record is suppressed
    /// rather than counted as a second run.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.record.run_id))
    }
}

#[cfg(test)]
mod tests {
    //! The row's bar, through the kernel: an agent invokes only what its
    //! grant names, and its outputs — the refusals included — are on the log
    //! a replay reads. Unit tests rather than `tests/` because seating an
    //! agent that overreaches needs `Platform::replace_organisation`, which
    //! is test-only on purpose.

    use super::*;
    use crate::config::PlatformConfig;
    use crate::cycle::Stage;
    use crate::platform::Platform;
    use qip_agents::capability::{Capability, CapabilitySet};
    use qip_agents::finding::{AgentBrief, AgentFinding};
    use qip_agents::manifest::AgentManifest;
    use qip_agents::runtime::{Agent, AgentContext, AgentHost};
    use qip_ai::language::DeterministicModel;
    use qip_core::decimal::Decimal;
    use qip_core::error::Result;
    use qip_core::ids::ObjectId;
    use qip_core::time::{Duration, Timestamp};
    use qip_events::{EventFilter, EventLog};
    use qip_financial::asset_class::{InstrumentType, Sector};
    use qip_financial::costs::LiquidityProfile;
    use qip_financial::object::FinancialObject;
    use qip_financial::quality::Provenance;
    use qip_financial::universe::Universe;
    use qip_investment_agents::Organisation;
    use qip_observability::Telemetry;
    use qip_risk::limits::{Limit, LimitKind, LimitSet};
    use std::sync::Arc;

    const ROGUE: &str = "rogue-simulator";

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    /// Quiet, then a nine percent drop on the last bar: the tape the
    /// symbolic-contradiction tests use, on which the panel convenes.
    fn tape() -> Vec<qip_market_ingestion::adapter::SensedRecord> {
        use qip_market::bar::{Bar, Interval};
        const TAPE: usize = 120;
        let mut price = 100.0_f64;
        (0..TAPE)
            .map(|i| {
                let noise = ((i as f64 * 0.754_877_666_2) % 1.0 - 0.5) * 0.008;
                let drop = if i == TAPE - 1 { -0.09 } else { 0.0 };
                let open = price;
                price *= 1.0 + noise + drop;
                let at = start().saturating_sub(Duration::from_days((TAPE - i) as i64));
                qip_market_ingestion::adapter::SensedRecord::Bar(Box::new(Bar {
                    object_id: ObjectId::from_string("obj-AAA"),
                    venue: "XNYS".to_string(),
                    interval: Interval::Day,
                    open_time: at,
                    open: Decimal::from_f64(open).expect("a price"),
                    high: Decimal::from_f64(open.max(price) * 1.002).expect("a price"),
                    low: Decimal::from_f64(open.min(price) * 0.998).expect("a price"),
                    close: Decimal::from_f64(price).expect("a price"),
                    volume: Decimal::from_int(1_000_000),
                    trade_count: 5_000,
                    vwap: Decimal::from_f64((open + price) / 2.0),
                    quality: qip_financial::quality::DataQuality::default(),
                }))
            })
            .collect()
    }

    fn platform() -> Platform {
        let config = PlatformConfig {
            initial_equity: Decimal::from_int(1_000_000),
            ..PlatformConfig::default()
        };
        let (context, _clock) = qip_core::Context::deterministic(start(), config.seed);
        let mut universe = Universe::new();
        universe
            .insert(
                FinancialObject::builder(
                    ObjectId::from_string("obj-AAA"),
                    "AAA",
                    InstrumentType::CommonStock,
                    LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
                )
                .venue("XNYS")
                .geography("US")
                .sector(Sector::InformationTechnology)
                .price(Decimal::from_int(100))
                .provenance(Provenance::synthetic("test", start()))
                .build(start())
                .expect("valid object"),
            )
            .expect("insertable");
        let limits = LimitSet::new("kernel-test").with(
            Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
                .with_rationale("gross exposure is capped at 2x equity"),
        );
        Platform::new(config, context, Telemetry::silent(), universe, limits)
            .expect("the platform assembles")
    }

    /// A research agent that reaches for the simulator, which its read-only
    /// grant does not name, and then reports honestly that it has no view.
    /// It swallows the refusal, so the run succeeds: what is under test is
    /// whether the attempt still reaches the log, not whether a failure does.
    #[derive(Debug)]
    struct RogueSimulator {
        manifest: AgentManifest,
    }

    impl Agent for RogueSimulator {
        fn manifest(&self) -> &AgentManifest {
            &self.manifest
        }

        fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
            let refused = ctx
                .authorise(Capability::RunSimulation, "simulator")
                .is_err();
            let summary = if refused {
                "the simulator was refused; no view"
            } else {
                "the simulator ran"
            };
            Ok(AgentFinding::no_view(
                ctx.run_id().clone(),
                ROGUE,
                ctx.now(),
                brief.as_of,
                summary,
            ))
        }
    }

    /// The shipped panel plus the rogue, on the shipped roster plus its
    /// manifest, so `Organisation::new` reviews it like any other.
    fn with_rogue(platform: &mut Platform) {
        let manifest = AgentManifest::research(ROGUE, "Rogue", "reaches past its grant", start())
            .with_competencies(vec!["test fixture scope".to_string()]);
        assert!(
            !manifest.capabilities.contains(Capability::RunSimulation),
            "premise: the rogue's grant does not name the simulator"
        );
        let mut roster = qip_investment_agents::roster(start());
        roster.add(manifest.clone());
        let mut agents = qip_investment_agents::chief::standard_agents(
            platform.desk().clone(),
            start(),
            Vec::new(),
            false,
        );
        agents.push(Arc::new(RogueSimulator { manifest }));
        let host = AgentHost::new(7).with_language_model(Arc::new(DeterministicModel::new()));
        let organisation =
            Organisation::new(roster, agents, host, start()).expect("the roster validates");
        platform.replace_organisation(organisation);
    }

    fn journaled(platform: &Platform) -> Vec<AgentRunJournal> {
        platform
            .replay_journal(&EventFilter::new().topic(Topic::AgentRunCompleted))
            .expect("the journal replays")
            .into_iter()
            .filter(|e| e.idempotency_key().contains(IDEMPOTENCY_PREFIX))
            .map(|e| e.decode::<AgentRunJournal>().expect("an agent run").body)
            .collect()
    }

    /// Every run the panel made is on the log, unabridged and in dispatch
    /// order, and the findings on the log are the findings the stage used.
    /// Mutation: make the loop in `stage_reason` skip `journal_record` and
    /// the log holds no run, so the equality fails.
    #[test]
    fn every_run_the_panel_makes_is_on_the_log_with_the_finding_it_returned() {
        let mut platform = platform();
        platform.observe(tape());
        platform.run_cycle(start());

        let audited = platform.organisation().audit().records().to_vec();
        assert!(
            !audited.is_empty(),
            "premise: the panel convened, so there are runs to journal"
        );
        assert!(
            audited.iter().any(|run| run.finding.is_some()),
            "premise: at least one run returned a finding"
        );
        let on_the_log: Vec<_> = journaled(&platform)
            .into_iter()
            .map(|entry| entry.record)
            .collect();
        assert_eq!(
            on_the_log, audited,
            "the log and the in-memory audit trail disagree about what the panel did"
        );
    }

    /// The row's verification: an agent invokes only what its grant names,
    /// and the attempt it was refused is on the log beside the output it
    /// returned. Mutations: drop the journal write (no rogue run on the log);
    /// make `AgentContext::authorise` grant everything (the access reads
    /// granted and the finding says the simulator ran).
    #[test]
    fn an_agent_reaching_past_its_grant_is_refused_and_the_refusal_is_on_the_log() {
        let mut platform = platform();
        with_rogue(&mut platform);
        platform.observe(tape());
        let report = platform.run_cycle(start());
        let reason = report.stage(Stage::Reason).expect("REASON reports");
        assert!(
            reason
                .problems
                .iter()
                .any(|p| p == &format!("{ROGUE} attempted something its manifest does not grant")),
            "premise: the stage saw the violation: {:?}",
            reason.problems
        );

        let rogue: Vec<_> = journaled(&platform)
            .into_iter()
            .filter(|entry| entry.record.agent_id == ROGUE)
            .collect();
        assert_eq!(rogue.len(), 1, "the rogue's run is not on the log");
        let record = &rogue[0].record;
        let denied = record.denied_accesses();
        assert_eq!(denied.len(), 1, "{:?}", record.accesses);
        assert_eq!(denied[0].capability, Capability::RunSimulation);
        assert_eq!(denied[0].facility, "simulator");
        assert!(
            !record
                .capabilities_used()
                .contains(&Capability::RunSimulation),
            "the run exercised a capability its grant does not name"
        );
        let finding = record
            .finding
            .as_ref()
            .expect("the rogue's output is on the log");
        assert_eq!(finding.claim, "the simulator was refused; no view");
        assert!(
            !CapabilitySet::read_only().contains(Capability::RunSimulation),
            "premise: the simulator is not a read-only capability"
        );
    }

    /// Fail-closed: a run the log refused is a run no hypothesis rests on.
    /// Mutation: delete the early return in `stage_reason` and REASON forms
    /// a hypothesis on findings the log never kept.
    #[test]
    fn a_panel_whose_runs_the_log_refused_forms_no_hypothesis() {
        let mut control = platform();
        control.observe(tape());
        let report = control.run_cycle(start());
        let detail = &report.stage(Stage::Reason).expect("REASON reports").detail;
        assert!(
            detail.contains("hypothesis "),
            "premise: with a working log this tape forms a hypothesis: {detail}"
        );

        let mut platform = platform();
        platform.observe(tape());
        let dir = std::env::temp_dir().join(format!(
            "qip-kernel-agent-journal-refused-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("events.jsonl");
        std::fs::write(&path, "").expect("an empty log file");
        platform.replace_event_log(EventLog::inspect(&path).expect("a read-only log"));

        let report = platform.run_cycle(start());
        let reason = report.stage(Stage::Reason).expect("REASON reports");
        assert!(
            reason
                .detail
                .contains("unjournaled, so no hypothesis was formed"),
            "{}",
            reason.detail
        );
        assert!(
            reason
                .problems
                .iter()
                .any(|p| p.starts_with("agent run ") && p.contains("could not be journaled")),
            "{:?}",
            reason.problems
        );
        assert!(journaled(&platform).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
