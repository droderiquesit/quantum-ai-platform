//! Contract tests for the agency crate. Each asserts its own premise first so
//! an empty fixture cannot pass a refusal test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic_in_result_fn,
    clippy::type_complexity
)]

use qip_agency::affordance::{AffordanceGraph, ToolEdgeDraft};
use qip_agency::attribution::EffectAttributionDraft;
use qip_agency::comparison::ComparisonDraft;
use qip_agency::goal::{GoalClass, GoalSpec, GoalSpecDraft};
use qip_agency::plan::{ActingIdentity, Candidate, CandidateSet, InterventionPlan, PlanNode, Step};
use qip_agency::tools::{ToolDraft, ToolRegistry};
use qip_core::Decimal;
use std::collections::BTreeSet;

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}
fn s(x: &str) -> Option<String> {
    Some(x.to_string())
}
fn list(x: &[&str]) -> Option<Vec<String>> {
    Some(x.iter().map(|v| v.to_string()).collect())
}

fn draft(class: GoalClass) -> GoalSpecDraft {
    GoalSpecDraft {
        class: Some(class),
        target_state: s("spread under 5 bps"),
        entities_affected: list(&["venue-a"]),
        time_horizon_secs: Some(3600),
        success_metric: s("median spread"),
        acceptable_uncertainty: Some(Decimal::from_scaled(2, 1).unwrap()),
        budget: Some(d(100)),
        risk_envelope: Some(d(50)),
        jurisdictions: list(&["US"]),
        acting_identity: s("desk-agent"),
        prohibited_side_effects: list(&["public_statement"]),
        prohibited_methods: list(&["wash_trade"]),
        stop_conditions: list(&["spread widens"]),
    }
}

// ---- AGENCY-003 / 041 ------------------------------------------------------

#[test]
fn a_goal_missing_any_one_of_its_twelve_declarations_is_refused_naming_that_field() {
    type Wipe = fn(&mut GoalSpecDraft);
    let wipes: [(&str, Wipe); 12] = [
        ("target_state", |g| g.target_state = None),
        ("entities_affected", |g| g.entities_affected = None),
        ("time_horizon_secs", |g| g.time_horizon_secs = None),
        ("success_metric", |g| g.success_metric = None),
        ("acceptable_uncertainty", |g| {
            g.acceptable_uncertainty = None
        }),
        ("budget", |g| g.budget = None),
        ("risk_envelope", |g| g.risk_envelope = None),
        ("jurisdictions", |g| g.jurisdictions = None),
        ("acting_identity", |g| g.acting_identity = None),
        ("prohibited_side_effects", |g| {
            g.prohibited_side_effects = None
        }),
        ("prohibited_methods", |g| g.prohibited_methods = None),
        ("stop_conditions", |g| g.stop_conditions = None),
    ];
    assert!(
        draft(GoalClass::Financial).build().is_ok(),
        "premise: the full draft builds"
    );
    for (field, wipe) in wipes {
        let mut g = draft(GoalClass::Financial);
        wipe(&mut g);
        let err = g.build().unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
    }
}

#[test]
fn a_goal_of_each_of_the_eight_classes_round_trips_through_serde_unchanged() {
    let classes = [
        GoalClass::Financial,
        GoalClass::Operational,
        GoalClass::Research,
        GoalClass::Product,
        GoalClass::Liquidity,
        GoalClass::RiskReduction,
        GoalClass::InformationAcquisition,
        GoalClass::Communications,
    ];
    assert_eq!(classes.len(), 8);
    let mut seen = BTreeSet::new();
    for c in classes {
        let goal = draft(c).build().unwrap();
        let json = serde_json::to_string(&goal).unwrap();
        seen.insert(json.clone());
        let back: GoalSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, goal);
    }
    assert_eq!(seen.len(), 8, "each class serialises distinctly");
}

#[test]
fn a_goal_document_missing_a_field_is_refused_on_deserialisation_not_defaulted() {
    let mut json: serde_json::Value =
        serde_json::to_value(draft(GoalClass::Research).build().unwrap()).unwrap();
    assert!(json.as_object_mut().unwrap().remove("budget").is_some());
    let err = serde_json::from_value::<GoalSpec>(json)
        .unwrap_err()
        .to_string();
    // serde's own "missing field `budget`" also names the field; only the
    // constructor says "not declared", so this proves the document went
    // through `build` rather than a derived deserialiser.
    assert!(err.contains("`budget` is not declared"), "{err}");
}

// ---- AGENCY-012 / 004 / 042 / 001 / 005 / 016 ------------------------------

fn edge(tool: &str, cost: i64, authority: &str, side: &[&str]) -> ToolEdgeDraft {
    ToolEdgeDraft {
        tool: s(tool),
        latency_ms: Some(10),
        cost: Some(d(cost)),
        side_effects: list(side),
        dependencies: list(&[]),
        authority: s(authority),
    }
}

fn graph() -> AffordanceGraph {
    let mut g = AffordanceGraph::new();
    g.add_variable("quote_size", true, true).unwrap();
    g.add_variable("spread", true, false).unwrap();
    g.add_variable("macro_rate", true, false).unwrap();
    g.add_cause("quote_size", "spread").unwrap();
    g.add_cause("macro_rate", "spread").unwrap();
    g.add_tool_edge("quote_size", edge("quoter", 30, "quote", &[]))
        .unwrap();
    g
}

fn who(auth: &[&str]) -> ActingIdentity {
    ActingIdentity {
        name: "desk-agent".into(),
        authorities: auth.iter().map(|a| a.to_string()).collect(),
    }
}

fn step(tool: &str, exposure: i64) -> PlanNode {
    PlanNode::Step(Step {
        tool: tool.into(),
        variable: "quote_size".into(),
        exposure: d(exposure),
    })
}

#[test]
fn a_tool_edge_missing_a_declaration_or_on_an_observable_only_variable_is_refused() {
    let mut g = graph();
    assert!(g.is_observable("spread").unwrap());
    let cases: [(&str, fn(&mut ToolEdgeDraft)); 5] = [
        ("latency_ms", |e| e.latency_ms = None),
        ("cost", |e| e.cost = None),
        ("side_effects", |e| e.side_effects = None),
        ("dependencies", |e| e.dependencies = None),
        ("authority", |e| e.authority = None),
    ];
    for (field, wipe) in cases {
        let mut e = edge("t2", 1, "a", &[]);
        wipe(&mut e);
        let err = g.add_tool_edge("quote_size", e).unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
    }
    let err = g
        .add_tool_edge("spread", edge("t3", 1, "a", &[]))
        .unwrap_err();
    assert!(err.to_string().contains("observable-only"));
    assert_eq!(g.edges("quote_size").len(), 1, "no refused edge was kept");
}

#[test]
fn levers_over_generated_graphs_are_only_controllable_variables_with_a_tool_edge() {
    let mut seed: u64 = 7;
    let mut next = move |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut nonempty = 0;
    for _ in 0..200 {
        let n = 3 + next(6) as usize;
        let names: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        let mut g = AffordanceGraph::new();
        let mut controllable = BTreeSet::new();
        for name in &names {
            let c = next(2) == 0;
            g.add_variable(name, true, c).unwrap();
            if c {
                controllable.insert(name.clone());
                if next(3) != 0 {
                    g.add_tool_edge(name, edge("t", 1, "a", &[])).unwrap();
                }
            }
        }
        for _ in 0..n {
            g.add_cause(
                &names[next(n as u64) as usize],
                &names[next(n as u64) as usize],
            )
            .unwrap();
        }
        let levers = g.levers(&["v0"]);
        nonempty += usize::from(!levers.is_empty());
        for l in levers {
            assert!(controllable.contains(&l), "{l} is not controllable");
            assert!(!g.edges(&l).is_empty(), "{l} has no tool edge");
        }
    }
    assert!(
        nonempty > 10,
        "premise: generation produced levers to check"
    );
}

#[test]
fn a_plan_within_budget_and_envelope_is_admitted_and_one_beyond_either_is_refused() {
    let goal = draft(GoalClass::Liquidity).build().unwrap();
    let g = graph();
    let ok = InterventionPlan::plan(&goal, &g, &who(&["quote"]), step("quoter", 20)).unwrap();
    assert_eq!((ok.total_cost, ok.total_exposure), (d(30), d(20)));
    // cost: two uses of a 30-cost tool then another, 90 -> fine, 120 -> over 100.
    let three = PlanNode::Sequence(vec![
        step("quoter", 1),
        step("quoter", 1),
        step("quoter", 1),
    ]);
    assert_eq!(
        InterventionPlan::plan(&goal, &g, &who(&["quote"]), three)
            .unwrap()
            .total_cost,
        d(90)
    );
    let four = PlanNode::Parallel((0..4).map(|_| step("quoter", 1)).collect());
    let err = InterventionPlan::plan(&goal, &g, &who(&["quote"]), four).unwrap_err();
    assert!(err.to_string().contains("exceeds budget"), "{err}");
    let err = InterventionPlan::plan(&goal, &g, &who(&["quote"]), step("quoter", 51)).unwrap_err();
    assert!(err.to_string().contains("exceeds risk envelope"), "{err}");
}

#[test]
fn a_plan_never_uses_a_lever_whose_authority_the_acting_identity_does_not_hold() {
    // However large the budget, wanting the outcome grants no permission.
    let mut g = draft(GoalClass::Financial);
    g.budget = Some(d(1_000_000_000));
    let goal = g.build().unwrap();
    let graph = graph();
    assert!(InterventionPlan::plan(&goal, &graph, &who(&["quote"]), step("quoter", 1)).is_ok());
    let err =
        InterventionPlan::plan(&goal, &graph, &who(&["other"]), step("quoter", 1)).unwrap_err();
    assert!(
        err.to_string().contains("does not hold authority `quote`"),
        "{err}"
    );
    let mut wrong = who(&["quote"]);
    wrong.name = "intruder".into();
    assert!(InterventionPlan::plan(&goal, &graph, &wrong, step("quoter", 1)).is_err());
    // A tool unknown to the graph is not a lever at all.
    assert!(InterventionPlan::plan(&goal, &graph, &who(&["quote"]), step("ghost", 1)).is_err());
}

#[test]
fn a_prohibited_method_or_side_effect_makes_the_plan_infeasible() {
    let goal = draft(GoalClass::Communications).build().unwrap();
    let mut g = graph();
    g.add_tool_edge("quote_size", edge("wash_trade", 1, "quote", &[]))
        .unwrap();
    g.add_tool_edge(
        "quote_size",
        edge("announce", 1, "quote", &["public_statement"]),
    )
    .unwrap();
    let me = who(&["quote"]);
    let method = InterventionPlan::plan(&goal, &g, &me, step("wash_trade", 1)).unwrap_err();
    assert!(method.to_string().contains("prohibited"), "{method}");
    let side = InterventionPlan::plan(&goal, &g, &me, step("announce", 1)).unwrap_err();
    assert!(side.to_string().contains("public_statement"), "{side}");
}

#[test]
fn a_candidate_set_needs_exactly_one_no_action_baseline_and_one_alternative() {
    let goal = draft(GoalClass::Financial).build().unwrap();
    let plan =
        InterventionPlan::plan(&goal, &graph(), &who(&["quote"]), step("quoter", 1)).unwrap();
    let act = Candidate::Plan(plan);
    assert!(CandidateSet::new(vec![Candidate::NoAction, act.clone()]).is_ok());
    assert!(
        CandidateSet::new(vec![act.clone(), act.clone()]).is_err(),
        "no baseline"
    );
    assert!(
        CandidateSet::new(vec![Candidate::NoAction]).is_err(),
        "no alternative"
    );
    assert!(
        CandidateSet::new(vec![Candidate::NoAction, Candidate::NoAction, act]).is_err(),
        "two baselines"
    );
}

#[test]
fn a_conditional_step_runs_only_when_its_condition_holds_and_an_adaptive_one_switches() {
    let plan = PlanNode::Sequence(vec![
        step("quoter", 1),
        PlanNode::Conditional {
            condition: "evidence_confirmed".into(),
            then: Box::new(step("quoter", 2)),
        },
        PlanNode::Adaptive {
            primary: Box::new(step("quoter", 3)),
            trigger: "adverse_response".into(),
            fallback: Box::new(step("quoter", 4)),
        },
    ]);
    let run = |facts: &[&str]| -> Vec<Decimal> {
        let f: BTreeSet<String> = facts.iter().map(|x| x.to_string()).collect();
        plan.simulate(&f).iter().map(|st| st.exposure).collect()
    };
    assert_eq!(run(&[]), vec![d(1), d(3)]);
    assert_eq!(run(&["evidence_confirmed"]), vec![d(1), d(2), d(3)]);
    assert_eq!(run(&["adverse_response"]), vec![d(1), d(4)]);
    let par = PlanNode::Parallel(vec![step("quoter", 5), step("quoter", 6)]);
    assert_eq!(par.simulate(&BTreeSet::new()).len(), 2);
}

#[test]
fn a_bound_is_checked_against_every_branch_not_only_the_one_that_fires() {
    let goal = draft(GoalClass::Financial).build().unwrap();
    // Primary 30 + fallback 30 + conditional 30 + 30 = 120 > 100 even though
    // any single run costs at most 90.
    let plan = PlanNode::Sequence(vec![
        step("quoter", 1),
        PlanNode::Conditional {
            condition: "c".into(),
            then: Box::new(step("quoter", 1)),
        },
        PlanNode::Adaptive {
            primary: Box::new(step("quoter", 1)),
            trigger: "t".into(),
            fallback: Box::new(step("quoter", 1)),
        },
    ]);
    let err = InterventionPlan::plan(&goal, &graph(), &who(&["quote"]), plan).unwrap_err();
    assert!(err.to_string().contains("exceeds budget"), "{err}");
}

// ---- AGENCY-026 ------------------------------------------------------------

fn tool(name: &str) -> ToolDraft {
    ToolDraft {
        name: s(name),
        acting_identity: s("desk-agent"),
        permission_scope: s("quotes"),
        disclosure_policy: s("none required"),
        jurisdiction_channel_policy: s("US paper venue"),
        rate_limit_calls: Some(2),
        rate_window_secs: Some(60),
        rollback_stop_control: s("cancel all resting quotes"),
    }
}

#[test]
fn a_tool_missing_any_of_its_six_declarations_is_not_registered() {
    let cases: [(&str, fn(&mut ToolDraft)); 6] = [
        ("acting_identity", |t| t.acting_identity = None),
        ("permission_scope", |t| t.permission_scope = None),
        ("disclosure_policy", |t| t.disclosure_policy = None),
        ("jurisdiction_channel_policy", |t| {
            t.jurisdiction_channel_policy = None
        }),
        ("rate_limit_calls", |t| t.rate_limit_calls = None),
        ("rollback_stop_control", |t| t.rollback_stop_control = None),
    ];
    let mut reg = ToolRegistry::new();
    assert!(
        reg.register(tool("probe")).is_ok(),
        "premise: the full draft registers"
    );
    for (field, wipe) in cases {
        let mut t = tool(field);
        wipe(&mut t);
        let err = reg.register(t).unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
        assert!(reg.invoke(field, 0).is_err(), "{field} stayed unregistered");
    }
}

#[test]
fn a_tripped_rate_limit_or_stop_control_refuses_the_next_call() {
    let mut reg = ToolRegistry::new();
    reg.register(tool("quoter")).unwrap();
    reg.invoke("quoter", 0).unwrap();
    reg.invoke("quoter", 10).unwrap();
    let err = reg.invoke("quoter", 20).unwrap_err();
    assert!(err.to_string().contains("rate limit"), "{err}");
    reg.invoke("quoter", 61)
        .expect("the window slid past the first call");
    reg.stop("quoter").unwrap();
    let err = reg.invoke("quoter", 1000).unwrap_err();
    assert!(err.to_string().contains("stopped"), "{err}");
}

// ---- AGENCY-036 / 048 ------------------------------------------------------

#[test]
fn a_comparison_missing_any_of_the_nine_axes_is_refused_before_ranking() {
    let full = || ComparisonDraft {
        expected_causal_effect: Some(d(1)),
        confidence: Some(Decimal::from_scaled(5, 1).unwrap()),
        cost: Some(d(1)),
        capital_usage: Some(d(1)),
        time_to_effect_secs: Some(60),
        reversibility: Some(d(1)),
        legally_eligible: Some(true),
        conduct_risk: Some(d(0)),
        downside: Some(d(2)),
    };
    assert!(full().build().is_ok(), "premise");
    let cases: [(&str, fn(&mut ComparisonDraft)); 9] = [
        ("expected_causal_effect", |c| {
            c.expected_causal_effect = None
        }),
        ("confidence", |c| c.confidence = None),
        ("cost", |c| c.cost = None),
        ("capital_usage", |c| c.capital_usage = None),
        ("time_to_effect_secs", |c| c.time_to_effect_secs = None),
        ("reversibility", |c| c.reversibility = None),
        ("legally_eligible", |c| c.legally_eligible = None),
        ("conduct_risk", |c| c.conduct_risk = None),
        ("downside", |c| c.downside = None),
    ];
    for (field, wipe) in cases {
        let mut c = full();
        wipe(&mut c);
        let err = c.build().unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
    }
}

#[test]
fn an_attribution_without_a_chain_confounders_rivals_or_confidence_is_refused() {
    let full = || EffectAttributionDraft {
        kpi_change: Some(d(3)),
        causal_chain: list(&["quote_size", "spread"]),
        confounders: list(&["macro_rate"]),
        competing_explanations: list(&["seasonality"]),
        confidence: Some(Decimal::from_scaled(7, 1).unwrap()),
    };
    assert!(full().build().is_ok(), "premise");
    let cases: [(&str, fn(&mut EffectAttributionDraft)); 4] = [
        ("causal_chain", |a| a.causal_chain = None),
        ("confounders", |a| a.confounders = None),
        ("competing_explanations", |a| {
            a.competing_explanations = None
        }),
        ("confidence", |a| a.confidence = None),
    ];
    for (field, wipe) in cases {
        let mut a = full();
        wipe(&mut a);
        let err = a.build().unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
    }
    let mut empty_chain = full();
    empty_chain.causal_chain = Some(vec![]);
    assert!(
        empty_chain.build().is_err(),
        "an empty chain is not a chain"
    );
}
