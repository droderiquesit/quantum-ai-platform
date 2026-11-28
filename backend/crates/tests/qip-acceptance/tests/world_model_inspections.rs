//! Inspection maps for the world-model federation rows of the capability map
//! (R12-WORLD-1178 and R12-WORLD-1181). Each scope item names a seam and a
//! test that exist today; a seam renamed away or a test deleted fails here
//! naming the item, which is the register's "a missing item fails
//! inspection".
//!
//! This is inspection, not behaviour: it proves the named symbol and the
//! named test are in the tree, not that the test passes — those run in their
//! own crates. The members are named by importing their `pub const`, so a
//! renamed or removed member is a compile error, not a silent string.
//!
//! What this map does NOT say: `geopolitical`, `supply-chain`, `physical`
//! and `event` models have no member, and the capital family has no
//! collateral, optionality or mobility model. They are absent from
//! `REQUIRED_FAMILIES`, so no scope item for them is listed below, and
//! listing them would be asserting what the tree does not hold. R12-WORLD-1178
//! therefore stays open; see the lane report.

use qip_kernel::federation_stage as fs;
use std::fs as stdfs;

fn read(relative: &str) -> String {
    let path = qip_acceptance::repository_root()
        .join("backend")
        .join(relative);
    stdfs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{relative} is named by the scope map and is missing"))
}

fn holds(relative: &str, needle: &str, item: &str) {
    assert!(
        read(relative).contains(needle),
        "scope item `{item}`: `{needle}` is no longer in {relative}"
    );
}

const STAGE: &str = "crates/runtime/qip-kernel/src/federation_stage.rs";
const CYCLE_TEST: &str = "crates/runtime/qip-kernel/tests/symbolic_federation.rs";
const CYCLE_TEST_NAME: &str = "fn every_cycle_journals_one_federation_review_and_each_replays_and_reproduces_its_arbitrations";
const WM_TESTS: &str = "crates/services/qip-world-model/tests/federation.rs";
const WM_SRC: &str = "crates/services/qip-world-model/src/federation.rs";

/// (scope item, member id the stage declares, test file, test name).
fn members() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        ("global", fs::MOMENTUM, CYCLE_TEST, CYCLE_TEST_NAME),
        (
            "macro",
            fs::MACRO,
            CYCLE_TEST,
            "fn a_cycle_with_a_macro_surprise_feeds_the_macro_member_and_a_cycle_without_one_reports_it_silent",
        ),
        (
            "company",
            fs::COMPANY,
            STAGE,
            "fn a_fundamental_surprise_on_an_issuer_gives_the_company_member_a_view_and_its_absence_names_the_family_uncovered",
        ),
        (
            "microstructure",
            fs::ORDER_FLOW,
            CYCLE_TEST,
            "fn signed_trade_flow_gives_the_order_flow_member_a_view",
        ),
        (
            "portfolio/capital",
            fs::CAPITAL,
            STAGE,
            "fn a_book_with_a_thesis_waiting_gives_the_capital_member_a_funding_view_the_next_pass_resolves",
        ),
        (
            "counterparty",
            fs::COUNTERPARTY,
            STAGE,
            "fn a_venue_whose_refusals_rose_gives_the_counterparty_member_a_view_resolved_next_pass",
        ),
        ("regional", fs::REGIONAL_PREFIX, CYCLE_TEST, CYCLE_TEST_NAME),
        (
            "self",
            fs::SELF_SYSTEM,
            STAGE,
            "fn a_slow_stage_or_a_stale_capability_moves_the_self_system_members_state",
        ),
        (
            "scenario",
            fs::STRESS_SCENARIO,
            STAGE,
            "fn the_rare_regime_member_reads_the_day_after_tail_moves_and_the_alternate_future_inverts_the_stress_world",
        ),
        (
            "counterfactual",
            fs::ALTERNATE_FUTURE,
            STAGE,
            "fn the_rare_regime_member_reads_the_day_after_tail_moves_and_the_alternate_future_inverts_the_stress_world",
        ),
    ]
}

#[test]
fn every_present_federation_member_is_declared_by_the_stage_and_names_a_test_that_still_exists() {
    let members = members();
    assert_eq!(members.len(), 10, "premise: ten scope items have a member");
    let stage = read(STAGE);
    for (item, id, test_file, test) in members {
        assert!(!id.is_empty(), "scope item `{item}` names no member");
        assert!(
            stage.matches(id).count() >= 1,
            "scope item `{item}`: member `{id}` is not in {STAGE}"
        );
        holds(test_file, test, item);
    }
}

#[test]
fn the_stage_is_reached_from_the_platform_cycle_in_which_every_central_binary_runs() {
    // The call, not the name: a definition nothing calls would satisfy a
    // search for the bare symbol.
    holds(
        "crates/runtime/qip-kernel/src/platform.rs",
        "= self.understand_federation(now);",
        "federation reached from the cycle",
    );
    // The fast brain's step runs that cycle, and the REASON seam the
    // contradiction reaches reads what UNDERSTAND left.
    holds(
        "crates/apps/qip-fastbrain/src/node.rs",
        "platform.run_cycle(now)",
        "the fast brain runs the cycle",
    );
    holds(
        "crates/runtime/qip-kernel/src/platform.rs",
        ".and_then(|subject| self.federation_contradicted.get(subject.as_str()))",
        "REASON reads the contradicted instruments",
    );
}

const PLATFORM: &str = "crates/runtime/qip-kernel/src/platform.rs";
const TAPE_TEST: &str = "crates/apps/qip-fastbrain/tests/contradiction.rs";

/// R12-WORLD-1181: (scope item, seam file, seam symbol, test file, test).
///
/// Every item's seam is on the default cycle's path: the stage runs from
/// `understand_federation`, which `Platform::run_cycle` reaches in every
/// central binary (checked below). The test named for each item drives
/// that path, the stage's own `run` or the cycle, never a hand-built
/// arbitration alone.
const ARBITRATION: [(&str, &str, &str, &str, &str); 11] = [
    (
        "compare competing states",
        WM_SRC,
        "pub fn arbitrate",
        WM_TESTS,
        "fn arbitration_over_generated_diverging_views_keeps_every_view_and_measures_the_spread",
    ),
    (
        "preserve minority explanations",
        STAGE,
        "Decision::Branch =>",
        STAGE,
        "fn a_branch_decision_opens_one_live_branch_of_the_leader_and_journals_its_lineage",
    ),
    (
        "score calibration and evidence fit",
        WM_SRC,
        "pub fn calibration",
        WM_TESTS,
        "fn arbitration_records_all_four_dimensions_and_ranks_by_evidence_fit_when_only_that_differs",
    ),
    (
        "detect contradictions",
        STAGE,
        "pub fn contradictions_in",
        CYCLE_TEST,
        "fn opposed_views_past_the_margin_are_journaled_as_a_contradiction_distinct_from_a_wide_spread",
    ),
    (
        "detect contradictions: the margin measured on the shipped feed",
        STAGE,
        "pub const CONTRADICTION_MARGIN: f64",
        TAPE_TEST,
        "fn the_contradiction_margin_sits_in_an_empty_band_of_the_shipped_feeds_opposed_views",
    ),
    (
        "maintain uncertainty",
        WM_SRC,
        "pub uncertainty: f64",
        WM_TESTS,
        "fn a_population_differs_on_all_five_axes_and_every_member_produced_state_this_cycle",
    ),
    (
        "decide branch, merge, abstain or request information",
        WM_SRC,
        "RequestMoreInformation(InformationRequest)",
        WM_TESTS,
        "fn arbitration_produces_each_of_the_four_actions_with_a_reason_and_the_right_output",
    ),
    (
        "decide merge, on real passes",
        STAGE,
        "Decision::Merge =>",
        STAGE,
        "fn real_passes_open_two_branches_that_both_hold_views_and_a_merge_decision_folds_them",
    ),
    (
        "request information on a contradiction",
        STAGE,
        "pub fn contradiction_request",
        STAGE,
        "fn each_contradiction_journals_a_request_naming_its_two_members_and_replay_refuses_an_edited_one",
    ),
    (
        "abstain on a contradiction, through the cycle",
        PLATFORM,
        "self.federation_contradicted = crate::federation_stage::contradicted_instruments(&review);",
        CYCLE_TEST,
        "fn a_contradicted_instrument_journals_a_request_naming_both_views_and_reason_holds_its_thesis_to_even_odds",
    ),
    (
        "abstain on a contradiction, on the shipped tape",
        PLATFORM,
        ".min(crate::federation_stage::CONTRADICTED_CONFIDENCE_CAP);",
        TAPE_TEST,
        "fn a_thesis_on_a_contradicted_instrument_is_held_to_even_odds_on_the_shipped_tape",
    ),
];

#[test]
fn every_arbitration_scope_item_names_a_seam_and_a_test_that_still_exist() {
    assert_eq!(
        ARBITRATION.len(),
        11,
        "premise: the row's six items and their five reaction and measurement seams"
    );
    for (item, seam_file, seam, test_file, test) in ARBITRATION {
        holds(seam_file, seam, item);
        // The open parenthesis delimits the name: a test whose name merely
        // starts with the one listed is not the listed test.
        holds(test_file, &format!("{test}("), item);
    }
}
