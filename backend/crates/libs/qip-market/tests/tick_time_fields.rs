//! TICK-003 / R12-TICK-2049, 2051: event time, capture time and the feed's
//! sequence number are never collapsed on the central tick, quote and trade.
//!
//! This file used to construct each type with `capture_time: None`, carry a
//! block of commented-out "MISSING ASSERTIONS", and assert only `tick.at` —
//! a test named for the property that would have passed with the property
//! deleted. Each test now asserts that the three facts survive a round trip
//! as three distinct values, and that a record received before it happened
//! is refused.

use qip_core::{ObjectId, Timestamp, dec};
use qip_market::quote::{Quote, Tick, Trade, TradeCondition, check_capture_order};

const EVENT: i64 = 1_000;
const CAPTURE: i64 = 1_010;

fn tick() -> Tick {
    Tick {
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: Timestamp::from_millis(EVENT),
        capture_time: Timestamp::from_millis(CAPTURE),
        sequence: Some(42),
        price: dec!("100"),
        volume: dec!("1000"),
        quality: Default::default(),
    }
}

#[test]
fn a_tick_keeps_event_time_capture_time_and_sequence_apart_through_a_round_trip() {
    let original = tick();
    // Premise: the three are distinct to begin with, or equality proves nothing.
    assert_ne!(original.at, original.capture_time);

    let back: Tick = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(back.at, Timestamp::from_millis(EVENT));
    assert_eq!(back.capture_time, Timestamp::from_millis(CAPTURE));
    assert_eq!(back.sequence, Some(42));
    assert!(back.validate().is_empty());
}

#[test]
fn a_quote_keeps_event_time_capture_time_and_sequence_apart_through_a_round_trip() {
    let original = Quote {
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: Timestamp::from_millis(EVENT),
        capture_time: Timestamp::from_millis(CAPTURE),
        sequence: Some(7),
        bid: dec!("99.5"),
        ask: dec!("100.5"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: Default::default(),
    };
    let back: Quote = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(back.at, Timestamp::from_millis(EVENT));
    assert_eq!(back.capture_time, Timestamp::from_millis(CAPTURE));
    assert_eq!(back.sequence, Some(7));
    assert!(back.validate().is_empty(), "{:?}", back.validate());
}

#[test]
fn a_trade_keeps_event_time_capture_time_and_sequence_apart_through_a_round_trip() {
    let original = Trade {
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: Timestamp::from_millis(EVENT),
        capture_time: Timestamp::from_millis(CAPTURE),
        sequence: Some(9),
        price: dec!("100"),
        size: dec!("100"),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    };
    let back: Trade = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(back.at, Timestamp::from_millis(EVENT));
    assert_eq!(back.capture_time, Timestamp::from_millis(CAPTURE));
    assert_eq!(back.sequence, Some(9));
}

#[test]
fn a_tick_captured_before_it_occurred_is_refused_and_one_captured_after_is_not() {
    let mut early = tick();
    early.capture_time = Timestamp::from_millis(EVENT - 1);
    assert_eq!(early.validate().len(), 1, "{:?}", early.validate());

    // Boundary: captured at the event instant is possible (a zero-latency
    // simulator), and is admitted.
    assert!(
        check_capture_order(Timestamp::from_millis(EVENT), Timestamp::from_millis(EVENT)).is_none()
    );
}
