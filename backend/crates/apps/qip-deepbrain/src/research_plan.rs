//! A sweep's goal and constraints compiled into a deterministic, cited
//! `qip_agency` plan (R12-REASON-3061).
//!
//! # Why this exists
//!
//! The research loop decides each sweep what to start and what to spend, and
//! until now that decision left behind only counters: which items started was
//! visible, *why this set within this budget* was not, and nothing stopped the
//! set from quietly exceeding the budget the goal declared. `qip-agency`
//! already owns the type that proves a plan sits inside its goal
//! ([`InterventionPlan::plan`]): every step's cost comes from an affordance
//! graph, every tool needs an authority the acting identity holds, and a plan
//! that exceeds its budget or envelope is an error, not a trimmed plan. This
//! file states the sweep as a [`GoalSpec`], builds that graph from the tools
//! the node registered, and asks the agency to prove the plan.
//!
//! # What it may not do
//!
//! Act. The goal is a research goal with a risk envelope of zero, the one tool
//! method is `Research`, and nothing here sends anything: the plan is a record
//! of what the sweep's started items amount to, journaled beside the signals
//! that raised them.

use qip_agency::affordance::{AffordanceGraph, ToolEdgeDraft};
use qip_agency::plan::{ActingIdentity, InterventionPlan, PlanNode, Step};
use qip_contracts::action::Method;
use qip_contracts::expansion::{GoalClass, GoalSpec, GoalSpecDraft};
use qip_core::error::Result;
use qip_core::{Decimal, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The identity every sweep plan acts as.
pub const PLANNER: &str = "qip-deepbrain/research-loop";
/// The authority a research tool requires; the only one the planner holds.
pub const AUTHORITY: &str = "research";
/// The variable every research tool acts on.
pub const VARIABLE: &str = "research-queue";

/// One started item as the planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedItem {
    pub question: String,
    /// The registered tool the item's task runs on.
    pub tool: &'static str,
    /// What the item costs to run: the desk's declared cost for its kind.
    pub cost: Decimal,
    /// The evidence the item's gap cited.
    pub cites: Vec<String>,
}

/// A sweep's plan as it is journaled. Plain data: the agency's plan type is
/// not serialisable, and a record that cannot be read back is not a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepPlanRecord {
    /// Names the plan by what it contains, so the same sweep compiles to the
    /// same id on a replay.
    pub plan_id: String,
    pub goal_target: String,
    pub budget: Decimal,
    pub total_cost: Decimal,
    /// `(tool, question)` in the order the desk started them.
    pub steps: Vec<(String, String)>,
    /// Every evidence id the started items cited, sorted and de-duplicated.
    pub cites: Vec<String>,
}

/// Compile the sweep's started items into a plan inside `budget`.
///
/// `None` where nothing started: there is no plan to prove for a sweep that
/// chose nothing. A sweep whose items together cost more than the budget is an
/// error naming the budget, never a smaller plan.
pub fn compile(
    items: &[StartedItem],
    budget: Decimal,
    jurisdiction: &str,
    horizon_secs: u64,
) -> Result<Option<SweepPlanRecord>> {
    if items.is_empty() {
        return Ok(None);
    }
    let goal = goal(items, budget, jurisdiction, horizon_secs)?;
    let mut graph = AffordanceGraph::new();
    graph.add_variable(VARIABLE, true, true)?;
    // A tool two kinds of task share costs the dearer of them: the graph
    // prices a tool once, and an understated edge would let a plan through
    // that the budget should have refused.
    let mut edge_cost: BTreeMap<&str, Decimal> = BTreeMap::new();
    for item in items {
        let cost = edge_cost.entry(item.tool).or_insert(item.cost);
        if item.cost > *cost {
            *cost = item.cost;
        }
    }
    for (tool, cost) in &edge_cost {
        graph.add_tool_edge(
            VARIABLE,
            ToolEdgeDraft {
                tool: Some((*tool).to_string()),
                method: Some(Method::Research),
                reversible: Some(true),
                latency_ms: Some(0),
                cost: Some(*cost),
                side_effects: Some(Vec::new()),
                dependencies: Some(Vec::new()),
                authority: Some(AUTHORITY.to_string()),
            },
        )?;
    }
    let who = ActingIdentity {
        name: PLANNER.to_string(),
        authorities: BTreeSet::from([AUTHORITY.to_string()]),
    };
    let steps: Vec<PlanNode> = items
        .iter()
        .map(|item| {
            PlanNode::Step(Step {
                tool: item.tool.to_string(),
                variable: VARIABLE.to_string(),
                exposure: Decimal::ZERO,
            })
        })
        .collect();
    let plan = InterventionPlan::plan(&goal, &graph, &who, PlanNode::Sequence(steps))?;
    let cites: BTreeSet<String> = items.iter().flat_map(|i| i.cites.iter().cloned()).collect();
    let steps: Vec<(String, String)> = items
        .iter()
        .map(|i| (i.tool.to_string(), i.question.clone()))
        .collect();
    let digest_input: String = steps
        .iter()
        .map(|(tool, question)| format!("{tool}|{question}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Some(SweepPlanRecord {
        plan_id: format!("sweep-plan-{}", &sha256_hex(digest_input.as_bytes())[..16]),
        goal_target: goal.target_state.clone(),
        budget,
        total_cost: plan.total_cost,
        steps,
        cites: cites.into_iter().collect(),
    }))
}

fn goal(
    items: &[StartedItem],
    budget: Decimal,
    jurisdiction: &str,
    horizon_secs: u64,
) -> Result<GoalSpec> {
    GoalSpecDraft {
        class: Some(GoalClass::Research),
        target_state: Some(format!(
            "the {} started research item(s) are carried out inside the sweep's compute budget",
            items.len()
        )),
        entities_affected: Some(vec![VARIABLE.to_string()]),
        time_horizon_secs: Some(horizon_secs),
        success_metric: Some("uncertainty reduced on each started question".to_string()),
        acceptable_uncertainty: Some(Decimal::ONE),
        budget: Some(budget),
        risk_envelope: Some(Decimal::ZERO),
        jurisdictions: Some(vec![jurisdiction.to_string()]),
        acting_identity: Some(PLANNER.to_string()),
        prohibited_side_effects: Some(Vec::new()),
        prohibited_methods: Some(Vec::new()),
        stop_conditions: Some(vec!["the sweep's compute budget is spent".to_string()]),
        rollback_conditions: Some(vec!["withdraw the item from the queue".to_string()]),
    }
    .build()
}
