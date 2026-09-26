//! `qip_contracts::market_event::MarketEvent` — the seam red-team finding B1
//! named as missing: nothing on the path built a value of this type, so the
//! checks it forces (a non-negative uncertainty, a payload that still hashes
//! to what it claims, an active entitlement) never ran.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn origin(feed: &str, partition: u32, sequence: u64) -> Origin {
    Origin::new(VenueId::new("XNYS"), feed, partition, sequence)
}

fn message(origin: Origin, price: Decimal) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        origin,
        MessageBody::Trade {
            price,
            quantity: dec!("100"),
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Bid),
        },
        t(0),
        t(0),
    )
}

fn granted(expires_at: Timestamp) -> Entitlement {
    Entitlement::Granted {
        dataset: "xnys-itch".to_string(),
        usage: Usage::Trade,
        expires_at,
    }
}

#[test]
fn event_receive_and_normalized_time_are_kept_apart_and_a_negative_uncertainty_is_refused()
-> Result<()> {
    let payload = message(origin("itch-a", 0, 1), dec!("100.25"));
    let hash = MarketEvent::hash_payload(&payload)?;

    let event = MarketEvent::new(
        payload.clone(),
        t(10),
        t(20),
        t(30),
        Duration::from_millis(5),
        hash.clone(),
        granted(t(10_000)),
    )?;

    // The three clocks are stored as the three distinct values they were
    // given, not collapsed into one field that happens to answer three
    // accessors the same way.
    assert_eq!(event.event_time(), t(10));
    assert_eq!(event.receive_time(), t(20));
    assert_eq!(event.normalized_time(), t(30));
    assert_ne!(
        event.event_time(),
        event.receive_time(),
        "event time and receive time were conflated"
    );
    assert_ne!(
        event.receive_time(),
        event.normalized_time(),
        "receive time and normalized time were conflated"
    );
    assert_ne!(
        event.event_time(),
        event.normalized_time(),
        "event time and normalized time were conflated"
    );

    // Premise: the identical construction with a zero (non-negative)
    // uncertainty succeeds, so the refusal below is about the sign of the
    // value and nothing else about this construction.
    assert!(
        MarketEvent::new(
            payload.clone(),
            t(10),
            t(20),
            t(30),
            Duration::ZERO,
            hash.clone(),
            granted(t(10_000)),
        )
        .is_ok(),
        "a zero uncertainty, which is not negative, was refused"
    );

    let negative = MarketEvent::new(
        payload,
        t(10),
        t(20),
        t(30),
        Duration::from_nanos(-1),
        hash,
        granted(t(10_000)),
    );
    assert!(
        negative.is_err(),
        "a market event with a negative uncertainty was accepted"
    );
    Ok(())
}

#[test]
fn a_market_event_whose_payload_does_not_hash_to_its_recorded_hash_is_refused() -> Result<()> {
    let payload = message(origin("itch-a", 0, 1), dec!("100.25"));
    let correct_hash = MarketEvent::hash_payload(&payload)?;

    // Premise: the correct hash really does construct, so the refusal below
    // is attributable to the tampered hash and not to some other defect in
    // this construction.
    assert!(
        MarketEvent::new(
            payload.clone(),
            t(0),
            t(0),
            t(0),
            Duration::ZERO,
            correct_hash.clone(),
            granted(t(10_000)),
        )
        .is_ok(),
        "the payload's own correct hash was refused"
    );

    // A hash the same length as the real one but not equal to it — not a
    // malformed string, a wrong one, which is the case a recompute-and-skip
    // implementation would let through.
    let wrong_hash = "0".repeat(correct_hash.len());
    assert_ne!(
        wrong_hash, correct_hash,
        "test fixture picked the real hash by accident"
    );

    let tampered = MarketEvent::new(
        payload,
        t(0),
        t(0),
        t(0),
        Duration::ZERO,
        wrong_hash,
        granted(t(10_000)),
    );
    assert!(
        tampered.is_err(),
        "a payload that does not hash to its recorded source hash was accepted"
    );
    Ok(())
}

#[test]
fn a_market_event_without_an_entitlement_is_refused() -> Result<()> {
    let payload = message(origin("itch-a", 0, 1), dec!("100.25"));
    let hash = MarketEvent::hash_payload(&payload)?;

    // Premise: the same payload and hash construct fine under a genuine
    // grant, so the refusal below is attributable to the entitlement alone.
    assert!(
        MarketEvent::new(
            payload.clone(),
            t(0),
            t(0),
            t(0),
            Duration::ZERO,
            hash.clone(),
            granted(t(10_000)),
        )
        .is_ok(),
        "construction under a genuine grant was refused"
    );

    let denied = Entitlement::Denied {
        dataset: "xnys-itch".to_string(),
        usage: Usage::Trade,
        reason: "the agreement covers research only".to_string(),
    };
    let without_grant = MarketEvent::new(
        payload.clone(),
        t(0),
        t(0),
        t(0),
        Duration::ZERO,
        hash.clone(),
        denied,
    );
    assert!(
        without_grant.is_err(),
        "a market event was built on a denied entitlement"
    );

    // An entitlement that once existed but has expired before this event's
    // own time is, for this event, no entitlement at all.
    let expired = Entitlement::Granted {
        dataset: "xnys-itch".to_string(),
        usage: Usage::Trade,
        expires_at: t(-1),
    };
    let after_expiry = MarketEvent::new(payload, t(0), t(0), t(0), Duration::ZERO, hash, expired);
    assert!(
        after_expiry.is_err(),
        "a market event was built on an entitlement that had already expired"
    );
    Ok(())
}

#[test]
fn a_gap_in_source_sequence_is_detectable_between_consecutive_events() -> Result<()> {
    let build =
        |origin: Origin, event_time: Timestamp, receive_time: Timestamp| -> Result<MarketEvent> {
            let payload = message(origin, dec!("100.25"));
            let hash = MarketEvent::hash_payload(&payload)?;
            MarketEvent::new(
                payload,
                event_time,
                receive_time,
                receive_time,
                Duration::ZERO,
                hash,
                granted(t(10_000)),
            )
        };

    let earlier = build(origin("itch-a", 0, 41), t(100), t(100))?;

    // Contiguous: sequence 42 directly follows 41, so there is no gap
    // whatever the clocks say. This event's own event_time sits *before*
    // `earlier`'s and its receive_time sits *after* — a check that leaned on
    // either time field would already have to pick a direction, and picking
    // either one gives the wrong general answer somewhere in this test.
    let contiguous = build(origin("itch-a", 0, 42), t(50), t(400))?;
    assert_eq!(
        contiguous.sequence_gap(&earlier),
        None,
        "two adjacent sequence numbers were reported as a gap"
    );

    // A gap of exactly two missing messages (42 and 43), but event_time only
    // advances by a single nanosecond — routine on a busy book, and exactly
    // what a time-based check would read as "no time has passed, so nothing
    // was lost".
    let after_gap = build(
        origin("itch-a", 0, 44),
        t(100).saturating_add(Duration::from_nanos(1)),
        t(100),
    )?;
    assert_eq!(
        after_gap.sequence_gap(&earlier),
        Some(2),
        "a two-message sequence gap was not detected"
    );

    // A different feed on the same venue is a different stream: sequence
    // numbers are only comparable within one stream, so no gap is reported
    // even though the raw numbers alone would suggest one.
    let other_feed = build(origin("itch-b", 0, 45), t(500), t(500))?;
    assert_eq!(
        other_feed.sequence_gap(&earlier),
        None,
        "a sequence gap was computed across two different streams"
    );
    Ok(())
}
