//! One admission header for every sensed record (ADR 0132, R12-TICK-2053).

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::admission::{Admissible, Admission, Admitted, OwnTimes};
use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp, dec};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn granted() -> Entitlement {
    Entitlement::Granted {
        dataset: "xnys-itch".to_string(),
        usage: Usage::Trade,
        expires_at: t(10_000),
    }
}

fn message(venue_time: Timestamp, capture_time: Timestamp) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XNYS"), "itch-a", 0, 1),
        MessageBody::Trade {
            price: dec!("100.25"),
            quantity: dec!("100"),
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Bid),
        },
        venue_time,
        capture_time,
    )
}

/// The leak the header exists to stop: an event stamped after the instant
/// this platform received it, by more than the clocks are doubted by, was
/// knowable before it was true. Inside the doubt the two instants are one
/// instant as far as the clocks can tell. Mutation: drop the `lead >
/// uncertainty` refusal in `Admission::check_order` and the first assertion
/// fails.
#[test]
fn an_admission_whose_event_time_follows_its_receive_time_beyond_uncertainty_is_refused()
-> Result<()> {
    let doubt = Duration::from_millis(5);
    let refused = Admission::new(t(20), t(10), t(10), doubt, "hash", granted())
        .expect_err("received ten seconds before it happened");
    assert!(
        refused.message().contains("received before it happened"),
        "{}",
        refused.message()
    );
    // Within the doubt: admitted, and the header keeps both instants apart.
    let inside = Admission::new(
        Timestamp::from_nanos(t(10).as_nanos() + doubt.as_nanos()),
        t(10),
        t(10),
        doubt,
        "hash",
        granted(),
    )?;
    assert_ne!(inside.event_time(), inside.receive_time());
    // One nanosecond past the doubt: refused.
    assert!(
        Admission::new(
            Timestamp::from_nanos(t(10).as_nanos() + doubt.as_nanos() + 1),
            t(10),
            t(10),
            doubt,
            "hash",
            granted(),
        )
        .is_err(),
        "a lead one nanosecond past the clocks' doubt was admitted"
    );
    // The ordinary case — received after it happened — is admitted.
    let late = Admission::new(t(10), t(20), t(20), Duration::ZERO, "hash", granted())?;
    assert_eq!(late.event_time(), t(10));
    assert_eq!(late.receive_time(), t(20));
    Ok(())
}

/// The other refusals the one constructor holds: a negative doubt, an
/// entitlement not granted at the event's time, an empty source hash.
#[test]
fn the_one_constructor_refuses_a_negative_doubt_a_lapsed_entitlement_and_an_empty_hash() {
    assert!(
        Admission::new(
            t(10),
            t(20),
            t(20),
            Duration::from_millis(-1),
            "hash",
            granted()
        )
        .is_err()
    );
    let lapsed = Entitlement::Granted {
        dataset: "xnys-itch".to_string(),
        usage: Usage::Trade,
        expires_at: t(5),
    };
    assert!(Admission::new(t(10), t(20), t(20), Duration::ZERO, "hash", lapsed).is_err());
    assert!(Admission::new(t(10), t(20), t(20), Duration::ZERO, "  ", granted()).is_err());
}

/// A record whose own times disagree with its header carries two accounts of
/// one fact; the header is the authority and the record is refused, not
/// reconciled. Mutation: drop either comparison in `Admitted::admit` and the
/// matching assertion fails.
#[test]
fn a_record_whose_own_times_disagree_with_its_header_is_refused() -> Result<()> {
    let admission = Admission::new(t(10), t(20), t(20), Duration::ZERO, "hash", granted())?;
    let agreeing = Admitted::admit(message(t(10), t(20)), admission.clone())?;
    assert_eq!(agreeing.record().venue_time, t(10));
    assert_eq!(agreeing.admission().receive_time(), t(20));

    let other_event = Admitted::admit(message(t(11), t(20)), admission.clone())
        .expect_err("the record says it happened a second later than its header");
    assert!(
        other_event.message().contains("happened at"),
        "{}",
        other_event.message()
    );
    let other_receipt = Admitted::admit(message(t(10), t(21)), admission)
        .expect_err("the record says it was received a second later than its header");
    assert!(
        other_receipt.message().contains("received at"),
        "{}",
        other_receipt.message()
    );
    Ok(())
}

/// `MarketEvent::new` admits through the same constructor, so the edge
/// feed's events — the only production caller — are held to the order rule
/// without a second copy of it, and the header read back from an event is
/// the one it was admitted under. Mutation: make `MarketEvent::new` build its
/// fields without `Admission::new` and the refusal below is not raised.
#[test]
fn a_market_event_is_admitted_through_the_one_constructor_and_yields_its_header() -> Result<()> {
    let payload = message(t(10), t(20));
    let hash = MarketEvent::hash_payload(&payload)?;
    let event = MarketEvent::new(
        payload.clone(),
        t(10),
        t(20),
        t(21),
        Duration::from_millis(5),
        hash.clone(),
        granted(),
    )?;
    let admission = event.admission();
    assert_eq!(admission.event_time(), t(10));
    assert_eq!(admission.receive_time(), t(20));
    assert_eq!(admission.normalized_time(), t(21));
    assert_eq!(admission.uncertainty(), Duration::from_millis(5));
    assert_eq!(admission.source_hash(), hash);
    assert_eq!(
        payload.own_times(),
        OwnTimes {
            event_time: Some(t(10)),
            receive_time: Some(t(20)),
        }
    );

    let refused = MarketEvent::new(
        payload,
        t(30),
        t(20),
        t(20),
        Duration::from_millis(5),
        hash,
        granted(),
    )
    .expect_err("an event received ten seconds before it happened");
    assert!(
        refused.message().contains("received before it happened"),
        "{}",
        refused.message()
    );
    Ok(())
}
