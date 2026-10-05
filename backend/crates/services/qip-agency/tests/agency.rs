//! Contract tests for the agency crate. Each asserts its own premise first so
//! an empty fixture cannot pass a refusal test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic_in_result_fn,
    clippy::type_complexity
)]

use qip_agency::affordance::{AffordanceGraph, Method, ToolEdgeDraft};
use qip_agency::attribution::{EffectAttributionDraft, Identification};
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
        method: Some(Method::Market),
        reversible: Some(true),
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
    let cases: [(&str, fn(&mut ToolEdgeDraft)); 7] = [
        ("method", |e| e.method = None),
        ("reversible", |e| e.reversible = None),
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
        action_class: Some(Method::Market),
        identification: Some(Identification::Experiment),
        kpi_change: Some(d(3)),
        causal_chain: list(&["quote_size", "spread"]),
        confounders: list(&["macro_rate"]),
        competing_explanations: list(&["seasonality"]),
        confidence: Some(Decimal::from_scaled(7, 1).unwrap()),
    };
    assert!(full().build().is_ok(), "premise");
    let cases: [(&str, fn(&mut EffectAttributionDraft)); 6] = [
        ("action_class", |a| a.action_class = None),
        ("identification", |a| a.identification = None),
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

// ============================================================================
// The engine, the gates and the autonomy ladder.
// ============================================================================

use qip_agency::attribution::EffectAttribution;
use qip_agency::autonomy::{ActionPolicy, Authority, WIDEN_AUTHORITY};
use qip_agency::engine::{
    Adapter, Decision, EdgeDecl, LeverEvidence, ORDER, Outcome, ProposalDraft, Request, Stage,
    StageLog, VariableDecl, decide, execute, run,
};
use qip_agency::memory::{ActionOutcomeDraft, ActionOutcomeMemory};
use qip_agency::plan::{Proposal, select};
use qip_core::Error;
use std::collections::BTreeMap;

/// A deterministic generator: the workspace permits no property-test crate.
fn lcg(mut seed: u64) -> impl FnMut(u64) -> u64 {
    move |n| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    }
}

fn tenths(n: i128) -> Decimal {
    Decimal::from_scaled(n, 1).unwrap()
}

fn method_edge(tool: &str, method: Method, reversible: bool) -> ToolEdgeDraft {
    ToolEdgeDraft {
        method: Some(method),
        reversible: Some(reversible),
        ..edge(tool, 0, "quote", &[])
    }
}

fn prediction(effect: Decimal, eligible: bool) -> ComparisonDraft {
    ComparisonDraft {
        expected_causal_effect: Some(effect),
        confidence: Some(tenths(5)),
        cost: Some(d(1)),
        capital_usage: Some(d(1)),
        time_to_effect_secs: Some(60),
        reversibility: Some(d(1)),
        legally_eligible: Some(eligible),
        conduct_risk: Some(d(0)),
        downside: Some(d(2)),
    }
}

fn proposal(root: PlanNode, effect: Decimal) -> Proposal {
    Proposal {
        root,
        comparison: prediction(effect, true).build().unwrap(),
    }
}

/// Every effect a learned model might report, including the one chosen to
/// dominate anything it is compared with.
fn estimates() -> [Decimal; 4] {
    [d(0), d(1), d(1_000_000_000), Decimal::MAX]
}

const DECEPTIVE: [Method; 9] = [
    Method::FabricatedIdentity,
    Method::Sockpuppet,
    Method::FakeConsensus,
    Method::MisleadingClaim,
    Method::UndisclosedPromotion,
    Method::RumourManufacture,
    Method::WashTrading,
    Method::Spoofing,
    Method::PumpAndDump,
];

// ---- AGENCY-047 ------------------------------------------------------------

#[test]
fn a_plan_with_a_deceptive_step_anywhere_in_it_is_infeasible_whatever_its_expected_effect() {
    // The failure prevented: a planner that scores deception as a cost. Any
    // cost loses to a large enough expected effect; a refusal does not.
    for lawful in [
        Method::Communication,
        Method::Capital,
        Method::Product,
        Method::Operational,
        Method::Research,
        Method::Market,
    ] {
        assert!(!lawful.is_deceptive(), "{lawful:?}");
    }
    let goal = draft(GoalClass::Communications).build().unwrap();
    let mut g = graph();
    g.add_tool_edge("quote_size", method_edge("honest", Method::Market, true))
        .unwrap();
    for (i, method) in DECEPTIVE.iter().enumerate() {
        assert!(method.is_deceptive(), "{method:?}");
        g.add_tool_edge("quote_size", method_edge(&format!("m{i}"), *method, true))
            .unwrap();
    }
    let me = who(&["quote"]);
    let levers = g.levers(&["spread"]);
    let honest = || step("honest", 1);
    let mut next = lcg(47);
    let mut shapes = BTreeSet::new();
    for _ in 0..300 {
        let bad = step(&format!("m{}", next(9)), 1);
        let mut leaves: Vec<PlanNode> = (0..1 + next(4)).map(|_| honest()).collect();
        let clean = PlanNode::Sequence(leaves.clone());
        let shape = next(4);
        shapes.insert(shape);
        let dirty = match shape {
            0 => {
                leaves.insert(next(leaves.len() as u64 + 1) as usize, bad);
                PlanNode::Sequence(leaves)
            }
            1 => {
                leaves.push(bad);
                PlanNode::Parallel(leaves)
            }
            // The deceptive step sits on a branch that would not fire.
            2 => PlanNode::Sequence(vec![
                clean.clone(),
                PlanNode::Conditional {
                    condition: "never_recorded".into(),
                    then: Box::new(bad),
                },
            ]),
            _ => PlanNode::Adaptive {
                primary: Box::new(clean.clone()),
                trigger: "never_recorded".into(),
                fallback: Box::new(bad),
            },
        };
        // Premise: the same plan without the step is feasible and is chosen.
        let chosen = select(&goal, &g, &me, &levers, vec![proposal(clean, d(1))]).unwrap();
        assert!(matches!(chosen.chosen, Candidate::Plan(_)), "premise");

        let err = InterventionPlan::plan(&goal, &g, &me, dirty.clone()).unwrap_err();
        assert!(err.to_string().contains("deceptive"), "{err}");
        for effect in estimates() {
            let picked = select(
                &goal,
                &g,
                &me,
                &levers,
                vec![proposal(dirty.clone(), effect)],
            )
            .unwrap();
            assert_eq!(picked.chosen, Candidate::NoAction, "effect {effect}");
            assert_eq!(picked.refused.len(), 1);
        }
    }
    assert_eq!(shapes.len(), 4, "premise: every plan shape was generated");
}

// ---- AGENCY-043 ------------------------------------------------------------

#[test]
fn no_learned_effect_estimate_however_large_gets_a_plan_past_a_hard_constraint() {
    let goal = draft(GoalClass::Liquidity).build().unwrap();
    let mut g = graph();
    g.add_variable("elsewhere", true, true).unwrap();
    g.add_tool_edge("elsewhere", edge("stray", 1, "quote", &[]))
        .unwrap();
    g.add_tool_edge("quote_size", edge("privileged", 1, "treasury", &[]))
        .unwrap();
    g.add_tool_edge("quote_size", edge("wash_trade", 1, "quote", &[]))
        .unwrap();
    g.add_tool_edge(
        "quote_size",
        edge("announce", 1, "quote", &["public_statement"]),
    )
    .unwrap();
    let me = who(&["quote"]);
    let levers = g.levers(&["spread"]);
    assert_eq!(levers, vec!["quote_size".to_string()], "premise");

    // Premise: the ranker does rank. Of two admissible plans the larger
    // estimate wins, so a violator losing below is not the ranker being inert.
    let modest = || proposal(step("quoter", 1), d(1));
    let both = select(
        &goal,
        &g,
        &me,
        &levers,
        vec![modest(), proposal(step("quoter", 2), d(10))],
    )
    .unwrap();
    let Candidate::Plan(won) = both.chosen else {
        panic!("premise: an admissible plan is chosen")
    };
    assert_eq!(
        won.total_exposure,
        d(2),
        "premise: the larger estimate wins"
    );

    let stray = PlanNode::Step(Step {
        tool: "stray".into(),
        variable: "elsewhere".into(),
        exposure: d(1),
    });
    let violators: [(&str, PlanNode, bool); 7] = [
        (
            "exceeds budget",
            PlanNode::Parallel((0..4).map(|_| step("quoter", 1)).collect()),
            true,
        ),
        ("exceeds risk envelope", step("quoter", 51), true),
        ("does not hold authority", step("privileged", 1), true),
        ("prohibited by the goal", step("wash_trade", 1), true),
        ("prohibited side effect", step("announce", 1), true),
        ("not a lever", stray, true),
        ("not legally eligible", step("quoter", 1), false),
    ];
    for (reason, root, eligible) in violators {
        for effect in estimates() {
            let violator = Proposal {
                root: root.clone(),
                comparison: prediction(effect, eligible).build().unwrap(),
            };
            let alone = select(&goal, &g, &me, &levers, vec![violator.clone()]).unwrap();
            assert_eq!(alone.chosen, Candidate::NoAction, "{reason} at {effect}");
            // Asserted as a count before it is indexed: an estimate of zero
            // loses to the baseline on its own, so "not chosen" is not yet
            // "refused".
            assert_eq!(alone.refused.len(), 1, "{reason} at {effect}: not refused");
            assert!(
                alone.refused[0].1.to_string().contains(reason),
                "{reason}: {}",
                alone.refused[0].1
            );
            // Beside an admissible plan worth far less, it still loses.
            let beside = select(&goal, &g, &me, &levers, vec![violator, modest()]).unwrap();
            let Candidate::Plan(chosen) = beside.chosen else {
                panic!("{reason}: the admissible plan should be chosen")
            };
            assert_eq!(chosen.total_exposure, d(1), "{reason} at {effect}");
            assert_eq!(beside.refused.len(), 1, "{reason}");
        }
    }
}

// ---- AGENCY-045 ------------------------------------------------------------

#[test]
fn a_plan_whose_levers_are_weakly_identified_is_never_returned_for_execution() {
    // acceptable_uncertainty is 0.2, so the floor is 0.8.
    let goal = draft(GoalClass::Research).build().unwrap();
    let mut g = AffordanceGraph::new();
    g.add_variable("target", true, false).unwrap();
    for i in 0..3 {
        let name = format!("l{i}");
        g.add_variable(&name, true, true).unwrap();
        g.add_cause(&name, "target").unwrap();
        g.add_tool_edge(&name, edge("t", 1, "quote", &[])).unwrap();
    }
    let me = who(&["quote"]);
    let mut next = lcg(45);
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    for _ in 0..600 {
        let used = 1 + next(3) as usize;
        let root = PlanNode::Sequence(
            (0..used)
                .map(|i| {
                    PlanNode::Step(Step {
                        tool: "t".into(),
                        variable: format!("l{i}"),
                        exposure: d(1),
                    })
                })
                .collect(),
        );
        let plan = InterventionPlan::plan(&goal, &g, &me, root).unwrap();
        let mut evidence = BTreeMap::new();
        let (mut missing, mut dead, mut weak) = (false, false, false);
        for i in 0..used {
            let kind = next(6);
            let strong = Decimal::from_scaled(800 + next(201) as i128, 3).unwrap();
            let feeble = Decimal::from_scaled(next(800) as i128, 3).unwrap();
            let known = match kind {
                0 => {
                    missing = true;
                    continue;
                }
                1 => {
                    dead = true;
                    (feeble, false)
                }
                2 => {
                    weak = true;
                    (feeble, true)
                }
                _ => (strong, next(2) == 0),
            };
            evidence.insert(
                format!("l{i}"),
                LeverEvidence {
                    identifiability: known.0,
                    experimentable: known.1,
                },
            );
        }
        let decision = decide(&goal, plan, &evidence).unwrap();
        let label = match &decision {
            Decision::Execute(_) => "execute",
            Decision::Abstain(_) => "abstain",
            Decision::GatherEvidence(_) => "gather",
            Decision::ProposeExperiment(_) => "experiment",
        };
        *seen.entry(label).or_default() += 1;
        let expected = if dead {
            "abstain"
        } else if missing {
            "gather"
        } else if weak {
            "experiment"
        } else {
            "execute"
        };
        assert_eq!(label, expected, "{evidence:?}");
    }
    for label in ["execute", "abstain", "gather", "experiment"] {
        assert!(
            seen.get(label).copied().unwrap_or(0) > 10,
            "premise: {label} {seen:?}"
        );
    }

    // The floor is the goal's own, exactly: 0.8 passes, a hair under does not.
    let one = |identifiability: Decimal| {
        let plan = InterventionPlan::plan(
            &goal,
            &g,
            &me,
            PlanNode::Step(Step {
                tool: "t".into(),
                variable: "l0".into(),
                exposure: d(1),
            }),
        )
        .unwrap();
        let evidence = BTreeMap::from([(
            "l0".to_string(),
            LeverEvidence {
                identifiability,
                experimentable: true,
            },
        )]);
        decide(&goal, plan, &evidence)
    };
    assert!(matches!(one(tenths(8)).unwrap(), Decision::Execute(_)));
    assert!(matches!(
        one(Decimal::from_scaled(799_999_999, 9).unwrap()).unwrap(),
        Decision::ProposeExperiment(_)
    ));
    assert!(one(tenths(15)).is_err(), "an out-of-range score is refused");
}

// ---- AGENCY-034 ------------------------------------------------------------

#[test]
fn a_stage_is_admitted_only_in_the_declared_order_and_never_after_a_failure() {
    assert_eq!(ORDER.len(), 11);
    assert_eq!(ORDER[5..8], [Stage::Simulate, Stage::Gate, Stage::Act]);
    assert_eq!(
        ORDER[8..],
        [Stage::ObserveEffect, Stage::Attribute, Stage::Update]
    );
    let mut next = lcg(34);
    let (mut acted, mut refused_act) = (0, 0);
    for _ in 0..2000 {
        let mut log = StageLog::new();
        let mut failed = false;
        for _ in 0..16 {
            let position = log.passed().len();
            // Mostly the right stage, so some passes get as far as acting.
            let stage = if next(8) != 0 && position < 11 {
                ORDER[position]
            } else {
                ORDER[next(11) as usize]
            };
            let in_order = !failed && ORDER.get(position) == Some(&stage);
            if next(12) == 0 {
                assert_eq!(log.fail(stage).is_ok(), in_order, "fail {stage:?}");
                failed |= in_order;
            } else {
                assert_eq!(log.pass(stage).is_ok(), in_order, "pass {stage:?}");
                refused_act += u32::from(stage == Stage::Act && !in_order);
            }
        }
        // Whatever was attempted, what is recorded is a prefix of the order.
        assert_eq!(log.passed(), &ORDER[..log.passed().len()]);
        if log.passed().contains(&Stage::Act) {
            acted += 1;
            assert!(log.passed().contains(&Stage::Simulate));
            assert!(log.passed().contains(&Stage::Gate));
        }
    }
    assert!(acted > 50, "premise: some passes reached Act ({acted})");
    assert!(
        refused_act > 50,
        "premise: Act was refused out of order ({refused_act})"
    );
}

#[derive(Default)]
struct Counting {
    calls: Vec<String>,
}

impl Adapter for Counting {
    fn call(&mut self, step: &Step) -> Result<(), Error> {
        self.calls.push(step.tool.clone());
        Ok(())
    }
}

fn request() -> Request {
    Request {
        goal: draft(GoalClass::Liquidity).build().unwrap(),
        targets: vec!["spread".into()],
        variables: vec![
            VariableDecl {
                name: "quote_size".into(),
                observable: true,
                controllable: true,
                owned: false,
            },
            VariableDecl {
                name: "spread".into(),
                observable: true,
                controllable: false,
                owned: false,
            },
            VariableDecl {
                name: "hidden".into(),
                observable: false,
                controllable: false,
                owned: false,
            },
        ],
        causes: vec![("quote_size".into(), "spread".into())],
        tool_edges: vec![EdgeDecl {
            variable: "quote_size".into(),
            edge: edge("quoter", 30, "quote", &[]),
        }],
        identity: who(&["quote"]),
        evidence: BTreeMap::from([(
            "quote_size".to_string(),
            LeverEvidence {
                identifiability: tenths(9),
                experimentable: true,
            },
        )]),
        observed_facts: BTreeSet::new(),
        proposals: vec![ProposalDraft {
            root: step("quoter", 20),
            comparison: prediction(d(5), true),
        }],
    }
}

fn identified(class: Method) -> EffectAttribution {
    EffectAttributionDraft {
        action_class: Some(class),
        identification: Some(Identification::Experiment),
        kpi_change: Some(d(3)),
        causal_chain: list(&["quote_size", "spread"]),
        confounders: list(&[]),
        competing_explanations: list(&[]),
        confidence: Some(tenths(7)),
    }
    .build()
    .unwrap()
}

fn operator() -> ActingIdentity {
    ActingIdentity {
        name: "risk-officer".into(),
        authorities: BTreeSet::from([WIDEN_AUTHORITY.to_string()]),
    }
}

/// A market policy holding narrow-reversible authority, obtained the only
/// way there is.
fn narrow() -> ActionPolicy {
    let mut policy = ActionPolicy::new(
        Method::Market,
        d(1),
        d(5),
        BTreeSet::from(["wider_quotes".to_string()]),
    )
    .unwrap();
    policy
        .widen(&identified(Method::Market), &operator())
        .unwrap();
    assert_eq!(policy.authority(), Authority::NarrowReversible);
    policy
}

#[test]
fn over_generated_passes_an_adapter_is_called_only_after_simulation_and_the_gate_passed() {
    // The policy may act throughout, so the only thing standing between a
    // failed stage and an adapter call is the stage order itself.
    let policy = narrow();
    let mut next = lcg(3434);
    let mut kinds: BTreeMap<u64, u32> = BTreeMap::new();
    let mut executed = 0;
    for _ in 0..400 {
        let kind = next(7);
        *kinds.entry(kind).or_default() += 1;
        let mut r = request();
        let expect_failed = match kind {
            0 => None,
            1 => {
                r.targets = vec!["hidden".into()];
                Some(Stage::Observe)
            }
            2 => {
                r.proposals[0].comparison.downside = None;
                Some(Stage::Predict)
            }
            3 => {
                r.tool_edges.clear();
                Some(Stage::IdentifyLevers)
            }
            4 => {
                r.proposals[0].root = step("quoter", 51);
                None // refused at Generate; the baseline is chosen
            }
            5 => {
                r.proposals[0].root = PlanNode::Conditional {
                    condition: "not_observed".into(),
                    then: Box::new(step("quoter", 20)),
                };
                Some(Stage::Simulate)
            }
            _ => {
                r.evidence.clear();
                Some(Stage::Gate)
            }
        };
        let mut adapter = Counting::default();
        let report = run(r, Some(&policy), &mut adapter).unwrap();
        assert_eq!(report.failed, expect_failed, "kind {kind}");
        assert_eq!(report.passed, &ORDER[..report.passed.len()], "kind {kind}");
        if kind == 0 {
            executed += 1;
            assert_eq!(adapter.calls, vec!["quoter".to_string()]);
            assert_eq!(report.passed, &ORDER[..8], "observe through act");
            assert!(matches!(report.outcome, Outcome::Executed(_)));
        } else {
            assert!(adapter.calls.is_empty(), "kind {kind} reached an adapter");
            assert!(!report.passed.contains(&Stage::Act), "kind {kind}");
            assert!(!matches!(report.outcome, Outcome::Executed(_)));
        }
    }
    assert!(executed > 10, "premise: the clean pass does execute");
    assert_eq!(kinds.len(), 7, "premise: every injected failure occurred");

    // `execute` itself, handed a log that never simulated or gated.
    let r = request();
    let plan = InterventionPlan::plan(&r.goal, &graph(), &r.identity, step("quoter", 20)).unwrap();
    let mut adapter = Counting::default();
    let mut fresh = StageLog::new();
    let err = execute(
        &mut fresh,
        Some(&policy),
        &graph(),
        &plan,
        &r.observed_facts,
        &mut adapter,
    )
    .unwrap_err();
    assert!(err.to_string().contains("out of order"), "{err}");
    let mut gate_failed = StageLog::new();
    for stage in &ORDER[..6] {
        gate_failed.pass(*stage).unwrap();
    }
    gate_failed.fail(Stage::Gate).unwrap();
    assert!(
        execute(
            &mut gate_failed,
            Some(&policy),
            &graph(),
            &plan,
            &r.observed_facts,
            &mut adapter
        )
        .is_err()
    );
    assert!(adapter.calls.is_empty());
}

// ---- AGENCY-055 ------------------------------------------------------------

#[test]
fn the_default_authority_is_shadow_and_narrow_reversible_refuses_an_irreversible_tool() {
    assert_eq!(Authority::default(), Authority::Shadow);
    let fresh = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    assert_eq!(fresh.authority(), Authority::Shadow);

    // Premise: at narrow-reversible the very same request does reach the
    // adapter, so the silence below is the authority and not a broken pass.
    let policy = narrow();
    let mut adapter = Counting::default();
    let acted = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(matches!(acted.outcome, Outcome::Executed(_)), "premise");
    assert_eq!(adapter.calls.len(), 1, "premise");

    for shadow in [None, Some(&fresh)] {
        let mut adapter = Counting::default();
        let report = run(request(), shadow, &mut adapter).unwrap();
        let Outcome::Shadowed(steps) = &report.outcome else {
            panic!("expected a shadowed plan, got {:?}", report.outcome)
        };
        assert_eq!(steps.len(), 1, "the report says what would have run");
        assert!(adapter.calls.is_empty(), "shadow called an adapter");
        // Act is not recorded, so nothing can be attributed to it later.
        assert_eq!(report.passed, &ORDER[..7]);
    }

    // An irreversible tool, alone and behind a reversible one: refused before
    // the first call either way.
    let irreversible = |root: PlanNode| {
        let mut r = request();
        r.tool_edges.push(EdgeDecl {
            variable: "quote_size".into(),
            edge: method_edge("delist", Method::Market, false),
        });
        r.proposals[0].root = root;
        r
    };
    for root in [
        step("delist", 1),
        PlanNode::Sequence(vec![step("quoter", 1), step("delist", 1)]),
    ] {
        let mut adapter = Counting::default();
        let err = run(irreversible(root), Some(&policy), &mut adapter).unwrap_err();
        assert!(err.to_string().contains("irreversible"), "{err}");
        assert!(
            adapter.calls.is_empty(),
            "a step was sent before the refusal"
        );
    }
    // Authority for one class is not authority for another.
    let mut r = request();
    r.tool_edges[0].edge.method = Some(Method::Research);
    let mut adapter = Counting::default();
    let err = run(r, Some(&policy), &mut adapter).unwrap_err();
    assert!(err.to_string().contains("Market only"), "{err}");
    assert!(adapter.calls.is_empty());
}

// ---- AGENCY-056 ------------------------------------------------------------

#[test]
fn autonomy_widens_only_on_an_identified_attribution_and_operator_authority_together() {
    let mut policy = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    // A forecast that came true, held with near-certainty, is still not
    // identified.
    let correlational = EffectAttributionDraft {
        action_class: Some(Method::Market),
        identification: Some(Identification::NotIdentified),
        kpi_change: Some(d(3)),
        causal_chain: list(&["quote_size", "spread"]),
        confounders: list(&[]),
        competing_explanations: list(&[]),
        confidence: Some(Decimal::from_scaled(99, 2).unwrap()),
    }
    .build()
    .unwrap();
    let err = policy.widen(&correlational, &operator()).unwrap_err();
    assert!(err.to_string().contains("not identified"), "{err}");
    assert_eq!(policy.authority(), Authority::Shadow);

    // Identified evidence passes the precondition and grants nothing alone.
    let nobody = who(&["quote"]);
    let err = policy
        .widen(&identified(Method::Market), &nobody)
        .unwrap_err();
    assert!(err.to_string().contains(WIDEN_AUTHORITY), "{err}");
    assert_eq!(policy.authority(), Authority::Shadow);

    // Evidence about another class of action is not evidence about this one.
    let err = policy
        .widen(&identified(Method::Research), &operator())
        .unwrap_err();
    assert!(err.to_string().contains("widens no other"), "{err}");
    assert_eq!(policy.authority(), Authority::Shadow);

    policy
        .widen(&identified(Method::Market), &operator())
        .unwrap();
    assert_eq!(policy.authority(), Authority::NarrowReversible);
    let err = policy
        .widen(&identified(Method::Market), &operator())
        .unwrap_err();
    assert!(err.to_string().contains("needs an ADR"), "{err}");

    // "Observational" with no strategy named is not a way round it.
    let unnamed = EffectAttributionDraft {
        action_class: Some(Method::Market),
        identification: Some(Identification::Observational {
            strategy: " ".into(),
        }),
        kpi_change: Some(d(3)),
        causal_chain: list(&["a"]),
        confounders: list(&[]),
        competing_explanations: list(&[]),
        confidence: Some(tenths(7)),
    };
    assert!(unnamed.build().is_err());
}

// ---- AGENCY-054 ------------------------------------------------------------

#[test]
fn an_effect_outside_the_predicted_interval_or_an_unpredicted_side_effect_returns_a_policy_to_shadow()
 {
    // Predicted uplift [1, 5], predicted side effect "wider_quotes".
    let expected = ["wider_quotes".to_string()];
    let mut policy = narrow();
    for effect in [d(1), d(3), d(5)] {
        assert_eq!(
            policy.observe(effect, &expected),
            Authority::NarrowReversible
        );
        assert_eq!(policy.observe(effect, &[]), Authority::NarrowReversible);
    }
    for (effect, side_effects) in [
        (d(6), vec![]),
        (d(0), vec![]),
        (d(3), vec!["venue_complaint".to_string()]),
    ] {
        let mut policy = narrow();
        assert_eq!(
            policy.observe(effect, &side_effects),
            Authority::Shadow,
            "{effect} {side_effects:?}"
        );
        // It stays down: a later in-range effect is not an operator.
        assert_eq!(policy.observe(d(3), &[]), Authority::Shadow);
        let mut adapter = Counting::default();
        let report = run(request(), Some(&policy), &mut adapter).unwrap();
        assert!(matches!(report.outcome, Outcome::Shadowed(_)));
        assert!(adapter.calls.is_empty());
    }
}

// ---- AGENCY-023 ------------------------------------------------------------

#[test]
fn an_operational_tool_on_a_system_outside_the_owned_registry_is_refused() {
    let mut g = AffordanceGraph::new();
    for name in ["own_router", "vendor_router"] {
        g.add_variable(name, true, true).unwrap();
    }
    g.declare_owned("own_router").unwrap();
    assert!(g.declare_owned("undeclared").is_err());
    let reroute = || method_edge("reroute", Method::Operational, true);
    g.add_tool_edge("own_router", reroute())
        .expect("premise: an owned system takes an operational tool");
    for method in [Method::Operational, Method::Product] {
        let err = g
            .add_tool_edge("vendor_router", method_edge("reroute", method, true))
            .unwrap_err();
        assert!(err.to_string().contains("owned-system registry"), "{err}");
    }
    assert!(g.edges("vendor_router").is_empty(), "no refused edge kept");

    let goal = draft(GoalClass::Operational).build().unwrap();
    let on = |variable: &str| {
        PlanNode::Step(Step {
            tool: "reroute".into(),
            variable: variable.into(),
            exposure: d(1),
        })
    };
    let me = who(&["quote"]);
    assert!(InterventionPlan::plan(&goal, &g, &me, on("own_router")).is_ok());
    assert!(InterventionPlan::plan(&goal, &g, &me, on("vendor_router")).is_err());
    // Ownership is asked of actions that change a system, not of every tool.
    g.add_tool_edge(
        "vendor_router",
        method_edge("probe", Method::Research, true),
    )
    .unwrap();
}

// ---- AGENCY-035 ------------------------------------------------------------

#[test]
fn a_decomposition_yields_child_goals_none_of_which_steps_outside_the_parent() {
    // Parent: budget 100, envelope 50, identity desk-agent, jurisdiction US.
    let parent = draft(GoalClass::Financial).build().unwrap();
    let part = |budget: i64, envelope: i64| {
        let mut g = draft(GoalClass::Research);
        g.budget = Some(d(budget));
        g.risk_envelope = Some(d(envelope));
        g.acting_identity = None;
        g.prohibited_methods = list(&[]);
        g.prohibited_side_effects = list(&[]);
        g
    };
    let children = parent
        .decompose(vec![part(60, 20), part(40, 30)])
        .expect("premise: a split that fits is admitted");
    assert_eq!(children.len(), 2);
    for child in &children {
        assert_eq!(child.acting_identity, parent.acting_identity);
        assert!(child.prohibited_methods.contains(&"wash_trade".to_string()));
        assert!(
            child
                .prohibited_side_effects
                .contains(&"public_statement".to_string())
        );
    }

    let refused = |parts: Vec<GoalSpecDraft>| parent.decompose(parts).unwrap_err().to_string();
    // Each child fits alone; together they spend the budget twice.
    assert!(refused(vec![part(60, 20), part(41, 30)]).contains("budgets sum to 101"));
    assert!(refused(vec![part(60, 20), part(40, 31)]).contains("envelopes sum to 51"));
    let mut other = part(10, 10);
    other.acting_identity = s("someone-else");
    assert!(refused(vec![part(10, 10), other]).contains("outside its parent's identity"));
    let mut abroad = part(10, 10);
    abroad.jurisdictions = list(&["US", "KY"]);
    assert!(refused(vec![part(10, 10), abroad]).contains("jurisdiction `KY`"));
    let mut longer = part(10, 10);
    longer.time_horizon_secs = Some(3601);
    assert!(refused(vec![part(10, 10), longer]).contains("horizon"));
    assert!(refused(vec![part(10, 10)]).contains("at least two"));
    // A child is still a GoalSpec: one missing a declaration is refused.
    let mut vague = part(10, 10);
    vague.success_metric = None;
    assert!(refused(vec![part(10, 10), vague]).contains("`success_metric`"));
}

// ---- AGENCY-050 / 015 ------------------------------------------------------

fn outcome(state: &str, observed: i64, counterfactual: i64) -> ActionOutcomeDraft {
    ActionOutcomeDraft {
        intervention_context: s("tighten quotes on venue-a"),
        environment_state: s(state),
        action_sequence: list(&["quoter"]),
        causal_hypothesis: s("larger quote size narrows the spread"),
        predicted_effect: Some(d(5)),
        actual_observations: Some(d(observed)),
        attribution_confidence: Some(tenths(7)),
        side_effects: list(&[]),
        no_action_counterfactual: Some(d(counterfactual)),
    }
}

#[test]
fn an_action_outcome_missing_any_of_its_nine_fields_is_refused_and_not_remembered() {
    let cases: [(&str, fn(&mut ActionOutcomeDraft)); 9] = [
        ("intervention_context", |o| o.intervention_context = None),
        ("environment_state", |o| o.environment_state = None),
        ("action_sequence", |o| o.action_sequence = None),
        ("causal_hypothesis", |o| o.causal_hypothesis = None),
        ("predicted_effect", |o| o.predicted_effect = None),
        ("actual_observations", |o| o.actual_observations = None),
        ("attribution_confidence", |o| {
            o.attribution_confidence = None
        }),
        ("side_effects", |o| o.side_effects = None),
        ("no_action_counterfactual", |o| {
            o.no_action_counterfactual = None
        }),
    ];
    let mut memory = ActionOutcomeMemory::with_capacity(2).unwrap();
    memory
        .record(outcome("calm", 10, 4))
        .expect("premise: the full record is kept");
    assert_eq!(memory.len(), 1);
    for (field, wipe) in cases {
        let mut o = outcome("calm", 10, 4);
        wipe(&mut o);
        let err = memory.record(o).unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
        assert_eq!(memory.len(), 1, "{field}: a refused record was kept");
    }
    let mut no_actions = outcome("calm", 10, 4);
    no_actions.action_sequence = Some(vec![]);
    assert!(memory.record(no_actions).is_err());
    // Bounded: the third record displaces the first.
    memory.record(outcome("calm", 10, 4)).unwrap();
    memory.record(outcome("calm", 10, 4)).unwrap();
    assert_eq!(memory.len(), 2);
    assert!(ActionOutcomeMemory::with_capacity(0).is_err());
}

#[test]
fn the_learned_policy_selects_an_action_where_it_measurably_worked_and_no_action_where_it_did_not()
{
    // The observation is 10 in both regimes. Only against the counterfactual
    // do they differ: in `calm` nothing would have happened without the
    // action (4), in `stressed` the same 10 would have happened anyway.
    let mut memory = ActionOutcomeMemory::with_capacity(16).unwrap();
    for _ in 0..3 {
        memory.record(outcome("calm", 10, 4)).unwrap();
        memory.record(outcome("stressed", 10, 10)).unwrap();
    }
    assert_eq!(memory.len(), 6, "premise: both regimes were replayed");
    assert_eq!(
        memory.recommend("calm", 3).unwrap(),
        Some(vec!["quoter".to_string()])
    );
    assert_eq!(memory.recommend("stressed", 3).unwrap(), None);
    // Too few observations is not a finding, and neither is another regime's.
    assert_eq!(memory.recommend("calm", 4).unwrap(), None);
    assert_eq!(memory.recommend("unseen", 1).unwrap(), None);
    assert!(memory.recommend("calm", 0).is_err());
}
