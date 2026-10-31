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
        "fn the_organisation_assembles_and_dispatches",
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

/// R12-MESH-1167: the arbitrage scope, item by item — 2 to 20 legs,
/// cross-venue, cross-region, cross-asset and cross-currency cycles, and
/// size-aware graph costs — each held to the seam a composition root reaches
/// and a test that names it. The desk is installed by `qip-edge-node`'s
/// `ArbitrageInstaller` from the whitelist `qip-api`'s `load_arbitrage_policy`
/// ships, so the first two rows pin those roots; every other row pins the
/// library seam the installed desk runs through and the test that drives
/// it. A seam renamed away or a test deleted fails here naming the item.
/// What this does not claim: that a cycle crosses a region on a deployed
/// node — the peer saga's seam is spawned behind the peer-mesh switch, and
/// the row's gap says so.
const ARBITRAGE_SCOPE: &[(&str, &str, &str, &str, &str)] = &[
    (
        "desk installed from the centre's whitelist at the node's root",
        "crates/apps/qip-edge-node/src/main.rs",
        "ArbitrageInstaller::new(strategy, config.venues.clone())",
        "crates/apps/qip-edge-node/tests/arbitrage.rs",
        "fn the_node_installs_a_desk_from_the_payloads_whitelist_once_capital_for_it_has_arrived",
    ),
    (
        "policy loaded at the centre's root",
        "crates/apps/qip-api/src/main.rs",
        "fn load_arbitrage_policy()",
        "crates/runtime/qip-kernel/src/central/whitelist.rs",
        "fn a_markets_stated_costs_ship_on_both_of_its_conversions_and_an_impossible_one_is_refused",
    ),
    (
        "2 to 20 legs: the node's leg bound",
        "crates/apps/qip-edge-node/src/arbitrage.rs",
        "pub fn parse_max_legs(",
        "crates/apps/qip-edge-node/tests/arbitrage.rs",
        "fn the_configured_maximum_leg_count_is_refused_outside_two_to_twenty_and_unset_is_the_engine_default",
    ),
    (
        "2 to 20 legs: the search's ceiling",
        "crates/edge/qip-arbitrage/src/search.rs",
        "max_cycle_edges",
        "crates/edge/qip-arbitrage/tests/leg_limit.rs",
        "fn with_the_maximum_at_twenty_cycles_of_two_and_twenty_legs_are_accepted_and_one_and_twenty_one_are_refused",
    ),
    (
        "cross-venue: a transfer edge between venues",
        "crates/edge/qip-arbitrage/src/graph.rs",
        "pub fn add_transfer(",
        "crates/edge/qip-edge/tests/cross_region.rs",
        "fn the_same_cell_with_both_venues_at_home_is_assigned_path_two_exactly_as_before",
    ),
    (
        "cross-venue: a two-venue cycle's legs book on the ledger",
        "crates/edge/qip-arbitrage/src/graph.rs",
        "pub fn add_transfer(",
        "crates/tests/qip-acceptance/tests/cycle_ledger.rs",
        "fn every_filled_leg_of_a_completed_cycle_posts_balanced_and_the_cycle_cash_nets_to_zero",
    ),
    (
        "cross-region: mirror edges and the measured round trip",
        "crates/edge/qip-edge/src/mirror.rs",
        "pub fn with_round_trip(",
        "crates/edge/qip-edge/tests/cross_region.rs",
        "fn a_cell_told_one_of_its_venues_is_abroad_composes_mirror_edges_and_is_assigned_path_three",
    ),
    (
        "cross-region: legs executed between two nodes over the peer wire",
        "crates/apps/qip-edge-node/src/main.rs",
        "qip_peer_link::spawn(settings, Arc::clone(&clock))",
        "crates/edge/qip-peer-link/tests/saga.rs",
        "fn two_nodes_publishing_only_through_their_ports_run_a_cross_region_cycle_to_its_unwind",
    ),
    (
        "cross-asset: a triangle over three instruments",
        "crates/edge/qip-arbitrage/src/graph.rs",
        "pub fn add_trade(",
        "crates/edge/qip-edge/tests/arbitrage.rs",
        "fn a_cycle_on_the_cells_own_books_becomes_its_legs_as_orders_in_one_pass",
    ),
    (
        "cross-currency: a currency conversion leg in the cell's graph",
        "crates/edge/qip-arbitrage/src/graph.rs",
        "pub fn add_trade(",
        "crates/edge/qip-edge/tests/cross_region.rs",
        "\"USDTUSD\"",
    ),
    (
        "size-aware graph costs: spread and slippage charged on the size walked",
        "crates/edge/qip-arbitrage/src/netedge.rs",
        "fn book_cost_fractions(",
        "crates/edge/qip-arbitrage/tests/arbitrage.rs",
        "fn spread_and_slippage_are_charged_as_separate_costs",
    ),
    (
        "size-aware graph costs: a path with no stated size is refused",
        "crates/edge/qip-arbitrage/src/scan.rs",
        "pub struct SizePolicy",
        "crates/edge/qip-arbitrage/tests/arbitrage.rs",
        "fn a_path_with_no_stated_size_is_refused_rather_than_guessed",
    ),
];

#[test]
fn every_arbitrage_scope_item_maps_to_a_seam_a_composition_root_reaches_and_a_named_test() {
    assert_eq!(
        ARBITRAGE_SCOPE.len(),
        12,
        "premise: the map covers the leg bound, three cross dimensions, currency and size-aware costs"
    );
    // The two roots: a desk installed only from the centre's whitelist, a
    // policy loaded only at the API's root. Pinned by the lines the other
    // rows hang off, so a root that stops installing fails here first.
    holds(
        "crates/apps/qip-edge-node/src/main.rs",
        ".in_region(&config.region)",
        "desk installed in the cell's region",
    );
    for (item, seam_file, seam, test_file, test) in ARBITRAGE_SCOPE {
        holds(seam_file, seam, item);
        holds(test_file, test, item);
    }
}
