//! The simulated venue's settlement clock (ADR 0530, R12-LEDGER-1197).
//!
//! The end-to-end proof — a T+2 fill reaching the book's cash on the second
//! settlement day, through the cell, the spool and the production ledger —
//! is `qip-acceptance`'s `vertical_slice_cell_ledger.rs`. This suite holds
//! the node's own seams around it: the variable that states the terms, the
//! refusal to change terms under orders already accepted, and the re-keying
//! a requoted child needs so a settlement names the order the ledger holds.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::dec;
use qip_core::error::Result;
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_edge::cell::{Placer, SettlementReport};
use qip_edge::settlement::{SettlementConvention, SettlementTerms};
use qip_edge_node::gateway::{SETTLEMENT_VARIABLE, SimulatedGateway, parse_settlement};
use qip_execution_engine::creation::{
    CreationAction, CreationAuthority, CreationPermissions, CreationRequest,
};
use qip_execution_engine::order::Side;

const VENUE: &str = "XLON";

/// Thursday 2025-10-09 08:53:20 UTC, inside the weekday 16:00 cut-off.
fn thursday() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-ACME".to_string())
}

fn permitting(gateway: SimulatedGateway) -> Result<SimulatedGateway> {
    let venue = gateway.venue().to_string();
    let mut permissions = CreationPermissions::new();
    for action in [
        CreationAction::Create,
        CreationAction::List,
        CreationAction::Seed,
    ] {
        let request = CreationRequest::new(action, venue.as_str(), object().as_str(), "paper");
        for authority in CreationAuthority::ALL {
            permissions.record(authority, &request, "test fixture")?;
        }
    }
    gateway.with_listing_permissions(permissions, "paper")
}

/// A T+2 venue with an offer resting at 101, and one buy filled against it
/// as `order_id`, its report already handed over.
fn filled_t2(order_id: &str) -> Result<SimulatedGateway> {
    let mut gateway = permitting(SimulatedGateway::new(VenueId::new(VENUE), 7, thursday())?)?;
    gateway.install_settlement_terms(SettlementTerms::weekday(SettlementConvention::T2)?)?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), thursday())?;
    gateway.place(
        order_id,
        &object(),
        &VenueId::new(VENUE),
        BookSide::Ask,
        dec!("10"),
        dec!("101"),
        thursday(),
    )?;
    assert_eq!(
        gateway.execution_reports().len(),
        1,
        "premise: the venue filled the order"
    );
    Ok(gateway)
}

fn monday_nine() -> Timestamp {
    thursday()
        .start_of_day()
        .saturating_add(Duration::from_secs(4 * 86_400 + 9 * 3_600))
}

#[test]
fn the_settlement_variable_is_instant_unset_a_convention_when_named_and_refused_otherwise() {
    // Unset is the simulator as it was before it had a clock; a stated
    // convention is a cycle; anything else stops the node naming the
    // forms, rather than being read as instant. Mutation verified: mapping
    // the parse error to `Ok(SettlementTerms::instant())` accepts `T+3` and
    // fails the refusal assertion.
    assert!(parse_settlement(None).expect("unset").is_instant());
    assert!(parse_settlement(Some("  ")).expect("blank").is_instant());
    assert!(
        parse_settlement(Some("instant"))
            .expect("named")
            .is_instant()
    );
    assert_eq!(
        parse_settlement(Some("T+2"))
            .expect("a convention")
            .convention(),
        Some(SettlementConvention::T2)
    );
    let refusal = parse_settlement(Some("T+3")).expect_err("T+3 is no convention here");
    assert!(
        refusal.message().starts_with("configuration:")
            && refusal.message().contains(SETTLEMENT_VARIABLE),
        "{}",
        refusal.message()
    );
}

#[test]
fn settlement_terms_cannot_change_once_the_venue_has_accepted_an_order() -> Result<()> {
    // A fill booked under one calendar and settled under another would be
    // dated by terms the cell never judged it against. Mutation verified:
    // removing the `submitted_count() > 0` check accepts the change and
    // fails the refusal.
    let mut gateway = filled_t2("o-1")?;
    let refusal = gateway
        .install_settlement_terms(SettlementTerms::instant())
        .expect_err("terms changed under an accepted order");
    assert!(
        refusal.message().contains("before the first order"),
        "{}",
        refusal.message()
    );
    assert_eq!(
        gateway.settlement_terms().convention(),
        Some(SettlementConvention::T2),
        "the refused change was applied"
    );
    Ok(())
}

#[test]
fn a_requoted_childs_fill_settles_under_the_order_id_the_cell_and_the_ledger_hold() -> Result<()> {
    // The requoter hands the cell a child's fill under its parent's id, so
    // the ledger holds the parent's in-settlement row. A settlement naming
    // the child would settle nothing there and park the cell. Mutation
    // verified: an empty `relabel_unsettled` body leaves the entry under
    // `o-1-r1` and the settlement names it, failing the equality.
    let mut gateway = filled_t2("o-1-r1")?;
    gateway.relabel_unsettled("o-1-r1", "o-1");
    gateway.advance_to(monday_nine())?;
    assert_eq!(
        gateway.settlements(),
        vec![SettlementReport {
            order_id: "o-1".to_string(),
            venue: VenueId::new(VENUE),
            settlement_date: monday_nine(),
        }]
    );
    assert_eq!(gateway.unsettled_count(), 0);
    assert!(
        gateway.settlements().is_empty(),
        "a settlement was reported twice"
    );
    Ok(())
}

#[test]
fn a_venue_on_instant_terms_holds_nothing_and_reports_no_settlement() -> Result<()> {
    // The default, unchanged: cash at the fill, so nothing waits and the
    // ledger books nothing in settlement. Mutation verified: dropping the
    // `on_cycle` guard in `SettlementClock::filled` holds the instant fill
    // and fails the first assertion.
    let mut gateway = permitting(SimulatedGateway::new(VenueId::new(VENUE), 7, thursday())?)?;
    gateway.seed_touch(&object(), Side::Sell, dec!("101"), dec!("400"), thursday())?;
    gateway.place(
        "o-1",
        &object(),
        &VenueId::new(VENUE),
        BookSide::Ask,
        dec!("10"),
        dec!("101"),
        thursday(),
    )?;
    assert_eq!(gateway.execution_reports().len(), 1, "premise: filled");
    assert_eq!(gateway.unsettled_count(), 0);
    gateway.advance_to(monday_nine())?;
    assert!(gateway.settlements().is_empty());
    Ok(())
}
