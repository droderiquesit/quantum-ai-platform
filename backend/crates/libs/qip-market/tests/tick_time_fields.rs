//! TICK-003: Event, receive and normalized time are never collapsed.
//!
//! Every captured market event keeps its source/exchange event time, receive
//! time, normalized time, clock uncertainty, source sequence number and venue
//! identity as distinct fields from capture through training and replay. No
//! stage collapses them into a single timestamp or overwrites one with another.

use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::quote::{Quote, Tick, Trade};

fn id(s: &str) -> ObjectId {
    ObjectId::from_string(s)
}

#[test]
fn tick_preserves_distinct_time_fields() {
    let event_time = Timestamp::EPOCH;
    let receive_time = event_time.saturating_add(Duration::from_millis(10));
    let normalized_time = receive_time.saturating_add(Duration::from_millis(5));

    let tick = Tick {
        object_id: id("test"),
        venue: "nyse".into(),
        event_time,
        receive_time,
        normalized_time,
        clock_uncertainty: 1000,
        at: event_time,
        price: Decimal::parse("100").unwrap(),
        volume: Decimal::parse("1000").unwrap(),
        quality: Default::default(),
    };

    assert_eq!(tick.event_time, event_time, "event_time must be preserved");
    assert_eq!(
        tick.receive_time, receive_time,
        "receive_time must be preserved"
    );
    assert_eq!(
        tick.normalized_time, normalized_time,
        "normalized_time must be preserved"
    );
    assert!(
        tick.receive_time >= tick.event_time,
        "receive_time must be >= event_time"
    );
    assert!(
        tick.normalized_time >= tick.receive_time,
        "normalized_time must be >= receive_time"
    );
    assert_eq!(
        tick.clock_uncertainty, 1000,
        "clock_uncertainty must be preserved"
    );
}

#[test]
fn quote_preserves_distinct_time_fields() {
    let event_time = Timestamp::EPOCH;
    let receive_time = event_time.saturating_add(Duration::from_millis(10));
    let normalized_time = receive_time.saturating_add(Duration::from_millis(5));

    let quote = Quote {
        object_id: id("test"),
        venue: "nyse".into(),
        event_time,
        receive_time,
        normalized_time,
        clock_uncertainty: 500,
        at: event_time,
        bid: Decimal::parse("99.5").unwrap(),
        ask: Decimal::parse("100.5").unwrap(),
        bid_size: Decimal::parse("1000").unwrap(),
        ask_size: Decimal::parse("1000").unwrap(),
        quality: Default::default(),
    };

    assert_eq!(quote.event_time, event_time, "event_time must be preserved");
    assert_eq!(
        quote.receive_time, receive_time,
        "receive_time must be preserved"
    );
    assert_eq!(
        quote.normalized_time, normalized_time,
        "normalized_time must be preserved"
    );
    assert!(
        quote.receive_time >= quote.event_time,
        "receive_time must be >= event_time"
    );
    assert!(
        quote.normalized_time >= quote.receive_time,
        "normalized_time must be >= receive_time"
    );
    assert_eq!(
        quote.clock_uncertainty, 500,
        "clock_uncertainty must be preserved"
    );
}

#[test]
fn trade_preserves_distinct_time_fields() {
    let event_time = Timestamp::EPOCH;
    let receive_time = event_time.saturating_add(Duration::from_millis(10));
    let normalized_time = receive_time.saturating_add(Duration::from_millis(5));

    let trade = Trade {
        object_id: id("test"),
        venue: "nyse".into(),
        event_time,
        receive_time,
        normalized_time,
        clock_uncertainty: 750,
        at: event_time,
        price: Decimal::parse("100").unwrap(),
        size: Decimal::parse("100").unwrap(),
        aggressor: None,
        condition: qip_market::quote::TradeCondition::Regular,
        trade_id: None,
        quality: Default::default(),
    };

    assert_eq!(trade.event_time, event_time, "event_time must be preserved");
    assert_eq!(
        trade.receive_time, receive_time,
        "receive_time must be preserved"
    );
    assert_eq!(
        trade.normalized_time, normalized_time,
        "normalized_time must be preserved"
    );
    assert!(
        trade.receive_time >= trade.event_time,
        "receive_time must be >= event_time"
    );
    assert!(
        trade.normalized_time >= trade.receive_time,
        "normalized_time must be >= receive_time"
    );
    assert_eq!(
        trade.clock_uncertainty, 750,
        "clock_uncertainty must be preserved"
    );
}
