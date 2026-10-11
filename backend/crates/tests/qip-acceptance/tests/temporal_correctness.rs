//! Temporal correctness of bitemporal feature tracking.
//!
//! The platform tracks two times for each event:
//! - instant_true (at): when the event occurred in the market
//! - knowable_at (capture_time): when the platform learned about it
//!
//! This distinction prevents point-in-time leakage: a feature readable before
//! its knowable instant corrupts backtests, training and live trading.
//! See `.claude/rules/domains/data-and-streaming.md`.
//!
//! Until 2026-10-11 `capture_time` was optional, and two tests here asserted
//! that a record without one serialised without it and that the kernel then
//! read it as knowable at its event instant. That fallback *was* the leak —
//! a record that never said when it arrived was filed as known the moment it
//! happened — so those tests are replaced by their opposites: a record that
//! cannot say when it arrived does not decode, and one that says it arrived
//! before it happened is refused by validation.

#![allow(clippy::panic_in_result_fn)]

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::{Tick, Trade, TradeCondition};

fn trade(at: Timestamp, capture_time: Timestamp, id: &str) -> Trade {
    Trade {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at,
        capture_time,
        sequence: Some(7),
        price: Decimal::from_int(100),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some(id.to_string()),
        quality: DataQuality::default(),
    }
}

fn tick(at: Timestamp, capture_time: Timestamp) -> Tick {
    Tick {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at,
        capture_time,
        sequence: Some(11),
        price: Decimal::from_int(105),
        volume: Decimal::from_int(5),
        quality: DataQuality::default(),
    }
}

/// Trade serialization preserves capture_time and the feed sequence.
#[test]
fn trade_serialization_preserves_capture_time_and_sequence() {
    let event_time = Timestamp::from_millis(1000);
    let capture_time = Timestamp::from_millis(1050);
    let original = trade(event_time, capture_time, "1");

    let json = serde_json::to_string(&original).expect("serializes");
    let deserialized: Trade = serde_json::from_str(&json).expect("deserializes");

    assert_eq!(deserialized.at, event_time);
    assert_eq!(deserialized.capture_time, capture_time);
    assert_eq!(deserialized.sequence, Some(7));
}

/// A trade on the wire with no capture instant does not decode: the
/// alternative is reading it as knowable at its event instant.
#[test]
fn a_trade_on_the_wire_without_a_capture_time_does_not_decode() {
    let original = trade(
        Timestamp::from_millis(1000),
        Timestamp::from_millis(1050),
        "2",
    );
    let mut value = serde_json::to_value(&original).expect("serializes");
    // Premise: the field is present before it is removed.
    let object = value
        .as_object_mut()
        .expect("a trade serialises as an object");
    assert!(object.remove("capture_time").is_some());

    let refused = serde_json::from_value::<Trade>(value);
    assert!(
        refused.is_err(),
        "a trade with no capture instant decoded, and would be filed as knowable when it printed"
    );
}

/// Tick serialization preserves capture_time.
#[test]
fn tick_serialization_preserves_capture_time() {
    let event_time = Timestamp::from_millis(2000);
    let capture_time = Timestamp::from_millis(2075);
    let original = tick(event_time, capture_time);

    let json = serde_json::to_string(&original).expect("serializes");
    let deserialized: Tick = serde_json::from_str(&json).expect("deserializes");

    assert_eq!(deserialized.at, event_time);
    assert_eq!(deserialized.capture_time, capture_time);
}

/// A tick on the wire with no capture instant does not decode.
#[test]
fn a_tick_on_the_wire_without_a_capture_time_does_not_decode() {
    let original = tick(Timestamp::from_millis(3000), Timestamp::from_millis(3010));
    let mut value = serde_json::to_value(&original).expect("serializes");
    let object = value
        .as_object_mut()
        .expect("a tick serialises as an object");
    assert!(object.remove("capture_time").is_some());

    assert!(serde_json::from_value::<Tick>(value).is_err());
}

/// Capture_time is distinct from event time (at).
#[test]
fn trade_with_capture_time_has_distinct_timestamps() {
    let event_time = Timestamp::from_millis(1000);
    let capture_time = Timestamp::from_millis(1100); // 100ms later
    let trade = trade(event_time, capture_time, "3");

    assert_eq!(trade.at, event_time);
    assert_eq!(trade.capture_time, capture_time);
    assert_ne!(
        trade.at, capture_time,
        "The platform needs distinct event time and capture time to prevent point-in-time leakage"
    );
    assert!(trade.validate().is_empty(), "{:?}", trade.validate());
}

/// A record captured before it happened is refused by validation, which is
/// the gate `SensedRecord::validate` puts in front of the world model.
#[test]
fn a_trade_captured_before_it_printed_is_refused_by_validation() {
    let sound = trade(
        Timestamp::from_millis(1000),
        Timestamp::from_millis(1000),
        "4",
    );
    assert!(
        sound.validate().is_empty(),
        "premise: an on-time capture is sound"
    );

    let early = trade(
        Timestamp::from_millis(1000),
        Timestamp::from_millis(999),
        "5",
    );
    let issues = early.validate();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(issues[0].contains("before it occurred"), "{issues:?}");
}
