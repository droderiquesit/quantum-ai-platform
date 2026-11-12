//! R12-TICK-2053: every market-event kind normalises into its typed canonical
//! record, and none lands in a generic bucket.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId, VenueStatus};
use qip_core::{Context, Lineage, ObjectId, Timestamp, dec};
use qip_financial::quality::DataQuality;
use qip_market_ingestion::adapter::SensedRecord;
use qip_market_ingestion::session::{Canonical, normalise};

fn at() -> Timestamp {
    Timestamp::from_secs(1_787_583_600)
}

fn message(body: MessageBody) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string("XNAS:NWSC"),
        Origin::new(VenueId::new("XNAS"), "itch-a", 0, 41),
        body,
        at(),
        at().saturating_add(qip_core::Duration::from_millis(3)),
    )
}

/// One message of every kind the wire contract names. The match below has
/// no wildcard, so a kind added to `MessageBody` is a compile error here
/// until a case for it is written.
fn every_kind() -> Vec<MessageBody> {
    let kinds = vec![
        MessageBody::OrderAdded {
            order_ref: 1,
            side: BookSide::Bid,
            price: dec!("10.00"),
            quantity: dec!("100"),
        },
        MessageBody::OrderReduced {
            order_ref: 1,
            remaining: dec!("40"),
        },
        MessageBody::OrderRemoved { order_ref: 1 },
        MessageBody::OrderReplaced {
            order_ref: 1,
            price: dec!("10.01"),
            quantity: dec!("50"),
        },
        MessageBody::LevelSet {
            side: BookSide::Ask,
            price: dec!("10.02"),
            quantity: dec!("300"),
            order_count: Some(3),
        },
        MessageBody::Quote {
            bid: Some((dec!("10.00"), dec!("200"))),
            ask: Some((dec!("10.02"), dec!("300"))),
        },
        MessageBody::Trade {
            price: dec!("10.01"),
            quantity: dec!("25"),
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Ask),
        },
        MessageBody::StatusChange {
            status: VenueStatus::Halted,
        },
        MessageBody::AuctionUpdate {
            indicative_price: Some(dec!("10.015")),
            paired: dec!("900"),
            imbalance: dec!("150"),
            imbalance_side: Some(BookSide::Bid),
        },
        MessageBody::Reset {
            reason: "gap at 40 abandoned".into(),
        },
    ];
    for body in &kinds {
        match body {
            MessageBody::OrderAdded { .. }
            | MessageBody::OrderReduced { .. }
            | MessageBody::OrderRemoved { .. }
            | MessageBody::OrderReplaced { .. }
            | MessageBody::LevelSet { .. }
            | MessageBody::Quote { .. }
            | MessageBody::Trade { .. }
            | MessageBody::StatusChange { .. }
            | MessageBody::AuctionUpdate { .. }
            | MessageBody::Reset { .. } => {}
        }
    }
    kinds
}

/// The typed variant each kind must become, by name.
fn expected(body: &MessageBody) -> &'static str {
    match body {
        MessageBody::OrderAdded { .. }
        | MessageBody::OrderReduced { .. }
        | MessageBody::OrderRemoved { .. }
        | MessageBody::OrderReplaced { .. }
        | MessageBody::LevelSet { .. } => "depth",
        MessageBody::Quote { .. } => "quote",
        MessageBody::Trade { .. } => "trade",
        MessageBody::StatusChange { .. } => "status",
        MessageBody::AuctionUpdate { .. } => "auction",
        MessageBody::Reset { .. } => "reset",
    }
}

fn landed(canonical: &Canonical) -> &'static str {
    match canonical {
        Canonical::DepthUpdate => "depth",
        Canonical::Record(record) => match record.as_ref() {
            SensedRecord::Quote(_) => "quote",
            SensedRecord::Trade(_) => "trade",
            SensedRecord::Status(_) => "status",
            SensedRecord::Auction(_) => "auction",
            SensedRecord::Reset(_) => "reset",
            _ => "some other record",
        },
    }
}

/// The requirement's bar, as written: each event kind decodes to its typed
/// variant, and the test fails if any kind lands in a generic bucket.
///
/// Mutation: in `src/session.rs` route `MessageBody::AuctionUpdate` to
/// `Canonical::DepthUpdate` — "auction_update landed in depth" fails.
/// Mutation: map `MessageBody::StatusChange` to a `SensedRecord::Reset` —
/// "status_change landed in reset" fails.
#[test]
fn every_market_event_kind_normalises_to_its_own_typed_record() {
    let kinds = every_kind();
    let distinct: std::collections::BTreeSet<&str> = kinds.iter().map(MessageBody::kind).collect();
    assert_eq!(
        distinct.len(),
        10,
        "premise: one message of each of ten kinds"
    );

    let quality = DataQuality::default();
    for body in kinds {
        let message = message(body.clone());
        let canonical = normalise(&message, &ObjectId::from_string("NWSC"), quality.clone())
            .unwrap_or_else(|error| panic!("{} did not normalise: {error}", body.kind()));
        assert_eq!(
            landed(&canonical),
            expected(&body),
            "{} landed in {}",
            body.kind(),
            landed(&canonical)
        );
        // Both instants travel, so the record can be held to the admission
        // rule at the centre (`Platform::absorb_records`).
        if let Canonical::Record(record) = &canonical {
            use qip_contracts::admission::Admissible;
            let own = record.own_times();
            assert_eq!(own.event_time, Some(message.venue_time), "{}", body.kind());
            assert_eq!(
                own.receive_time,
                Some(message.capture_time),
                "{}",
                body.kind()
            );
            assert!(record.validate().is_empty(), "{:?}", record.validate());
        }
    }
}

/// A one-sided quote is not a top of book. Zero for the missing side would
/// publish a spread nobody quoted.
///
/// Mutation: fill a missing side with `Decimal::ZERO` in `normalise` — the
/// refusal assertion fails.
#[test]
fn a_one_sided_quote_is_refused_rather_than_given_a_zero_side() {
    let message = message(MessageBody::Quote {
        bid: Some((dec!("10.00"), dec!("200"))),
        ask: None,
    });
    let error = normalise(
        &message,
        &ObjectId::from_string("NWSC"),
        DataQuality::default(),
    )
    .expect_err("a one-sided quote is refused");
    assert!(error.message().contains("one-sided quote"), "{error}");
}

/// The three session records share `market.session` and come back as the
/// variant they went out as: the body is tagged, so a subscriber decodes by
/// what the record says it is rather than by which shape happens to fit.
///
/// The decode cannot be mutated into a status-only one: `VenueStatusUpdate`
/// is not an `EventBody`, so `SessionRecord` is the only body the topic has.
/// Mutation: drop `Topic::MarketSession` from `SensedRecord::TOPICS` (and
/// its length to 11) — "the bridge subscribes to market.session" fails.
#[test]
fn each_session_record_round_trips_through_its_event_as_its_own_variant() {
    let (context, _clock) = Context::deterministic(at(), 1);
    let lineage = Lineage::root(context.ids().generate(at()), "test");
    let object_id = ObjectId::from_string("NWSC");
    let mut seen = Vec::new();
    for body in [
        MessageBody::StatusChange {
            status: VenueStatus::CircuitBreaker,
        },
        MessageBody::AuctionUpdate {
            indicative_price: None,
            paired: dec!("900"),
            imbalance: dec!("0"),
            imbalance_side: None,
        },
        MessageBody::Reset {
            reason: "resubscribe".into(),
        },
    ] {
        let Canonical::Record(record) =
            normalise(&message(body), &object_id, DataQuality::default()).expect("normalises")
        else {
            panic!("a session fact is a record of its own");
        };
        let event = record
            .to_event(context.ids().generate(at()), at(), lineage.clone())
            .expect("erases");
        assert_eq!(event.topic, qip_events::Topic::MarketSession);
        assert!(
            SensedRecord::TOPICS.contains(&event.topic),
            "the bridge subscribes to market.session"
        );
        let back = SensedRecord::from_event(&event).expect("decodes");
        assert_eq!(&back, record.as_ref());
        seen.push(back.topic());
    }
    assert_eq!(seen.len(), 3);
}
