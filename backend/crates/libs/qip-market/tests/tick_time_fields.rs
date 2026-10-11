//! TICK-003: event time and receive time are never collapsed.
//!
//! The requirement names six fields — event time, receive time, normalized
//! time, clock uncertainty, source sequence and venue. The market types carry
//! **two** of the times today: `at` (when it was true in the market) and
//! `capture_time` (when it became knowable to the platform), plus `venue`.
//! Normalized time, clock uncertainty and a source sequence number do not
//! exist on `Tick`, `Quote` or `Trade`; that is a production gap and these
//! tests do not pretend otherwise.
//!
//! What they pin is the part that exists. An earlier version of this file
//! built a `Tick` with `capture_time: None`, asserted `tick.at == tick.at`'s
//! input, and carried the real assertions as comments under "MISSING
//! ASSERTIONS" — it passed with `capture_time` deleted from the type, and
//! with a serde attribute that dropped it on the wire.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{ObjectId, Timestamp, dec};
use qip_events::EventBody;
use qip_market::quote::{Quote, Tick, Trade, TradeCondition};

const EVENT_MS: i64 = 1_000;
const RECEIVE_MS: i64 = 1_010;

fn tick(at: Timestamp, capture: Option<Timestamp>) -> Tick {
    Tick {
        capture_time: capture,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at,
        price: dec!("100"),
        volume: dec!("1000"),
        quality: Default::default(),
    }
}

fn quote(at: Timestamp, capture: Option<Timestamp>) -> Quote {
    Quote {
        capture_time: capture,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at,
        bid: dec!("99.5"),
        ask: dec!("100.5"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: Default::default(),
    }
}

fn trade(at: Timestamp, capture: Option<Timestamp>) -> Trade {
    Trade {
        capture_time: capture,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at,
        price: dec!("100"),
        size: dec!("100"),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("t-1".to_string()),
        quality: Default::default(),
    }
}

/// Decode `value` as `T`, asserting it decodes.
fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &T,
) -> (serde_json::Value, T) {
    let json = serde_json::to_value(value).expect("encodes");
    let back: T = serde_json::from_value(json.clone()).expect("decodes");
    (json, back)
}

/// Read a millisecond timestamp field back out of the encoded form, so the
/// assertion is about the wire and not about the struct we built.
fn wire_time(json: &serde_json::Value, key: &str) -> serde_json::Value {
    json.get(key)
        .cloned()
        .unwrap_or_else(|| panic!("the encoded record has no `{key}` field: {json}"))
}

#[test]
fn a_tick_keeps_event_time_and_capture_time_as_two_distinct_values_across_the_wire() {
    let event = Timestamp::from_millis(EVENT_MS);
    let receive = Timestamp::from_millis(RECEIVE_MS);
    // Premise: the two instants differ, or "not collapsed" is unobservable.
    assert_ne!(event, receive);

    let original = tick(event, Some(receive));
    let (json, back) = round_trip(&original);

    // Both fields are on the wire, under their own keys, with their own values.
    assert_ne!(wire_time(&json, "at"), wire_time(&json, "capture_time"));
    assert_eq!(back.at, event, "event time survived as event time");
    assert_eq!(
        back.capture_time,
        Some(receive),
        "receive time survived as receive time, not dropped and not replaced by event time"
    );
    assert_eq!(back, original);
}

#[test]
fn a_quote_keeps_event_time_and_capture_time_as_two_distinct_values_across_the_wire() {
    let event = Timestamp::from_millis(EVENT_MS);
    let receive = Timestamp::from_millis(RECEIVE_MS);
    assert_ne!(event, receive);

    let original = quote(event, Some(receive));
    let (json, back) = round_trip(&original);

    assert_ne!(wire_time(&json, "at"), wire_time(&json, "capture_time"));
    assert_eq!(back.at, event);
    assert_eq!(back.capture_time, Some(receive));
}

#[test]
fn a_trade_keeps_event_time_and_capture_time_as_two_distinct_values_across_the_wire() {
    let event = Timestamp::from_millis(EVENT_MS);
    let receive = Timestamp::from_millis(RECEIVE_MS);
    assert_ne!(event, receive);

    let original = trade(event, Some(receive));
    let (json, back) = round_trip(&original);

    assert_ne!(wire_time(&json, "at"), wire_time(&json, "capture_time"));
    assert_eq!(back.at, event);
    assert_eq!(back.capture_time, Some(receive));
}

/// A record captured before `capture_time` existed must decode as "receive
/// time unknown". Borrowing the event time would silently claim the platform
/// knew the price at the instant it printed — point-in-time leakage.
#[test]
fn a_tick_with_no_capture_time_decodes_as_unknown_rather_than_borrowing_its_event_time() {
    let event = Timestamp::from_millis(EVENT_MS);
    let (mut json, _) = round_trip(&tick(event, Some(Timestamp::from_millis(RECEIVE_MS))));
    // Premise: the field was there before we remove it.
    assert!(
        json.as_object_mut()
            .expect("object")
            .remove("capture_time")
            .is_some()
    );

    let back: Tick = serde_json::from_value(json).expect("an older record still decodes");
    assert_eq!(back.at, event);
    assert_eq!(back.capture_time, None);
}

/// A venue that resends the same print on reconnect is received twice, at two
/// receive times. Dedup must key on what happened, not when we heard it —
/// keying on receive time would double-count every resend.
#[test]
fn two_receipts_of_one_tick_share_an_idempotency_key_and_two_events_do_not() {
    let event = Timestamp::from_millis(EVENT_MS);
    let first = tick(event, Some(Timestamp::from_millis(RECEIVE_MS)));
    let resend = tick(event, Some(Timestamp::from_millis(RECEIVE_MS + 500)));
    let later = tick(
        Timestamp::from_millis(EVENT_MS + 1),
        Some(Timestamp::from_millis(RECEIVE_MS)),
    );

    let key = first.idempotency_key().expect("a tick always has a key");
    assert_eq!(resend.idempotency_key(), Some(key.clone()));
    assert_ne!(later.idempotency_key(), Some(key));
}

#[test]
fn two_receipts_of_one_quote_share_an_idempotency_key_and_two_events_do_not() {
    let event = Timestamp::from_millis(EVENT_MS);
    let first = quote(event, Some(Timestamp::from_millis(RECEIVE_MS)));
    let resend = quote(event, Some(Timestamp::from_millis(RECEIVE_MS + 500)));
    let later = quote(
        Timestamp::from_millis(EVENT_MS + 1),
        Some(Timestamp::from_millis(RECEIVE_MS)),
    );

    let key = first.idempotency_key().expect("a quote always has a key");
    assert_eq!(resend.idempotency_key(), Some(key.clone()));
    assert_ne!(later.idempotency_key(), Some(key));
}
