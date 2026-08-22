//! Tests for the agent runtime.
//!
//! Most of these assert a *prohibition*. The value of this crate is that
//! certain things cannot happen, and a prohibition only holds if something
//! fails when you try to violate it.

use qip_agents::budget::{Budget, BudgetLedger, BudgetLine};
use qip_agents::capability::{Capability, CapabilitySet};
use qip_agents::finding::{
    AgentBrief, AgentFinding, Direction, FindingStatus, NumericFact, NumericProvenance,
};
use qip_agents::governance::{Roster, Severity};
use qip_agents::manifest::{AgentManifest, AgentRole, EscalationPolicy};
use qip_agents::memory::{Episode, EpisodeOutcome, Lesson, PromotionPolicy, ResearchMemory};
use qip_agents::runtime::{Agent, AgentContext, AgentHost, Gated, RunStatus};
use qip_ai::language::{DeterministicModel, ModelRequest};
use qip_core::error::Result;
use qip_core::ids::{AgentRunId, LessonId};
use qip_core::lineage::{CorrelationId, Lineage};
use qip_core::testing::is_exactly_zero;
use qip_core::time::{Duration, Timestamp};
use std::sync::Arc;

fn now() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn run_id(n: u64) -> AgentRunId {
    AgentRunId::from_string(format!("run-{n}"))
}

fn lineage() -> Lineage {
    Lineage::root(CorrelationId::from_string("cor-7"), "test")
}

fn brief() -> AgentBrief {
    AgentBrief::new(
        "does the widening credit spread matter",
        now(),
        Duration::from_days(30),
    )
}

// --- capabilities -----------------------------------------------------------

#[test]
fn a_capability_set_refuses_what_it_does_not_grant() {
    let set = CapabilitySet::of([Capability::ReadMarketData, Capability::ReadNews]);
    assert!(set.require(Capability::ReadMarketData, "macro").is_ok());
    let denied = set.require(Capability::SubmitOrder, "macro").unwrap_err();
    assert!(denied.message().contains("submit_order"));
    assert!(
        denied.message().contains("macro"),
        "the denial must name the agent to fix"
    );
}

#[test]
fn read_only_is_actually_read_only() {
    let set = CapabilitySet::read_only();
    assert!(!set.is_empty());
    for capability in set.iter() {
        assert_eq!(capability.sensitivity(), 0, "{capability} is not read-only");
        assert!(!capability.touches_market());
    }
}

#[test]
fn capability_round_trips_through_its_string_form() {
    for capability in Capability::all() {
        assert_eq!(Capability::parse(capability.as_str()).unwrap(), capability);
    }
    assert!(Capability::parse("become_root").is_err());
}

#[test]
fn a_delegated_grant_cannot_exceed_its_delegator() {
    let delegator = CapabilitySet::of([Capability::ReadMarketData, Capability::ReadNews]);
    let ok = CapabilitySet::of([Capability::ReadNews]);
    let excessive = CapabilitySet::of([Capability::ReadNews, Capability::ProposeTrade]);
    assert!(ok.is_subset_of(&delegator));
    assert!(!excessive.is_subset_of(&delegator));
    assert_eq!(
        excessive.excess_over(&delegator),
        vec![Capability::ProposeTrade]
    );
}

// --- manifests: the separation of duties ------------------------------------

#[test]
fn a_research_agent_cannot_be_granted_order_submission() {
    let manifest = AgentManifest::research("macro", "Macro", "reads the world", now())
        .with_capability(Capability::SubmitOrder);
    let error = manifest.validate().unwrap_err();
    assert!(
        error.message().contains("may not hold submit_order"),
        "unexpected message: {}",
        error.message()
    );
}

#[test]
fn proposing_and_approving_cannot_be_the_same_agent() {
    let manifest = AgentManifest::research("pm", "PM", "builds the book", now())
        .with_role(AgentRole::Construction)
        .with_capability(Capability::ProposeTrade)
        .with_capability(Capability::ApproveProposal);
    let error = manifest.validate().unwrap_err();
    assert!(error.message().contains("proposing and approving"));
}

#[test]
fn a_proposer_cannot_hold_the_veto_that_checks_it() {
    let manifest = AgentManifest::research("pm", "PM", "builds the book", now())
        .with_role(AgentRole::Construction)
        .with_capability(Capability::ProposeTrade)
        .with_capability(Capability::VetoProposal);
    assert!(manifest.validate().unwrap_err().message().contains("veto"));
}

#[test]
fn no_agent_may_change_its_own_autonomy_level() {
    // The most dangerous capability in the platform: an agent that can raise
    // the autonomy level can grant itself live trading.
    for role in [
        AgentRole::Research,
        AgentRole::Control,
        AgentRole::Execution,
        AgentRole::Coordination,
    ] {
        let manifest = AgentManifest::research("any", "Any", "does a thing", now())
            .with_role(role)
            .with_capabilities(CapabilitySet::of([Capability::ChangeAutonomyLevel]));
        let error = manifest.validate().unwrap_err();
        assert!(
            error.message().contains("autonomy level"),
            "role {role} was allowed to change the autonomy level"
        );
    }
}

#[test]
fn only_a_control_agent_may_trigger_the_kill_switch() {
    let research = AgentManifest::research("macro", "Macro", "reads the world", now())
        .with_capability(Capability::TriggerKillSwitch);
    assert!(research.validate().is_err());

    let control = AgentManifest::research("risk", "Risk", "says no", now())
        .with_role(AgentRole::Control)
        .with_capabilities(
            CapabilitySet::read_only()
                .with(Capability::VetoProposal)
                .with(Capability::TriggerKillSwitch),
        );
    assert!(control.validate().is_ok());
}

#[test]
fn an_execution_agent_may_not_originate_the_theses_it_trades() {
    let manifest = AgentManifest::research("exec", "Execution", "works orders", now())
        .with_role(AgentRole::Execution)
        .with_capabilities(
            CapabilitySet::read_only()
                .with(Capability::SubmitOrder)
                .with(Capability::PublishHypothesis),
        );
    assert!(
        manifest
            .validate()
            .unwrap_err()
            .message()
            .contains("must not originate")
    );
}

#[test]
fn an_adversarial_agent_challenges_rather_than_publishes() {
    let manifest = AgentManifest::research("red", "Red Team", "attacks theses", now())
        .with_role(AgentRole::Adversarial);
    // The research constructor grants PublishHypothesis; the adversarial role
    // must reject it.
    assert!(
        manifest
            .validate()
            .unwrap_err()
            .message()
            .contains("does not publish")
    );
}

#[test]
fn an_agent_without_a_purpose_or_an_owner_does_not_validate() {
    let mut manifest = AgentManifest::research("macro", "Macro", "reads the world", now());
    manifest.purpose = "  ".to_string();
    assert!(manifest.validate().is_err());

    let mut manifest = AgentManifest::research("macro", "Macro", "reads the world", now());
    manifest.owner = String::new();
    assert!(manifest.validate().is_err());
}

#[test]
fn an_expired_authorisation_is_not_authorisation() {
    let manifest = AgentManifest::research("macro", "Macro", "reads the world", now());
    let within = now().saturating_add(Duration::from_days(80));
    let beyond = now().saturating_add(Duration::from_days(100));
    assert!(manifest.authorisation(within).is_ok());
    let error = manifest.authorisation(beyond).unwrap_err();
    assert!(error.message().contains("expired"));
}

#[test]
fn delegation_must_name_a_delegate_and_not_be_self_referential() {
    let mut manifest = AgentManifest::research("macro", "Macro", "reads the world", now());
    manifest.escalation = EscalationPolicy::Delegate;
    assert!(manifest.validate().is_err());

    let manifest =
        AgentManifest::research("macro", "Macro", "reads the world", now()).escalating_to("macro");
    assert!(
        manifest
            .validate()
            .unwrap_err()
            .message()
            .contains("itself")
    );
}

// --- budgets ----------------------------------------------------------------

#[test]
fn the_ledger_refuses_the_call_that_would_exceed_the_budget() {
    let budget = Budget::research_default();
    let mut ledger = BudgetLedger::new(budget, "macro");
    for _ in 0..budget.tool_calls {
        ledger.charge_tool_call().unwrap();
    }
    let error = ledger.charge_tool_call().unwrap_err();
    assert!(error.message().contains("tool_calls"));
    assert_eq!(ledger.exhausted(), Some(BudgetLine::ToolCalls));
    assert!(!ledger.has_headroom());
}

#[test]
fn tokens_are_charged_before_the_call_not_after() {
    // The budget allows 12 calls but only 60k tokens; a 50k-token request
    // followed by another must fail on the second request, not after it.
    let mut ledger = BudgetLedger::new(Budget::research_default(), "macro");
    ledger.charge_language_model(50_000, 100).unwrap();
    let error = ledger.charge_language_model(50_000, 100).unwrap_err();
    assert!(error.message().contains("tokens"));
    assert_eq!(
        ledger.spend().tokens,
        50_000,
        "the refused call must not be billed"
    );
}

#[test]
fn a_fast_path_budget_forbids_language_models_entirely() {
    let budget = Budget::fast_path();
    assert_eq!(budget.language_model_calls, 0);
    let mut ledger = BudgetLedger::new(budget, "microstructure");
    assert!(ledger.charge_language_model(1, 0).is_err());
    // The fast path must still be able to read the book.
    assert!(ledger.charge_tool_call().is_ok());
}

#[test]
fn utilisation_reports_the_tightest_line() {
    let budget = Budget::research_default();
    let mut ledger = BudgetLedger::new(budget, "macro");
    ledger.charge_language_model(30_000, 0).unwrap();
    // 30k of 60k tokens is 0.5; one of 12 calls is 0.083. The tighter is 0.5.
    assert!((ledger.utilisation() - 0.5).abs() < 1e-9);
}

#[test]
fn an_overrun_is_an_error_rather_than_a_silent_truncation() {
    let mut ledger = BudgetLedger::new(Budget::research_default(), "macro");
    assert!(ledger.observe_elapsed(Duration::from_mins(4)).is_ok());
    let error = ledger.observe_elapsed(Duration::from_mins(6)).unwrap_err();
    assert!(error.message().contains("wall_time"));
}

// --- findings and numeric provenance ----------------------------------------

#[test]
fn every_number_an_agent_reports_carries_a_provenance() {
    let observed = NumericFact::observed("spread_bps", 42.0, "bps", "synthetic", now(), "q-1");
    assert!(observed.validate().is_ok());
    let computed = NumericFact::computed(
        "expected_move_bps",
        18.0,
        "bps",
        "credit-model-v3",
        vec!["spread_bps".into(), "duration".into()],
    );
    assert!(computed.validate().is_ok());
    assert!(matches!(
        computed.provenance,
        NumericProvenance::Computed { .. }
    ));
}

#[test]
fn a_number_computed_from_nothing_is_rejected() {
    // The shape a hallucinated number would have to take: a claim of
    // computation with no inputs to compute from.
    let fact = NumericFact::computed("target_price", 210.0, "usd", "analysis", Vec::new());
    assert!(
        fact.validate()
            .unwrap_err()
            .message()
            .contains("from nothing")
    );
}

#[test]
fn an_unlabelled_or_unitless_number_is_rejected() {
    let mut fact = NumericFact::observed("spread", 42.0, "bps", "s", now(), "r");
    fact.unit = String::new();
    assert!(fact.validate().is_err());
    fact.unit = "bps".into();
    fact.label = "  ".into();
    assert!(fact.validate().is_err());
    fact.label = "spread".into();
    fact.value = f64::NAN;
    assert!(fact.validate().is_err());
}

#[test]
fn a_directional_claim_without_a_falsifier_is_not_a_finding() {
    let finding = AgentFinding::new(run_id(1), "macro", now(), now(), "rates will fall")
        .with_direction(Direction::Positive, 0.7)
        .with_evidence(vec!["doc-1".into()]);
    let error = finding.validate().unwrap_err();
    assert!(error.message().contains("falsifier"));
}

#[test]
fn a_conviction_with_no_evidence_is_rejected() {
    let finding = AgentFinding::new(run_id(1), "macro", now(), now(), "rates will fall")
        .with_direction(Direction::Negative, 0.9)
        .with_falsifiers(vec!["the next CPI print comes in hot".into()]);
    assert!(
        finding
            .validate()
            .unwrap_err()
            .message()
            .contains("no evidence")
    );
}

#[test]
fn a_finding_cannot_be_produced_before_the_time_it_reasons_about() {
    let finding = AgentFinding::new(
        run_id(1),
        "macro",
        now(),
        now().saturating_add(Duration::from_days(1)),
        "rates will fall",
    );
    assert!(
        finding
            .validate()
            .unwrap_err()
            .message()
            .contains("in its future")
    );
}

#[test]
fn a_partial_finding_must_say_what_was_missing() {
    let finding = AgentFinding::new(run_id(1), "macro", now(), now(), "probably fine").partial();
    assert!(
        finding
            .validate()
            .unwrap_err()
            .message()
            .contains("what was missing")
    );

    let honest = AgentFinding::new(run_id(1), "macro", now(), now(), "probably fine")
        .with_missing_inputs(vec!["Q3 filings".into()]);
    assert_eq!(honest.status, FindingStatus::Partial);
    assert!(honest.validate().is_ok());
}

#[test]
fn a_deferred_finding_carries_no_weight() {
    // Ignorance must not be averaged in as a neutral vote — that would let an
    // agent with nothing to say dilute one with a strong view.
    let deferred = AgentFinding::deferred(run_id(1), "macro", now(), now(), "out of scope");
    assert!(!deferred.status.is_informative());
    assert!(is_exactly_zero(deferred.effective_conviction()));

    let partial = AgentFinding::new(run_id(2), "macro", now(), now(), "leaning positive")
        .with_direction(Direction::Positive, 1.0)
        .with_missing_inputs(vec!["Q3 filings".into()]);
    assert!((partial.effective_conviction() - 0.6).abs() < 1e-12);
}

#[test]
fn looking_and_finding_nothing_is_a_real_answer() {
    let finding = AgentFinding::no_view(run_id(1), "macro", now(), now(), "no macro angle here");
    assert_eq!(finding.status, FindingStatus::NoView);
    assert!(finding.status.is_informative());
    assert!(finding.validate().is_ok());
}

// --- the runtime: gated facilities ------------------------------------------

#[derive(Debug)]
struct MarketFeed {
    last: f64,
}

/// An agent that tries to read something it was not granted.
#[derive(Debug)]
struct Overreaching {
    manifest: AgentManifest,
    portfolio: Gated<MarketFeed>,
}

impl Agent for Overreaching {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }
    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        // Deliberately does not check its own permissions first.
        let feed = self.portfolio.get(ctx)?;
        Ok(AgentFinding::new(
            ctx.run_id().clone(),
            "overreach",
            ctx.now(),
            brief.as_of,
            "read it",
        )
        .with_fact(NumericFact::observed(
            "last",
            feed.last,
            "usd",
            "feed",
            brief.as_of,
            "r-1",
        )))
    }
}

#[test]
fn a_gated_facility_cannot_be_reached_without_its_capability() {
    let manifest = AgentManifest::research("overreach", "Overreach", "tries", now())
        .with_capabilities(CapabilitySet::of([Capability::ReadMarketData]));
    let agent = Overreaching {
        manifest,
        portfolio: Gated::new(
            MarketFeed { last: 100.0 },
            Capability::ReadPortfolio,
            "portfolio",
        ),
    };
    let host = AgentHost::new(11);
    let record = host.run(&agent, &brief(), now(), lineage(), run_id(1));

    assert!(matches!(record.status, RunStatus::Failed { .. }));
    assert!(record.finding.is_none());
    // The attempt is recorded even though it was refused.
    let denied = record.denied_accesses();
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].capability, Capability::ReadPortfolio);
    assert_eq!(denied[0].facility, "portfolio");
}

#[test]
fn a_granted_facility_is_reachable_and_charged() {
    let manifest = AgentManifest::research("reader", "Reader", "reads", now()).with_capabilities(
        CapabilitySet::of([Capability::ReadPortfolio, Capability::PublishHypothesis]),
    );
    let agent = Overreaching {
        manifest,
        portfolio: Gated::new(
            MarketFeed { last: 100.0 },
            Capability::ReadPortfolio,
            "portfolio",
        ),
    };
    let host = AgentHost::new(11);
    let record = host.run(&agent, &brief(), now(), lineage(), run_id(2));

    assert!(record.status.is_success(), "{:?}", record.status);
    assert_eq!(record.spend.tool_calls, 1);
    assert_eq!(record.capabilities_used(), vec![Capability::ReadPortfolio]);
    assert!(record.denied_accesses().is_empty());
    let finding = record.finding.unwrap();
    assert!((finding.fact("last").unwrap().value - 100.0).abs() < 1e-12);
}

// --- the runtime: refusals and records --------------------------------------

#[derive(Debug)]
struct Trivial {
    manifest: AgentManifest,
}

impl Agent for Trivial {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }
    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        Ok(AgentFinding::no_view(
            ctx.run_id().clone(),
            "trivial",
            ctx.now(),
            brief.as_of,
            "nothing to add",
        ))
    }
}

#[test]
fn an_expired_agent_is_refused_before_it_runs() {
    let agent = Trivial {
        manifest: AgentManifest::research("trivial", "Trivial", "does little", now()),
    };
    let host = AgentHost::new(3);
    let late = now().saturating_add(Duration::from_days(120));
    let record = host.run(&agent, &brief(), late, lineage(), run_id(3));
    match record.status {
        RunStatus::Refused { reason } => assert!(reason.contains("expired")),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(record.accesses.is_empty(), "a refused run does nothing");
}

#[test]
fn an_agent_cannot_be_briefed_on_a_time_it_has_not_reached() {
    let agent = Trivial {
        manifest: AgentManifest::research("trivial", "Trivial", "does little", now()),
    };
    let future = AgentBrief::new(
        "what happens tomorrow",
        now().saturating_add(Duration::from_days(1)),
        Duration::from_days(30),
    );
    let record = AgentHost::new(3).run(&agent, &future, now(), lineage(), run_id(4));
    match record.status {
        RunStatus::Refused { reason } => assert!(reason.contains("after the run start")),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_run_record_pins_the_manifest_it_ran_under() {
    let agent = Trivial {
        manifest: AgentManifest::research("trivial", "Trivial", "does little", now()),
    };
    let host = AgentHost::new(3);
    let first = host.run(&agent, &brief(), now(), lineage(), run_id(5));

    let widened = Trivial {
        manifest: AgentManifest::research("trivial", "Trivial", "does little", now())
            .with_capability(Capability::RunSimulation),
    };
    let second = host.run(&widened, &brief(), now(), lineage(), run_id(6));

    assert_ne!(
        first.manifest_digest, second.manifest_digest,
        "widening a grant must change the digest, or a past run's permissions could be rewritten"
    );
}

#[derive(Debug)]
struct Chatty {
    manifest: AgentManifest,
}

impl Agent for Chatty {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }
    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        for _ in 0..100 {
            let request = ModelRequest::new("you summarise", "summarise the situation");
            ctx.complete(&request)?;
        }
        Ok(AgentFinding::no_view(
            ctx.run_id().clone(),
            "chatty",
            ctx.now(),
            brief.as_of,
            "done",
        ))
    }
}

#[test]
fn an_agent_that_runs_out_of_budget_escalates_rather_than_lying() {
    let manifest = AgentManifest::research("chatty", "Chatty", "talks a lot", now())
        .with_capability(Capability::CallLanguageModel);
    let agent = Chatty { manifest };
    let model = Arc::new(DeterministicModel::new());
    let host = AgentHost::new(5).with_language_model(model);
    let record = host.run(&agent, &brief(), now(), lineage(), run_id(7));

    match &record.status {
        RunStatus::Escalated { policy, .. } => {
            assert_eq!(*policy, EscalationPolicy::ReturnPartial);
        }
        other => panic!("expected escalation, got {other:?}"),
    }
    assert!(
        record.finding.is_none(),
        "a truncated run must not present a finding"
    );
    assert_eq!(
        record.spend.language_model_calls, 12,
        "the run must stop exactly at the allowance, not before or after"
    );
    assert!(record.utilisation >= 1.0);
}

// --- roster governance ------------------------------------------------------

fn viable_roster() -> Roster {
    let analyst = AgentManifest::research("macro", "Macro", "reads the world", now())
        .with_owner("investment-research");
    let red = AgentManifest::research("red-team", "Red Team", "attacks conclusions", now())
        .with_role(AgentRole::Adversarial)
        .with_capabilities(CapabilitySet::read_only().with(Capability::ChallengeHypothesis))
        .with_owner("independent-review");
    let pm = AgentManifest::research("pm", "Portfolio Manager", "builds the book", now())
        .with_role(AgentRole::Construction)
        .with_capabilities(CapabilitySet::read_only().with(Capability::ProposeTrade))
        .with_owner("portfolio-management");
    let risk = AgentManifest::research("risk", "Risk", "says no", now())
        .with_role(AgentRole::Control)
        .with_capabilities(
            CapabilitySet::read_only()
                .with(Capability::VetoProposal)
                .with(Capability::TriggerKillSwitch),
        )
        .with_owner("risk-management");
    let exec = AgentManifest::research("execution", "Execution", "works orders", now())
        .with_role(AgentRole::Execution)
        .with_capabilities(
            CapabilitySet::read_only()
                .with(Capability::SubmitOrder)
                .with(Capability::CancelOrder),
        )
        .with_owner("trading");
    Roster::from_manifests(vec![analyst, red, pm, risk, exec])
}

#[test]
fn a_viable_roster_validates() {
    let roster = viable_roster();
    let findings = roster.validate(now()).unwrap();
    assert!(
        findings.iter().all(|f| f.severity == Severity::Warning),
        "unexpected errors: {findings:?}"
    );
}

#[test]
fn a_roster_that_proposes_trades_with_nobody_holding_a_veto_is_refused() {
    let mut manifests: Vec<AgentManifest> = viable_roster().iter().cloned().collect();
    manifests.retain(|m| m.id != "risk");
    let roster = Roster::from_manifests(manifests);
    let error = roster.validate(now()).unwrap_err();
    assert!(error.message().contains("veto"));
}

#[test]
fn a_roster_with_no_adversarial_function_is_refused() {
    let mut manifests: Vec<AgentManifest> = viable_roster().iter().cloned().collect();
    manifests.retain(|m| m.id != "red-team");
    let error = Roster::from_manifests(manifests)
        .validate(now())
        .unwrap_err();
    assert!(error.message().contains("attacking"));
}

#[test]
fn risk_and_trading_cannot_be_owned_by_the_same_team() {
    let manifests: Vec<AgentManifest> = viable_roster()
        .iter()
        .cloned()
        .map(|mut m| {
            if m.id == "risk" {
                m.owner = "trading".to_string();
            }
            m
        })
        .collect();
    let error = Roster::from_manifests(manifests)
        .validate(now())
        .unwrap_err();
    assert!(error.message().contains("share owner"));
}

#[test]
fn an_escalation_cycle_is_caught() {
    let a = AgentManifest::research("a", "A", "first", now()).escalating_to("b");
    let b = AgentManifest::research("b", "B", "second", now()).escalating_to("a");
    let findings = Roster::from_manifests(vec![a, b]).review(now());
    assert!(
        findings
            .iter()
            .any(|f| f.rule == "escalation-terminates" && f.severity == Severity::Error),
        "{findings:?}"
    );
}

#[test]
fn an_escalation_to_a_missing_agent_is_caught() {
    let a = AgentManifest::research("a", "A", "first", now()).escalating_to("nobody");
    let findings = Roster::from_manifests(vec![a]).review(now());
    assert!(
        findings
            .iter()
            .any(|f| f.rule == "escalation-target-exists")
    );
}

#[test]
fn duplicate_agent_ids_are_caught() {
    let a = AgentManifest::research("a", "A", "first", now());
    let b = AgentManifest::research("a", "A again", "second", now());
    let findings = Roster::from_manifests(vec![a, b]).review(now());
    assert!(findings.iter().any(|f| f.rule == "unique-id"));
}

// --- memory -----------------------------------------------------------------

fn episode(n: u64, correct: bool, at: Timestamp) -> Episode {
    Episode {
        run_id: run_id(n),
        agent_id: "macro".to_string(),
        occurred_at: at,
        question: "will spreads widen".to_string(),
        conclusion: "yes".to_string(),
        evidence: vec!["doc-1".to_string()],
        conviction: 0.7,
        outcome: Some(EpisodeOutcome {
            resolved_at: at.saturating_add(Duration::from_days(30)),
            correct,
            realised_bps: Some(if correct { 40.0 } else { -25.0 }),
            explanation: "the print landed".to_string(),
        }),
    }
}

#[test]
fn a_lesson_learned_tomorrow_cannot_inform_a_decision_today() {
    let mut memory = ResearchMemory::new();
    let later = now().saturating_add(Duration::from_days(10));
    memory.record_lesson(Lesson {
        lesson_id: LessonId::from_string("lesson-1"),
        learned_at: later,
        learned_by: "learning-engine".to_string(),
        applies_when: "regime=stressed".to_string(),
        statement: "credit leads equity by two days".to_string(),
        supporting_episodes: vec![run_id(1)],
        contradicting_episodes: Vec::new(),
        superseded_at: None,
    });
    assert!(memory.lessons_as_of(now()).is_empty());
    assert_eq!(memory.lessons_as_of(later).len(), 1);
}

#[test]
fn a_superseded_lesson_stays_readable_for_the_dates_it_governed() {
    let mut memory = ResearchMemory::new();
    let id = LessonId::from_string("lesson-1");
    memory.record_lesson(Lesson {
        lesson_id: id.clone(),
        learned_at: now(),
        learned_by: "learning-engine".to_string(),
        applies_when: String::new(),
        statement: "old rule".to_string(),
        supporting_episodes: vec![run_id(1)],
        contradicting_episodes: Vec::new(),
        superseded_at: None,
    });
    let retired = now().saturating_add(Duration::from_days(5));
    assert!(memory.supersede(&id, retired));
    assert_eq!(
        memory.lessons_as_of(now()).len(),
        1,
        "history must stay reconstructable"
    );
    assert!(memory.lessons_as_of(retired).is_empty());
}

#[test]
fn a_lesson_from_one_episode_is_not_promoted() {
    let mut memory = ResearchMemory::new();
    let thin = Lesson {
        lesson_id: LessonId::from_string("lesson-1"),
        learned_at: now(),
        learned_by: "learning-engine".to_string(),
        applies_when: String::new(),
        statement: "it always works".to_string(),
        supporting_episodes: vec![run_id(1)],
        contradicting_episodes: Vec::new(),
        superseded_at: None,
    };
    let refusal = memory
        .promote(thin, PromotionPolicy::default())
        .unwrap_err();
    assert!(refusal.contains("supporting episodes"));
    assert_eq!(memory.lesson_count(), 0);
}

#[test]
fn smoothing_stops_a_perfect_short_record_looking_like_certainty() {
    let three_for_three = Lesson {
        lesson_id: LessonId::from_string("lesson-1"),
        learned_at: now(),
        learned_by: "learning-engine".to_string(),
        applies_when: String::new(),
        statement: "works so far".to_string(),
        supporting_episodes: vec![run_id(1), run_id(2), run_id(3)],
        contradicting_episodes: Vec::new(),
        superseded_at: None,
    };
    assert!((three_for_three.support() - 1.0).abs() < 1e-12);
    assert!((three_for_three.smoothed_support() - 0.8).abs() < 1e-12);
}

#[test]
fn an_agent_with_no_track_record_has_no_hit_rate() {
    let mut memory = ResearchMemory::new();
    assert_eq!(memory.hit_rate("macro", now()), None);
    memory.record_episode(episode(1, true, now()));
    memory.record_episode(episode(2, false, now()));
    assert_eq!(memory.hit_rate("macro", now()), Some(0.5));
}

#[test]
fn episodes_are_read_point_in_time() {
    let mut memory = ResearchMemory::new();
    memory.record_episode(episode(1, true, now()));
    memory.record_episode(episode(
        2,
        true,
        now().saturating_add(Duration::from_days(5)),
    ));
    assert_eq!(memory.episodes_as_of("macro", now()).len(), 1);
    assert_eq!(
        memory
            .episodes_as_of("macro", now().saturating_add(Duration::from_days(5)))
            .len(),
        2
    );
}
