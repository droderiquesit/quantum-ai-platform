//! GOV-016, GOV-017, GOV-018 and GOV-021: the action boundary.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use qip_contracts::action::{
    ActionClass, ExecutableScope, ExecutionRecord, GoalApproval, Ground, PlanApproval, Routed,
    ToolRegistration, ToolRegistry, check_justification,
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
        provenance: "in-tree test tool".into(),
        sandbox_tests: ["sandbox-smoke"].into_iter().map(String::from).collect(),
        cost_latency: qip_contracts::action::CostLatency {
            cost_units: 1,
            latency_ms: 1,
        },
        allowed_callers: ["agent-research"].into_iter().map(String::from).collect(),
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
                provenance: "in-tree test tool".into(),
                sandbox_tests: ["sandbox-smoke"].into_iter().map(String::from).collect(),
                cost_latency: qip_contracts::action::CostLatency {
                    cost_units: 1,
                    latency_ms: 1,
                },
                allowed_callers: [" "].into_iter().map(String::from).collect(),
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

/// R12-RES-7109: a revoked tool comes back only through `reauthorise`, on a
/// named operator's authority. A reauthorisation that names nobody, or names
/// a tool that was never revoked, is refused, because it would put a name on
/// a decision nobody made.
///
/// Mutation: in `ToolRegistry::reauthorise`, drop the blank-operator refusal.
/// The blank call clears the flag and the "still revoked" assertion fails.
#[test]
fn a_revoked_tool_stays_refused_until_a_named_operator_reauthorises_it() {
    let mut registry = ToolRegistry::new();
    registry.register(reg("payment")).unwrap();
    assert!(
        registry
            .authorise("payment", "desk-chat", 1, 1, Timestamp::from_secs(1))
            .is_ok(),
        "premise: the registered tool acts"
    );
    assert!(
        registry.reauthorise("payment", "ops-lead").is_err(),
        "not revoked"
    );
    assert_eq!(registry.reauthorised_by("payment"), None);

    registry.revoke("payment").unwrap();
    assert!(registry.is_revoked("payment"));
    assert!(
        registry.reauthorise("payment", " ").is_err(),
        "blank operator"
    );
    assert!(registry.is_revoked("payment"), "still revoked");
    assert!(
        registry.reauthorise("ghost", "ops-lead").is_err(),
        "unregistered"
    );
    let err = registry
        .authorise("payment", "desk-chat", 1, 1, Timestamp::from_secs(2))
        .unwrap_err();
    assert!(err.message().contains("tool is revoked"), "{err}");
    assert!(
        registry.register(reg("payment")).is_err(),
        "re-registering is no way round the stop"
    );

    registry.reauthorise("payment", "ops-lead").unwrap();
    assert!(!registry.is_revoked("payment"));
    assert_eq!(registry.reauthorised_by("payment"), Some("ops-lead"));
    assert!(
        registry
            .authorise("payment", "desk-chat", 2, 1, Timestamp::from_secs(3))
            .is_ok(),
        "the reauthorised tool acts again"
    );
}

/// Mutation: drop the `licensed` conjunct in `ExecutableScope::route_with_class`
/// — the unlicensed instrument then routes `Executable`.
#[test]
fn the_same_opportunity_is_researched_and_kept_shadow_in_an_unlicensed_jurisdiction() {
    // AAA is governed by the US alone; BBB by the EU alone; CCC by both; DDD
    // names none. The platform is licensed in the US only.
    let scope = ExecutableScope::new(["AAA", "BBB", "CCC", "DDD"].map(String::from))
        .with_jurisdictions(
            ["united_states".to_string()],
            [
                ("AAA".to_string(), set(&["united_states"])),
                ("BBB".to_string(), set(&["european_union"])),
                ("CCC".to_string(), set(&["united_states", "european_union"])),
                ("DDD".to_string(), set(&[])),
            ],
        );

    // Premise: the unlicensed instruments are inside the technical scope, so
    // it is the licence and not the instrument list that refuses them.
    let technical = ExecutableScope::new(["BBB", "CCC", "DDD"].map(String::from));
    for instrument in ["BBB", "CCC", "DDD"] {
        assert!(matches!(
            technical.route("o", instrument),
            Routed::Executable(_)
        ));
    }

    assert!(matches!(scope.route("o1", "AAA"), Routed::Executable(_)));
    for instrument in ["BBB", "CCC", "DDD"] {
        assert!(
            matches!(scope.route("o1", instrument), Routed::Shadow(ref s) if s.instrument() == instrument),
            "{instrument} is not licensed and must stay shadow"
        );
    }
    // Instruments the technical scope does not name stay shadow too.
    assert!(matches!(scope.route("o1", "ZZZ"), Routed::Shadow(_)));
    // Research admits every jurisdiction: the research class routes through.
    for instrument in ["AAA", "BBB", "CCC", "DDD"] {
        assert!(matches!(
            scope.route_with_class("o1", instrument, ActionClass::Research),
            Routed::Executable(_)
        ));
    }
    // Premise for that last check: the same instrument as a capital action
    // is shadow, so the class and not the instrument moved the answer.
    assert!(matches!(
        scope.route_with_class("o1", "BBB", ActionClass::Capital),
        Routed::Shadow(_)
    ));
    // A licence that names no jurisdictions authorises nothing.
    let nothing = ExecutableScope::new(["AAA".to_string()]).with_jurisdictions(
        Vec::<String>::new(),
        [("AAA".to_string(), set(&["united_states"]))],
    );
    assert!(matches!(nothing.route("o", "AAA"), Routed::Shadow(_)));
}

/// Mutation: drop the `provenance` entry from the `missing` table in
/// `ToolRegistry::register` — the registration without provenance is admitted.
#[test]
fn a_registration_missing_provenance_sandbox_tests_cost_latency_or_callers_is_refused_and_a_full_one_round_trips()
 {
    let mut registry = ToolRegistry::new();
    assert!(
        registry.register(reg("probe")).is_ok(),
        "premise: the full registration is admitted"
    );
    type Wipe = fn(&mut ToolRegistration);
    let cases: [(&str, Wipe); 5] = [
        ("provenance", |t| t.provenance.clear()),
        ("sandbox tests", |t| t.sandbox_tests.clear()),
        ("cost and latency", |t| t.cost_latency.cost_units = 0),
        ("cost and latency", |t| t.cost_latency.latency_ms = 0),
        ("allowed callers", |t| t.allowed_callers.clear()),
    ];
    for (n, (field, wipe)) in cases.into_iter().enumerate() {
        let mut broken = reg(&format!("tool-{n}"));
        wipe(&mut broken);
        let refused = registry.register(broken).unwrap_err().to_string();
        assert!(refused.contains(field), "{field}: {refused}");
        assert!(
            registry.registration(&format!("tool-{n}")).is_none(),
            "{field}: the refused tool was registered anyway"
        );
    }

    // A per-call cost above the whole budget could never be admitted.
    let mut dear = reg("dear");
    dear.cost_latency.cost_units = dear.budget_units + 1;
    assert!(
        registry
            .register(dear)
            .unwrap_err()
            .to_string()
            .contains("no call could ever be admitted")
    );

    let text = serde_json::to_string(&reg("wire")).unwrap();
    let back: ToolRegistration = serde_json::from_str(&text).unwrap();
    assert_eq!(back, reg("wire"));
    // A document that omits the new declarations does not decode: they are
    // required, not defaulted.
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value.as_object_mut().unwrap().remove("allowed_callers");
    assert!(serde_json::from_value::<ToolRegistration>(value).is_err());
}

/// Mutation: make `permits_caller` return `Ok(())` unconditionally — the
/// outsider is admitted.
#[test]
fn only_a_declared_caller_may_invoke_a_tool() {
    let mut registry = ToolRegistry::new();
    let mut tool = reg("quoter");
    tool.allowed_callers = set(&["desk-agent"]);
    registry.register(tool).unwrap();
    assert!(registry.permits_caller("quoter", "desk-agent").is_ok());
    let refused = registry
        .permits_caller("quoter", "intruder")
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("not among the callers allowed"),
        "{refused}"
    );
    assert!(registry.permits_caller("absent", "desk-agent").is_err());
}

/// R12-GOV-6127: a `Justification` exists only for grounds naming
/// independent evidence — built through `of`, decoded through the same
/// check — so a seam taking one cannot be reached by an action whose only
/// reasons are the profit it expects and the price it wants to move.
/// Mutation: in `Justification::of`, skip `check_justification` and both
/// profit-only cases are admitted.
#[test]
fn a_justification_exists_only_for_an_evidence_ground_built_or_decoded() {
    use qip_contracts::action::{Ground, Justification};
    for grounds in [
        vec![],
        vec![Ground::ExpectedProfit],
        vec![Ground::ExpectedProfit, Ground::DesiredPriceImpact],
        vec![Ground::Evidence("   ".to_string())],
    ] {
        assert!(
            Justification::of(grounds.clone()).is_err(),
            "{grounds:?} justified an action"
        );
        let wire = serde_json::to_string(&grounds).unwrap();
        assert!(
            serde_json::from_str::<Justification>(&wire).is_err(),
            "{grounds:?} decoded as a justification"
        );
    }
    let grounds = vec![
        Ground::ExpectedProfit,
        Ground::Evidence("filing/E-1".to_string()),
    ];
    let justified = Justification::of(grounds.clone()).unwrap();
    assert_eq!(justified.grounds(), grounds.as_slice());
    let wire = serde_json::to_string(&justified).unwrap();
    assert_eq!(
        serde_json::from_str::<Justification>(&wire).unwrap(),
        justified
    );
}
