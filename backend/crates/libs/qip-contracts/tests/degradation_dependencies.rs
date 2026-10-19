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
    assert_eq!(
        names,
        [
            "ledger",
            "custody",
            "clock",
            "fabric",
            "feed",
            "venue_session"
        ]
    );
    let state = with(Capability::Ledger, Freshness::Stale);
    let text = serde_json::to_string(&state).unwrap();
    assert!(text.contains("\"ledger\":\"stale\""));
    let back: DegradationState = serde_json::from_str(&text).unwrap();
    assert_eq!(back, state);
    assert_eq!(back.narrowed().len(), 1);
}

#[test]
fn every_dependency_declares_what_its_loss_does_to_size() {
    // The envelope each dependency of a reflex cell carries, stated as the
    // multiplier its stale and unavailable readings compound to. A partial
    // loss of the per-venue rows is one because the cell's per-venue gates
    // already act on it; the total loss is zero because nothing is left to
    // trade against. A dependency absent from this table would be one whose
    // loss the platform had not decided anything about.
    let declared = [
        (Capability::Ledger, dec!("0.5"), dec!("0")),
        (Capability::Custody, dec!("0.75"), dec!("0.5")),
        (Capability::Clock, dec!("0.75"), dec!("0.5")),
        (Capability::Fabric, dec!("1"), dec!("1")),
        (Capability::Feed, dec!("1"), dec!("0")),
        (Capability::VenueSession, dec!("1"), dec!("0")),
    ];
    assert_eq!(
        declared.len(),
        Capability::dependencies().len(),
        "a dependency has no declared envelope"
    );
    for capability in Capability::dependencies() {
        let (_, stale, unavailable) = declared
            .iter()
            .find(|(declared, _, _)| *declared == capability)
            .unwrap_or_else(|| panic!("{} has no declared envelope", capability.as_str()));
        assert_eq!(
            with(capability, Freshness::Stale).dependency_multiplier(),
            *stale,
            "{} stale",
            capability.as_str()
        );
        assert_eq!(
            with(capability, Freshness::Unavailable).dependency_multiplier(),
            *unavailable,
            "{} unavailable",
            capability.as_str()
        );
        assert_eq!(
            with(capability, Freshness::Fresh).dependency_multiplier(),
            dec!("1"),
            "{} fresh",
            capability.as_str()
        );
    }
}

#[test]
fn a_clock_that_was_observed_degraded_pauses_cross_venue_ordering_and_one_never_observed_does_not()
{
    let healthy = DegradationState::nothing_known();
    // The premise: the clock really is unobserved here, so `freshness` reads
    // it unavailable and a pause on that reading would pause everything.
    assert_eq!(healthy.observed(Capability::Clock), None);
    assert_eq!(healthy.freshness(Capability::Clock), Freshness::Unavailable);
    assert!(!healthy.pauses_cross_venue_ordering());

    assert!(!with(Capability::Clock, Freshness::Fresh).pauses_cross_venue_ordering());
    assert!(with(Capability::Clock, Freshness::Stale).pauses_cross_venue_ordering());
    assert!(with(Capability::Clock, Freshness::Unavailable).pauses_cross_venue_ordering());
    // Another dependency's loss is not a clock fault.
    assert!(!with(Capability::Feed, Freshness::Unavailable).pauses_cross_venue_ordering());
}
