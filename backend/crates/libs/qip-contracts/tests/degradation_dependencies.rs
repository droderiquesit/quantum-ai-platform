//! The four infrastructure rows of the v12 failure table — ledger, custody,
//! clock, fabric — beside the cognitive rows of §6.2 (R12-COVERAGE-7168).

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::degradation::{Capability, DegradationState, Freshness};
use qip_core::{Decimal, dec};

fn with(capability: Capability, freshness: Freshness) -> DegradationState {
    let mut state = DegradationState::fully_available();
    state.observe(capability, freshness);
    state
}

/// Mutation: swap `LEDGER_UNAVAILABLE_MULTIPLIER` and `FABRIC_MULTIPLIER` in
/// `dependency_multiplier` — the lost ledger then sizes at one and the lost
/// fabric at nothing, and both assertions below fail.
#[test]
fn an_unavailable_fabric_still_permits_a_sized_pass_while_an_unavailable_ledger_halts_new_exposure()
{
    let healthy = DegradationState::fully_available();
    assert_eq!(
        healthy.dependency_multiplier(),
        Decimal::from_int(1),
        "premise: a healthy platform is not narrowed by any dependency"
    );
    assert!(healthy.permits_new_exposure());

    for freshness in [Freshness::Stale, Freshness::Unavailable] {
        let fabric = with(Capability::Fabric, freshness);
        assert_eq!(
            fabric.dependency_multiplier(),
            Decimal::from_int(1),
            "a {freshness:?} fabric narrowed size; cells decide alone"
        );
        assert!(fabric.permits_new_exposure());
        assert!(
            !fabric.narrowed().is_empty(),
            "the fabric loss must still be reported, only not charged twice"
        );
    }

    let ledger = with(Capability::Ledger, Freshness::Unavailable);
    assert_eq!(ledger.dependency_multiplier(), Decimal::ZERO);
    assert!(!ledger.permits_new_exposure());
    // Narrowing, not stopping: the platform itself does not halt, so cancels
    // and flattening are not prevented by the table.
    assert!(!ledger.halts());

    let stale_ledger = with(Capability::Ledger, Freshness::Stale);
    assert_eq!(stale_ledger.dependency_multiplier(), dec!("0.5"));
    assert!(stale_ledger.permits_new_exposure());
}

#[test]
fn custody_and_clock_narrow_and_compound_without_halting() {
    assert_eq!(
        with(Capability::Custody, Freshness::Stale).dependency_multiplier(),
        dec!("0.75")
    );
    assert_eq!(
        with(Capability::Custody, Freshness::Unavailable).dependency_multiplier(),
        dec!("0.5")
    );
    assert_eq!(
        with(Capability::Clock, Freshness::Stale).dependency_multiplier(),
        dec!("0.75")
    );
    let mut both = with(Capability::Custody, Freshness::Unavailable);
    both.observe(Capability::Clock, Freshness::Unavailable);
    assert_eq!(both.dependency_multiplier(), dec!("0.25"));
    assert!(both.permits_new_exposure());
}

#[test]
fn a_dependency_nobody_observed_neither_narrows_nor_reads_as_fresh() {
    let silent = DegradationState::nothing_known();
    for capability in Capability::dependencies() {
        assert!(capability.is_dependency());
        assert_eq!(
            silent.freshness(capability),
            Freshness::Unavailable,
            "{} must still fail closed when asked",
            capability.as_str()
        );
        assert_eq!(silent.observed(capability), None);
    }
    assert_eq!(
        silent.dependency_multiplier(),
        Decimal::from_int(1),
        "an unmeasured dependency must not scale size by a number nobody computed"
    );
    assert!(
        silent
            .narrowed()
            .keys()
            .all(|capability| !capability.is_dependency()),
        "unwired dependency rows were reported as narrowing"
    );
    // The cognitive table is untouched by the new rows.
    assert_eq!(silent.sizing_multiplier(), dec!("0.375"));
    assert_eq!(silent.central_sizing_multiplier(), dec!("0.1875"));
    assert!(Capability::all().iter().all(|c| !c.is_dependency()));
}

#[test]
fn the_dependency_rows_have_names_and_survive_a_round_trip() {
    let names: Vec<&str> = Capability::dependencies()
        .iter()
        .map(|capability| capability.as_str())
        .collect();
    assert_eq!(names, ["ledger", "custody", "clock", "fabric"]);
    let state = with(Capability::Ledger, Freshness::Stale);
    let text = serde_json::to_string(&state).unwrap();
    assert!(text.contains("\"ledger\":\"stale\""));
    let back: DegradationState = serde_json::from_str(&text).unwrap();
    assert_eq!(back, state);
    assert_eq!(back.narrowed().len(), 1);
}
