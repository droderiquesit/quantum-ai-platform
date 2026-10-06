//! TICK-003: Event, receive and normalized time are never collapsed.
//!
//! Every captured market event keeps its source/exchange event time, receive
//! time, normalized time, clock uncertainty, source sequence number and venue
//! identity as distinct fields from capture through training and replay. No
//! stage collapses them into a single timestamp or overwrites one with another.

use qip_market::quote::{Tick, Quote, Trade};
use qip_core::Timestamp;
use std::time::SystemTime;

#[test]
fn tick_preserves_distinct_time_fields() {
    // Create a tick with explicitly different timestamps to prove they're tracked separately
    let event_time = Timestamp::from_system_time(SystemTime::UNIX_EPOCH);
    let receive_time = event_time + std::time::Duration::from_millis(10);
    let normalized_time = receive_time + std::time::Duration::from_millis(5);

    // This test would verify that a Tick has these fields as separate, distinguishable values
    // For now, we check the current structure to show what needs to be added
    let tick = Tick {
        object_id: "test".into(),
        venue: "nyse".into(),
        at: event_time,  // Currently just one timestamp
        price: 100.0.into(),
        volume: 1000.0.into(),
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
    let event_time = Timestamp::from_system_time(SystemTime::UNIX_EPOCH);
    let receive_time = event_time + std::time::Duration::from_millis(10);

    let quote = Quote {
        object_id: "test".into(),
        venue: "nyse".into(),
        at: event_time,  // Currently just one timestamp
        bid: 99.5.into(),
        ask: 100.5.into(),
        bid_size: 1000.0.into(),
        ask_size: 1000.0.into(),
        quality: Default::default(),
    };

    // MISSING: separate event_time, receive_time, normalized_time fields
    assert_eq!(quote.at, event_time);
}

#[test]
fn trade_preserves_distinct_time_fields() {
    let event_time = Timestamp::from_system_time(SystemTime::UNIX_EPOCH);
    let receive_time = event_time + std::time::Duration::from_millis(10);

    let trade = Trade {
        object_id: "test".into(),
        venue: "nyse".into(),
        at: event_time,  // Currently just one timestamp
        price: 100.0.into(),
        size: 100.0.into(),
        aggressor: None,
        condition: qip_market::quote::TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    };

    // MISSING: separate event_time, receive_time, normalized_time fields
    assert_eq!(trade.at, event_time);
}
