//! The capture path: every stored market event carries the corrections the
//! clock discipline applied, and is aligned against its stream's previous
//! event (R12-TICK-2049, 2051, 2089).

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody, TradeCondition};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Duration, ObjectId, Timestamp, dec};
use qip_sequencing::{CaptureStream, DisciplineSettings};
use qip_storage::lake::Lake;
use qip_storage::tick_capture::{RetentionRight, TickLakeWriter};
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::BTreeMap;
use std::sync::Arc;

fn t(millis: i64) -> Timestamp {
    Timestamp::from_millis(1_790_000_000_000 + millis)
}

/// A trade the venue stamped at `venue_ms` and this cell received 2 ms later.
fn trade(sequence: u64, venue_ms: i64) -> Result<MarketEvent> {
    let payload = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XNYS"), "itch-a", 0, sequence),
        MessageBody::Trade {
            price: dec!("100.25"),
            quantity: dec!("100"),
            condition: TradeCondition::Regular,
            aggressor: Some(BookSide::Bid),
        },
        t(venue_ms),
        t(venue_ms + 2),
    );
    let hash = MarketEvent::hash_payload(&payload)?;
    MarketEvent::new(
        payload,
        t(venue_ms),
        t(venue_ms + 2),
        t(venue_ms + 2),
        Duration::from_millis(5),
        hash,
        Entitlement::Granted {
            dataset: "xnys-itch".to_string(),
            usage: Usage::Trade,
            expires_at: t(10_000_000),
        },
    )
}

fn stream() -> Result<CaptureStream> {
    CaptureStream::new(DisciplineSettings {
        window: 32,
        min_samples: 4,
        max_uncertainty: Duration::from_millis(5),
    })
}

#[test]
fn an_event_is_stored_with_the_offset_correction_once_the_estimate_is_trustworthy_and_without_one_before()
-> Result<()> {
    let mut capture = stream()?;
    let mut captured = Vec::new();
    for i in 0..6_u64 {
        captured.push(capture.capture(&trade(i + 1, i as i64 * 100)?, None)?);
    }
    assert_eq!(captured.len(), 6, "premise: six events captured");

    // Before four samples there is no trustworthy estimate: no correction,
    // the venue's time unchanged, and the event's own stated uncertainty.
    let early = &captured[0];
    assert!(early.corrections.is_empty(), "{:?}", early.corrections);
    assert_eq!(early.normalized_time, early.event_time);
    assert_eq!(early.uncertainty, Duration::from_millis(5));
    assert_eq!(
        early.arrival_order, None,
        "the first event has nothing to follow"
    );

    // From the fourth on, the 2 ms offset is applied and recorded.
    let late = &captured[5];
    assert_eq!(late.corrections.len(), 1, "{:?}", late.corrections);
    assert_eq!(late.corrections[0].kind, "offset_estimate");
    assert_eq!(
        late.corrections[0].delta_nanos,
        Duration::from_millis(2).as_nanos()
    );
    assert_eq!(
        late.normalized_time,
        late.event_time.saturating_add(Duration::from_millis(2))
    );
    assert_eq!(late.arrival_order.as_deref(), Some("before"));
    assert_eq!(late.sequence, Some(6));
    assert!(late.validate().is_ok());
    Ok(())
}

#[test]
fn an_event_stamped_before_its_predecessor_is_floored_recorded_and_aligned_as_unordered()
-> Result<()> {
    let mut capture = stream()?;
    for i in 0..6_u64 {
        capture.capture(&trade(i + 1, i as i64 * 100)?, None)?;
    }
    // The venue stamps the next event earlier than the last one.
    let backwards = capture.capture(&trade(7, 450)?, None)?;
    let kinds: Vec<&str> = backwards
        .corrections
        .iter()
        .map(|c| c.kind.as_str())
        .collect();
    assert_eq!(kinds, vec!["offset_estimate", "monotonic_floor"]);
    assert!(backwards.normalized_time > backwards.event_time);
    assert_eq!(backwards.arrival_order.as_deref(), Some("unordered"));
    assert!(
        backwards.validate().is_ok(),
        "the floor is lineage, so the times reconcile"
    );
    Ok(())
}

#[test]
fn a_captured_event_round_trips_through_the_tick_lake_with_its_corrections() -> Result<()> {
    let mut capture = stream()?;
    let store = Arc::new(MemoryBlobStore::new());
    let shared: Arc<dyn BlobStore> = store.clone();
    let mut writer = TickLakeWriter::new(
        shared,
        BTreeMap::from([(
            "xnys-itch".to_string(),
            RetentionRight {
                canonical: true,
                raw: false,
            },
        )]),
        6,
        64,
    )?;
    let mut sealed = Vec::new();
    for i in 0..6_u64 {
        let event = capture.capture(
            &trade(i + 1, i as i64 * 100)?,
            Some(b"venue bytes".to_vec()),
        )?;
        sealed.extend(writer.capture(event)?);
    }
    assert_eq!(sealed.len(), 1, "premise: the batch sealed one segment");
    let replayed = Lake::new(store.as_ref()).read_market_segment(&sealed[0])?;
    assert_eq!(replayed.len(), 6);
    assert_eq!(replayed[5].corrections.len(), 1);
    assert!(replayed[5].receive_time > replayed[5].event_time);
    assert!(sealed[0].raw_key.is_none(), "the right keeps no raw bytes");
    Ok(())
}
