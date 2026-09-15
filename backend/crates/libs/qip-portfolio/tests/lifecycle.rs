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
    assert_eq!(position.lifecycle(), PositionLifecycle::Opened);
}

#[test]
fn an_opened_position_moves_to_held_on_its_first_confirmed_lot() {
    let mut position = opened_position();
    assert_eq!(position.lifecycle(), PositionLifecycle::Opened);

    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);

    // Premise: the fill actually opened a lot.
    assert_eq!(position.lots.len(), 1);
    assert_eq!(position.lifecycle(), PositionLifecycle::Held);
}

#[test]
fn closing_the_last_lot_moves_a_position_to_closed_and_the_closed_state_refuses_further_transitions()
 {
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    assert_eq!(position.lifecycle(), PositionLifecycle::Held);

    // Close the whole position with an opposite fill.
    position.apply_fill(dec!("-100"), dec!("11"), dec!("0"), now(), None);

    // Premise: the position is actually flat now.
    assert!(position.is_flat());
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);

    // The refusal is structural, not just an artefact of `apply_fill`:
    // calling the transition function itself is refused too, so nothing
    // can walk a flat, closed record back to held by assertion alone.
    let outcome = position.move_lifecycle(PositionLifecycle::Held);
    assert!(outcome.is_err());
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);
}

#[test]
fn a_confirmed_lot_on_a_closed_record_starts_a_new_round_trip_that_closes_again() {
    // The failure this prevents happened: an earlier version of this file
    // asserted that a fill after closure left the lifecycle at `Closed`.
    // `apply_fill` accepts the lot into the ledger regardless, so the record
    // then held shares while reading as closed, and the close that followed
    // tried `Closed -> Closed` and tripped the terminal guard in every suite
    // that traded a name twice through one record (the simulated exchange's
    // books test, the deep brain's evolution rounds). The lifecycle must
    // describe the ledger; refusing a late report is the caller's job before
    // the lot exists, not this field's after it does.
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    position.apply_fill(dec!("-100"), dec!("11"), dec!("0"), now(), None);
    // Premise: the first round trip is closed and booked.
    assert!(position.is_flat());
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);
    assert_eq!(position.trade_count(), 1);

    // A new confirmed lot re-enters the name through the same record.
    position.apply_fill(dec!("50"), dec!("12"), dec!("0"), now(), None);
    assert_eq!(position.quantity(), dec!("50"));
    assert_eq!(
        position.lifecycle(),
        PositionLifecycle::Held,
        "a record holding fifty shares must not read as closed"
    );
    // The first round trip's history is kept, not overwritten.
    assert_eq!(position.trade_count(), 1);

    // And the second round trip closes through the table's own edge.
    position.apply_fill(dec!("-50"), dec!("13"), dec!("0"), now(), None);
    assert!(position.is_flat());
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);
    assert_eq!(position.trade_count(), 2);
}

#[test]
fn a_flagged_position_cannot_be_reopened_without_a_new_lot() {
    let mut position = opened_position();
    position.apply_fill(dec!("100"), dec!("10"), dec!("0"), now(), None);
    assert_eq!(position.lifecycle(), PositionLifecycle::Held);

    position
        .move_lifecycle(PositionLifecycle::Flagged)
        .expect("held -> flagged is legal");
    assert_eq!(position.lifecycle(), PositionLifecycle::Flagged);

    // Premise: the position still has its open lot; nothing about the flag
    // touched the ledger.
    assert_eq!(position.lots.len(), 1);

    // There is no direct move from Flagged back to Held: reopening requires
    // the ledger event (a new lot), not a lifecycle call that skips it.
    let outcome = position.move_lifecycle(PositionLifecycle::Held);
    assert!(outcome.is_err());
    assert_eq!(position.lifecycle(), PositionLifecycle::Flagged);
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
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);
}

#[test]
fn lots_created_via_the_stand_alone_constructor_do_not_affect_lifecycle() {
    // Constructing a `Lot` directly (as internal accounting helpers do) is
    // not the same event as a position receiving a confirmed fill; only the
    // latter should move the lifecycle field.
    let lot = Lot::new(dec!("10"), dec!("5"), now());
    assert_eq!(lot.quantity, dec!("10"));

    let position = opened_position();
    assert_eq!(position.lifecycle(), PositionLifecycle::Opened);
}

// --- the two seams that raise §35.1's middle states -------------------------
//
// `Flagged` and `Unwinding` had no writer outside a test until these existed,
// which is the failure mode the whole lifecycle was at risk of: a table that
// refuses illegal moves perfectly and describes three states nothing ever
// enters reads as a working control and is a decoration.

use qip_core::{Currency, PortfolioId};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::costs::LiquidityProfile;
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_portfolio::portfolio::Portfolio;

fn instrument() -> FinancialObject {
    FinancialObject::builder(
        object_id(),
        "TEST",
        InstrumentType::CommonStock,
        LiquidityProfile::listed(dec!("1000000"), 3.0),
    )
    .venue("XNYS")
    .sector(Sector::InformationTechnology)
    .price(dec!("10"))
    .provenance(Provenance::synthetic("test", now()))
    .build(now())
    .expect("the fixture instrument is well formed")
}

/// A book holding one hundred units of [`instrument`].
fn book_holding_one_position() -> Portfolio {
    let mut portfolio = Portfolio::new(
        PortfolioId::from_string("PRT0000000000000000000001"),
        "test",
        Currency::USD,
        dec!("100000"),
        now(),
    );
    portfolio.apply_fill(
        &instrument(),
        dec!("100"),
        dec!("10"),
        dec!("0"),
        now(),
        None,
    );
    portfolio
}

#[test]
fn a_held_position_whose_thesis_is_withdrawn_is_flagged_and_raising_the_concern_again_records_nothing_new()
 {
    let mut portfolio = book_holding_one_position();
    // Premise: the fill actually reached the book and left it held, so what
    // the flag below moves is a real holding rather than an empty record.
    let position = portfolio
        .position(&object_id())
        .expect("the fill created the position");
    assert_eq!(position.quantity(), dec!("100"));
    assert_eq!(position.lifecycle(), PositionLifecycle::Held);

    assert_eq!(
        portfolio.flag_position(&object_id()),
        Ok(true),
        "the first concern against a held position is a new fact"
    );
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position is still there")
            .lifecycle(),
        PositionLifecycle::Flagged
    );

    // The ledger is untouched: a flag is a statement about the thesis, not a
    // trade. A flag that silently moved quantity would be a phantom fill.
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position is still there")
            .quantity(),
        dec!("100")
    );

    assert_eq!(
        portfolio.flag_position(&object_id()),
        Ok(false),
        "re-raising a concern that already stands is not a second fact"
    );
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position is still there")
            .lifecycle(),
        PositionLifecycle::Flagged,
        "a second flag must not have walked the record anywhere"
    );
}

#[test]
fn a_deliberate_close_may_only_begin_after_the_concern_that_caused_it_was_raised() {
    let mut portfolio = book_holding_one_position();
    // Premise: the position is merely held — no concern stands against it.
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the fill created the position")
            .lifecycle(),
        PositionLifecycle::Held
    );

    // The table has no `Held -> Unwinding` edge, and the seam carries that
    // refusal through instead of inventing a shortcut around it.
    let refused = portfolio.begin_unwind(&object_id());
    assert!(
        refused.is_err(),
        "a merely held position must not start unwinding: {refused:?}"
    );
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position is still there")
            .lifecycle(),
        PositionLifecycle::Held,
        "the refused move must leave the record exactly where it was"
    );

    // With the concern raised, the same call is admitted — the half that
    // distinguishes a working gate from one that refuses everything.
    assert_eq!(portfolio.flag_position(&object_id()), Ok(true));
    assert_eq!(portfolio.begin_unwind(&object_id()), Ok(true));
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position is still there")
            .lifecycle(),
        PositionLifecycle::Unwinding
    );
    assert_eq!(
        portfolio.begin_unwind(&object_id()),
        Ok(false),
        "re-sizing a resting exit is the same exit, not a second one"
    );
}

#[test]
fn a_position_that_finished_unwinding_closes_and_the_closed_record_refuses_both_seams() {
    let mut portfolio = book_holding_one_position();
    assert_eq!(portfolio.flag_position(&object_id()), Ok(true));
    assert_eq!(portfolio.begin_unwind(&object_id()), Ok(true));

    portfolio.apply_fill(
        &instrument(),
        dec!("-100"),
        dec!("11"),
        dec!("0"),
        now(),
        None,
    );

    // Premise: the unwind actually reached flat.
    let position = portfolio
        .position(&object_id())
        .expect("the position record survives the close");
    assert!(position.is_flat());
    assert_eq!(position.lifecycle(), PositionLifecycle::Closed);

    // `Closed` is terminal, and neither seam is a way around that.
    assert!(
        portfolio.flag_position(&object_id()).is_err(),
        "a closed position is not flagged"
    );
    assert!(
        portfolio.begin_unwind(&object_id()).is_err(),
        "a closed position is not unwound"
    );
    assert_eq!(
        portfolio
            .position(&object_id())
            .expect("the position record survives")
            .lifecycle(),
        PositionLifecycle::Closed
    );
}

#[test]
fn a_concern_raised_against_a_position_the_book_does_not_hold_is_refused_rather_than_creating_one()
{
    let mut portfolio = book_holding_one_position();
    let absent = ObjectId::from_string("OBJ0000000000000000000099");
    // Premise: the book holds one position and it is not this one.
    assert_eq!(portfolio.all_positions().count(), 1);
    assert!(portfolio.position(&absent).is_none());

    let refused = portfolio.flag_position(&absent);
    assert!(refused.is_err(), "{refused:?}");
    let Err(error) = refused else {
        panic!("a concern against a holding nobody has must be refused");
    };
    assert!(
        error.message().contains("holds no position"),
        "the refusal must name what is wrong: {}",
        error.message()
    );
    assert!(
        portfolio.begin_unwind(&absent).is_err(),
        "nor may an unwind conjure a position"
    );
    assert_eq!(
        portfolio.all_positions().count(),
        1,
        "a refused lifecycle move must not have inserted a position"
    );
}
