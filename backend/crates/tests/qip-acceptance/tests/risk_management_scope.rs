//! R12-RISK-1162: the twelve scope items of "Risk management" — pre-trade,
//! post-trade, market, liquidity, counterparty, concentration, causal-driver,
//! settlement, model, data, operational and systemic risk — each held to the
//! seam that implements it and a named test that drives it.
//!
//! The register's bar is "each scope item maps to code reached from a
//! composition root with a named test; a missing item fails inspection".
//! Three tests below carry the three halves of that sentence:
//!
//! * [`every_risk_management_scope_item_names_a_seam_and_a_test_that_still_exist`]
//!   — the map. A seam renamed away or a test deleted fails here naming the
//!   item, and a thirteenth item cannot be added or one dropped without
//!   changing the premise count.
//! * [`the_central_risk_read_charges_every_book_level_item_and_each_root_builds_a_platform`]
//!   — the reach. Every book-level item is charged inside
//!   `Platform::risk_state_from`, the one state the pre-trade veto and the
//!   monitor both read, and each central composition root builds a
//!   `Platform`. Settlement is the cell's, and the edge node's root assembles
//!   the cell whose pass applies it.
//! * [`every_book_level_axis_item_is_capped_by_the_shipped_limit_set`] — the
//!   enforcement. An axis that is charged and capped by nothing is a figure,
//!   not a control.
//!
//! This is inspection, not behaviour: the named tests run in their own
//! crates' suites. What this suite adds is that none of them can leave
//! without this file noticing.

use std::fs;
use std::path::PathBuf;

fn backend() -> PathBuf {
    qip_acceptance::repository_root().join("backend")
}

fn read(relative: &str) -> String {
    fs::read_to_string(backend().join(relative))
        .unwrap_or_else(|_| panic!("{relative} is named by the risk scope map and is missing"))
}

fn holds(relative: &str, needle: &str, item: &str) {
    assert!(
        read(relative).contains(needle),
        "risk scope item `{item}`: `{needle}` is no longer in {relative}"
    );
}

const PLATFORM: &str = "crates/runtime/qip-kernel/src/platform.rs";

/// (item, seam file, seam symbol, test file, test name).
const RISK_MANAGEMENT: [(&str, &str, &str, &str, &str); 12] = [
    (
        "pre-trade",
        "crates/services/qip-risk-engine/src/pretrade.rs",
        "pub fn check(",
        "crates/services/qip-risk-engine/tests/risk_engine.rs",
        "fn an_oversized_single_order_is_refused()",
    ),
    (
        "post-trade",
        "crates/services/qip-risk-engine/src/monitor.rs",
        "pub fn observe(",
        PLATFORM,
        "fn a_fill_that_gapped_past_the_pre_trade_assumption_is_charged_at_the_price_it_filled_at()",
    ),
    (
        "market",
        "crates/libs/qip-risk/src/limits.rs",
        "pub fn with_tail_risk(",
        PLATFORM,
        "fn the_expected_shortfall_limit_can_actually_fire()",
    ),
    (
        "liquidity",
        PLATFORM,
        "fn liquidatable_within(",
        PLATFORM,
        "fn the_liquidity_floor_limit_can_actually_fire()",
    ),
    (
        "counterparty",
        "crates/libs/qip-risk/src/limits.rs",
        "pub const COUNTERPARTY_AXIS",
        "crates/runtime/qip-kernel/tests/risk_aggregates.rs",
        "fn a_fill_is_charged_to_the_venue_that_executed_it_and_an_order_that_would_overfill_that_counterparty_is_refused()",
    ),
    (
        "concentration",
        "crates/libs/qip-risk/src/limits.rs",
        "\"sector-concentration\"",
        "crates/runtime/qip-kernel/tests/risk_aggregates.rs",
        "fn a_fill_is_charged_to_its_sector_bucket_and_an_order_that_would_overfill_the_bucket_is_refused()",
    ),
    (
        "causal-driver",
        "crates/runtime/qip-kernel/src/shared_cause.rs",
        "fn seat_causal_drivers(",
        PLATFORM,
        "fn the_per_causal_driver_limit_can_actually_fire()",
    ),
    (
        "settlement",
        "crates/edge/qip-edge/src/settlement.rs",
        "pub const GATE_SETTLEMENT",
        "crates/edge/qip-edge/tests/settlement.rs",
        "fn a_cycle_whose_leg_spends_proceeds_that_settle_on_a_calendar_is_refused_whole_before_any_capital_is_held()",
    ),
    (
        "model",
        PLATFORM,
        "pub fn venue_misfit(",
        "crates/runtime/qip-kernel/tests/learning.rs",
        "fn a_venue_whose_twin_is_persistently_wrong_is_narrowed_to_a_quarter_of_equity()",
    ),
    (
        "data",
        "crates/libs/qip-risk/src/limits.rs",
        "pub unevaluated: BTreeMap<String, String>",
        PLATFORM,
        "fn a_book_whose_liquidity_could_not_be_read_refuses_orders_rather_than_admitting_them()",
    ),
    (
        "operational",
        "crates/libs/qip-risk/src/operational.rs",
        "pub fn with_operational_failure(",
        PLATFORM,
        "fn gross_stranded_at_a_venue_the_platform_withdrew_blocks_through_risk_state()",
    ),
    (
        "systemic",
        "crates/libs/qip-risk/src/systemic.rs",
        "pub fn with_systemic_contagion(",
        PLATFORM,
        "fn one_causal_driver_reaching_two_regions_blocks_through_risk_state_and_one_region_does_not()",
    ),
];

#[test]
fn every_risk_management_scope_item_names_a_seam_and_a_test_that_still_exist() {
    assert_eq!(
        RISK_MANAGEMENT.len(),
        12,
        "premise: the capability map lists twelve risk types"
    );
    let mut items: Vec<&str> = RISK_MANAGEMENT.iter().map(|row| row.0).collect();
    items.sort_unstable();
    items.dedup();
    assert_eq!(items.len(), 12, "premise: no item is listed twice");
    for (item, seam_file, seam, test_file, test) in RISK_MANAGEMENT {
        holds(seam_file, seam, item);
        let tests = read(test_file);
        let at = tests.find(test).unwrap_or_else(|| {
            panic!("risk scope item `{item}`: the test `{test}` is no longer in {test_file}")
        });
        // The name alone is not a test: it must sit under a test attribute,
        // or deleting `#[test]` would leave a helper this suite still counts.
        let before = &tests[..at];
        let attribute = before
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("//"))
            .unwrap_or_default();
        assert!(
            attribute.starts_with("#[test]") || before.trim_end().ends_with("#[test]"),
            "risk scope item `{item}`: `{test}` in {test_file} is not under #[test] \
             (the line above reads {attribute:?})"
        );
    }
}

/// The body of `fn name` in `source`, from its signature to the next `fn`
/// at the same indentation. Enough for inspection; not a parser.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is missing"));
    let rest = &source[start..];
    let tail = &rest[signature.len()..];
    let end = ["\n    pub fn ", "\n    fn ", "\n    pub(crate) fn "]
        .iter()
        .filter_map(|next| tail.find(next))
        .min()
        .map_or(rest.len(), |offset| offset + signature.len());
    &rest[..end]
}

#[test]
fn the_central_risk_read_charges_every_book_level_item_and_each_root_builds_a_platform() {
    let platform = read(PLATFORM);
    let risk_read = body_of(&platform, "pub fn risk_state_from(");
    // Each book-level producer, in the one state the veto and the monitor
    // read. A producer moved out of this function stops reaching the gate.
    for (item, call) in [
        ("market", "with_tail_risk("),
        (
            "causal-driver and systemic",
            "crate::shared_cause::observe(",
        ),
        ("counterparty margin", "self.with_margin_and_funding("),
        ("model", "self.with_twin_misfit("),
        (
            "operational",
            "qip_risk::operational::with_operational_failure(",
        ),
        ("liquidity", "self.liquidatable_within("),
        ("data", "with_unevaluated(LIQUIDITY_FIGURE"),
    ] {
        assert!(
            risk_read.contains(call),
            "risk scope item `{item}`: `{call}` no longer runs inside Platform::risk_state_from"
        );
    }
    holds(
        "crates/runtime/qip-kernel/src/shared_cause.rs",
        "qip_risk::systemic::with_systemic_contagion(",
        "systemic",
    );
    // Each central composition root builds a Platform, so the risk read is
    // reached from a deployed binary and not only from tests.
    for root in [
        "crates/apps/qip-api/src/main.rs",
        "crates/apps/qip-fastbrain/src/main.rs",
        "crates/apps/qip-deepbrain/src/main.rs",
    ] {
        let main = read(root);
        assert!(
            main.contains("Platform::new(")
                || main.contains("Platform::new_serving(")
                || main.contains("Platform::with_model_provider("),
            "{root} no longer builds a Platform, so the central risk read has no root there"
        );
    }
    // Settlement is the cell's gate: the edge node's root assembles the cell
    // with its risk gate, and the cell's pass applies the settlement gate.
    holds(
        "crates/apps/qip-edge-node/src/lib.rs",
        "assemble_with_risk(",
        "settlement",
    );
    holds(
        "crates/edge/qip-edge/src/cell.rs",
        "GATE_SETTLEMENT,",
        "settlement",
    );
    holds(
        "crates/apps/qip-edge-node/src/pass.rs",
        "cell.work(",
        "settlement",
    );
}

#[test]
fn every_book_level_axis_item_is_capped_by_the_shipped_limit_set() {
    let shipped = qip_risk::LimitSet::conservative_default();
    for (item, axis) in [
        ("concentration", "sector"),
        ("causal-driver", qip_risk::CAUSAL_DRIVER_AXIS),
        ("model", qip_risk::limits::VENUE_TWIN_MISFIT_AXIS),
        ("operational", qip_risk::OPERATIONAL_FAILURE_AXIS),
        ("systemic", qip_risk::SYSTEMIC_CONTAGION_AXIS),
    ] {
        assert!(
            shipped.limits.iter().any(|limit| matches!(
                &limit.kind,
                qip_risk::LimitKind::MaxAxisWeight { axis: named, .. } if named == axis
            )),
            "risk scope item `{item}`: the shipped set caps no `{axis}` axis, so the figure \
             is charged and nothing refuses on it"
        );
    }
    assert!(
        shipped
            .limits
            .iter()
            .any(|limit| matches!(limit.kind, qip_risk::LimitKind::MaxExpectedShortfall { .. })),
        "risk scope item `market`: the shipped set carries no expected-shortfall cap"
    );
    assert!(
        shipped
            .limits
            .iter()
            .any(|limit| matches!(limit.kind, qip_risk::LimitKind::MinLiquidity { .. })),
        "risk scope item `liquidity`: the shipped set carries no liquidity floor"
    );
}
