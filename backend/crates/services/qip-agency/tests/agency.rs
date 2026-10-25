//! Contract tests for the agency crate. Each asserts its own premise first so
//! an empty fixture cannot pass a refusal test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic_in_result_fn,
    clippy::type_complexity
)]

use qip_agency::affordance::{AffordanceGraph, Method, ToolEdgeDraft};
use qip_agency::attribution::{Effect, HorizonObservation, Identification};
use qip_agency::comparison::ComparisonDraft;
use qip_agency::goal::{GoalClass, GoalSpec, GoalSpecDraft};
use qip_agency::plan::{ActingIdentity, Candidate, CandidateSet, InterventionPlan, PlanNode, Step};
use qip_agency::tools::{ToolRegistration, ToolRegistry};
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
        rollback_conditions: list(&["restore the prior quote width"]),
    }
}

// ---- AGENCY-003 / 041 ------------------------------------------------------

#[test]
fn a_goal_missing_any_one_of_its_thirteen_declarations_is_refused_naming_that_field() {
    type Wipe = fn(&mut GoalSpecDraft);
    let wipes: [(&str, Wipe); 13] = [
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
        ("rollback_conditions", |g| g.rollback_conditions = None),
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

fn tool(name: &str) -> ToolRegistration {
    ToolRegistration {
        tool: name.to_string(),
        acting_identity: "desk-agent".to_string(),
        permission_scope: BTreeSet::from(["quotes".to_string()]),
        disclosure_policy: "none required".to_string(),
        channels: BTreeSet::from(["paper-venue".to_string()]),
        max_actions_per_window: 2,
        budget_units: 100,
        revocation_path: "cancel all resting quotes".to_string(),
        provenance: "in-tree test tool".into(),
        sandbox_tests: ["sandbox-smoke"].into_iter().map(String::from).collect(),
        cost_latency: qip_contracts::action::CostLatency {
            cost_units: 1,
            latency_ms: 1,
        },
        allowed_callers: ["desk-agent"].into_iter().map(String::from).collect(),
    }
}

#[test]
fn a_tool_missing_any_of_its_declarations_is_not_registered() {
    let cases: [(&str, fn(&mut ToolRegistration)); 11] = [
        ("acting identity", |t| t.acting_identity = " ".into()),
        ("permission scope", |t| t.permission_scope.clear()),
        ("disclosure policy", |t| t.disclosure_policy.clear()),
        ("audience/channel rules", |t| t.channels.clear()),
        ("rate limit", |t| t.max_actions_per_window = 0),
        ("budget", |t| t.budget_units = 0),
        ("revocation path", |t| t.revocation_path.clear()),
        ("provenance", |t| t.provenance.clear()),
        ("sandbox tests", |t| t.sandbox_tests.clear()),
        ("cost and latency", |t| t.cost_latency.latency_ms = 0),
        ("allowed callers", |t| t.allowed_callers.clear()),
    ];
    let mut reg = ToolRegistry::new();
    assert!(
        reg.register(tool("probe")).is_ok(),
        "premise: the full registration is admitted"
    );
    for (field, wipe) in cases {
        let mut t = tool(field);
        wipe(&mut t);
        let err = reg.register(t).unwrap_err().to_string();
        assert!(err.contains(field), "{field}: {err}");
        assert!(
            reg.admits(field, "paper-venue", 0, 0).is_err(),
            "{field} stayed unregistered"
        );
    }
}

#[test]
fn a_tripped_rate_limit_or_revocation_refuses_the_next_call_and_every_decision_is_audited() {
    let at = qip_core::Timestamp::from_secs(1);
    let mut reg = ToolRegistry::new();
    reg.register(tool("quoter")).unwrap();
    reg.authorise("quoter", "paper-venue", 0, 1, at).unwrap();
    reg.authorise("quoter", "paper-venue", 0, 1, at).unwrap();
    let err = reg
        .authorise("quoter", "paper-venue", 0, 1, at)
        .unwrap_err();
    assert!(err.to_string().contains("rate limit"), "{err}");
    reg.authorise("quoter", "paper-venue", 1, 1, at)
        .expect("the next window admits it again");
    reg.revoke("quoter").unwrap();
    let err = reg
        .authorise("quoter", "paper-venue", 2, 1, at)
        .unwrap_err();
    assert!(err.to_string().contains("revoked"), "{err}");
    assert!(
        reg.register(tool("quoter")).is_err(),
        "re-registering is not a way round revocation"
    );
    assert_eq!(reg.audit().len(), 5);
    assert_eq!(reg.audit().iter().filter(|a| a.admitted).count(), 3);
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
fn an_attribution_without_a_chain_confounders_rivals_or_horizons_is_refused() {
    let full = || attribution(Method::Market, Identification::Experiment, 0.7);
    assert!(full().validate().is_ok(), "premise");
    let cases: [(&str, fn(&mut EffectAttribution)); 5] = [
        ("causal_chain", |a| a.causal_chain.clear()),
        ("confounders", |a| a.confounders.clear()),
        ("competing_explanations", |a| {
            a.competing_explanations.clear()
        }),
        ("observations", |a| a.observations.clear()),
        ("identifiability", |a| a.identifiability = 1.5),
    ];
    for (field, wipe) in cases {
        let mut a = full();
        wipe(&mut a);
        let err = a.validate().unwrap_err().to_string();
        assert!(err.contains(field), "{field}: {err}");
    }
}

// ============================================================================
// The engine, the gates and the autonomy ladder.
// ============================================================================

use qip_agency::autonomy::{ActionPolicy, Authority, WIDEN_AUTHORITY};
use qip_agency::causal::{Direction, Objective, Scenario, StructuralLink, StructuralModelDecl};
use qip_agency::conduct_brain::{
    CommunicationDraft, ConductPolicy, Disclosure, EvidenceCitation, EvidenceDecl,
    EvidenceManifest, HoldingsList, POSITION_DISCLOSURE,
};
use qip_agency::engine::{
    Adapter, Alternative, Decision, EdgeDecl, Execution, Gated, Governance, LearningOutcome,
    LeverEvidence, ORDER, Outcome, ProposalDraft, Request, Stage, StageLog, VariableDecl, decide,
    execute, learn, learn_shadowed, run, run_governed_with, run_with, widen,
};
use qip_agency::estimate::{
    Estimate, EstimatorKind, Experiment, bayesian_causal, correlation, matched_control,
    operator_formed_comparison, stage_assignment, staged_experiment,
};
use qip_agency::generate::ScenarioSource;
use qip_agency::generate::StructuralDecl;
use qip_agency::guard::{ActionGuard, ApprovalDecl, GuardDecl};
use qip_agency::memory::{ActionOutcomeDraft, ActionOutcomeMemory, Observed};
use qip_agency::plan::{Proposal, select};
use qip_agency::simulated;
use qip_contracts::action::Ground;
use qip_contracts::expansion::EffectAttribution;
use qip_core::Error;
use std::collections::BTreeMap;
use std::sync::Arc;

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

/// A complete attribution of one action, valid as built.
fn attribution(
    class: Method,
    identification: Identification,
    identifiability: f64,
) -> EffectAttribution {
    EffectAttribution {
        goal_id: "g1".into(),
        intervention_id: "i1".into(),
        action_id: "a1".into(),
        action_class: class,
        identification,
        observations: vec![
            HorizonObservation {
                horizon_ms: 60_000,
                value: 3.0,
            },
            HorizonObservation {
                horizon_ms: 3_600_000,
                value: 2.5,
            },
        ],
        counterfactual_method: "staged rollout control".into(),
        effect: Effect {
            mean: 3.0,
            std_dev: 0.5,
        },
        causal_chain: vec!["quote_size".into(), "spread".into()],
        confounders: vec!["macro_rate".into()],
        competing_explanations: vec!["seasonality".into()],
        side_effects: vec!["none observed".into()],
        identifiability,
        resulting_updates: vec!["raise quote-size prior".into()],
    }
}

/// The public face of the `quoter` step: what the venue and the tape see.
fn quote_draft() -> CommunicationDraft {
    CommunicationDraft {
        id: "quote-1".into(),
        content: "Quote 20 lots on venue-a at the published spread".into(),
        channel: "paper-venue".into(),
        sender_identity: "desk-agent".into(),
        evidence_citations: vec![EvidenceCitation {
            source_id: "venue-feed".into(),
            evidence_id: "spread-series-1".into(),
            claim: None,
            confidence_bps: 9000,
        }],
        disclosures: vec![Disclosure {
            kind: "paper_venue".into(),
            text: "Paper venue: no live order is placed".into(),
        }],
        target_audience: "venue-a participants".into(),
        jurisdiction: "US".into(),
    }
}

fn approvals() -> ApprovalDecl {
    ApprovalDecl {
        goal_approver: "pm-a".into(),
        goal_approved_at: qip_core::Timestamp::from_secs(10),
        plan_approver: "risk-b".into(),
        plan_approved_at: qip_core::Timestamp::from_secs(20),
        execute_at: qip_core::Timestamp::from_secs(30),
    }
}

/// A guard that admits the `quoter` step of [`request`], and nothing else.
fn guard_decl() -> GuardDecl {
    GuardDecl {
        conduct: ConductPolicy {
            authorized_sources: vec!["venue-feed".into()],
            authorized_identities: vec!["desk-agent".into()],
            required_disclosures: vec!["paper_venue".into()],
            channel_policies: BTreeMap::from([(
                "paper-venue".to_string(),
                vec!["venue-a participants".to_string()],
            )]),
            permitted_jurisdictions: vec!["US".into()],
            conduct_rules: qip_agency::conduct_brain::MANDATORY_RULES
                .iter()
                .map(|r| r.to_string())
                .collect(),
        },
        tools: vec![ToolRegistration {
            max_actions_per_window: 10,
            budget_units: 1_000,
            ..tool("quoter")
        }],
        communications: BTreeMap::from([("quoter".to_string(), quote_draft())]),
        grounds: vec![Ground::Evidence(
            "spread series spread-series-1 shows quotes under-sized".into(),
        )],
        approvals: approvals(),
        rate_window: 1,
    }
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

/// The desk's request. It carries the desk's structural model because a
/// pass may act only over a plan simulated against counterfactual branches
/// (R12-MODEL-4014); a test about a pass without one removes it.
fn request() -> Request {
    with_structural_model(Request {
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
        guards: Some(guard_decl()),
        pass: None,
        structural: None,
        touches: Vec::new(),
    })
}

fn identified(class: Method) -> EffectAttribution {
    attribution(class, Identification::Experiment, 0.7)
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
                // Without a structural model, so the plan reaches the
                // Simulate stage rather than being refused at Generate for
                // a claim its simulation does not bear out.
                r.structural = None;
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
    let g = graph();
    let gated = Gated {
        goal: &r.goal,
        graph: &g,
        plan: &plan,
        facts: &r.observed_facts,
        simulated: None,
    };
    let err = execute(&mut fresh, Some(&policy), gated, None, &mut adapter).unwrap_err();
    assert!(err.to_string().contains("out of order"), "{err}");
    let mut gate_failed = StageLog::new();
    for stage in &ORDER[..6] {
        gate_failed.pass(*stage).unwrap();
    }
    gate_failed.fail(Stage::Gate).unwrap();
    assert!(execute(&mut gate_failed, Some(&policy), gated, None, &mut adapter).is_err());
    assert!(adapter.calls.is_empty());
}

/// RM-P10-01 / AGENCY-012: a goal that no controllable variable with a tool
/// can reach is reported unreachable — stopped at `IdentifyLevers`, saying
/// so — and is never accepted or planned against.
///
/// Two failures this prevents. A planner that treated reachability loosely
/// would pull the one tool it holds whether or not that tool moves anything
/// the goal is stated over, and here a policy that may act stands behind it.
/// And without the stop, `select` refuses the stray proposal and the pass
/// ends `NoAction` — "nothing beat doing nothing" — which reads as a goal
/// that was considered and declined, not one that cannot be reached at all.
#[test]
fn a_goal_with_no_controllable_lever_is_reported_unreachable_and_never_planned_against() {
    let policy = narrow();

    // Premise: the unaltered request, under the same policy, executes. So
    // the goal, the proposal, the authority, the evidence and the guard are
    // sound, and whatever stops the passes below is the missing lever.
    let mut adapter = Counting::default();
    let reachable = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(
        matches!(reachable.outcome, Outcome::Executed(_)),
        "premise: the reachable goal executes, got {:?}",
        reachable.outcome
    );
    assert_eq!(reachable.levers, vec!["quote_size".to_string()]);
    assert_eq!(adapter.calls, vec!["quoter".to_string()]);

    // The tool moves `quote_size`, which no longer causes `spread`; what
    // does is a macro rate nobody here can move.
    let mut tool_off_the_path = request();
    tool_off_the_path.variables.push(VariableDecl {
        name: "macro_rate".into(),
        observable: true,
        controllable: false,
        owned: false,
    });
    tool_off_the_path.causes = vec![("macro_rate".into(), "spread".into())];
    // `quote_size` still causes `spread` and is controllable, but nothing
    // holds a tool on it.
    let mut lever_without_a_tool = request();
    lever_without_a_tool.tool_edges.clear();

    for (case, r) in [
        ("tool off the causal path", tool_off_the_path),
        ("controllable but no tool", lever_without_a_tool),
    ] {
        // Premise: an observable target and a complete proposal, so neither
        // Observe nor Predict is what stops this pass.
        assert_eq!(r.targets, vec!["spread".to_string()], "{case}");
        assert!(
            r.variables
                .iter()
                .any(|v| v.name == "spread" && v.observable),
            "{case}: premise, the target is observable"
        );
        assert_eq!(r.proposals.len(), 1, "{case}: premise, one proposal");

        let mut adapter = Counting::default();
        let report = run(r, Some(&policy), &mut adapter).unwrap();

        // Reported unreachable, by the stage that decides it, with its reason.
        assert_eq!(
            report.failed,
            Some(Stage::IdentifyLevers),
            "{case}: a goal no lever reaches did not stop where levers are identified; \
             outcome {:?}",
            report.outcome
        );
        assert_eq!(report.passed, &ORDER[..3], "{case}");
        assert_eq!(
            report.outcome,
            Outcome::Stopped("no controllable variable with a tool reaches a target".to_string()),
            "{case}"
        );
        assert!(report.levers.is_empty(), "{case}: {:?}", report.levers);
        // Never planned against: nothing generated, refused, simulated or
        // called.
        assert!(report.candidates.is_empty(), "{case}");
        assert!(report.refused.is_empty(), "{case}");
        assert!(report.simulations.is_empty(), "{case}");
        assert!(adapter.calls.is_empty(), "{case}: {:?}", adapter.calls);
    }
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
    let correlational = attribution(Method::Market, Identification::NotIdentified, 0.0);
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
    let unnamed = attribution(
        Method::Market,
        Identification::Observational {
            strategy: " ".into(),
        },
        0.7,
    );
    assert!(unnamed.validate().is_err());
    let mut fresh = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    assert!(fresh.widen(&unnamed, &operator()).is_err());
    assert_eq!(fresh.authority(), Authority::Shadow);
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
        audience: s("venue-a participants"),
        delay_secs: Some(300),
        action_sequence: list(&["quoter"]),
        causal_hypothesis: s("larger quote size narrows the spread"),
        predicted_effect: Some(d(5)),
        actual_observations: Some(d(observed)),
        attribution_confidence: Some(tenths(7)),
        side_effects: list(&[]),
        no_action_counterfactual: Some(d(counterfactual)),
        observed: Some(Observed::Measured),
    }
}

#[test]
fn an_action_outcome_missing_any_of_its_nine_fields_or_its_audience_or_delay_is_refused_and_not_remembered()
 {
    let cases: [(&str, fn(&mut ActionOutcomeDraft)); 12] = [
        ("intervention_context", |o| o.intervention_context = None),
        ("environment_state", |o| o.environment_state = None),
        ("audience", |o| o.audience = None),
        ("delay_secs", |o| o.delay_secs = None),
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
        // The twelfth: whether the figures were measured or simulated. A
        // record that does not say is a simulation one reader away from
        // being filed as a measurement.
        ("observed", |o| o.observed = None),
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

// ---- R12-RES-7110: a withheld plan is kept, and a truthful alternative sought ----

/// The desk's request with a second public step, `announcer`, beside the
/// `quoter`. `announcement` is what the announcer says.
fn two_voice_request(announcement: &str) -> Request {
    let mut r = request();
    r.tool_edges.push(EdgeDecl {
        variable: "quote_size".into(),
        edge: edge("announcer", 10, "quote", &[]),
    });
    r.proposals[0].root = PlanNode::Sequence(vec![step("quoter", 10), step("announcer", 5)]);
    let guards = r.guards.as_mut().unwrap();
    guards.tools.push(ToolRegistration {
        max_actions_per_window: 10,
        budget_units: 1_000,
        ..tool("announcer")
    });
    let mut said = quote_draft();
    said.id = "announce-1".into();
    said.content = announcement.into();
    guards.communications.insert("announcer".to_string(), said);
    r
}

#[test]
fn a_withheld_plan_is_kept_with_its_reasons_and_the_alternative_is_the_plan_without_the_step_that_lied()
 {
    let policy = narrow();
    let honest = "Announce 5 lots on venue-a at the published spread";
    let mut adapter = Counting::default();
    let clear = run(two_voice_request(honest), Some(&policy), &mut adapter).unwrap();
    assert!(
        matches!(&clear.outcome, Outcome::Executed(steps) if steps.len() == 2),
        "premise: with an honest announcement both steps are sent: {:?}",
        clear.outcome
    );
    assert!(
        clear.blocked.is_none(),
        "premise: nothing withheld, nothing kept"
    );

    let mut adapter = Counting::default();
    let report = run(
        two_voice_request("Announcing a guaranteed fill on venue-a"),
        Some(&policy),
        &mut adapter,
    )
    .unwrap();
    assert!(
        matches!(report.outcome, Outcome::Stopped(_)),
        "{:?}",
        report.outcome
    );
    assert!(adapter.calls.is_empty(), "the lie reached no adapter");
    let blocked = report.blocked.expect("the withheld plan is retained");
    let tools = |steps: &[Step]| steps.iter().map(|s| s.tool.clone()).collect::<Vec<_>>();
    assert_eq!(tools(&blocked.steps), ["quoter", "announcer"]);
    assert_eq!(tools(&blocked.offending), ["announcer"]);
    assert!(
        blocked.reason.contains("no_false_claims"),
        "{}",
        blocked.reason
    );
    match &blocked.alternative {
        Alternative::Reduced(steps) => assert_eq!(tools(steps), ["quoter"]),
        other => panic!("the quoter alone clears, so it is the alternative: {other:?}"),
    }

    // Every step lying leaves nothing to propose but the baseline.
    let mut all = two_voice_request("Announcing a guaranteed fill on venue-a");
    all.guards
        .as_mut()
        .unwrap()
        .communications
        .get_mut("quoter")
        .unwrap()
        .content = "Quote 20 lots: a guaranteed fill on venue-a".into();
    let report = run(all, Some(&policy), &mut Counting::default()).unwrap();
    let blocked = report.blocked.expect("retained");
    assert_eq!(blocked.offending.len(), 2);
    assert!(matches!(blocked.alternative, Alternative::NoAction(_)));
}

// ---- R12-COVERAGE-7188: an experiment this crate runs, and one it does not --------

fn cells() -> Vec<String> {
    (0..12).map(|i| format!("cell-{i:02}")).collect()
}

/// An outcome per cell: held-out cells drift around 1.0, treated ones 4.0.
fn spreads(experiment: &Experiment) -> BTreeMap<String, f64> {
    cells()
        .into_iter()
        .enumerate()
        .map(|(i, cell)| {
            let base = if experiment.treated().contains(&cell) {
                4.0
            } else {
                1.0
            };
            (cell, base + (i % 3) as f64 * 0.1)
        })
        .collect()
}

/// An attribution that claims exactly what `estimate` shows.
fn attributing(estimate: &Estimate) -> EffectAttribution {
    let mut a = attribution(Method::Market, estimate.identification().clone(), 0.7);
    if *estimate.identification() == Identification::NotIdentified {
        a.identifiability = 0.0;
    }
    a.effect.mean = estimate.effect();
    a
}

/// R12-COVERAGE-7188: two groups an operator formed earn no experiment
/// label, even when they are the very groups a hold-out drew and carry the
/// very outcomes it analysed. The failure prevented is the selection-effect
/// bypass: a hold-out refused for a dropped cell, resubmitted as two groups
/// the caller says were randomised, used to widen on `Experiment`.
///
/// Mutation: label `operator_formed_comparison` `Identification::Experiment`
/// — the label assertion fails, `widen` accepts, and `supports` admits an
/// attribution claiming `Experiment`.
#[test]
fn an_operator_formed_group_comparison_earns_no_experiment_label_and_widens_nothing() {
    let experiment = Experiment::design("seed-2026-10-18", &cells(), 2500).unwrap();
    let outcomes = spreads(&experiment);
    let drawn = experiment.analyse(&outcomes).unwrap();
    let group =
        |cells: &BTreeSet<String>| -> Vec<f64> { cells.iter().map(|c| outcomes[c]).collect() };
    let formed =
        operator_formed_comparison(&group(experiment.treated()), &group(experiment.control()))
            .unwrap();

    // Premise: the same data through the drawn assignment earns the label,
    // widens, and the two estimates agree on the effect — so what differs is
    // the label, not the arithmetic.
    assert_eq!(*drawn.identification(), Identification::Experiment);
    assert!((formed.effect() - drawn.effect()).abs() < 1e-9);
    let mut admitted = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    admitted.widen(&attributing(&drawn), &operator()).unwrap();

    assert_eq!(*formed.identification(), Identification::NotIdentified);
    // Claiming the label the drawn assignment earned is refused by the
    // estimator, and an honest attribution of the formed groups is refused
    // by the policy.
    let claimed = attributing(&drawn);
    let err = formed.supports(&claimed).unwrap_err();
    assert!(err.to_string().contains("earned by the method"), "{err}");
    let mut policy = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    let honest = attributing(&formed);
    formed.supports(&honest).unwrap();
    let err = policy.widen(&honest, &operator()).unwrap_err();
    assert!(err.to_string().contains("not identified"), "{err}");
    assert_eq!(policy.authority(), Authority::Shadow);
}

#[test]
fn a_hold_out_drawn_here_earns_the_experiment_label_and_a_correlation_of_the_same_cells_widens_nothing()
 {
    let units = cells();
    let experiment = Experiment::design("seed-2026-10-18", &units, 2500).unwrap();
    assert_eq!(experiment.control().len(), 3, "premise: a quarter held out");
    assert_eq!(experiment.treated().len(), 9);
    assert!(
        experiment.treated().is_disjoint(experiment.control()),
        "premise: no cell on both sides"
    );
    // Pure in the seed and the units; and the seed matters, or the operator
    // could pick the split by choosing nothing.
    assert_eq!(
        experiment,
        Experiment::design("seed-2026-10-18", &units, 2500).unwrap()
    );
    let splits: BTreeSet<Vec<String>> = (0..8)
        .map(|n| {
            Experiment::design(&format!("seed-{n}"), &units, 2500)
                .unwrap()
                .control()
                .iter()
                .cloned()
                .collect()
        })
        .collect();
    assert!(splits.len() > 1, "eight seeds drew one split: {splits:?}");

    let estimate = experiment.analyse(&spreads(&experiment)).unwrap();
    assert_eq!(estimate.kind(), EstimatorKind::HoldOutExperiment);
    assert_eq!(*estimate.identification(), Identification::Experiment);
    assert!(
        estimate.effect() > 2.9 && estimate.effect() < 3.1,
        "{}",
        estimate.effect()
    );

    // The experimental attribution widens; the observational one over the
    // same cells does not, and cannot borrow the label either.
    let mut policy = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    let experimental = attributing(&estimate);
    estimate.supports(&experimental).unwrap();
    policy.widen(&experimental, &operator()).unwrap();
    assert_eq!(policy.authority(), Authority::NarrowReversible);

    let outcomes = spreads(&experiment);
    let (x, y): (Vec<f64>, Vec<f64>) = units
        .iter()
        .map(|u| (f64::from(experiment.treated().contains(u)), outcomes[u]))
        .unzip();
    let observed = correlation(&x, &y).unwrap();
    assert_eq!(*observed.identification(), Identification::NotIdentified);
    let mut fresh = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    let err = fresh
        .widen(&attributing(&observed), &operator())
        .unwrap_err();
    assert!(err.to_string().contains("not identified"), "{err}");
    assert_eq!(fresh.authority(), Authority::Shadow);
    assert!(
        observed.supports(&experimental).is_err(),
        "the label is earned by the method"
    );

    // Per horizon, every figure identified by the same assignment.
    let horizons = BTreeMap::from([(60, spreads(&experiment)), (3_600, spreads(&experiment))]);
    let per = experiment.analyse_horizons(&horizons).unwrap();
    assert_eq!(per.len(), 2);
    assert!(
        per.values()
            .all(|e| *e.identification() == Identification::Experiment)
    );
}

#[test]
fn an_experiment_is_refused_when_its_hold_out_is_degenerate_or_its_outcomes_do_not_match_the_design()
 {
    let units = cells();
    assert!(Experiment::design("", &units, 2500).is_err(), "blank seed");
    assert!(Experiment::design("s", &units, 0).is_err(), "no hold-out");
    assert!(
        Experiment::design("s", &units, 10_000).is_err(),
        "all held out"
    );
    assert!(
        Experiment::design("s", &units[..3], 5000).is_err(),
        "one side under two"
    );
    let mut twice = units.clone();
    twice.push(units[0].clone());
    assert!(
        Experiment::design("s", &twice, 2500).is_err(),
        "a unit twice"
    );

    let experiment = Experiment::design("s", &units, 2500).unwrap();
    let full = spreads(&experiment);
    assert!(
        experiment.analyse(&full).is_ok(),
        "premise: the full set is analysed"
    );
    let mut dropped = full.clone();
    let unit = experiment.control().iter().next().unwrap().clone();
    dropped.remove(&unit);
    assert!(
        experiment.analyse(&dropped).is_err(),
        "a held-out cell dropped after the fact"
    );
    let mut extra = full;
    extra.insert("cell-99".into(), 9.0);
    assert!(
        experiment.analyse(&extra).is_err(),
        "an outcome for a cell never assigned"
    );
    assert!(experiment.analyse_horizons(&BTreeMap::new()).is_err());
}

// ---- R12-AGENCY-6102: matched, staged and Bayesian estimators ----------------

#[test]
fn matched_controls_recover_an_effect_a_naive_comparison_misses_and_refuse_a_treated_unit_with_no_match()
 {
    // Outcome = 2 x covariate + 3 if treated. The treated sit high on the
    // covariate, so a plain difference in means credits them with the lift
    // the covariate gave them.
    let treated_x = [5.0, 6.0, 7.0];
    let pool_x = [1.0, 2.0, 3.0, 4.0, 5.1, 6.1, 7.1];
    let treated_y: Vec<f64> = treated_x.iter().map(|x| 2.0 * x + 3.0).collect();
    let pool_y: Vec<f64> = pool_x.iter().map(|x| 2.0 * x).collect();
    let rows = |xs: &[f64]| xs.iter().map(|x| vec![*x]).collect::<Vec<_>>();
    let naive = treated_y.iter().sum::<f64>() / 3.0 - pool_y.iter().sum::<f64>() / 7.0;
    assert!(
        (naive - 3.0).abs() > 3.0,
        "premise: the naive gap {naive} is mostly the covariate, not the effect of 3"
    );

    let matched =
        matched_control(&rows(&treated_x), &treated_y, &rows(&pool_x), &pool_y, 0.2).unwrap();
    assert_eq!(matched.kind(), EstimatorKind::MatchedControl);
    // Each pair differs by 0.1 on the covariate, worth 0.2: 3 - 0.2.
    assert!(
        (matched.effect() - 2.8).abs() < 1e-9,
        "{}",
        matched.effect()
    );
    assert!(
        matches!(matched.identification(), Identification::Observational { strategy } if strategy.contains("matching")),
        "{:?}",
        matched.identification()
    );
    // Without a unit near enough, the estimate is refused, not computed over
    // the rest.
    let far = matched_control(
        &rows(&[5.0, 6.0, 40.0]),
        &[13.0, 15.0, 83.0],
        &rows(&pool_x),
        &pool_y,
        0.2,
    );
    assert!(
        far.unwrap_err()
            .to_string()
            .contains("no unused control within")
    );
    assert!(
        matched_control(
            &rows(&treated_x),
            &treated_y,
            &rows(&pool_x[..2]),
            &pool_y[..2],
            0.2
        )
        .is_err()
    );
    assert!(matched_control(&rows(&treated_x), &treated_y, &rows(&pool_x), &pool_y, 0.0).is_err());
    assert!(matched_control(&vec![vec![]; 3], &treated_y, &vec![vec![]; 7], &pool_y, 0.2).is_err());
}

#[test]
fn a_staged_rollout_drawn_here_is_an_experiment_in_every_period_and_a_missing_period_or_unit_is_refused()
 {
    let units = cells();
    let stage_of = stage_assignment("rollout-seed", &units, 3).unwrap();
    let per_stage: Vec<usize> = (0..3)
        .map(|s| stage_of.values().filter(|v| **v == s).count())
        .collect();
    assert_eq!(
        per_stage,
        [4, 4, 4],
        "premise: twelve cells in three equal waves"
    );
    assert_ne!(
        stage_of,
        stage_assignment("another-seed", &units, 3).unwrap(),
        "premise: the seed moves the waves"
    );
    let outcomes = |lift: f64| -> BTreeMap<usize, BTreeMap<String, f64>> {
        (1..3)
            .map(|period| {
                let per_unit = units
                    .iter()
                    .map(|u| {
                        let treated = stage_of[u] < period;
                        (u.clone(), 1.0 + if treated { lift } else { 0.0 })
                    })
                    .collect();
                (period, per_unit)
            })
            .collect()
    };
    let estimate = staged_experiment("rollout-seed", &units, 3, &outcomes(2.0)).unwrap();
    assert_eq!(estimate.kind(), EstimatorKind::StagedExperiment);
    assert_eq!(*estimate.identification(), Identification::Experiment);
    assert!(
        (estimate.effect() - 2.0).abs() < 1e-9,
        "{}",
        estimate.effect()
    );

    let mut no_second_period = outcomes(2.0);
    no_second_period.remove(&2);
    assert!(staged_experiment("rollout-seed", &units, 3, &no_second_period).is_err());
    let mut dropped = outcomes(2.0);
    dropped.get_mut(&1).unwrap().remove(&units[0]);
    assert!(staged_experiment("rollout-seed", &units, 3, &dropped).is_err());
    assert!(staged_experiment("", &units, 3, &outcomes(2.0)).is_err());
    assert!(
        stage_assignment("s", &units[..5], 3).is_err(),
        "waves of one have no variance"
    );
    assert!(stage_assignment("s", &units, 1).is_err());
}

#[test]
fn a_bayesian_causal_estimate_follows_the_data_under_a_wide_prior_and_the_prior_under_a_tight_one()
{
    // y = 1 + 3 d + 0.5 x with a small deterministic wobble; treated units
    // have larger x, so x is a confounder the model adjusts for.
    let n = 12;
    let d: Vec<f64> = (0..n).map(|i| f64::from(i % 2 == 0)).collect();
    let x: Vec<Vec<f64>> = (0..n)
        .map(|i| vec![f64::from(i) + if i % 2 == 0 { 3.0 } else { 0.0 }])
        .collect();
    let y: Vec<f64> = (0..n as usize)
        .map(|i| 1.0 + 3.0 * d[i] + 0.5 * x[i][0] + [0.05, -0.04, 0.02][i % 3])
        .collect();
    let wide = bayesian_causal(&d, &x, &y, 100.0).unwrap();
    assert_eq!(wide.kind(), EstimatorKind::BayesianCausal);
    assert!((wide.effect() - 3.0).abs() < 0.1, "{}", wide.effect());
    assert!(wide.standard_error().unwrap() > 0.0);
    assert!(
        matches!(wide.identification(), Identification::Observational { strategy } if strategy.contains("Bayesian")),
        "{:?}",
        wide.identification()
    );
    let tight = bayesian_causal(&d, &x, &y, 1e-3).unwrap();
    assert!(
        tight.effect().abs() < 0.5,
        "a prior of width 0.001 pulls the effect toward zero: {}",
        tight.effect()
    );
    // Refused: a non-indicator, everyone treated, a blank prior, collinear
    // covariates.
    let mut odd = d.clone();
    odd[0] = 2.0;
    assert!(bayesian_causal(&odd, &x, &y, 1.0).is_err());
    assert!(bayesian_causal(&vec![1.0; n as usize], &x, &y, 1.0).is_err());
    assert!(bayesian_causal(&d, &x, &y, 0.0).is_err());
    let copies: Vec<Vec<f64>> = d.iter().map(|v| vec![*v]).collect();
    assert!(
        bayesian_causal(&d, &copies, &y, 1.0).is_err(),
        "the treatment as its own covariate"
    );
}

// ---- R12-AGENCY-6088: scenarios a composition root derives from a twin -----------

#[derive(Debug)]
struct FakeTwin(Vec<Scenario>);

impl ScenarioSource for FakeTwin {
    fn scenarios(&self) -> Result<Vec<Scenario>, Error> {
        Ok(self.0.clone())
    }
}

fn twin_scenario(name: &str, spread: f64) -> Scenario {
    Scenario {
        name: name.into(),
        exogenous: BTreeMap::from([("spread".to_string(), spread)]),
    }
}

#[test]
fn a_candidate_is_simulated_once_per_model_and_per_scenario_the_composition_roots_twin_adds() {
    let governance = || Governance {
        policy: None,
        guard: None,
        eligibility: None,
    };
    let mut plain = request();
    plain.guards = None;
    let base =
        run_governed_with(plain.clone(), governance(), None, &mut Counting::default()).unwrap();
    let models = 1;
    let declared = 2;
    assert_eq!(
        base.simulations[0].results.len(),
        models * declared,
        "premise: the request's one model under its two declared scenarios"
    );

    let twin = FakeTwin(vec![
        twin_scenario("twin-a", 9.0),
        twin_scenario("twin-b", -3.0),
    ]);
    let with = run_governed_with(
        plain.clone(),
        governance(),
        Some(&twin),
        &mut Counting::default(),
    )
    .unwrap();
    let results = &with.simulations[0].results;
    assert_eq!(results.len(), models * (declared + 2), "{results:?}");
    let scenarios: BTreeSet<&str> = results.iter().map(|r| r.scenario.as_str()).collect();
    assert_eq!(
        scenarios,
        BTreeSet::from(["calm", "stressed", "twin-a", "twin-b"])
    );
    let a = results.iter().find(|r| r.scenario == "twin-a").unwrap();
    let calm = results.iter().find(|r| r.scenario == "calm").unwrap();
    assert!(
        (a.no_action - calm.no_action).abs() > 1e-9,
        "the twin's exogenous value moved the branch: {a:?} vs {calm:?}"
    );

    // A twin scenario named like a declared one is two branches under one name.
    let clash = FakeTwin(vec![twin_scenario("calm", 1.0)]);
    let err =
        run_governed_with(plain, governance(), Some(&clash), &mut Counting::default()).unwrap_err();
    assert!(err.to_string().contains("already declares"), "{err}");
}

// ---- R12-AGENCY-1056 / 1057: citations resolved against the platform's record -----

const SPREAD_STATEMENT: &str = "quoted spreads on venue-a are wider than the desk target";

fn spread_manifest(source: &str) -> Arc<EvidenceManifest> {
    Arc::new(
        EvidenceManifest::new(vec![EvidenceDecl {
            id: "spread-series-1".into(),
            source_id: source.into(),
            statement: SPREAD_STATEMENT.into(),
        }])
        .unwrap(),
    )
}

/// The `quoter` decl with its citation stating the claim it supports.
fn cited_decl() -> GuardDecl {
    let mut decl = guard_decl();
    decl.communications
        .get_mut("quoter")
        .unwrap()
        .evidence_citations[0]
        .claim = Some(SPREAD_STATEMENT.into());
    decl
}

#[test]
fn a_citation_that_resolves_to_no_record_the_wrong_source_or_another_statement_withholds_the_plan_before_any_adapter_call()
 {
    type Mutate = fn(&mut GuardDecl);
    let policy = narrow();
    let attempt = |decl: GuardDecl, manifest: Arc<EvidenceManifest>| {
        let mut guard = ActionGuard::from_decl(decl)
            .unwrap()
            .with_evidence(manifest);
        let mut plain = request();
        plain.guards = None;
        let mut adapter = Counting::default();
        let report = run_with(plain, Some(&policy), Some(&mut guard), &mut adapter).unwrap();
        (report.outcome, adapter.calls.len())
    };

    let (outcome, calls) = attempt(cited_decl(), spread_manifest("venue-feed"));
    assert!(
        matches!(outcome, Outcome::Executed(_)),
        "premise: a citation that resolves and says what the record says is sent: {outcome:?}"
    );
    assert_eq!(calls, 1, "premise");

    let cases: Vec<(&str, &str, Mutate)> = vec![
        // Both checks refuse an unresolved id; this needle is factual support's.
        (
            "is not support until it names a record",
            "venue-feed",
            |d| {
                d.communications
                    .get_mut("quoter")
                    .unwrap()
                    .evidence_citations[0]
                    .evidence_id = "spread-series-9".into();
            },
        ),
        ("is not the statement the record holds", "venue-feed", |d| {
            d.communications
                .get_mut("quoter")
                .unwrap()
                .evidence_citations[0]
                .claim = Some("quoted spreads on venue-a are tight".into());
        }),
        ("states no claim", "venue-feed", |d| {
            d.communications
                .get_mut("quoter")
                .unwrap()
                .evidence_citations[0]
                .claim = None;
        }),
        // The citation declares the authorized source; the record says the
        // evidence came from somewhere else.
        ("the record names 'blog'", "blog", |_| {}),
    ];
    for (needle, recorded_source, mutate) in cases {
        let mut decl = cited_decl();
        mutate(&mut decl);
        let (outcome, calls) = attempt(decl, spread_manifest(recorded_source));
        match outcome {
            Outcome::Stopped(why) => assert!(why.contains(needle), "{needle}: {why}"),
            other => panic!("{needle}: the plan was not withheld: {other:?}"),
        }
        assert_eq!(calls, 0, "{needle}: an adapter was called");
    }
}

#[test]
fn an_evidence_manifest_refuses_a_blank_or_repeated_record() {
    let record = |id: &str, source: &str, statement: &str| EvidenceDecl {
        id: id.into(),
        source_id: source.into(),
        statement: statement.into(),
    };
    assert!(
        EvidenceManifest::new(vec![record("a", "s", "x")]).is_ok(),
        "premise"
    );
    assert!(EvidenceManifest::new(vec![record(" ", "s", "x")]).is_err());
    assert!(EvidenceManifest::new(vec![record("a", "", "x")]).is_err());
    assert!(EvidenceManifest::new(vec![record("a", "s", "  ")]).is_err());
    assert!(
        EvidenceManifest::new(vec![record("a", "s", "x"), record("a", "t", "y")]).is_err(),
        "two records under one id"
    );
}

// ---- R12-AGENCY-1066: undisclosed promotion of what the platform holds ------------

#[test]
fn a_draft_naming_an_instrument_the_platform_holds_needs_a_position_disclosure_or_is_withheld() {
    let policy = narrow();
    let holdings = || Arc::new(HoldingsList::new(vec!["venue-a bonds".to_string()]).unwrap());
    let attempt = |decl: GuardDecl, held: Arc<HoldingsList>| {
        let mut guard = ActionGuard::from_decl(decl).unwrap().with_holdings(held);
        let mut plain = request();
        plain.guards = None;
        let mut adapter = Counting::default();
        let report = run_with(plain, Some(&policy), Some(&mut guard), &mut adapter).unwrap();
        (report.outcome, adapter.calls.len())
    };
    let promotional = |decl: &mut GuardDecl| {
        decl.communications.get_mut("quoter").unwrap().content =
            "Quote 20 lots: venue-a bonds are the better risk on this tape".into();
    };

    // Premise: the same words, with the platform holding nothing of the kind,
    // pass; so the refusal below is the holding and not the wording.
    let mut elsewhere = guard_decl();
    promotional(&mut elsewhere);
    let (outcome, calls) = attempt(
        elsewhere,
        Arc::new(HoldingsList::new(vec!["venue-b notes".to_string()]).unwrap()),
    );
    assert!(
        matches!(outcome, Outcome::Executed(_)),
        "premise: {outcome:?}"
    );
    assert_eq!(calls, 1, "premise");

    let mut undisclosed = guard_decl();
    promotional(&mut undisclosed);
    let (outcome, calls) = attempt(undisclosed, holdings());
    match outcome {
        Outcome::Stopped(why) => {
            assert!(why.contains("undisclosed promotion"), "{why}");
            assert!(why.contains("venue-a bonds"), "{why}");
        }
        other => panic!("the promotion was not withheld: {other:?}"),
    }
    assert_eq!(calls, 0, "an adapter was called");

    // A blank position disclosure discloses nothing.
    let mut blank = guard_decl();
    promotional(&mut blank);
    blank
        .communications
        .get_mut("quoter")
        .unwrap()
        .disclosures
        .push(Disclosure {
            kind: POSITION_DISCLOSURE.into(),
            text: "  ".into(),
        });
    let (outcome, calls) = attempt(blank, holdings());
    assert!(matches!(outcome, Outcome::Stopped(_)), "{outcome:?}");
    assert_eq!(calls, 0);

    let mut disclosed = guard_decl();
    promotional(&mut disclosed);
    disclosed
        .communications
        .get_mut("quoter")
        .unwrap()
        .disclosures
        .push(Disclosure {
            kind: POSITION_DISCLOSURE.into(),
            text: "The desk holds venue-a bonds".into(),
        });
    let (outcome, calls) = attempt(disclosed, holdings());
    assert!(matches!(outcome, Outcome::Executed(_)), "{outcome:?}");
    assert_eq!(calls, 1);

    assert!(
        HoldingsList::new(vec!["  ".into()]).is_err(),
        "a blank holding names nothing"
    );
}

// ---- R12-AGENCY-6108: for whom, and with what delay -------------------------

#[test]
fn what_worked_for_one_audience_is_not_recommended_for_another_and_the_delay_is_learned() {
    let reaching = |audience: &str, observed: i64, delay: u64| {
        let mut o = outcome("calm", observed, 4);
        o.audience = s(audience);
        o.delay_secs = Some(delay);
        o
    };
    let mut memory = ActionOutcomeMemory::with_capacity(16).unwrap();
    for delay in [100, 200, 300] {
        memory.record(reaching("dealers", 10, delay)).unwrap();
        memory.record(reaching("retail", 4, delay)).unwrap();
    }
    let any = memory.recommend_for("calm", None, 3).unwrap();
    assert!(any.is_some(), "premise: pooled, the action wins");
    let dealers = memory
        .recommend_for("calm", Some("dealers"), 3)
        .unwrap()
        .unwrap();
    assert_eq!(dealers.action_sequence, vec!["quoter".to_string()]);
    assert_eq!(dealers.mean_delay_secs, 200);
    assert_eq!(dealers.mean_effect, d(6));
    assert_eq!(dealers.support, 3);
    // For retail the observation equals the counterfactual: doing nothing wins.
    assert_eq!(
        memory.recommend_for("calm", Some("retail"), 3).unwrap(),
        None
    );
}

#[test]
fn three_measured_outcomes_at_zero_attribution_confidence_recommend_nothing_however_positive_their_mean()
 {
    // The failure this prevents (ADR 0145): the ambient door files its
    // measured outcomes at confidence zero, so before the filter the only
    // records `recommend_for` could read on the running path were unidentified
    // ones, and a subject whose reading happened to rise after three drafts
    // yielded `Some(..)`. The only guard was the door's discretion.
    let at_confidence = |tenths_of: i128| {
        let mut memory = ActionOutcomeMemory::with_capacity(16).unwrap();
        for _ in 0..3 {
            let mut o = outcome("calm", 10, 4);
            o.attribution_confidence = Some(tenths(tenths_of));
            memory.record(o).unwrap();
        }
        memory
    };
    assert!(
        at_confidence(7)
            .recommend_for("calm", None, 3)
            .unwrap()
            .is_some(),
        "premise: the same three at confidence 0.7 recommend the action"
    );
    let unidentified = at_confidence(0);
    assert_eq!(unidentified.len(), 3, "the records are kept regardless");
    assert_eq!(
        unidentified.recommend_for("calm", None, 3).unwrap(),
        None,
        "and none of them is evidence a recommendation may rest on"
    );
}

// ---- R12-AGENCY-1056..1069 / 6091 / GOV-6123 / 6124 / 6127: the guard -------

#[test]
fn an_adapter_is_never_called_without_a_guard_even_at_narrow_reversible_authority() {
    let policy = narrow();
    let mut adapter = Counting::default();
    let acted = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(
        matches!(acted.outcome, Outcome::Executed(_)),
        "premise: guarded, the same request is sent"
    );
    assert_eq!(adapter.calls.len(), 1, "premise");

    let mut unguarded = request();
    unguarded.guards = None;
    let mut adapter = Counting::default();
    let err = run(unguarded.clone(), Some(&policy), &mut adapter).unwrap_err();
    assert!(err.to_string().contains("no action guard"), "{err}");
    assert!(adapter.calls.is_empty());
    // Shadow without a guard is the shipped CLI's path and is unchanged.
    let report = run(unguarded, None, &mut adapter).unwrap();
    assert!(matches!(report.outcome, Outcome::Shadowed(_)));
    assert!(report.approvals.is_none());
}

/// One mutation of the guard per case; each alone withholds the plan with
/// the reason naming it, in shadow and at narrow-reversible alike, and no
/// adapter is called. The unmutated guard executes, so every withheld case
/// below is that one change.
#[test]
fn each_guard_check_alone_withholds_the_plan_before_any_adapter_call() {
    type Mutate = fn(&mut Request);
    let cases: Vec<(&str, Mutate)> = vec![
        // R12-AGENCY-1065: promissory content, refused before the adapter.
        ("no_false_claims", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().content =
                "Quote 20 lots: a guaranteed fill on venue-a".into();
        }),
        // R12-AGENCY-1059: the required disclosure absent, and present with
        // blank text, each withhold; a disclosure is its text, not its kind.
        ("disclosures", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications
                .get_mut("quoter")
                .unwrap()
                .disclosures
                .clear();
        }),
        ("Required disclosure 'paper_venue'", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().disclosures[0].text = "  ".into();
        }),
        ("no_fake_consensus", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().content =
                "Quote 20 lots: everyone is buying venue-a".into();
        }),
        ("no_rumour_engineering", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().content =
                "Quote 20 lots; rumour has it venue-a widens".into();
        }),
        ("no_manipulation", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().content =
                "Layering 20 lots of quotes on venue-a".into();
        }),
        ("audience_permissions", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().target_audience = "retail".into();
        }),
        ("provenance", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications
                .get_mut("quoter")
                .unwrap()
                .evidence_citations[0]
                .source_id = "chat-room".into();
        }),
        ("jurisdiction", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().jurisdiction = "KY".into();
        }),
        ("fabricated identity", |r| {
            let g = r.guards.as_mut().unwrap();
            g.communications.get_mut("quoter").unwrap().sender_identity = "desk-agent-2".into();
            g.conduct.authorized_identities.push("desk-agent-2".into());
        }),
        ("not registered", |r| {
            r.guards.as_mut().unwrap().tools.clear();
        }),
        ("no drafted public face", |r| {
            r.guards.as_mut().unwrap().communications.clear();
        }),
        ("budget would be exceeded", |r| {
            r.guards.as_mut().unwrap().tools[0].budget_units = 29;
        }),
        ("outside the tool's channels", |r| {
            r.guards.as_mut().unwrap().tools[0].channels = BTreeSet::from(["press".to_string()]);
        }),
        ("whole non-negative units", |r| {
            r.tool_edges[0].edge.cost = Some(Decimal::from_scaled(305, 1).unwrap());
        }),
        ("expected profit", |r| {
            r.guards.as_mut().unwrap().grounds = vec![Ground::ExpectedProfit];
        }),
        ("approving its own action", |r| {
            r.guards.as_mut().unwrap().approvals.plan_approver = "desk-agent".into();
        }),
        ("precede the plan approval", |r| {
            r.guards.as_mut().unwrap().approvals.execute_at = qip_core::Timestamp::from_secs(15);
        }),
    ];
    let policy = narrow();
    for (why, mutate) in cases {
        for authority in [Some(&policy), None] {
            let mut r = request();
            mutate(&mut r);
            let mut adapter = Counting::default();
            let report = run(r, authority, &mut adapter).unwrap();
            let Outcome::Stopped(reason) = &report.outcome else {
                panic!("{why}: expected withheld, got {:?}", report.outcome)
            };
            assert!(reason.contains(why), "{why}: {reason}");
            assert!(adapter.calls.is_empty(), "{why}: an adapter was called");
            assert_eq!(report.failed, Some(Stage::Act), "{why}");
            assert_eq!(report.passed, &ORDER[..7], "{why}");
            assert!(report.approvals.is_none(), "{why}");
        }
    }
}

#[test]
fn a_whole_word_conduct_rule_does_not_withhold_a_quote_whose_words_merely_contain_one() {
    let policy = narrow();
    let mut r = request();
    r.guards
        .as_mut()
        .unwrap()
        .communications
        .get_mut("quoter")
        .unwrap()
        .content = "Quote 20 lots on unwashed, spoofproof, storefront-running venue-a".into();
    let mut adapter = Counting::default();
    let report = run(r, Some(&policy), &mut adapter).unwrap();
    assert!(
        matches!(report.outcome, Outcome::Executed(_)),
        "{:?}",
        report.outcome
    );
    assert_eq!(adapter.calls.len(), 1);
}

#[test]
fn an_executed_pass_leaves_three_linked_approval_records_and_an_audited_registry_decision() {
    let policy = narrow();
    let mut adapter = Counting::default();
    let report = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(matches!(report.outcome, Outcome::Executed(_)), "premise");
    let chain = report.approvals.expect("approvals recorded");
    let execution = chain.execution.expect("an execution record once sent");
    assert_eq!(chain.plan.goal, chain.goal.id);
    assert_eq!(execution.plan, chain.plan.id);
    assert_eq!(
        (
            chain.goal.approver.as_str(),
            chain.plan.approver.as_str(),
            execution.executor.as_str()
        ),
        ("pm-a", "risk-b", "desk-agent")
    );
    assert_eq!(report.audit.len(), 1);
    assert!(report.audit[0].admitted && report.audit[0].tool == "quoter");

    // In shadow the goal and plan approvals exist; no execution does.
    let report = run(request(), None, &mut adapter).unwrap();
    assert!(matches!(report.outcome, Outcome::Shadowed(_)));
    assert!(report.approvals.unwrap().execution.is_none());
    assert!(report.audit.is_empty(), "shadow spent nothing");
}

#[test]
fn a_guard_kept_across_passes_holds_the_rate_window_and_the_revocation() {
    let policy = narrow();
    let mut decl = guard_decl();
    decl.tools[0].max_actions_per_window = 1;
    let mut guard = ActionGuard::from_decl(decl).unwrap();
    let bare = || {
        let mut r = request();
        r.guards = None;
        r
    };
    let mut adapter = Counting::default();
    let first = run_with(bare(), Some(&policy), Some(&mut guard), &mut adapter).unwrap();
    assert!(matches!(first.outcome, Outcome::Executed(_)), "premise");
    let second = run_with(bare(), Some(&policy), Some(&mut guard), &mut adapter).unwrap();
    let Outcome::Stopped(why) = &second.outcome else {
        panic!("{:?}", second.outcome)
    };
    assert!(why.contains("rate limit"), "{why}");
    guard.advance(2, approvals());
    let third = run_with(bare(), Some(&policy), Some(&mut guard), &mut adapter).unwrap();
    assert!(matches!(third.outcome, Outcome::Executed(_)));
    guard.revoke("quoter").unwrap();
    guard.advance(3, approvals());
    let fourth = run_with(bare(), Some(&policy), Some(&mut guard), &mut adapter).unwrap();
    let Outcome::Stopped(why) = &fourth.outcome else {
        panic!("{:?}", fourth.outcome)
    };
    assert!(why.contains("revoked"), "{why}");
    assert_eq!(adapter.calls.len(), 2, "only the two admitted passes sent");
    // Two guards are two sources of truth.
    assert!(run_with(request(), Some(&policy), Some(&mut guard), &mut adapter).is_err());
}

/// R12-AGENCY-1064 / 6099: the same words under a second identity, in a
/// later pass. Inside one pass every draft speaks as the goal's one identity,
/// so the coordinated-voices check cannot see this; the guard's memory of
/// what it released, and the journal it is resumed from, can. No phrase is
/// involved: the message is an innocent quote, and the pattern is the only
/// thing that is wrong with it.
#[test]
fn the_same_message_released_under_a_second_identity_in_a_later_pass_is_withheld_as_fake_consensus()
{
    use qip_agency::guard::{GuardPolicy, Voice, voice_fingerprint};
    let policy = narrow();
    // A second desk identity with its own registered tool, saying the same
    // thing the first says.
    let second_draft = || CommunicationDraft {
        id: "quote-2".into(),
        sender_identity: "desk-agent-2".into(),
        ..quote_draft()
    };
    let second_request = || {
        let mut r = request();
        r.guards = None;
        r.goal = GoalSpecDraft {
            acting_identity: s("desk-agent-2"),
            ..draft(GoalClass::Liquidity)
        }
        .build()
        .unwrap();
        r.identity = ActingIdentity {
            name: "desk-agent-2".into(),
            ..who(&["quote"])
        };
        r.tool_edges[0].edge.tool = s("quoter-2");
        r.proposals[0].root = step("quoter-2", 20);
        r
    };
    let decl = || {
        let mut decl = guard_decl();
        decl.conduct
            .authorized_identities
            .push("desk-agent-2".into());
        decl.tools.push(ToolRegistration {
            acting_identity: "desk-agent-2".into(),
            max_actions_per_window: 10,
            budget_units: 1_000,
            allowed_callers: BTreeSet::from(["desk-agent-2".to_string()]),
            ..tool("quoter-2")
        });
        decl.communications
            .insert("quoter-2".to_string(), second_draft());
        decl
    };
    let first_request = || {
        let mut r = request();
        r.guards = None;
        r
    };

    // Premise: alone, the second identity's pass is admitted and sent.
    let mut fresh = ActionGuard::from_decl(decl()).unwrap();
    let mut adapter = Counting::default();
    let alone = run_with(
        second_request(),
        Some(&policy),
        Some(&mut fresh),
        &mut adapter,
    )
    .unwrap();
    assert!(
        matches!(alone.outcome, Outcome::Executed(_)),
        "premise: {:?}",
        alone.outcome
    );
    assert_eq!(adapter.calls, vec!["quoter-2".to_string()]);
    assert_eq!(
        alone.released,
        vec![Voice {
            tool: "quoter-2".into(),
            identity: "desk-agent-2".into(),
            fingerprint: voice_fingerprint(&quote_draft().content),
            content: Some(quote_draft().content),
        }],
        "the report carries the voice the pass released"
    );

    // One guard across passes: the first identity says it, then the second.
    let mut guard = ActionGuard::from_decl(decl()).unwrap();
    let mut adapter = Counting::default();
    let first = run_with(
        first_request(),
        Some(&policy),
        Some(&mut guard),
        &mut adapter,
    )
    .unwrap();
    assert!(matches!(first.outcome, Outcome::Executed(_)), "premise");
    guard.advance(2, approvals());
    let second = run_with(
        second_request(),
        Some(&policy),
        Some(&mut guard),
        &mut adapter,
    )
    .unwrap();
    let Outcome::Stopped(why) = &second.outcome else {
        panic!("{:?}", second.outcome)
    };
    assert!(why.contains("fake consensus across passes"), "{why}");
    assert!(
        why.contains("desk-agent-2") && why.contains("`desk-agent`"),
        "{why}"
    );
    assert_eq!(
        adapter.calls,
        vec!["quoter".to_string()],
        "nothing more was sent"
    );
    assert_eq!(second.failed, Some(Stage::Act));
    assert!(second.released.is_empty());

    // The first identity repeating itself is not consensus.
    guard.advance(3, approvals());
    let again = run_with(
        first_request(),
        Some(&policy),
        Some(&mut guard),
        &mut adapter,
    )
    .unwrap();
    assert!(
        matches!(again.outcome, Outcome::Executed(_)),
        "{:?}",
        again.outcome
    );

    // Resumed from what a previous process released, a fresh guard refuses
    // the same thing; resuming during a pass is refused.
    let mut resumed = ActionGuard::from_policy(GuardPolicy {
        conduct: decl().conduct,
        tools: decl().tools,
    })
    .unwrap();
    resumed.resume_voices(first.released.clone()).unwrap();
    resumed.advance(4, approvals());
    assert!(resumed.resume_voices(Vec::new()).is_err());
    let mut pass = second_request();
    pass.pass = Some(qip_agency::guard::PassDecl {
        communications: BTreeMap::from([("quoter-2".to_string(), second_draft())]),
        grounds: guard_decl().grounds,
        approvals: approvals(),
        rate_window: 4,
    });
    let mut adapter = Counting::default();
    let replayed = run_with(pass, Some(&policy), Some(&mut resumed), &mut adapter).unwrap();
    let Outcome::Stopped(why) = &replayed.outcome else {
        panic!("{:?}", replayed.outcome)
    };
    assert!(why.contains("fake consensus across passes"), "{why}");
    assert!(adapter.calls.is_empty());
}

// ---- R12-AGENCY-6103 / 6107 / 054: learning after an action ---------------

/// A log that has been through `Act`, as an executed pass leaves it.
fn acted_log() -> StageLog {
    let mut log = StageLog::new();
    for stage in &ORDER[..8] {
        log.pass(*stage).unwrap();
    }
    log
}

fn quoted() -> Vec<Step> {
    vec![Step {
        tool: "quoter".into(),
        variable: "quote_size".into(),
        exposure: d(20),
    }]
}

#[test]
fn learning_runs_only_after_act_and_only_about_the_steps_that_ran() {
    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();
    let evidence = identified(Method::Market);
    // Premise: after Act, the matching outcome is learned and remembered.
    let mut log = acted_log();
    let authority = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &evidence,
    )
    .unwrap();
    assert_eq!(authority, Authority::NarrowReversible);
    assert_eq!(memory.len(), 1);
    assert_eq!(log.passed(), &ORDER[..]);

    // Before Act: refused, nothing remembered.
    let mut before = StageLog::new();
    for stage in &ORDER[..7] {
        before.pass(*stage).unwrap();
    }
    let err = learn(
        &mut before,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &evidence,
    )
    .unwrap_err();
    assert!(err.to_string().contains("out of order"), "{err}");
    assert_eq!(memory.len(), 1);

    // Crediting a tool that did not run.
    let mut log = acted_log();
    let mut credited = outcome("calm", 7, 4);
    credited.action_sequence = list(&["delist"]);
    let err = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        credited,
        &evidence,
    )
    .unwrap_err();
    assert!(err.to_string().contains("ran"), "{err}");
    assert_eq!(log.failed(), Some(Stage::ObserveEffect));
    assert_eq!(memory.len(), 1);
}

#[test]
fn an_outcome_claiming_more_confidence_than_its_attribution_identifies_is_refused_and_not_remembered()
 {
    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();
    // The outcome claims 0.7.
    let at = |identifiability: f64| {
        attribution(Method::Market, Identification::Experiment, identifiability)
    };
    let mut log = acted_log();
    learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &at(0.7),
    )
    .expect("premise: an attribution identified at 0.7 supports 0.7");
    let mut log = acted_log();
    let err = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &at(0.2),
    )
    .unwrap_err();
    assert!(err.to_string().contains("not upgraded"), "{err}");
    assert_eq!(log.failed(), Some(Stage::Attribute));
    assert_eq!(memory.len(), 1, "the refused outcome was not kept");
    // An attribution of another class of action is not about this one.
    let mut log = acted_log();
    let other = attribution(Method::Research, Identification::Experiment, 0.9);
    assert!(
        learn(
            &mut log,
            &quoted(),
            &mut policy,
            &mut memory,
            outcome("calm", 7, 4),
            &other
        )
        .is_err()
    );
}

#[test]
fn an_effect_outside_the_prediction_learned_after_an_action_returns_the_policy_to_shadow() {
    // narrow() predicts uplift in [1, 5]; observed 14 against 4 is 10.
    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();
    let mut log = acted_log();
    let in_range = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &identified(Method::Market),
    )
    .unwrap();
    assert_eq!(in_range, Authority::NarrowReversible, "premise");
    let mut log = acted_log();
    let drifted = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 14, 4),
        &identified(Method::Market),
    )
    .unwrap();
    assert_eq!(drifted, Authority::Shadow);
    assert_eq!(memory.len(), 2, "the drifted outcome is remembered too");
    // And the next pass, guard and all, sends nothing.
    let mut adapter = Counting::default();
    let report = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(matches!(report.outcome, Outcome::Shadowed(_)));
    assert!(adapter.calls.is_empty());
}

#[test]
fn widening_leaves_a_record_of_its_evidence_and_approver_and_refuses_a_correlation() {
    let mut policy = ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap();
    let correlation = attribution(Method::Market, Identification::NotIdentified, 0.0);
    assert!(widen(&mut policy, &correlation, &operator()).is_err());
    // A correlation dressed up with a high identifiability is refused by
    // the contract before the policy reads it.
    let dressed = attribution(Method::Market, Identification::NotIdentified, 0.95);
    let err = widen(&mut policy, &dressed, &operator()).unwrap_err();
    assert!(err.to_string().contains("not identified"), "{err}");
    assert_eq!(policy.authority(), Authority::Shadow);
    let record = widen(&mut policy, &identified(Method::Market), &operator()).unwrap();
    assert_eq!(policy.authority(), Authority::NarrowReversible);
    assert_eq!(record.approver, "risk-officer");
    assert_eq!(record.class, Method::Market);
    assert_eq!(record.action_id, "a1");
}

#[test]
fn execute_reports_withheld_rather_than_shadow_when_the_guard_refuses_in_shadow() {
    let r = request();
    let plan = InterventionPlan::plan(&r.goal, &graph(), &r.identity, step("quoter", 20)).unwrap();
    let g = graph();
    let gated = Gated {
        goal: &r.goal,
        graph: &g,
        plan: &plan,
        facts: &r.observed_facts,
        simulated: None,
    };
    let at_gate = || {
        let mut log = StageLog::new();
        for stage in &ORDER[..7] {
            log.pass(*stage).unwrap();
        }
        log
    };
    let mut adapter = Counting::default();
    let mut guard = ActionGuard::from_decl(guard_decl()).unwrap();
    let acted = execute(&mut at_gate(), None, gated, Some(&mut guard), &mut adapter).unwrap();
    assert!(matches!(acted.execution, Execution::Shadow(_)), "premise");
    let mut decl = guard_decl();
    decl.grounds = vec![Ground::DesiredPriceImpact];
    let mut refusing = ActionGuard::from_decl(decl).unwrap();
    let mut log = at_gate();
    let acted = execute(&mut log, None, gated, Some(&mut refusing), &mut adapter).unwrap();
    assert!(matches!(acted.execution, Execution::Withheld(_)));
    assert_eq!(log.failed(), Some(Stage::Act));
}

/// R12-AGENCY-1063 / 1064: one voice posing as several. Two steps send the
/// same message, the second under another name: withheld before either call,
/// with no approval record. Two checks stand against it and the cheaper one
/// fires first: the identity check (`guard.rs`, "fabricated identity") runs
/// before the coordinated-voices check, and every step's identity must equal
/// the goal's, so the voices check is the backstop for the day that equality
/// is relaxed, not the line that fires today. Mutation: delete the
/// `draft.sender_identity != registration.acting_identity ...` refusal in
/// `ActionGuard::preflight` and this test fails on the reason, which becomes
/// `coordinated_voices` — proof the backstop holds and is the only thing then
/// between the sockpuppet and the adapter.
#[test]
fn the_same_message_under_two_identities_is_withheld_before_any_call() {
    let policy = narrow();
    // Premise: the fixture without the second voice clears every gate.
    let baseline = run(request(), Some(&policy), &mut Counting::default()).unwrap();
    assert!(
        matches!(baseline.outcome, Outcome::Executed(_)),
        "{:?}",
        baseline.outcome
    );
    let mut r = request();
    r.tool_edges.push(EdgeDecl {
        variable: "quote_size".into(),
        edge: edge("quoter2", 30, "quote", &[]),
    });
    r.proposals = vec![ProposalDraft {
        root: PlanNode::Sequence(vec![step("quoter", 20), step("quoter2", 20)]),
        comparison: prediction(d(5), true),
    }];
    let g = r.guards.as_mut().unwrap();
    g.tools.push(ToolRegistration {
        acting_identity: "desk-agent-2".into(),
        max_actions_per_window: 10,
        budget_units: 1_000,
        ..tool("quoter2")
    });
    g.conduct.authorized_identities.push("desk-agent-2".into());
    let mut second = quote_draft();
    second.id = "quote-2".into();
    second.sender_identity = "desk-agent-2".into();
    g.communications.insert("quoter2".into(), second);
    let mut adapter = Counting::default();
    let report = run(r, Some(&policy), &mut adapter).unwrap();
    let Outcome::Stopped(reason) = &report.outcome else {
        panic!("expected withheld, got {:?}", report.outcome)
    };
    assert!(reason.contains("fabricated identity"), "{reason}");
    assert!(adapter.calls.is_empty(), "an adapter was called");
    assert!(report.approvals.is_none());
    assert!(
        report.audit.is_empty(),
        "nothing was charged: {:?}",
        report.audit
    );
}

// ---- shadow learning: the simulated outcome, declared simulated ----------

/// `request()` with the desk's structural model: quote size narrows the
/// spread by 0.6 per unit, one quoter step moves it by 10, under a calm and
/// a stressed (+3) scenario. Hand computation, objective "decrease spread":
/// calm 6 -> 0, stressed 9 -> 3, effect 6 in both branches.
fn with_structural_model(mut request: Request) -> Request {
    request.structural = Some(StructuralDecl {
        models: vec![StructuralModelDecl {
            name: "desk".into(),
            links: vec![StructuralLink {
                cause: "quote_size".into(),
                effect: "spread".into(),
                coefficient: -0.6,
            }],
            variables: Vec::new(),
            intercepts: BTreeMap::from([("spread".to_string(), 6.0)]),
            noise_variance: BTreeMap::new(),
        }],
        fitted: Vec::new(),
        scenarios: vec![
            Scenario {
                name: "calm".into(),
                exogenous: BTreeMap::new(),
            },
            Scenario {
                name: "stressed".into(),
                exogenous: BTreeMap::from([("spread".to_string(), 3.0)]),
            },
        ],
        objective: Objective {
            variable: "spread".into(),
            direction: Direction::Decrease,
        },
        // Every tool a test registers on `quote_size` moves it alike, so a
        // test about a second tool is not a test about its actuation.
        actuation: ["quoter", "quoter2", "quoter-2", "delist", "announcer"]
            .into_iter()
            .map(|tool| (tool.to_string(), 10.0))
            .collect(),
        generate: None,
    });
    request
}

/// A shadowed act holds `Act`'s slot without passing it, and what may be
/// learned after it is exactly an outcome that says it is simulated: a
/// measured outcome after a shadowed act measured nothing, and a simulated
/// outcome after an executed act would replace the observation that exists.
/// Mutation: in `engine::learn_outcome`, make the `(true, Observed::Measured)`
/// arm fall through — the measured-after-shadowed assertions fail.
#[test]
fn learning_after_a_shadowed_act_admits_only_an_outcome_that_says_it_is_simulated() {
    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();

    // Executed act, simulated outcome: refused at observe-effect.
    let mut log = acted_log();
    let mut simulated = outcome("calm", 7, 4);
    simulated.observed = Some(Observed::Simulated);
    let err = learn(
        &mut log,
        &quoted(),
        &mut policy,
        &mut memory,
        simulated,
        &identified(Method::Market),
    )
    .unwrap_err();
    assert!(err.to_string().contains("prediction"), "{err}");
    assert_eq!(log.failed(), Some(Stage::ObserveEffect));
    assert!(memory.is_empty());

    // Shadowed act: off the passed list, and a measured outcome is refused.
    let report = run(request(), None, &mut Counting::default()).unwrap();
    assert!(matches!(report.outcome, Outcome::Shadowed(_)), "premise");
    assert!(report.act_shadowed, "premise: the act was shadowed");
    assert_eq!(report.passed, &ORDER[..7], "a shadowed act did not pass");
    let learning = learn_shadowed(
        &report,
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &identified(Method::Market),
    )
    .unwrap();
    let LearningOutcome::Withheld(reason) = &learning.outcome else {
        panic!("{learning:?}")
    };
    assert!(reason.contains("nothing ran"), "{reason}");
    assert_eq!(learning.failed, Some(Stage::ObserveEffect));
    assert!(memory.is_empty());

    // Shadowed act, simulated outcome: learned, through Update.
    let mut simulated = outcome("calm", 7, 4);
    simulated.observed = Some(Observed::Simulated);
    let learning = learn_shadowed(
        &report,
        &mut policy,
        &mut memory,
        simulated,
        &identified(Method::Market),
    )
    .unwrap();
    let LearningOutcome::Learned(learned) = &learning.outcome else {
        panic!("{learning:?}")
    };
    assert_eq!(learned.outcome.observed, Observed::Simulated);
    assert_eq!(learned.effect, d(3));
    assert_eq!(
        (learned.before, learned.after),
        (Authority::NarrowReversible, Authority::NarrowReversible)
    );
    assert_eq!(learning.passed, [&ORDER[..7], &ORDER[8..]].concat());
    assert_eq!(learning.failed, None);
    assert_eq!(memory.len(), 1);

    // A pass that shadowed no plan offers nothing: an error, not a guess.
    let mut over = request();
    over.proposals[0].root = step("quoter", 51);
    let report = run(over, None, &mut Counting::default()).unwrap();
    assert_eq!(report.outcome, Outcome::NoAction, "premise");
    assert!(!report.act_shadowed);
    let mut simulated = outcome("calm", 7, 4);
    simulated.observed = Some(Observed::Simulated);
    assert!(
        learn_shadowed(
            &report,
            &mut policy,
            &mut memory,
            simulated,
            &identified(Method::Market)
        )
        .is_err()
    );
    assert_eq!(memory.len(), 1);
}

/// The evidence a shadowed pass offers is its own simulation, computed from
/// the report and labelled simulated in every field a reader could mistake:
/// the figures match the hand computation in `with_structural_model`, the
/// drift rule runs on them (narrow() predicts [1, 5]; the model says 6, so
/// the widened policy returns to shadow), the memory keeps the record, and
/// recommends nothing on it however positive it is, while a measured record
/// in the same state is recommended. With no structural model the evidence
/// is refused by name, not invented.
/// Mutations: in `simulated::evidence`, write `Effect { mean: 0.0, std_dev:
/// 0.0 }` (the attribution assertions fail); in `memory::recommend_for`,
/// drop the `observed != Measured` filter (the `None` assertion fails).
#[test]
fn a_shadowed_pass_learns_its_simulated_effect_and_nothing_is_recommended_on_it() {
    let report = run(
        with_structural_model(request()),
        None,
        &mut Counting::default(),
    )
    .unwrap();
    assert!(
        matches!(report.outcome, Outcome::Shadowed(_)),
        "premise: {:?}",
        report.outcome
    );
    let evidence = simulated::evidence(&report, 7, "spread under 5 bps").unwrap();
    assert_eq!(evidence.class, Method::Market);
    let o = &evidence.outcome;
    assert_eq!(o.observed, Some(Observed::Simulated));
    assert_eq!(o.action_sequence, list(&["quoter"]));
    assert_eq!(
        o.environment_state.as_deref(),
        Some("simulated: desk/calm, desk/stressed")
    );
    assert_eq!(o.audience.as_deref(), Some("simulated: nobody reached"));
    assert_eq!(
        o.intervention_context.as_deref(),
        Some("shadow pass 7: spread under 5 bps")
    );
    assert_eq!(o.delay_secs, Some(60), "the plan's own time to effect");
    assert_eq!(o.predicted_effect, Some(d(5)));
    assert_eq!(o.attribution_confidence, Some(tenths(9)));
    assert_eq!(o.actual_observations, Decimal::from_f64(-1.5));
    assert_eq!(o.no_action_counterfactual, Decimal::from_f64(-7.5));
    assert_eq!(o.side_effects, Some(vec![]));
    let a = &evidence.attribution;
    assert_eq!(a.action_class, Method::Market);
    assert_eq!(
        a.effect,
        Effect {
            mean: 6.0,
            std_dev: 0.0
        }
    );
    assert_eq!(
        a.observations,
        vec![HorizonObservation {
            horizon_ms: 60_000,
            value: 6.0
        }]
    );
    assert!(
        (a.identifiability - 0.9).abs() < 1e-12,
        "{}",
        a.identifiability
    );
    assert_eq!(a.causal_chain, ["quote_size", "spread"]);
    assert_eq!(a.goal_id, "agency-pass-7:goal");
    assert!(
        matches!(&a.identification, Identification::Observational { strategy }
            if strategy.starts_with("simulated:") && strategy.contains("nothing was observed")),
        "{:?}",
        a.identification
    );
    assert!(a.validate().is_ok(), "the evidence is a valid attribution");

    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();
    let learning = learn_shadowed(
        &report,
        &mut policy,
        &mut memory,
        evidence.outcome.clone(),
        &evidence.attribution,
    )
    .unwrap();
    let LearningOutcome::Learned(learned) = &learning.outcome else {
        panic!("{learning:?}")
    };
    assert_eq!(learned.effect, d(6));
    assert_eq!(learned.predicted_uplift, (d(1), d(5)));
    assert_eq!(
        (learned.before, learned.after),
        (Authority::NarrowReversible, Authority::Shadow)
    );
    assert_eq!(policy.authority(), Authority::Shadow);
    assert_eq!(memory.len(), 1);

    // Positive, remembered, and recommended on by nothing.
    let state = "simulated: desk/calm, desk/stressed";
    assert_eq!(memory.recommend(state, 1).unwrap(), None);
    memory.record(outcome(state, 7, 4)).unwrap();
    assert_eq!(
        memory.recommend(state, 1).unwrap(),
        Some(vec!["quoter".to_string()]),
        "a measured record in the same state is"
    );

    // No model, no simulated outcome: refused by name.
    let mut unmodelled = request();
    unmodelled.structural = None;
    let bare = run(unmodelled, None, &mut Counting::default()).unwrap();
    assert!(matches!(bare.outcome, Outcome::Shadowed(_)), "premise");
    let err = simulated::evidence(&bare, 8, "spread under 5 bps").unwrap_err();
    assert!(
        err.to_string().contains("no structural model simulated"),
        "{err}"
    );
}

/// R12-AGENCY-1060: market conduct judged on the step, not only the words.
/// A pass that would release the same quote four times is withheld at the
/// act stage with no adapter call, though each copy reads innocently alone
/// and no phrase rule fires; the same plan releasing it three times — the
/// most one pass may — executes, so the gate is a rate and not a refusal of
/// repetition as such.
/// Mutation: in `guard::ActionGuard::preflight`, drop the
/// `self.gate.step_failure(&faces)` push — the four-step pass executes and
/// the first assertion fails.
#[test]
fn a_pass_releasing_one_message_above_the_repetition_rate_is_withheld_before_any_adapter_call() {
    let policy = narrow();
    let repeated = |times: usize| {
        let mut r = request();
        r.tool_edges[0].edge.cost = Some(d(1));
        r.proposals[0].root = PlanNode::Sequence((0..times).map(|_| step("quoter", 1)).collect());
        r
    };
    let mut adapter = Counting::default();
    let report = run(
        repeated(qip_agency::conduct_brain::MAX_RELEASES_PER_MESSAGE),
        Some(&policy),
        &mut adapter,
    )
    .unwrap();
    assert!(
        matches!(report.outcome, Outcome::Executed(_)),
        "premise: at the rate the pass executes, {:?}",
        report.outcome
    );
    assert_eq!(adapter.calls.len(), 3, "premise");

    for authority in [Some(&policy), None] {
        let mut adapter = Counting::default();
        let report = run(
            repeated(qip_agency::conduct_brain::MAX_RELEASES_PER_MESSAGE + 1),
            authority,
            &mut adapter,
        )
        .unwrap();
        let Outcome::Stopped(reason) = &report.outcome else {
            panic!("expected withheld, got {:?}", report.outcome)
        };
        assert!(reason.contains("step_conduct"), "{reason}");
        assert!(reason.contains("4 times"), "{reason}");
        assert!(adapter.calls.is_empty(), "an adapter was called");
        assert_eq!(report.failed, Some(Stage::Act));
    }
}

/// R12-CONTRACT-7082: a shadow pass the guard clears yields one §27
/// `ActionIntent` per step — who acts for whom, through which channel,
/// the payload and its digest, provenance, disclosure, every policy that
/// admitted it, an idempotency key and the audit lineage — and the record
/// round-trips through its strict decoder, while the same record with its
/// payload edited after review is refused. A withheld pass yields none.
/// Mutation: in `guard::intent`, digest a constant instead of `payload` —
/// `ActionIntent::new` refuses, the pass is withheld and the first
/// assertion fails.
#[test]
fn a_shadow_pass_yields_a_decodable_action_intent_and_an_edited_payload_is_refused() {
    use qip_contracts::action_intent::{ActionIntent, Verdict};
    let mut adapter = Counting::default();
    let report = run(request(), None, &mut adapter).unwrap();
    assert!(
        matches!(report.outcome, Outcome::Shadowed(_)),
        "premise: {:?}",
        report.outcome
    );
    assert!(adapter.calls.is_empty(), "premise: shadow called nothing");
    assert_eq!(report.intents.len(), 1, "one intent per cleared step");
    let intent = &report.intents[0];
    let chain = report.approvals.as_ref().unwrap();
    assert_eq!(intent.identity.actor, "desk-agent");
    assert_eq!(intent.identity.on_behalf_of, chain.goal.approver);
    assert_eq!(intent.channel, "paper-venue");
    assert!(intent.provenance.contains(&chain.plan.id));
    assert_eq!(
        intent.audit_lineage,
        vec![chain.goal.id.clone(), chain.plan.id.clone()]
    );
    assert!(
        intent
            .policy_decisions
            .iter()
            .all(|d| d.verdict == Verdict::Allowed)
    );

    let bytes = intent.encode().unwrap();
    assert_eq!(&ActionIntent::decode(&bytes).unwrap(), intent);
    let mut edited: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let payload = edited["payload"]
        .as_str()
        .unwrap()
        .replace("20 lots", "2000 lots");
    assert_ne!(
        payload, intent.payload,
        "premise: the edit changed the payload"
    );
    edited["payload"] = serde_json::Value::String(payload);
    let refusal = ActionIntent::decode(&serde_json::to_vec(&edited).unwrap()).unwrap_err();
    assert!(refusal.to_string().contains("payload_digest"), "{refusal}");

    let mut withheld = request();
    withheld.guards.as_mut().unwrap().grounds = vec![Ground::ExpectedProfit];
    let report = run(withheld, None, &mut adapter).unwrap();
    assert!(matches!(report.outcome, Outcome::Stopped(_)), "premise");
    assert!(
        report.intents.is_empty(),
        "a refused pass carries no intent"
    );
}

/// R12-RES-7111: a side effect the policy did not predict does not only
/// return it to shadow; the memory files a failure episode naming the plan,
/// the departure, the authority it cost, and the plan-specific stop and
/// roll-back. An outcome that held to the prediction files none.
/// Mutation: in `engine::learn_outcome`, drop `memory.keep_failure` — the
/// memory holds no episode and the second assertion block fails.
#[test]
fn a_side_effect_the_policy_did_not_predict_files_a_failure_episode_with_its_stop_and_roll_back() {
    use qip_agency::autonomy::Drift;
    use qip_agency::memory::Mitigation;
    let mut policy = narrow();
    let mut memory = ActionOutcomeMemory::with_capacity(4).unwrap();
    let held = learn(
        &mut acted_log(),
        &quoted(),
        &mut policy,
        &mut memory,
        outcome("calm", 7, 4),
        &identified(Method::Market),
    )
    .unwrap();
    assert_eq!(held, Authority::NarrowReversible, "premise");
    assert_eq!(
        memory.failures().count(),
        0,
        "a held prediction is no failure"
    );

    let mut surprising = outcome("calm", 7, 4);
    surprising.side_effects = list(&["venue_complaint"]);
    let after = learn(
        &mut acted_log(),
        &quoted(),
        &mut policy,
        &mut memory,
        surprising,
        &identified(Method::Market),
    )
    .unwrap();
    assert_eq!(after, Authority::Shadow);
    let episodes: Vec<_> = memory.failures().collect();
    assert_eq!(episodes.len(), 1, "one failure, one episode");
    let episode = episodes[0];
    assert_eq!(episode.action_sequence, vec!["quoter".to_string()]);
    assert_eq!(
        episode.drift,
        vec![Drift::UnpredictedSideEffect("venue_complaint".into())]
    );
    assert_eq!(
        (episode.authority_before, episode.authority_after),
        (Authority::NarrowReversible, Authority::Shadow)
    );
    assert_eq!(
        episode.mitigations,
        vec![
            Mitigation::Halt {
                tools: vec!["quoter".into()]
            },
            Mitigation::Reverse {
                tool: "quoter".into()
            },
        ],
        "a measured, executed plan is halted and its step reversed"
    );
}

/// R12-MODEL-4014 / R12-COVERAGE-7186: authority to act is used only over a
/// plan simulated against counterfactual branches, and only when every
/// branch of every competing model says it helps. The desk's model alone
/// agrees and the pass executes; with no model the same authority sends
/// nothing; with a second model that says quoting widens the spread, the
/// plan is weakly identified and nothing is sent, though in shadow it is
/// still simulated and shadowed (the gate is on acting, not on thinking).
/// Mutation: in `engine::counterfactually_supported`, read only the first
/// branch (`results.iter().take(1)`) — the dissenting model is never
/// consulted, the contested pass executes and the weak-identification
/// assertion fails.
#[test]
fn authority_is_used_only_over_a_plan_every_competing_model_agrees_helps() {
    let policy = narrow();
    let mut adapter = Counting::default();
    let agreed = run(request(), Some(&policy), &mut adapter).unwrap();
    assert!(
        matches!(agreed.outcome, Outcome::Executed(_)),
        "premise: one agreeing model, {:?}",
        agreed.outcome
    );
    assert_eq!(adapter.calls.len(), 1, "premise");

    let mut unmodelled = request();
    unmodelled.structural = None;
    let mut adapter = Counting::default();
    let report = run(unmodelled, Some(&policy), &mut adapter).unwrap();
    let Outcome::Stopped(reason) = &report.outcome else {
        panic!("expected withheld, got {:?}", report.outcome)
    };
    assert!(reason.contains("simulated against none"), "{reason}");
    assert!(adapter.calls.is_empty());
    assert_eq!(report.failed, Some(Stage::Act));

    let contested = || {
        let mut r = request();
        let decl = r.structural.as_mut().unwrap();
        let mut skeptic = decl.models[0].clone();
        skeptic.name = "skeptic".into();
        skeptic.links[0].coefficient = 0.6;
        decl.models.push(skeptic);
        r
    };
    let mut adapter = Counting::default();
    let report = run(contested(), Some(&policy), &mut adapter).unwrap();
    assert!(
        report
            .simulations
            .iter()
            .flat_map(|s| &s.results)
            .any(|b| b.model == "skeptic"),
        "premise: the second model was simulated"
    );
    let Outcome::Stopped(reason) = &report.outcome else {
        panic!("expected withheld, got {:?}", report.outcome)
    };
    assert!(reason.contains("weakly identified"), "{reason}");
    assert!(reason.contains("skeptic/"), "{reason}");
    assert!(adapter.calls.is_empty(), "a contested plan sends nothing");

    let shadow = run(contested(), None, &mut Counting::default()).unwrap();
    assert!(
        matches!(shadow.outcome, Outcome::Shadowed(_)),
        "{:?}",
        shadow.outcome
    );
}

/// R12-EXPAND-6071, ADR 0141: evidence and rollback requirements scale with
/// authority, read from one table. Every level past narrow-reversible refuses
/// the lesser evidence by name, item by item; full evidence for a declared
/// level is still refused because no record grants it, naming ADR 0141; and
/// external action and capital are refused naming the records that prohibit
/// them, before any evidence is read.
///
/// Mutations, each firing at its own assertion: delete the clean-days check
/// in `widen_to` (step 1 reports the compensation instead); delete the
/// compensation check (step 2 reports the approvers); count approvers by
/// signature rather than by distinct name (step 4 passes to the grant
/// refusal); delete the `Ungranted` refusal (step 5 widens); delete the
/// `Refused` check (External reports a level mismatch, not ADR 0114 D8);
/// delete the kill-path check (the broad step reports the level, not the
/// kill path).
#[test]
fn each_authority_level_refuses_the_lesser_evidence_and_names_the_record_that_refuses_it() {
    use qip_agency::autonomy::{AttributionNeed, Grant, SCHEDULE, WideningEvidence};

    // The table, as data a reader can check against ADR 0141.
    let levels: Vec<Authority> = SCHEDULE.iter().map(|r| r.level).collect();
    assert_eq!(
        levels,
        [
            Authority::Shadow,
            Authority::NarrowReversible,
            Authority::NarrowCompensable,
            Authority::BroadInternal,
            Authority::External,
            Authority::Capital,
        ]
    );
    assert_eq!(
        Authority::NarrowReversible.requirement().grant,
        Grant::Granted
    );
    for level in [Authority::NarrowCompensable, Authority::BroadInternal] {
        assert_eq!(
            level.requirement().grant,
            Grant::Ungranted {
                declared_by: "ADR 0141"
            }
        );
        assert_eq!(level.requirement().approvers, 2);
        assert!(level.requirement().compensation);
    }
    assert_eq!(
        Authority::BroadInternal.requirement().attribution,
        AttributionNeed::EveryClass
    );
    assert!(Authority::BroadInternal.requirement().kill_path);

    let second = ActingIdentity {
        name: "second-approver".into(),
        authorities: BTreeSet::from([WIDEN_AUTHORITY.to_string()]),
    };
    let market = identified(Method::Market);
    let compensated = |e: &mut WideningEvidence<'_>| {
        e.compensations
            .push((Method::Market, "re-widen quotes; drill 2026-10-01".into()));
    };

    let mut policy = narrow();
    assert_eq!(policy.authority(), Authority::NarrowReversible, "premise");
    let refuse = |policy: &mut ActionPolicy,
                  evidence: &WideningEvidence<'_>,
                  approvers: &[&ActingIdentity],
                  expected: &str| {
        let err = policy
            .widen_to(Authority::NarrowCompensable, evidence, approvers)
            .unwrap_err();
        assert!(err.to_string().contains(expected), "{expected}: {err}");
        assert_eq!(policy.authority(), Authority::NarrowReversible);
    };

    // 1. The first step's evidence is short of the second's: no clean period.
    let mut evidence = WideningEvidence::attribution(&market);
    refuse(
        &mut policy,
        &evidence,
        &[&operator(), &second],
        "day(s) at NarrowReversible without drift",
    );
    // 2. A clean period, and no compensating step.
    evidence.clean_days = 30;
    refuse(
        &mut policy,
        &evidence,
        &[&operator(), &second],
        "compensating step for Market",
    );
    // 3. Compensation, one approver: evidence does not stand in for one.
    compensated(&mut evidence);
    refuse(
        &mut policy,
        &evidence,
        &[&operator()],
        "2 distinct approver(s)",
    );
    // 4. One approver named twice is still one.
    refuse(
        &mut policy,
        &evidence,
        &[&operator(), &operator()],
        "2 distinct approver(s)",
    );
    // 5. Everything the row asks for, and still no grant.
    refuse(
        &mut policy,
        &evidence,
        &[&operator(), &second],
        "declared by ADR 0141 and granted by no record",
    );
    // Evidence for another class widens nothing.
    let research = identified(Method::Research);
    let mut other = WideningEvidence::attribution(&research);
    other.clean_days = 30;
    compensated(&mut other);
    refuse(
        &mut policy,
        &other,
        &[&operator(), &second],
        "widens no other",
    );

    // The broad level wants an attribution per class, a compensation per
    // class and a kill path, before it says the level is out of reach.
    let mut broad = WideningEvidence::attribution(&market);
    broad.classes = BTreeSet::from([Method::Market, Method::Research]);
    broad.clean_days = 30;
    let widen_broad = |policy: &mut ActionPolicy, e: &WideningEvidence<'_>| {
        policy
            .widen_to(Authority::BroadInternal, e, &[&operator(), &second])
            .unwrap_err()
            .to_string()
    };
    assert!(widen_broad(&mut policy, &broad).contains("attribution for Research"));
    broad.attributions.push(&research);
    compensated(&mut broad);
    assert!(widen_broad(&mut policy, &broad).contains("compensating step for Research"));
    broad
        .compensations
        .push((Method::Research, "retract note; drill 2026-10-02".into()));
    assert!(widen_broad(&mut policy, &broad).contains("kill path"));
    broad.kill_path = Some("agency:halt-all returns every class to shadow".into());
    let level = widen_broad(&mut policy, &broad);
    assert!(
        level.contains("reached only from NarrowCompensable"),
        "{level}"
    );

    // Prohibited levels refuse by their own records, whatever is offered.
    for (target, record) in [
        (Authority::External, "refused by ADR 0114 D8"),
        (Authority::Capital, "refused by ADR 0003, 0021 and 0108"),
    ] {
        for holder in [&mut narrow(), &mut fresh_market_policy()] {
            let err = holder
                .widen_to(target, &broad, &[&operator(), &second])
                .unwrap_err();
            assert!(err.to_string().contains(record), "{target:?}: {err}");
        }
    }

    // Drift still returns a policy to shadow from the level it holds.
    assert_eq!(policy.observe(d(50), &[]), Authority::Shadow);
}

fn fresh_market_policy() -> ActionPolicy {
    ActionPolicy::new(Method::Market, d(1), d(5), BTreeSet::new()).unwrap()
}

/// R12-GOV-3030: the post-release check re-reads what was released against
/// the rules the gate holds now, and reports the two releases it cannot
/// vouch for rather than passing them: one journaled without its text, and
/// one whose text is not the text that was released. A clean release
/// reports nothing. Mutation: in `recheck_released`, skip the fingerprint
/// comparison and the altered release reads as a breach of the rule instead.
#[test]
fn the_post_release_check_reports_a_later_breach_an_unreadable_release_and_an_altered_one() {
    use qip_agency::guard::{GuardPolicy, Voice, voice_fingerprint};
    let mut conduct = guard_decl().conduct;
    conduct
        .conduct_rules
        .push("prohibit:published spread".to_string());
    let mut guard = ActionGuard::from_policy(GuardPolicy {
        conduct,
        tools: guard_decl().tools,
    })
    .unwrap();
    let breached = "Quote 20 lots on venue-a at the published spread";
    let clean = "Quote 20 lots on venue-a";
    let voice = |fingerprint_of: &str, content: Option<&str>| Voice {
        tool: "quoter".to_string(),
        identity: "desk-agent".to_string(),
        fingerprint: voice_fingerprint(fingerprint_of),
        content: content.map(str::to_string),
    };
    guard
        .resume_voices(vec![
            voice(breached, Some(breached)),
            voice(clean, Some(clean)),
            voice(breached, None),
            voice(clean, Some(breached)),
        ])
        .unwrap();
    let findings = guard.recheck_released();
    let rules: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    assert_eq!(
        rules,
        [
            "prohibit:published spread",
            "unreadable",
            "fingerprint_mismatch"
        ]
    );
    assert_eq!(findings[0].fingerprint, voice_fingerprint(breached));
}

/// R12-QUANT-4043: a plan the planner *searched* -- `Structural::generate`
/// over the levers, with no plan handed in -- reaches an adapter only with a
/// classical causal score, a policy-gate pass and a simulation record. The
/// searcher here is the classical generator; ADR 0150/0190 build no quantum
/// one, and the guard sits on the act stage whatever produced the plan.
///
/// - Searched and supported by the one causal model: simulated under every
///   branch, selected, executed.
/// - The same search when a second structural model says the step widens
///   the spread: simulated, weakly identified, nothing sent.
/// - The same search when one step's exposure exceeds the goal's risk
///   envelope: the policy gate refuses it and nothing is sent.
///
/// Mutation: remove the `counterfactually_supported` refusal in
/// `engine::execute` -- the contested searched plan executes and the
/// `weakly identified` assertion fails.
#[test]
fn a_searched_plan_acts_only_with_a_causal_score_a_policy_gate_pass_and_a_simulation_record() {
    use qip_agency::generate::GenerateDecl;
    let searched = |exposure: Decimal| {
        let mut r = request();
        r.proposals.clear();
        r.structural.as_mut().unwrap().generate = Some(GenerateDecl {
            exposure: BTreeMap::from([("quoter".to_string(), exposure)]),
            max_steps: 1,
        });
        r
    };
    let policy = narrow();

    let mut adapter = Counting::default();
    let report = run(searched(d(1)), Some(&policy), &mut adapter).unwrap();
    assert!(
        !report.candidates.is_empty(),
        "premise: the generator searched and proposed a plan"
    );
    let simulated: Vec<_> = report.simulations.iter().flat_map(|s| &s.results).collect();
    assert!(
        simulated.len() >= 2,
        "the searched plan was not simulated under every branch: {simulated:?}"
    );
    assert!(
        matches!(report.outcome, Outcome::Executed(_)),
        "{:?}",
        report.outcome
    );
    assert_eq!(adapter.calls.len(), 1);

    let mut contested = searched(d(1));
    let decl = contested.structural.as_mut().unwrap();
    let mut skeptic = decl.models[0].clone();
    skeptic.name = "skeptic".into();
    // Disagrees, but mildly: the models' mean still favours the step, so
    // the generator ranks it and only the act stage's branch-by-branch
    // check can stop it.
    skeptic.links[0].coefficient = 0.2;
    decl.models.push(skeptic);
    let mut adapter = Counting::default();
    let report = run(contested, Some(&policy), &mut adapter).unwrap();
    let Outcome::Stopped(reason) = &report.outcome else {
        panic!("expected withheld, got {:?}", report.outcome)
    };
    assert!(reason.contains("weakly identified"), "{reason}");
    assert!(
        adapter.calls.is_empty(),
        "a contested searched plan was sent"
    );

    let mut adapter = Counting::default();
    let report = run(searched(d(500)), Some(&policy), &mut adapter).unwrap();
    assert!(
        !matches!(report.outcome, Outcome::Executed(_)),
        "a searched plan past the risk envelope executed: {:?}",
        report.outcome
    );
    assert!(adapter.calls.is_empty());
}
