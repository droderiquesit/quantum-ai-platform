//! Reading recorded market history out of the Tick Lake for replay
//! (R12-TICK-2059, 2076; R12-DATA-1039): pinned, entitled, bounded, and with
//! both time dimensions on every tick.

#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_history::HistoryQuery;
use qip_storage::{BlobStore, MemoryBlobStore};
use std::collections::BTreeSet;

fn at(ms: i64) -> Timestamp {
    // 2026-10-11T00:00:00Z plus `ms`.
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

/// One tick as the edge node's `LakeSink` writes it: the venue's instant,
/// the capture instant and the sequence beside the serialised event.
fn line(dataset: &str, sequence: u64, venue_ms: i64, capture_ms: i64) -> Result<String> {
    let message = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XLON"), "itch-a", 0, sequence),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(5),
            order_count: None,
        },
        at(venue_ms),
        at(capture_ms),
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        at(venue_ms),
        at(capture_ms),
        at(capture_ms),
        Duration::ZERO,
        hash,
        Licence::Granted {
            dataset: dataset.to_string(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    Ok(qip_core::canonical::canonical_json(&serde_json::json!({
        "exchange_time_ns": at(venue_ms).as_nanos(),
        "capture_time_ns": at(capture_ms).as_nanos(),
        "feed": "itch-a",
        "partition": 0,
        "sequence": sequence,
        "event": serde_json::to_value(&event)?,
    })))
}

fn seal(store: &MemoryBlobStore, dataset: &str, id: &str, lines: &[String]) -> Result<()> {
    let partition = Partition::new(
        RecordClass::Market,
        "XLON",
        "2026-10-11",
        "obj-ACME",
        Entitlement {
            id: dataset.into(),
            raw_retention_permitted: false,
        },
    )?;
    let mut body = Vec::new();
    for l in lines {
        body.extend_from_slice(l.as_bytes());
        body.push(b'\n');
    }
    Lake::new(store).write_segment(&partition, id, body, None)?;
    Ok(())
}

fn query(granted: &[&str], max_ticks: usize) -> HistoryQuery {
    HistoryQuery {
        from_date: "2026-10-11".into(),
        to_date: "2026-10-11".into(),
        venues: BTreeSet::new(),
        instruments: BTreeSet::new(),
        granted: granted.iter().map(|s| (*s).to_string()).collect(),
        max_segments: 16,
        max_ticks,
    }
}

/// Mutation: set `knowable_at` from `event_time()` in `decode_line` — the
/// knowable-instant assertion fails, which is the leak this guards.
#[test]
fn pinned_history_reads_back_every_tick_with_its_knowable_instant_and_entitlement() -> Result<()> {
    let store = MemoryBlobStore::new();
    seal(
        &store,
        "vendor-depth",
        "seg-1",
        &[
            line("vendor-depth", 1, 0, 40)?,
            line("vendor-depth", 2, 10, 55)?,
        ],
    )?;
    let lake = Lake::new(&store);
    let listing = lake.pin_market_history("run-1", &query(&["vendor-depth"], 100))?;
    assert_eq!(
        listing.segments.len(),
        1,
        "premise: the one segment is selected"
    );
    let history = lake.read_market_history("run-1", &["vendor-depth".to_string()].into(), 100)?;
    assert_eq!(history.ticks.len(), 2);
    assert_eq!(history.unparsed, 0);
    let first = &history.ticks[0];
    assert_eq!(first.event_time, at(0), "the venue's instant");
    assert_eq!(
        first.knowable_at,
        at(40),
        "the instant the platform received it, not the venue's"
    );
    assert_eq!(
        first.message.origin.sequence, 1,
        "the venue's sequence survives"
    );
    assert_eq!(first.entitlement, "vendor-depth");
    assert_eq!(
        history.entitlements_read,
        BTreeSet::from(["vendor-depth".to_string()])
    );
    Ok(())
}

/// Mutation: drop the `granted.contains` check in `list_market_history` —
/// the ungranted segment is pinned and read.
#[test]
fn a_segment_under_an_ungranted_entitlement_is_withheld_and_never_read() -> Result<()> {
    let store = MemoryBlobStore::new();
    seal(
        &store,
        "vendor-depth",
        "seg-1",
        &[line("vendor-depth", 1, 0, 1)?],
    )?;
    seal(
        &store,
        "research-only",
        "seg-2",
        &[line("research-only", 1, 0, 1)?],
    )?;
    let lake = Lake::new(&store);
    let listing = lake.pin_market_history("run-1", &query(&["vendor-depth"], 100))?;
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(
        listing.withheld.len(),
        1,
        "the ungranted segment is named as withheld"
    );
    assert!(listing.withheld[0].contains("entitlement=research-only"));
    let nothing = lake.pin_market_history("run-2", &query(&[], 100))?;
    assert!(nothing.segments.is_empty(), "no grant reads nothing");
    assert_eq!(nothing.withheld.len(), 2);
    Ok(())
}

/// Mutation: replace the `max_ticks` refusal with a `break` — the read
/// returns a prefix instead of an error.
#[test]
fn a_history_larger_than_its_tick_bound_is_refused_rather_than_cut() -> Result<()> {
    let store = MemoryBlobStore::new();
    let lines: Vec<String> = (1..=5)
        .map(|s| line("vendor-depth", s, s as i64, s as i64))
        .collect::<Result<_>>()?;
    seal(&store, "vendor-depth", "seg-1", &lines)?;
    let lake = Lake::new(&store);
    lake.pin_market_history("run-1", &query(&["vendor-depth"], 3))?;
    let refusal = lake
        .read_market_history("run-1", &["vendor-depth".to_string()].into(), 3)
        .expect_err("five ticks under a bound of three is refused");
    assert!(refusal.to_string().contains("more than 3"), "{refusal}");
    Ok(())
}

/// Mutation: skip the checksum in `Lake::read_canonical` — the altered
/// segment reads as if it were the pinned one.
#[test]
fn a_segment_changed_after_it_was_pinned_fails_the_read_by_name() -> Result<()> {
    let store = MemoryBlobStore::new();
    seal(
        &store,
        "vendor-depth",
        "seg-1",
        &[line("vendor-depth", 1, 0, 1)?],
    )?;
    let lake = Lake::new(&store);
    let listing = lake.pin_market_history("run-1", &query(&["vendor-depth"], 100))?;
    let key = listing.segments[0].key.clone();
    let mut bytes = store.get(&key)?.expect("sealed");
    bytes.extend_from_slice(b"\n");
    store.put(&key, bytes)?;
    let refusal = lake
        .read_market_history("run-1", &["vendor-depth".to_string()].into(), 100)
        .expect_err("a changed segment is not the pinned one");
    assert!(refusal.to_string().contains(&key), "{refusal}");
    Ok(())
}
