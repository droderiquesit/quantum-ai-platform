//! GOV-016, GOV-017, GOV-018 and GOV-021: the action boundary.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use qip_contracts::action::{
    ExecutableScope, ExecutionRecord, GoalApproval, Ground, PlanApproval, Routed, ToolRegistration,
    ToolRegistry, check_justification,
};
use qip_core::Timestamp;

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn reg(tool: &str) -> ToolRegistration {
    ToolRegistration {
        tool: tool.into(),
        acting_identity: "agent-research".into(),
        permission_scope: set(&["read-notes"]),
        disclosure_policy: "internal-only".into(),
        channels: set(&["desk-chat"]),
        max_actions_per_window: 2,
        budget_units: 10,
        revocation_path: "ops revoke".into(),
    }
}

#[test]
fn an_opportunity_outside_the_executable_scope_stays_shadow_however_it_is_rescored() {
    let scope = ExecutableScope::new(["AAA".to_string()]);
    // Premise: the scope does admit an in-scope instrument.
    assert!(matches!(scope.route("o1", "AAA"), Routed::Executable(_)));
    // Re-scoring the same opportunity many times re-enters `route`; none of
    // them may ever yield an intent.
    for n in 0..20 {
        let routed = scope.route(&format!("o-rescore-{n}"), "ZZZ");
        assert!(matches!(routed, Routed::Shadow(ref s) if s.instrument() == "ZZZ"));
    }
    // Fail closed: an empty scope shadows everything.
    assert!(matches!(
        ExecutableScope::default().route("o", "AAA"),
        Routed::Shadow(_)
    ));
}

#[test]
fn a_tool_missing_any_of_its_declarations_cannot_be_registered() {
    let mut r = ToolRegistry::new();
    assert!(
        r.register(reg("t")).is_ok(),
        "premise: the full form is admitted"
    );
    let cases: Vec<(&str, ToolRegistration)> = vec![
        (
            "identity",
            ToolRegistration {
                acting_identity: " ".into(),
                ..reg("a")
            },
        ),
        (
            "scope",
            ToolRegistration {
                permission_scope: set(&[]),
                ..reg("b")
            },
        ),
        (
            "disclosure",
            ToolRegistration {
                disclosure_policy: String::new(),
                ..reg("c")
            },
        ),
        (
            "channels",
            ToolRegistration {
                channels: set(&[]),
                ..reg("d")
            },
        ),
        (
            "rate",
            ToolRegistration {
                max_actions_per_window: 0,
                ..reg("e")
            },
        ),
        (
            "budget",
            ToolRegistration {
                budget_units: 0,
                ..reg("f")
            },
        ),
        (
            "revocation",
            ToolRegistration {
                revocation_path: String::new(),
                ..reg("g")
            },
        ),
    ];
    for (name, case) in cases {
        assert!(r.register(case).is_err(), "{name} missing must be refused");
    }
}

#[test]
fn an_unregistered_or_over_limit_tool_is_refused_and_every_decision_is_audited() {
    let at = Timestamp::from_secs(1);
    let mut r = ToolRegistry::new();
    r.register(reg("t")).unwrap();
    assert!(r.authorise("ghost", "desk-chat", 1, 1, at).is_err());
    assert!(
        r.authorise("t", "public-feed", 1, 1, at).is_err(),
        "channel outside the rules"
    );
    assert!(r.authorise("t", "desk-chat", 1, 1, at).is_ok());
    assert!(r.authorise("t", "desk-chat", 1, 1, at).is_ok());
    assert!(
        r.authorise("t", "desk-chat", 1, 1, at).is_err(),
        "third action in one window"
    );
    assert!(
        r.authorise("t", "desk-chat", 2, 1, at).is_ok(),
        "a new window resets the rate"
    );
    assert!(
        r.authorise("t", "desk-chat", 3, 8, at).is_err(),
        "budget 3 spent, 8 more exceeds 10"
    );
    assert_eq!(r.audit().len(), 7);
    assert_eq!(r.audit().iter().filter(|a| a.admitted).count(), 3);
}

#[test]
fn after_revocation_the_tools_next_action_is_refused_and_the_refusal_is_audited() {
    let at = Timestamp::from_secs(1);
    let mut r = ToolRegistry::new();
    r.register(reg("t")).unwrap();
    assert!(
        r.authorise("t", "desk-chat", 1, 1, at).is_ok(),
        "premise: it acts before revocation"
    );
    r.revoke("t").unwrap();
    assert!(r.authorise("t", "desk-chat", 2, 1, at).is_err());
    let last = r.audit().last().unwrap();
    assert!(!last.admitted && last.reason == "tool is revoked");
}

#[test]
fn a_justification_of_only_profit_or_price_impact_is_refused() {
    let evidence = Ground::Evidence("filing 10-K p4 discloses the restatement".into());
    assert!(
        check_justification(std::slice::from_ref(&evidence)).is_ok(),
        "premise: evidence passes"
    );
    assert!(check_justification(&[Ground::ExpectedProfit, evidence]).is_ok());
    assert!(check_justification(&[Ground::ExpectedProfit]).is_err());
    assert!(check_justification(&[Ground::DesiredPriceImpact]).is_err());
    assert!(check_justification(&[Ground::ExpectedProfit, Ground::DesiredPriceImpact]).is_err());
    assert!(check_justification(&[]).is_err());
    assert!(check_justification(&[Ground::Evidence("  ".into())]).is_err());
}

#[test]
fn one_intervention_leaves_three_linked_records_each_with_its_own_actor_and_time() {
    let goal = GoalApproval::approve("pm-a", Timestamp::from_secs(10), "rebalance").unwrap();
    let plan =
        PlanApproval::approve(&goal, "risk-b", Timestamp::from_secs(20), "rebalance").unwrap();
    let run = ExecutionRecord::execute(&plan, "sim-gateway", Timestamp::from_secs(30), "rebalance")
        .unwrap();
    assert_eq!(plan.goal, goal.id);
    assert_eq!(run.plan, plan.id);
    assert_ne!(goal.id, plan.id);
    assert_ne!(plan.id, run.id);
    assert_eq!(
        (
            goal.approver.as_str(),
            plan.approver.as_str(),
            run.executor.as_str()
        ),
        ("pm-a", "risk-b", "sim-gateway")
    );
    // Order and attribution are enforced, not merely recorded.
    assert!(PlanApproval::approve(&goal, "risk-b", Timestamp::from_secs(5), "x").is_err());
    assert!(ExecutionRecord::execute(&plan, "sim-gateway", Timestamp::from_secs(15), "x").is_err());
    assert!(ExecutionRecord::execute(&plan, "", Timestamp::from_secs(30), "x").is_err());
}
