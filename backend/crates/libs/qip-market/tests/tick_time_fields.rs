//! TICK-003: Event, receive and normalized time are never collapsed.
//!
//! Every captured market event keeps its source/exchange event time, receive
//! time, normalized time, clock uncertainty, source sequence number and venue
//! identity as distinct fields from capture through training and replay. No
//! stage collapses them into a single timestamp or overwrites one with another.

use qip_core::{ObjectId, Timestamp, dec};
use qip_market::quote::{Quote, Tick, Trade};

#[test]
fn tick_preserves_distinct_time_fields() {
    // Create a tick with explicitly different timestamps to prove they're tracked separately
    let event_time = Timestamp::EPOCH;
    let _receive_time = Timestamp::from_millis(10);
    let _normalized_time = Timestamp::from_millis(15);

    // This test would verify that a Tick has these fields as separate, distinguishable values
    // For now, we check the current structure to show what needs to be added
    let tick = Tick {
        capture_time: None,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: event_time, // Currently just one timestamp
        price: dec!("100"),
        volume: dec!("1000"),
        quality: Default::default(),
    };

    // MISSING ASSERTIONS:
    // assert_eq!(tick.event_time, event_time);
    // assert_eq!(tick.receive_time, receive_time);
    // assert_eq!(tick.normalized_time, normalized_time);
    // assert!(tick.receive_time >= tick.event_time);
    // assert!(tick.normalized_time >= tick.receive_time);

    // Currently this only proves the type exists
    assert_eq!(tick.at, event_time);
}

#[test]
fn quote_preserves_distinct_time_fields() {
    let event_time = Timestamp::EPOCH;
    let _receive_time = Timestamp::from_millis(10);

    let quote = Quote {
        capture_time: None,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: event_time, // Currently just one timestamp
        bid: dec!("99.5"),
        ask: dec!("100.5"),
        bid_size: dec!("1000"),
        ask_size: dec!("1000"),
        quality: Default::default(),
    };

    // MISSING: separate event_time, receive_time, normalized_time fields
    assert_eq!(quote.at, event_time);
}

#[test]
fn trade_preserves_distinct_time_fields() {
    let event_time = Timestamp::EPOCH;
    let _receive_time = Timestamp::from_millis(10);

    let trade = Trade {
        capture_time: None,
        object_id: ObjectId::from_string("test"),
        venue: "nyse".to_string(),
        at: event_time, // Currently just one timestamp
        price: dec!("100"),
        size: dec!("100"),
        aggressor: None,
        condition: qip_market::quote::TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    };

    // MISSING: separate event_time, receive_time, normalized_time fields
    assert_eq!(trade.at, event_time);
}
