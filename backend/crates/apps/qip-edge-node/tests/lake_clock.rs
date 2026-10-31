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
    tick_at("XLON", index + 1, venue_time, receive_time)
}

/// One tick of `venue`'s feed, numbered `sequence`, stamped by the venue at
/// `venue_time` and received at `receive_time`, stating
/// [`OWN_UNCERTAINTY_NS`] of clock error of its own.
fn tick_at(
    venue: &str,
    sequence: u64,
    venue_time: Timestamp,
    receive_time: Timestamp,
) -> Result<Record> {
    tick_stating(
        venue,
        sequence,
        venue_time,
        receive_time,
        OWN_UNCERTAINTY_NS,
    )
}

/// [`tick_at`], stating `own_uncertainty_ns` of clock error instead.
fn tick_stating(
    venue: &str,
    sequence: u64,
    venue_time: Timestamp,
    receive_time: Timestamp,
    own_uncertainty_ns: i64,
) -> Result<Record> {
    let message = MarketMessage::new(
        ObjectId::from_string("obj-ACME"),
        Origin::new(VenueId::new(venue), "simulated-depth", 0, sequence),
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
        Duration::from_nanos(own_uncertainty_ns),
        hash,
        Licence::Granted {
            dataset: SIMULATED_DATASET.into(),
            usage: Usage::Trade,
            expires_at: Timestamp::MAX,
        },
    )?;
    let payload = serde_json::to_value(&event)?;
    let payload_hash = sha256_hex(qip_core::canonical::canonical_json(&payload).as_bytes());
    let id = format!("edge-1/clock/{venue}/{sequence}");
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
        "europe-west2",
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

/// The region the sink is built for in this suite.
const REGION: &str = "europe-west2";

/// `records` through `sink` as one P2 batch at spool `offset`, and every
/// lake line `lake` then holds, keyed by venue and the feed's sequence.
/// Raw slots (`.raw`, ADR 0181) are skipped: they hold the venue's bytes,
/// not lines.
fn delivered(
    sink: &mut LakeSink,
    lake: &MemoryBlobStore,
    offset: u64,
    records: Vec<Record>,
) -> Result<std::collections::BTreeMap<(String, u64), serde_json::Value>> {
    let batch = Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        1,
        PayloadCodec::CanonicalJson,
        records,
    )?;
    sink.deliver(offset, QosClass::P2MarketJournal, &batch)?;
    let mut lines = std::collections::BTreeMap::new();
    for key in lake.list("lake/class=market/")? {
        if key.ends_with(".raw") {
            continue;
        }
        let body = lake.get(&key)?.expect("a listed segment is readable");
        for line in String::from_utf8(body).expect("canonical JSON").lines() {
            let line: serde_json::Value = serde_json::from_str(line)?;
            let event: MarketEvent = serde_json::from_value(line["event"].clone())?;
            lines.insert(
                (
                    event.origin().venue.as_str().to_string(),
                    event.origin().sequence,
                ),
                line,
            );
        }
    }
    Ok(lines)
}

/// A sink for cell `edge-1` in [`REGION`] over `lake`.
fn sink_over(lake: &Arc<MemoryBlobStore>) -> Result<LakeSink> {
    LakeSink::new(
        Arc::clone(lake) as Arc<dyn BlobStore>,
        "edge-1",
        REGION,
        default_retention(),
        SegmentCodec::Identity,
    )
}

/// R12-TICK-2048: two venues' ticks are stamped in the lake with the order
/// `qip_sequencing::align` supports across the two partitions, and with no
/// more order than that.
///
/// Every tick here states half a millisecond of uncertainty and arrives
/// before the discipline trusts an estimate. A `Timestamp` crosses the spool
/// as RFC 3339 at millisecond resolution, so the sink widens each side by
/// half a millisecond more to cover the floor (`SPOOL_TRUNCATION_HALF_WIDTH`).
/// The combined uncertainty of any two ticks is therefore exactly 2 ms. The
/// stamps sit on whole milliseconds so the floor moves none of them. The
/// figures are worked by hand, and they put a tick on each side of the bound:
///
/// - XLON #1 at `base`: nothing else has been seen, so it is aligned against
///   nothing;
/// - XPAR #1 at `base + 2 ms`: exactly the combined uncertainty from XLON #1.
///   The two intervals touch, so the result is `unordered`, not an order;
/// - XPAR #2 at `base + 3 ms`: 3 ms after XLON #1, outside 2 ms, so XLON #1
///   came `before`. XPAR's own earlier tick is not listed: a partition is
///   aligned against the others, and its own order is `arrival_order`;
/// - XLON #2 at `base + 5 ms`: 2 ms after XPAR #2, the partition's latest
///   tick rather than its first, so `unordered`.
///
/// Without this stamp the only order across partitions is the one a reader
/// gets by sorting on a timestamp column, and that sort ranks XPAR #1 after
/// XLON #1 when the two clocks cannot support it. A lead/lag model trained
/// on that order learns clock error as a reaction.
///
/// Mutation: in `LakeSink::align_across`, return an empty array instead of
/// the alignments (drop the cross-partition stamp). XPAR #1's line fails:
/// it carries no entry for XLON.
#[test]
fn two_venues_ticks_within_combined_uncertainty_are_lake_stamped_unordered_and_outside_it_ordered()
-> Result<()> {
    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = sink_over(&lake)?;
    let base = Timestamp::from_millis(venue_ms(0));
    let at = |offset_ns: i64| Timestamp::from_nanos(base.as_nanos() + offset_ns);
    let received = |offset_ns: i64| at(offset_ns + ONE_MS);
    let records = vec![
        tick_at("XLON", 1, at(0), received(0))?,
        tick_at("XPAR", 1, at(2 * ONE_MS), received(2 * ONE_MS))?,
        tick_at("XPAR", 2, at(3 * ONE_MS), received(3 * ONE_MS))?,
        tick_at("XLON", 2, at(5 * ONE_MS), received(5 * ONE_MS))?,
    ];
    let lines = delivered(&mut sink, &lake, 1, records)?;
    assert_eq!(lines.len(), 4, "premise: all four ticks reached the lake");
    for (key, line) in &lines {
        // Premise: the hand-worked figures assume every tick was placed by
        // its own stated uncertainty, before any estimate was trusted.
        assert_eq!(
            line["clock"]["uncertainty_ns"].as_i64(),
            Some(OWN_UNCERTAINTY_NS),
            "{key:?}: premise: placed by its own uncertainty: {line}"
        );
        assert_eq!(line["clock_source"], "venue_stated", "{key:?}: {line}");
        assert_eq!(line["region"], REGION, "{key:?}: {line}");
        assert_eq!(line["cell"], "edge-1", "{key:?}: {line}");
    }
    let xlon = "XLON/simulated-depth/0/obj-ACME";
    let xpar = "XPAR/simulated-depth/0/obj-ACME";
    let stamp = |venue: &str, sequence: u64| {
        lines[&(venue.to_string(), sequence)]["cross_partition"].clone()
    };

    assert_eq!(
        stamp("XLON", 1),
        serde_json::json!([]),
        "the first tick of the session has no other partition to be aligned against"
    );
    assert_eq!(
        stamp("XPAR", 1),
        serde_json::json!([{
            "partition": xlon,
            "sequence": 1,
            "order_of_other": "unordered",
            "lag_ns": 2 * ONE_MS,
            "lag_uncertainty_ns": 2 * ONE_MS,
            "other_known_at_arrival": true,
        }]),
        "2 ms apart on clocks each known to 1 ms after the floor touches the bound and is \
         not an order"
    );
    assert_eq!(
        stamp("XPAR", 2),
        serde_json::json!([{
            "partition": xlon,
            "sequence": 1,
            "order_of_other": "before",
            "lag_ns": 3 * ONE_MS,
            "lag_uncertainty_ns": 2 * ONE_MS,
            "other_known_at_arrival": true,
        }]),
        "3 ms apart on clocks each known to 1 ms after the floor is an order, and XPAR's own \
         earlier tick is not another partition"
    );
    assert_eq!(
        stamp("XLON", 2),
        serde_json::json!([{
            "partition": xpar,
            "sequence": 2,
            "order_of_other": "unordered",
            "lag_ns": 2 * ONE_MS,
            "lag_uncertainty_ns": 2 * ONE_MS,
            "other_known_at_arrival": true,
        }]),
        "aligned against the other partition's latest tick, not its first"
    );
    Ok(())
}

/// Two ticks a microsecond apart on clocks each known to 50 µs cannot be
/// ordered, and the lake must not order them because the spool floored their
/// instants to different milliseconds. XLON is stamped at `base + 0.999 ms`
/// and XPAR at `base + 1.000 ms`; across the spool they read `base` and
/// `base + 1 ms`. Aligned on their stated uncertainties alone that is 1 ms of
/// lag against a 0.1 ms bound, and XLON would be stamped `before` — the
/// false order an independent review found in the first version of the
/// cross-partition stamp. Widened by half the spool's millisecond each, the
/// bound is 1.1 ms and the pair is `unordered`.
///
/// Mutation: in `LakeSink::deliver`, build the `Placed` from
/// `captured.uncertainty` alone (drop `SPOOL_TRUNCATION_HALF_WIDTH`). XPAR's
/// line fails (left "before", right "unordered").
#[test]
fn ticks_the_spool_floored_to_different_milliseconds_are_not_ordered_by_the_floor() -> Result<()> {
    const FIFTY_MICROS: i64 = 50_000;
    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = sink_over(&lake)?;
    let base = Timestamp::from_millis(venue_ms(0));
    let at = |offset_ns: i64| Timestamp::from_nanos(base.as_nanos() + offset_ns);
    let records = vec![
        tick_stating("XLON", 1, at(999_000), at(999_000), FIFTY_MICROS)?,
        tick_stating("XPAR", 1, at(ONE_MS), at(ONE_MS), FIFTY_MICROS)?,
    ];
    let lines = delivered(&mut sink, &lake, 1, records)?;
    assert_eq!(lines.len(), 2, "premise: both ticks reached the lake");
    // Premise: the spool did floor XLON's instant to the millisecond below,
    // so the two lines read a whole millisecond apart.
    let normalized = |venue: &str| {
        lines[&(venue.to_string(), 1)]["clock"]["normalized_time_ns"]
            .as_i64()
            .expect("a placed tick carries its normalized instant")
    };
    assert_eq!(
        normalized("XPAR") - normalized("XLON"),
        ONE_MS,
        "premise: the floor put the two instants a millisecond apart"
    );
    let entry = &lines[&("XPAR".to_string(), 1)]["cross_partition"][0];
    assert_eq!(
        entry["partition"], "XLON/simulated-depth/0/obj-ACME",
        "{entry}"
    );
    assert_eq!(
        entry["order_of_other"], "unordered",
        "a microsecond apart on 50 µs clocks is no order, however the spool floored it: {entry}"
    );
    assert_eq!(entry["lag_uncertainty_ns"], 1_100_000, "{entry}");
    Ok(())
}

/// R12-TICK-2048: every lake line names the region and cell that captured
/// it and the clock its normalized instant came from. The recorded session
/// above crosses the discipline's threshold at line 15, so the same feed's
/// lines name two different clocks, and that difference is what this test
/// checks.
///
/// Mutation: in `clock_source`, return `venue_stated` for every placed tick.
/// Line 15 fails (left "venue_stated", right "offset_estimate").
#[test]
fn every_lake_line_names_its_region_its_cell_and_the_clock_its_instant_came_from() -> Result<()> {
    let lines = drained()?;
    assert_eq!(
        lines.len() as u64,
        TICKS,
        "premise: every tick reached the lake"
    );
    for (index, line) in (0u64..).zip(&lines) {
        assert_eq!(line["region"], REGION, "line {index}: {line}");
        assert_eq!(line["cell"], "edge-1", "line {index}: {line}");
        let expected = if index + 1 < MIN_SAMPLES {
            "venue_stated"
        } else {
            "offset_estimate"
        };
        assert_eq!(line["clock_source"], expected, "line {index}: {line}");
    }
    Ok(())
}

/// The cross-partition working set is bounded (R12-TICK-2048). One more
/// partition than [`MAX_ALIGNED_PARTITIONS`] (one more again, so eviction
/// runs twice) leaves the sink holding exactly the bound. No line is aligned
/// against more partitions than the bound, and the partition silent longest
/// is the one dropped. An unbounded map keyed by venue and instrument would
/// grow with every instrument a session ever touched, on a thread whose
/// stall halts the cell.
///
/// Mutation: skip the eviction in `LakeSink::align_across`. The sink holds
/// the bound plus two and the first assertion fails.
#[test]
fn the_cross_partition_alignment_holds_at_most_its_bound_of_partitions() -> Result<()> {
    use qip_edge_node::event_fabric::drain::MAX_ALIGNED_PARTITIONS;

    let lake = Arc::new(MemoryBlobStore::new());
    let mut sink = sink_over(&lake)?;
    let venues = MAX_ALIGNED_PARTITIONS + 2;
    let records = (0..venues)
        .map(|index| {
            let venue_time = Timestamp::from_millis(venue_ms(index as u64));
            let receive_time = Timestamp::from_nanos(venue_time.as_nanos() + ONE_MS);
            tick_at(&format!("XV{index:04}"), 1, venue_time, receive_time)
        })
        .collect::<Result<Vec<_>>>()?;
    let lines = delivered(&mut sink, &lake, 1, records)?;
    assert_eq!(
        lines.len(),
        venues,
        "premise: every venue's tick reached the lake"
    );
    assert_eq!(
        sink.aligned_partitions(),
        MAX_ALIGNED_PARTITIONS,
        "the sink holds more partitions than its bound"
    );
    for (key, line) in &lines {
        let entries = line["cross_partition"]
            .as_array()
            .expect("a placed tick's stamp is an array");
        assert!(
            entries.len() <= MAX_ALIGNED_PARTITIONS,
            "{key:?} was aligned against {} partitions",
            entries.len()
        );
    }
    let last = lines[&(format!("XV{:04}", venues - 1), 1)]["cross_partition"]
        .as_array()
        .expect("a placed tick's stamp is an array")
        .iter()
        .map(|entry| entry["partition"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(last.len(), MAX_ALIGNED_PARTITIONS);
    assert!(
        !last.iter().any(|name| name.starts_with("XV0000/")),
        "the partition silent longest was kept and a later one dropped"
    );
    assert!(
        last.iter().any(|name| name.starts_with("XV0001/")),
        "premise: only the one oldest partition had left when the last tick was aligned"
    );
    Ok(())
}
