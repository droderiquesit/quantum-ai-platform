//! What the tick lake records about the clock a tick was stamped by, checked
//! against figures worked out by hand rather than against a second copy of
//! the discipline (R12-TICK-2051, R12-TICK-1122).
//!
//! `decision_loop.rs` proves the lake line's clock block equals what an
//! independent `CaptureStream` makes of the same ticks. That comparison has
//! two blind spots, and both were found by audit rather than imagined. Every
//! tick the simulated node applies is stamped by the venue and received by
//! the cell at the same instant, so every line it drains carries an
//! uncertainty of zero, and overwriting `uncertainty_ns` with the constant
//! zero passed it. And because both sides of its comparison run the same
//! `qip-sequencing` code, deleting the offset correction from the discipline
//! changes both sides alike and passed it too.
//!
//! This suite feeds `LakeSink::deliver` — the drain's production sink — a
//! recorded session whose venue clock runs behind the cell's by a delay that
//! alternates between one and three milliseconds, with each tick stating
//! half a millisecond of uncertainty of its own. With the default discipline
//! (sixteen samples before an estimate is trusted, five milliseconds of
//! spread allowed) every figure on every line follows by arithmetic:
//!
//! - lines 0..15 are stamped before the estimate is trusted: the normalized
//!   instant is the venue's, and the uncertainty is the tick's own 0.5 ms;
//! - from line 15 the window holds at least sixteen delays, half of 1 ms and
//!   half of 3 ms, so its interquartile spread is 2 ms and its minimum 1 ms:
//!   the normalized instant is the venue's plus 1 ms, and the uncertainty is
//!   2 ms.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::expect_used)] // integration tests may expect: a panic is the failure report

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::venue::{Origin, VenueId};
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::lineage::Lineage;
use qip_core::{Decimal, Duration, EventId, ObjectId, Timestamp};
use qip_edge_node::event_fabric::drain::{LakeSink, Sink, default_retention};
use qip_edge_node::feed::SIMULATED_DATASET;
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::MARKET_EVENT_APPLIED;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_storage::lake_codec::SegmentCodec;
use qip_storage::{BlobStore, MemoryBlobStore};
use std::sync::Arc;

/// Ticks in the recorded session: enough to cross the default sixteen-sample
/// threshold and stay past it for a few lines.
const TICKS: u64 = 20;
/// The default discipline's sample threshold, restated so the arithmetic in
/// the module doc can be checked against the code without reading it.
const MIN_SAMPLES: u64 = 16;
const OWN_UNCERTAINTY_NS: i64 = 500_000;
const ONE_MS: i64 = 1_000_000;

fn venue_ms(index: u64) -> i64 {
    // One second apart, so no correction is large enough to run time
    // backwards and call the monotonic floor into the figures.
    1_791_676_800_000 + index as i64 * 1_000
}

/// The venue-to-cell delay of tick `index`: 1 ms on even ticks, 3 ms on odd.
fn delay_ns(index: u64) -> i64 {
    if index.is_multiple_of(2) {
        ONE_MS
    } else {
        3 * ONE_MS
    }
}

fn tick(index: u64) -> Result<Record> {
    let venue_time = Timestamp::from_millis(venue_ms(index));
    let receive_time = Timestamp::from_nanos(venue_time.as_nanos() + delay_ns(index));
    let message = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new("XLON"), "simulated-depth", 0, index + 1),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(5),
            order_count: None,
        },
        venue_time,
        receive_time,
    );
    let hash = MarketEvent::hash_payload(&message)?;
    let event = MarketEvent::new(
        message,
        venue_time,
        receive_time,
        receive_time,
        Duration::from_nanos(OWN_UNCERTAINTY_NS),
        hash,
        Licence::Granted {
            dataset: SIMULATED_DATASET.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    let payload = serde_json::to_value(&event)?;
    let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
    let id = format!("edge-1/clock/{index}");
    let any = AnyEvent {
        event_id: EventId::from_string(id.clone()),
        topic: MARKET_EVENT_APPLIED.topic,
        schema_version: MARKET_EVENT_APPLIED.schema_version,
        occurred_at: venue_time,
        recorded_at: receive_time,
        sequence: 0,
        lineage: Lineage::root(qip_core::lineage::CorrelationId::from_string("c-1"), "test"),
        idempotency_key: Some(id),
        payload,
        payload_hash,
    };
    Record::from_any_event(&any, PayloadCodec::CanonicalJson)
}

/// The session through the drain's lake sink, read back as lake lines in
/// sequence order.
fn drained() -> Result<Vec<serde_json::Value>> {
    drained_into(&Arc::new(MemoryBlobStore::new()))
}

/// [`drained`], into a store the caller keeps.
fn drained_into(lake: &Arc<MemoryBlobStore>) -> Result<Vec<serde_json::Value>> {
    let mut sink = LakeSink::new(
        Arc::clone(lake) as Arc<dyn BlobStore>,
        "edge-1",
        default_retention(),
        SegmentCodec::Identity,
    )?;
    let records = (0..TICKS).map(tick).collect::<Result<Vec<_>>>()?;
    let batch = Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        1,
        PayloadCodec::CanonicalJson,
        records,
    )?;
    sink.deliver(1, QosClass::P2MarketJournal, &batch)?;
    let mut lines = Vec::new();
    for key in lake.list("lake/class=market/")? {
        let body = lake.get(&key)?.expect("a listed segment is readable");
        for line in String::from_utf8(body).expect("canonical JSON").lines() {
            lines.push(serde_json::from_str::<serde_json::Value>(line)?);
        }
    }
    lines.sort_by_key(|line| line["sequence"].as_u64());
    Ok(lines)
}

/// Mutations: in `tick_line` write `uncertainty_ns` as the constant 0 — line 0
/// fails (left 0, right 500000); write `capture_time_ns` from the event time —
/// the capture assertion fails; in `ClockDiscipline::discipline_traced` skip
/// the offset correction — line 15's normalized instant fails (venue time,
/// not venue time + 1 ms) and its uncertainty falls back to the tick's own.
#[test]
fn a_skewed_venue_clock_reaches_the_lake_line_as_the_figures_arithmetic_predicts() -> Result<()> {
    let lines = drained()?;
    assert_eq!(
        lines.len() as u64,
        TICKS,
        "premise: every tick of the session reached the lake"
    );
    let mut warmed = 0u64;
    for (index, line) in (0u64..).zip(&lines) {
        let venue = Timestamp::from_millis(venue_ms(index)).as_nanos();
        let capture = venue + delay_ns(index);
        assert_eq!(line["sequence"].as_u64(), Some(index + 1));
        // The three instants are three fields, none written over another.
        assert_eq!(
            line["exchange_time_ns"].as_i64(),
            Some(venue),
            "line {index}: the venue's instant was not kept: {line}"
        );
        assert_eq!(
            line["capture_time_ns"].as_i64(),
            Some(capture),
            "line {index}: the capture instant was not kept: {line}"
        );
        let clock = &line["clock"];
        let (normalized, uncertainty) = if index + 1 < MIN_SAMPLES {
            (venue, OWN_UNCERTAINTY_NS)
        } else {
            warmed += 1;
            (venue + ONE_MS, 2 * ONE_MS)
        };
        assert_eq!(
            clock["normalized_time_ns"].as_i64(),
            Some(normalized),
            "line {index}: the normalized instant is not the hand-worked one: {line}"
        );
        assert_eq!(
            clock["uncertainty_ns"].as_i64(),
            Some(uncertainty),
            "line {index}: the uncertainty is not the hand-worked one: {line}"
        );
        // The event round-trips with its own stated uncertainty untouched:
        // the lake's figure sits beside it, it does not replace it.
        let event: MarketEvent = serde_json::from_value(line["event"].clone())?;
        assert_eq!(event.event_time().as_nanos(), venue);
        assert_eq!(event.receive_time().as_nanos(), capture);
        assert_eq!(event.uncertainty().as_nanos(), OWN_UNCERTAINTY_NS);
        // The venue and the feed's sequence survive as the event's origin.
        assert_eq!(event.origin().venue.as_str(), "XLON");
        assert_eq!(event.origin().sequence, index + 1);
    }
    assert_eq!(
        warmed,
        TICKS - MIN_SAMPLES + 1,
        "premise: the session crossed the discipline's sample threshold"
    );
    Ok(())
}

/// The replay digest of the session's clock facts: the same recorded session
/// drained twice yields one digest, and that digest is the one the
/// hand-worked figures give. A mutation of the clock-normalization stage
/// moves a normalized instant or an uncertainty, and with it the digest.
///
/// Mutation: skip the offset correction in
/// `ClockDiscipline::discipline_traced` — the replayed digest no longer equals
/// the hand-worked one.
#[test]
fn the_recorded_sessions_clock_digest_is_reproducible_and_is_the_hand_worked_one() -> Result<()> {
    let clock_facts = |lines: &[serde_json::Value]| {
        lines
            .iter()
            .map(|line| {
                format!(
                    "{}|{}|{}|{}|{}",
                    line["sequence"],
                    line["exchange_time_ns"],
                    line["capture_time_ns"],
                    line["clock"]["normalized_time_ns"],
                    line["clock"]["uncertainty_ns"]
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = drained()?;
    let second = drained()?;
    assert_eq!(
        first.len() as u64,
        TICKS,
        "premise: the session produced lines"
    );
    let replayed = sha256_hex(clock_facts(&first).as_bytes());
    assert_eq!(
        replayed,
        sha256_hex(clock_facts(&second).as_bytes()),
        "the same recorded session drained to two different clock digests"
    );
    let expected = (0..TICKS)
        .map(|index| {
            let venue = Timestamp::from_millis(venue_ms(index)).as_nanos();
            let (normalized, uncertainty) = if index + 1 < MIN_SAMPLES {
                (venue, OWN_UNCERTAINTY_NS)
            } else {
                (venue + ONE_MS, 2 * ONE_MS)
            };
            format!(
                "{}|{venue}|{}|{normalized}|{uncertainty}",
                index + 1,
                venue + delay_ns(index)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        replayed,
        sha256_hex(expected.as_bytes()),
        "the replayed clock digest is not the hand-worked one:\n{}",
        clock_facts(&first)
    );
    Ok(())
}

/// A replay reading the lake back gets the clock it was written with
/// (R12-TICK-1201): `qip_storage::lake_history`, the reader every replay job
/// uses, hands each tick its normalized instant and uncertainty, equal to
/// the hand-worked figures.
///
/// Mutation: `decode_line` returns `clock: None` for the edge node's shape —
/// line 0 fails, left None, right Some(..).
#[test]
fn reading_the_lake_back_returns_each_ticks_disciplined_clock() -> Result<()> {
    use qip_storage::lake::Lake;
    use qip_storage::lake_history::{HistoryQuery, RecordedClock};
    use std::collections::BTreeSet;

    let lake = Arc::new(MemoryBlobStore::new());
    drained_into(&lake)?;
    let granted = BTreeSet::from([SIMULATED_DATASET.to_string()]);
    let reader = Lake::new(lake.as_ref());
    reader.pin_market_history(
        "clock",
        &HistoryQuery {
            from_date: "2000-01-01".to_string(),
            to_date: "2100-01-01".to_string(),
            venues: BTreeSet::new(),
            instruments: BTreeSet::new(),
            granted: granted.clone(),
            max_segments: 100,
            max_ticks: 1_000,
        },
    )?;
    let mut ticks = reader.read_market_history("clock", &granted, 1_000)?.ticks;
    ticks.sort_by_key(|tick| tick.message.origin.sequence);
    assert_eq!(ticks.len() as u64, TICKS, "premise: every tick read back");
    for (index, tick) in (0u64..).zip(&ticks) {
        let venue = Timestamp::from_millis(venue_ms(index)).as_nanos();
        let (normalized, uncertainty) = if index + 1 < MIN_SAMPLES {
            (venue, OWN_UNCERTAINTY_NS)
        } else {
            (venue + ONE_MS, 2 * ONE_MS)
        };
        assert_eq!(
            tick.clock,
            Some(RecordedClock {
                normalized_time: Timestamp::from_nanos(normalized),
                uncertainty: Duration::from_nanos(uncertainty),
            }),
            "line {index}: the replayed clock is not the one the lake was written with"
        );
    }
    Ok(())
}
