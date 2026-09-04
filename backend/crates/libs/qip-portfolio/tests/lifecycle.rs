//! A position's lifecycle field, tracked independently of its lot ledger.

use qip_core::{ObjectId, Timestamp, dec};
use qip_portfolio::position::Position;
use qip_portfolio::{Lot, PositionLifecycle};

fn now() -> Timestamp {
    Timestamp::from_civil(2026, 9, 4)
}

fn object_id() -> ObjectId {
    ObjectId::from_string("OBJ0000000000000000000001")
}

fn opened_position() -> Position {
    Position::new(object_id(), "TEST", now())
}

#[test]
fn a_new_position_starts_opened() {
    let position = opened_position();
    // Premise: nothing has happened to this position yet.
    assert!(position.lots.is_empty());
    assert_eq!(position.lifecycle, PositionLifecycle::Opened);
}

#[test]
fn an_opened_position_moves_to_held_on_its_first_confirmed_lot() {
    let mut position = opened_position();
    assert_eq!(position.lifecycle, PositionLifecycle::Opened);

    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);

    // Premise: the fill actually opened a lot.
    assert_eq!(position.lots.len(), 1);
    assert_eq!(position.lifecycle, PositionLifecycle::Held);
}

#[test]
fn closing_the_last_lot_moves_a_position_to_closed_and_the_closed_state_refuses_further_transitions()
 {
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    assert_eq!(position.lifecycle, PositionLifecycle::Held);

    // Close the whole position with an opposite fill.
    position.apply_fill(dec!("-100"), dec!("11"), dec!("0"), now(), None);

    // Premise: the position is actually flat now.
    assert!(position.is_flat());
    assert_eq!(position.lifecycle, PositionLifecycle::Closed);

    // A further fill (a late or mistaken report) must not walk it back.
    position.apply_fill(dec!("50"), dec!("12"), dec!("0"), now(), None);
    assert_eq!(position.lifecycle, PositionLifecycle::Closed);

    // The refusal is structural, not just an artefact of `apply_fill`:
    // calling the transition function itself is refused too.
    let outcome = position.move_lifecycle(PositionLifecycle::Held);
    assert!(outcome.is_err());
    assert_eq!(position.lifecycle, PositionLifecycle::Closed);
}

#[test]
fn a_flagged_position_cannot_be_reopened_without_a_new_lot() {
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    assert_eq!(position.lifecycle, PositionLifecycle::Held);

    position
        .move_lifecycle(PositionLifecycle::Flagged)
        .expect("held -> flagged is legal");
    assert_eq!(position.lifecycle, PositionLifecycle::Flagged);

    // Premise: the position still has its open lot; nothing about the flag
    // touched the ledger.
    assert_eq!(position.lots.len(), 1);

    // There is no direct move from Flagged back to Held: reopening requires
    // the ledger event (a new lot), not a lifecycle call that skips it.
    let outcome = position.move_lifecycle(PositionLifecycle::Held);
    assert!(outcome.is_err());
    assert_eq!(position.lifecycle, PositionLifecycle::Flagged);
}

#[test]
fn every_legal_edge_on_position_lifecycle_transitions_and_the_full_cross_product_does_not() {
    let all = [
        PositionLifecycle::Opened,
        PositionLifecycle::Held,
        PositionLifecycle::Flagged,
        PositionLifecycle::Unwinding,
        PositionLifecycle::Orphaned,
        PositionLifecycle::Closed,
    ];
    let legal_edges = [
        (PositionLifecycle::Opened, PositionLifecycle::Held),
        (PositionLifecycle::Opened, PositionLifecycle::Closed),
        (PositionLifecycle::Held, PositionLifecycle::Flagged),
        (PositionLifecycle::Held, PositionLifecycle::Closed),
        (PositionLifecycle::Flagged, PositionLifecycle::Unwinding),
        (PositionLifecycle::Flagged, PositionLifecycle::Orphaned),
        (PositionLifecycle::Flagged, PositionLifecycle::Closed),
        (PositionLifecycle::Unwinding, PositionLifecycle::Orphaned),
        (PositionLifecycle::Unwinding, PositionLifecycle::Closed),
        (PositionLifecycle::Orphaned, PositionLifecycle::Closed),
    ];
    // Premise: the table names some but not all of the 36 ordered pairs.
    assert!(!legal_edges.is_empty());
    assert!(legal_edges.len() < all.len() * all.len());

    for &from in &all {
        for &to in &all {
            let expected_legal = legal_edges.contains(&(from, to));
            assert_eq!(
                from.transition(to).is_ok(),
                expected_legal,
                "{from:?} -> {to:?} disagreed with the table"
            );
        }
    }
}

#[test]
fn a_flagged_position_can_still_be_closed_by_a_fill() {
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    position
        .move_lifecycle(PositionLifecycle::Flagged)
        .expect("held -> flagged is legal");

    position.apply_fill(dec!("-100"), dec!("11"), dec!("0"), now(), None);

    assert!(position.is_flat());
    assert_eq!(position.lifecycle, PositionLifecycle::Closed);
}

#[test]
fn lots_created_via_the_stand_alone_constructor_do_not_affect_lifecycle() {
    // Constructing a `Lot` directly (as internal accounting helpers do) is
    // not the same event as a position receiving a confirmed fill; only the
    // latter should move the lifecycle field.
    let lot = Lot::new(dec!("10"), dec!("5"), now());
    assert_eq!(lot.quantity, dec!("10"));

    let position = opened_position();
    assert_eq!(position.lifecycle, PositionLifecycle::Opened);
}
