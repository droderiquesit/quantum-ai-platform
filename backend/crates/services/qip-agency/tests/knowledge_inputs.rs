//! AGENCY-058: The engine takes knowledge from arbitration and specialists.
//!
//! The Causal Agency Engine receives knowledge input from both the
//! Meta/Arbitration Brain and the Specialist Brain Society. Each
//! InterventionPlan records which knowledge inputs informed its creation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_agency::affordance::{AffordanceGraph, Method, ToolEdgeDraft};
use qip_agency::goal::{GoalClass, GoalSpecDraft};
use qip_agency::knowledge::{KnowledgeInput, KnowledgeSource};
use qip_agency::plan::{ActingIdentity, InterventionPlan, PlanNode, Step};
use qip_core::Decimal;

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}

fn s(x: &str) -> Option<String> {
    Some(x.to_string())
}

fn list(x: &[&str]) -> Option<Vec<String>> {
    Some(x.iter().map(|v| v.to_string()).collect())
}

fn arbitration_input() -> KnowledgeInput {
    KnowledgeInput::from_arbitration("arb:market_momentum", "Market momentum is rising")
}

fn specialist_input() -> KnowledgeInput {
    KnowledgeInput::from_specialist("specialist:chief", "Chief expects volatility spike")
}

fn edge(tool: &str, cost: i64, authority: &str) -> ToolEdgeDraft {
    ToolEdgeDraft {
        tool: s(tool),
        method: Some(Method::Market),
        reversible: Some(true),
        latency_ms: Some(10),
        cost: Some(d(cost)),
        side_effects: Some(vec![]),
        dependencies: Some(vec![]),
        authority: s(authority),
    }
}

#[test]
fn the_engine_consumes_arbitration_and_specialist_knowledge() {
    let input1 = arbitration_input();
    let input2 = specialist_input();

    assert_eq!(input1.source, KnowledgeSource::Arbitration);
    assert_eq!(input2.source, KnowledgeSource::Specialist);
    assert_eq!(input1.id, "arb:market_momentum");
    assert_eq!(input2.id, "specialist:chief");
}

#[test]
fn intervention_plan_records_knowledge_inputs() {
    let goal_draft = GoalSpecDraft {
        class: Some(GoalClass::Financial),
        target_state: s("increase returns"),
        entities_affected: list(&["portfolio"]),
        time_horizon_secs: Some(3600),
        success_metric: s("total return"),
        acceptable_uncertainty: Some(Decimal::from_scaled(1, 1).unwrap()),
        budget: Some(d(1000)),
        risk_envelope: Some(d(100)),
        jurisdictions: list(&["US"]),
        acting_identity: s("trader"),
        prohibited_side_effects: list(&["public_statement"]),
        prohibited_methods: list(&["manipulation"]),
        stop_conditions: list(&["limit exceeded"]),
    };
    let goal = goal_draft.build().expect("goal failed");

    let mut graph = AffordanceGraph::new();
    graph
        .add_variable("portfolio_size", true, true)
        .expect("add variable failed");
    graph
        .add_tool_edge(
            "portfolio_size",
            edge("add_capital", 10, "capital_authority"),
        )
        .expect("add tool edge failed");

    let identity = ActingIdentity {
        name: "trader".to_string(),
        authorities: vec!["capital_authority".to_string()].into_iter().collect(),
    };

    let plan_node = PlanNode::Step(Step {
        tool: "add_capital".to_string(),
        variable: "portfolio_size".to_string(),
        exposure: d(50),
    });

    let mut plan = InterventionPlan::plan(&goal, &graph, &identity, plan_node)
        .expect("plan construction failed");

    assert!(!plan.knowledge_log.has_inputs());

    let inputs = vec![arbitration_input(), specialist_input()];
    plan.with_knowledge(&inputs);

    assert!(plan.knowledge_log.has_inputs());
    assert_eq!(plan.knowledge_log.input_ids().len(), 2);
    assert!(
        plan.knowledge_log
            .input_ids()
            .contains("arb:market_momentum")
    );
    assert!(plan.knowledge_log.input_ids().contains("specialist:chief"));
}

#[test]
fn knowledge_log_tracks_source_of_inputs() {
    let arb_input = arbitration_input();
    let spec_input = specialist_input();

    assert_eq!(arb_input.source, KnowledgeSource::Arbitration);
    assert_eq!(spec_input.source, KnowledgeSource::Specialist);
}

#[test]
fn plan_with_knowledge_is_chainable() {
    let goal_draft = GoalSpecDraft {
        class: Some(GoalClass::Financial),
        target_state: s("increase returns"),
        entities_affected: list(&["portfolio"]),
        time_horizon_secs: Some(3600),
        success_metric: s("total return"),
        acceptable_uncertainty: Some(Decimal::from_scaled(1, 1).unwrap()),
        budget: Some(d(1000)),
        risk_envelope: Some(d(100)),
        jurisdictions: list(&["US"]),
        acting_identity: s("trader"),
        prohibited_side_effects: list(&["public_statement"]),
        prohibited_methods: list(&["manipulation"]),
        stop_conditions: list(&["limit exceeded"]),
    };
    let goal = goal_draft.build().expect("goal failed");

    let mut graph = AffordanceGraph::new();
    graph
        .add_variable("portfolio_size", true, true)
        .expect("add variable failed");
    graph
        .add_tool_edge(
            "portfolio_size",
            edge("add_capital", 10, "capital_authority"),
        )
        .expect("add tool edge failed");

    let identity = ActingIdentity {
        name: "trader".to_string(),
        authorities: vec!["capital_authority".to_string()].into_iter().collect(),
    };

    let plan_node = PlanNode::Step(Step {
        tool: "add_capital".to_string(),
        variable: "portfolio_size".to_string(),
        exposure: d(50),
    });

    let mut plan = InterventionPlan::plan(&goal, &graph, &identity, plan_node)
        .expect("plan construction failed");

    let result = plan
        .with_knowledge(&[arbitration_input()])
        .with_knowledge(&[specialist_input()]);

    assert!(result.knowledge_log.has_inputs());
    assert_eq!(result.knowledge_log.input_ids().len(), 2);
}

#[test]
fn mutation_test_empty_knowledge_log_means_no_inputs_recorded() {
    let goal_draft = GoalSpecDraft {
        class: Some(GoalClass::Financial),
        target_state: s("increase returns"),
        entities_affected: list(&["portfolio"]),
        time_horizon_secs: Some(3600),
        success_metric: s("total return"),
        acceptable_uncertainty: Some(Decimal::from_scaled(1, 1).unwrap()),
        budget: Some(d(1000)),
        risk_envelope: Some(d(100)),
        jurisdictions: list(&["US"]),
        acting_identity: s("trader"),
        prohibited_side_effects: list(&["public_statement"]),
        prohibited_methods: list(&["manipulation"]),
        stop_conditions: list(&["limit exceeded"]),
    };
    let goal = goal_draft.build().expect("goal failed");

    let mut graph = AffordanceGraph::new();
    graph
        .add_variable("portfolio_size", true, true)
        .expect("add variable failed");
    graph
        .add_tool_edge(
            "portfolio_size",
            edge("add_capital", 10, "capital_authority"),
        )
        .expect("add tool edge failed");

    let identity = ActingIdentity {
        name: "trader".to_string(),
        authorities: vec!["capital_authority".to_string()].into_iter().collect(),
    };

    let plan_node = PlanNode::Step(Step {
        tool: "add_capital".to_string(),
        variable: "portfolio_size".to_string(),
        exposure: d(50),
    });

    let plan = InterventionPlan::plan(&goal, &graph, &identity, plan_node)
        .expect("plan construction failed");

    assert!(!plan.knowledge_log.has_inputs());
    assert_eq!(plan.knowledge_log.input_ids().len(), 0);
}
