//! Inspection tests that hold two scope lists to the code: Phase 3's exit
//! capabilities (R12-ARCH-7140) and the ten items of "Financial AGI core"
//! (R12-WORLD-1156). Each names a seam and a test that exists today; a seam
//! renamed away, or a test deleted, fails here naming the item, which is the
//! register's "a missing item fails inspection".
//!
//! This is inspection, not behaviour: it proves the named symbol and the
//! named test are still in the tree, not that the test passes. The tests it
//! names run in their own crates' suites.

use qip_kernel::asset_class_registry::AssetClassRegistry;
use std::fs;
use std::path::PathBuf;

fn backend() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

fn read(relative: &str) -> String {
    fs::read_to_string(backend().join(relative))
        .unwrap_or_else(|_| panic!("{relative} is named by the scope map and is missing"))
}

fn holds(relative: &str, needle: &str, item: &str) {
    assert!(
        read(relative).contains(needle),
        "scope item `{item}`: `{needle}` is no longer in {relative}"
    );
}

#[test]
fn phase_three_exits_with_a_registry_of_more_than_one_class_beside_treasury_corridors_and_risk() {
    let registry = AssetClassRegistry::shipped().expect("the shipped registry assembles");
    assert!(
        registry.len() > 1,
        "premise: more than one class, or `multiple asset classes` is not shown"
    );
    holds(
        "crates/runtime/qip-kernel/src/asset_class_registry.rs",
        "pub fn shipped",
        "asset registry",
    );
    holds(
        "crates/services/qip-capital/src/lib.rs",
        "pub mod treasury;",
        "treasury",
    );
    holds(
        "crates/services/qip-capital-fabric/src/corridor.rs",
        "pub struct CorridorId",
        "custody corridors",
    );
    holds(
        "crates/services/qip-risk-engine/src/lib.rs",
        "pub mod pretrade;",
        "risk brain",
    );
    holds(
        "crates/runtime/qip-kernel/tests/asset_classes.rs",
        "fn a_universe_of_registered_classes_assembles_and_one_holding_an_unregistered_class_does_not",
        "asset registry gate",
    );
}

/// (item, seam file, seam symbol, test file, test name).
const FINANCIAL_AGI_CORE: [(&str, &str, &str, &str, &str); 10] = [
    (
        "planning",
        "crates/apps/qip-deepbrain/src/ambient_agency.rs",
        "pub fn route",
        "crates/apps/qip-deepbrain/tests/ambient_agency.rs",
        "fn the_run_loop_routes_activated_ambient_signals_through_the_guard_into_the_outbox",
    ),
    (
        "research",
        "crates/services/qip-expansion/src/plan.rs",
        "pub fn synthesise",
        "crates/services/qip-expansion/tests/loop.rs",
        "fn a_synthesised_plan_builds_a_missing_tool_first_and_runs_the_four_environments_in_order",
    ),
    (
        "tool use",
        "crates/libs/qip-agents/src/tools.rs",
        "pub struct ToolRegistry",
        "crates/libs/qip-agents/tests/expansion_registries.rs",
        "fn a_tool_of_each_of_the_ten_kinds_resolves_by_kind_with_its_scope",
    ),
    (
        "hypothesis generation",
        "crates/runtime/qip-kernel/src/platform.rs",
        "fn stage_reason",
        "crates/runtime/qip-kernel/tests/learning.rs",
        "fn reason_forms_a_hypothesis_from_a_sensed_anomaly_on_the_default_composition",
    ),
    (
        "causal reasoning",
        "crates/services/qip-world-model/src/causal.rs",
        "pub struct CausalGraph",
        "crates/services/qip-world-model/tests/inference.rs",
        "fn a_counterfactual_outcome_equals_the_hand_computed_value_and_cites_the_edges_it_used",
    ),
    (
        "counterfactuals",
        "crates/runtime/qip-kernel/src/platform.rs",
        "fn score_declined",
        "crates/runtime/qip-kernel/tests/learning.rs",
        "fn a_declined_actions_counterfactual_record_names_the_method_that_estimated_it_and_never_joins_the_observed_outcomes",
    ),
    (
        "self-model",
        "crates/services/qip-learning-engine/src/self_model.rs",
        "pub struct SelfModel",
        "crates/services/qip-learning-engine/tests/self_model.rs",
        "fn a_component_that_is_always_wrong_estimates_near_zero_and_one_always_right_near_one",
    ),
    (
        "uncertainty",
        "crates/runtime/qip-kernel/src/platform.rs",
        "fn calibrate_resolved",
        "crates/runtime/qip-kernel/tests/learning.rs",
        "fn a_resolved_prediction_updates_the_calibration_report",
    ),
    (
        "memory",
        "crates/libs/qip-ai/src/memory/store.rs",
        "pub struct EpisodicMemory",
        "crates/runtime/qip-kernel/tests/episodic.rs",
        "fn a_precedent_says_what_the_platform_declined_on_the_episodes_it_recalled_and_whether_it_should_have",
    ),
    (
        "specialist-agent orchestration",
        "crates/agents/qip-investment-agents/src/chief.rs",
        "pub fn dispatch",
        "crates/agents/qip-investment-agents/tests/organisation.rs",
        "fn the_declared_organisation_passes_its_own_governance",
    ),
];

#[test]
fn every_financial_agi_core_scope_item_names_a_seam_and_a_test_that_still_exist() {
    assert_eq!(
        FINANCIAL_AGI_CORE.len(),
        10,
        "premise: the blueprint lists ten items"
    );
    for (item, seam_file, seam, test_file, test) in FINANCIAL_AGI_CORE {
        holds(seam_file, seam, item);
        holds(test_file, test, item);
    }
}

#[test]
fn the_deep_brain_opens_its_ambient_agency_unconditionally_so_planning_and_tool_use_are_reached() {
    let main = read("crates/apps/qip-deepbrain/src/main.rs");
    let open = main
        .find("AmbientAgency::open")
        .expect("the deep brain's composition root opens the agency");
    let line_start = main[..open].rfind('\n').map_or(0, |i| i + 1);
    let before: Vec<&str> = main[..line_start].lines().rev().take(3).collect();
    assert!(
        before
            .iter()
            .all(|l| !l.trim_start().starts_with("if ") && !l.trim_start().starts_with("match ")),
        "the agency open sits behind a condition: {before:?}"
    );
    assert!(
        main.contains("agency: Some(&mut agency)"),
        "the agency is handed to the run loop"
    );
}

/// R12-REFLEX-1165: the reflex node's scope items, each held to the file
/// that composes it. The node's composition root builds the cell
/// (`assemble_with_risk`: feature engine, mode gate, risk gate) and `main`
/// supplies the strategy installer and the gateway; the simulated feed
/// tracks venue state; netting and the inventory-target read happen inside
/// `Cell::work`, which the node calls, so they are pinned in the cell rather
/// than invented as node symbols. A symbol that leaves its file fails here
/// naming the item.
#[test]
fn every_reflex_node_scope_item_is_still_composed_where_the_node_assembles_it() {
    const NODE: &str = "crates/apps/qip-edge-node/src";
    let items: [(&str, String, &str); 9] = [
        (
            "feature engine",
            format!("{NODE}/lib.rs"),
            "features: FeatureEngine",
        ),
        (
            "risk gate",
            format!("{NODE}/lib.rs"),
            ".with_risk_gate(risk_gate)",
        ),
        (
            "mode gate",
            format!("{NODE}/lib.rs"),
            ".with_mode_gate(mode_gate)",
        ),
        (
            "risk assembly",
            format!("{NODE}/main.rs"),
            "assemble_with_risk(",
        ),
        (
            "strategy installer",
            format!("{NODE}/main.rs"),
            "StrategyInstaller::new(",
        ),
        ("gateway", format!("{NODE}/main.rs"), "NodeGateway::open("),
        (
            "venue state",
            format!("{NODE}/feed.rs"),
            "VenueState::aggregated(",
        ),
        (
            "netting",
            "crates/edge/qip-edge/src/cell.rs".to_string(),
            "let nets = net(intents);",
        ),
        (
            "inventory targets",
            "crates/edge/qip-edge/src/cell.rs".to_string(),
            "let targets = self.inventory_targets();",
        ),
    ];
    for (item, file, needle) in &items {
        holds(file, needle, item);
    }
    // The pass that runs `Cell::work` is scheduled, not called from `main`.
    holds(
        &format!("{NODE}/scheduler.rs"),
        "run_pass(cell, simulated, feed, requoter, stats, now)",
        "pass",
    );
    holds(
        &format!("{NODE}/pass.rs"),
        "cell.work(now, &mut gateway)",
        "pass",
    );
}
