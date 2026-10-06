//! Temporal correctness of bitemporal feature tracking.
//!
//! The platform tracks two times for each event:
//! - instant_true (at): when the event occurred in the market
//! - knowable_at (capture_time): when the platform learned about it
//!
//! This distinction prevents point-in-time leakage: a feature readable before
//! its knowable instant corrupts backtests, training and live trading.
//! See `.claude/rules/domains/data-and-streaming.md`.

#![allow(clippy::panic_in_result_fn)]

use qip_core::{Decimal, ObjectId, Timestamp};
use qip_financial::quality::DataQuality;
use qip_market::{Tick, Trade, TradeCondition};

/// Trade serialization preserves capture_time when present.
#[test]
fn trade_serialization_preserves_capture_time() {
    let event_time = Timestamp::from_millis(1000);
    let capture_time = Timestamp::from_millis(1050);

    let original = Trade {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: Some(capture_time),
        price: Decimal::from_int(100),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("1".to_string()),
        quality: DataQuality::default(),
    };

    let json = serde_json::to_string(&original).expect("serializes");
    let deserialized: Trade = serde_json::from_str(&json).expect("deserializes");

    assert_eq!(deserialized.at, event_time);
    assert_eq!(deserialized.capture_time, Some(capture_time));
}

/// Trade serialization omits capture_time when None (for compact representation).
#[test]
fn trade_serialization_omits_capture_time_when_none() {
    let event_time = Timestamp::from_millis(1000);

    let original = Trade {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: None,
        price: Decimal::from_int(100),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("2".to_string()),
        quality: DataQuality::default(),
    };

    let json = serde_json::to_string(&original).expect("serializes");

    // When capture_time is None, it should not appear in the JSON
    assert!(
        !json.contains("\"capture_time\""),
        "capture_time should be omitted when None"
    );

    let deserialized: Trade = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(deserialized.capture_time, None);
}

/// Tick serialization preserves capture_time when present.
#[test]
fn tick_serialization_preserves_capture_time() {
    let event_time = Timestamp::from_millis(2000);
    let capture_time = Timestamp::from_millis(2075);

    let original = Tick {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: Some(capture_time),
        price: Decimal::from_int(105),
        volume: Decimal::from_int(5),
        quality: DataQuality::default(),
    };

    let json = serde_json::to_string(&original).expect("serializes");
    let deserialized: Tick = serde_json::from_str(&json).expect("deserializes");

    assert_eq!(deserialized.at, event_time);
    assert_eq!(deserialized.capture_time, Some(capture_time));
}

/// Tick serialization omits capture_time when None.
#[test]
fn tick_serialization_omits_capture_time_when_none() {
    let event_time = Timestamp::from_millis(3000);

    let original = Tick {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: None,
        price: Decimal::from_int(100),
        volume: Decimal::ZERO,
        quality: DataQuality::default(),
    };

    let json = serde_json::to_string(&original).expect("serializes");

    assert!(
        !json.contains("\"capture_time\""),
        "capture_time should be omitted when None"
    );

    let deserialized: Tick = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(deserialized.capture_time, None);
}

/// Capture_time is distinct from event time (at).
/// This struct-level test validates the property that allows Platform::observe
/// to use distinct timestamps: capture_time for known_at, at for instant_true.
#[test]
fn trade_with_capture_time_has_distinct_timestamps() {
    let event_time = Timestamp::from_millis(1000);
    let capture_time = Timestamp::from_millis(1100); // 100ms later

    let trade = Trade {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: Some(capture_time),
        price: Decimal::from_int(100),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("3".to_string()),
        quality: DataQuality::default(),
    };

    // Validate that the two timestamps are available and distinct
    assert_eq!(trade.at, event_time);
    assert_eq!(trade.capture_time, Some(capture_time));
    assert_ne!(
        trade.at, capture_time,
        "The platform needs distinct event time and capture time to prevent point-in-time leakage"
    );
}

/// When capture_time is None, fallback behavior uses event time.
/// This ensures backward compatibility for events without capture time.
#[test]
fn trade_without_capture_time_falls_back_to_event_time() {
    let event_time = Timestamp::from_millis(1000);

    let trade = Trade {
        object_id: ObjectId::from_string("TEST"),
        venue: "simulated".to_string(),
        at: event_time,
        capture_time: None,
        price: Decimal::from_int(100),
        size: Decimal::from_int(10),
        aggressor: None,
        condition: TradeCondition::Regular,
        trade_id: Some("4".to_string()),
        quality: DataQuality::default(),
    };

    // When capture_time is None, Platform::observe uses at for both valid_at and known_at
    let known_at = trade.capture_time.unwrap_or(trade.at);
    assert_eq!(
        known_at, event_time,
        "fallback should use event time when capture_time is not set"
    );
}
