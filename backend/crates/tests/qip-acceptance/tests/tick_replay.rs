//! Exact historical replay is deterministic (blueprint TICK-017).
//!
//! Each stage already had its own determinism test: the decoder is a pure
//! function of its bytes, the sequencer reads no clock, the book rebuilds to
//! identical snapshots. None of them replayed a session. A recorded session is
//! venue-native bytes, and what a replay owes its reader is the whole chain —
//! decode, sequence, rebuild the book, simulate a fill against it — reaching
//! the same event order, the same book at every step, the same fills and the
//! same integrity flags, every time and wherever it is run. A stage that is
//! deterministic alone and order-dependent at its seam with the next passes
//! every per-stage test and breaks exactly this.
//!
//! The session below is small and deliberately dirty: a packet out of order
//! inside the reorder window, a packet delivered twice, a halt and a resume,
//! and a hole that never fills, so the replay has to carry a reset and an
//! unreliable period through to its output rather than only succeed on a
//! clean tape.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::governance::{Entitlement as Licence, Usage};
use qip_contracts::market_event::MarketEvent;
use qip_contracts::{BookSide, MarketMessage, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_orderbook::replay::{ReplayOutput, replay};
use qip_orderbook::{Sweep, VenueCheckpoint, VenueState};
use qip_protocols::decoder::Diagnostics;
use qip_protocols::{Decoder, InstrumentPartitions, ItchDecoder};
use qip_sequencing::{Placed, ReorderPolicy, SequenceEvent, Sequencer, StreamStats, align, as_of};
use qip_storage::MemoryBlobStore;
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_history::HistoryQuery;
use qip_strategy::model::DistilledModel;
use qip_training::dataset::TrainingDataset;
use qip_training::distill::{StudentForm, distil};
use qip_training::{LocalTrainer, ModelFamily, TrainingSpec};
use serde::Serialize;

// --- the recorded session: ITCH 5.0 bytes as MoldUDP would have carried them -

fn put(buffer: &mut Vec<u8>, width: usize, value: u64) {
    for index in (0..width).rev() {
        buffer.push(((value >> (8 * index)) & 0xFF) as u8);
    }
}

fn put_stock(buffer: &mut Vec<u8>, stock: &str) {
    let bytes = stock.as_bytes();
    for index in 0..8 {
        buffer.push(bytes.get(index).copied().unwrap_or(b' '));
    }
}

fn header(kind: u8, nanos: u64) -> Vec<u8> {
    let mut out = vec![kind];
    put(&mut out, 2, 1); // stock locate
    put(&mut out, 2, 0); // tracking number
    put(&mut out, 6, nanos);
    out
}

fn add(nanos: u64, order_ref: u64, side: u8, shares: u32, price_ticks: u32) -> Vec<u8> {
    let mut out = header(b'A', nanos);
    put(&mut out, 8, order_ref);
    out.push(side);
    put(&mut out, 4, u64::from(shares));
    put_stock(&mut out, "AAPL");
    put(&mut out, 4, u64::from(price_ticks));
    out
}

fn executed(nanos: u64, order_ref: u64, shares: u32) -> Vec<u8> {
    let mut out = header(b'E', nanos);
    put(&mut out, 8, order_ref);
    put(&mut out, 4, u64::from(shares));
    put(&mut out, 8, 55); // match number
    out
}

fn cancel(nanos: u64, order_ref: u64, shares: u32) -> Vec<u8> {
    let mut out = header(b'X', nanos);
    put(&mut out, 8, order_ref);
    put(&mut out, 4, u64::from(shares));
    out
}

fn replace(nanos: u64, original: u64, new: u64, shares: u32, price_ticks: u32) -> Vec<u8> {
    let mut out = header(b'U', nanos);
    put(&mut out, 8, original);
    put(&mut out, 8, new);
    put(&mut out, 4, u64::from(shares));
    put(&mut out, 4, u64::from(price_ticks));
    out
}

fn trading_action(nanos: u64, state: u8) -> Vec<u8> {
    let mut out = header(b'H', nanos);
    put_stock(&mut out, "AAPL");
    out.push(state);
    out.push(b' ');
    out.extend_from_slice(b"    ");
    out
}

/// One packet as captured: the transport's sequence number, when this cell's
/// hardware saw it, and the venue's bytes.
struct Packet {
    sequence: u64,
    captured_micros: i64,
    bytes: Vec<u8>,
}

fn packet(sequence: u64, captured_micros: i64, bytes: Vec<u8>) -> Packet {
    Packet {
        sequence,
        captured_micros,
        bytes,
    }
}

fn session_midnight() -> Timestamp {
    Timestamp::from_civil(2024, 1, 2)
}

fn captured(micros: i64) -> Timestamp {
    session_midnight().saturating_add(Duration::from_micros(micros))
}

/// How long a hole may stay open before the sequencer gives up on it.
const GAP_TIMEOUT_MICROS: i64 = 500;

fn recorded_session() -> Vec<Packet> {
    vec![
        packet(1, 10, add(1_000, 1, b'B', 500, 1_002_500)),
        packet(2, 20, add(2_000, 2, b'S', 400, 1_003_000)),
        packet(3, 30, add(3_000, 3, b'B', 300, 1_002_400)),
        packet(4, 40, add(4_000, 4, b'S', 600, 1_003_100)),
        packet(5, 50, executed(5_000, 1, 200)),
        // Seven overtakes six on the wire, and six then arrives twice.
        packet(7, 60, add(7_000, 5, b'B', 250, 1_002_450)),
        packet(6, 70, cancel(6_000, 2, 100)),
        packet(6, 75, cancel(6_000, 2, 100)),
        packet(8, 80, replace(8_000, 3, 6, 350, 1_002_480)),
        packet(9, 90, trading_action(9_000, b'H')),
        packet(10, 100, trading_action(10_000, b'T')),
        // Eleven and twelve never arrive. Thirteen opens a hole that is held
        // until the deadline, then abandoned behind a reset.
        packet(13, 130, add(13_000, 7, b'S', 150, 1_002_900)),
        packet(14, 140, add(14_000, 8, b'B', 700, 1_002_300)),
        packet(15, 2_000, add(15_000, 9, b'S', 500, 1_003_200)),
        packet(16, 2_010, executed(16_000, 8, 300)),
        packet(17, 2_020, cancel(17_000, 9, 200)),
        packet(18, 2_030, add(18_000, 10, b'B', 120, 1_002_350)),
    ]
}

// --- one replay ---------------------------------------------------------------

/// What a simulated order would have got from the book at one step.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct SimulatedFills {
    buy: Option<Sweep>,
    sell: Option<Sweep>,
}

/// Everything a replay produces that a second replay must reproduce.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct ReplayRecord {
    /// The messages in the order the sequencer released them.
    events: Vec<MarketMessage>,
    /// Each event's venue and capture time in whole nanoseconds. Beside the
    /// events rather than inside them, because a timestamp serialises to the
    /// millisecond: without this the digest could not tell two replays apart
    /// that stamped an event microseconds differently.
    stamped_nanos: Vec<(i64, i64)>,
    /// What the sequencer observed on the way.
    sequence_events: Vec<SequenceEvent>,
    /// The book after each released message.
    book_states: Vec<String>,
    /// A marketable order each way against the book after each message.
    fills: Vec<SimulatedFills>,
    /// The integrity flags: the book replay's, the sequencer's, the decoder's.
    integrity: ReplayOutput,
    stream: Vec<(String, StreamStats)>,
    decoder: Diagnostics,
}

impl ReplayRecord {
    fn digest(&self) -> String {
        sha256_hex(&serde_json::to_vec(self).expect("a replay record serialises"))
    }
}

fn fresh_book() -> VenueState {
    VenueState::order_by_order(
        ObjectId::from_string("obj-aapl"),
        VenueId::new("XNAS"),
        VenueStatus::Open,
    )
}

/// The size a simulated order asks for: more than the touch holds, so a fill
/// has to walk levels and a wrong book shows up as a wrong price.
fn order_size() -> Decimal {
    Decimal::from_int(450)
}

/// Decode and sequence the session: bytes in, ordered canonical events out.
/// The book half of the record is left empty for the caller to fill.
fn decode_and_sequence(session: &[Packet]) -> Result<ReplayRecord> {
    let mut decoder = ItchDecoder::new(
        VenueId::new("XNAS"),
        "itch-a",
        InstrumentPartitions::new().with("AAPL", 3),
        session_midnight(),
    );
    let mut sequencer = Sequencer::new(ReorderPolicy::new(
        64,
        Duration::from_micros(GAP_TIMEOUT_MICROS),
    ));
    let mut events = Vec::new();
    let mut sequence_events = Vec::new();
    for packet in session {
        let at = captured(packet.captured_micros);
        // The deadline is advanced on the capture clock of the next packet,
        // which is the only clock a replay has.
        let expired = sequencer.poll(at);
        events.extend(expired.released);
        sequence_events.extend(expired.events);

        decoder.set_sequence(packet.sequence);
        let decoded = decoder.decode(&packet.bytes, at)?;
        let batch = sequencer.accept(decoded, at);
        events.extend(batch.released);
        sequence_events.extend(batch.events);
    }
    let stream = sequencer
        .streams()
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                sequencer.tracker(name).expect("a listed stream").stats(),
            )
        })
        .collect();
    let stamped_nanos = events
        .iter()
        .map(|event: &MarketMessage| (event.venue_time.as_nanos(), event.capture_time.as_nanos()))
        .collect();
    Ok(ReplayRecord {
        events,
        stamped_nanos,
        sequence_events,
        book_states: Vec::new(),
        fills: Vec::new(),
        integrity: ReplayOutput::default(),
        stream,
        decoder: decoder.diagnostics().clone(),
    })
}

fn observe(state: &VenueState) -> (String, SimulatedFills) {
    (
        state.snapshot().digest(),
        SimulatedFills {
            buy: state.sweep_cost(BookSide::Ask, order_size()),
            sell: state.sweep_cost(BookSide::Bid, order_size()),
        },
    )
}

/// Replay the session from its bytes, from an empty book.
fn replay_from_the_start(session: &[Packet]) -> Result<ReplayRecord> {
    let mut record = decode_and_sequence(session)?;
    for through in 1..=record.events.len() {
        let mut state = fresh_book();
        replay(&mut state, &record.events[..through])?;
        let (book, fill) = observe(&state);
        record.book_states.push(book);
        record.fills.push(fill);
    }
    let mut state = fresh_book();
    record.integrity = replay(&mut state, &record.events)?;
    Ok(record)
}

/// The book states and fills from `from` onward, starting from a checkpoint
/// taken after `from` messages.
///
/// **The checkpoint is held in memory, and that is a limit of this test
/// rather than a choice.** It was first written to go through the
/// checkpoint's serialised form, as one read back from storage would, and
/// that fails on unmodified code: `qip_core::Timestamp` serialises to
/// milliseconds, the last print's venue time here is five microseconds past
/// midnight, and the restored state therefore differs from the one that was
/// checkpointed. A checkpoint that has been stored does not resume to the
/// same book states for any session stamped below a millisecond, which is
/// every venue-native one. Making the serialised form lossless changes what
/// every hash over a sub-millisecond timestamp is computed from, so it is not
/// done here; TICK-017's register row carries it as the gap, and this helper
/// should go back through `serde_json` the day it is closed.
fn resumed_from_a_checkpoint(
    events: &[MarketMessage],
    from: usize,
) -> Result<(Vec<String>, Vec<SimulatedFills>)> {
    let mut state = fresh_book();
    replay(&mut state, &events[..from])?;
    let checkpoint: VenueCheckpoint = state.checkpoint();

    let mut book_states = Vec::new();
    let mut fills = Vec::new();
    for through in from + 1..=events.len() {
        let mut resumed = VenueState::restore(&checkpoint)?;
        replay(&mut resumed, &events[from..through])?;
        let (book, fill) = observe(&resumed);
        book_states.push(book);
        fills.push(fill);
    }
    Ok((book_states, fills))
}

/// The digest of the recorded session's replay, as first computed. Every host
/// that runs this suite recomputes it from the bytes above and compares, which
/// is the form "once more on a different host" takes in a test: nothing in
/// the chain may depend on the machine, the process or the run. If a change
/// to a decoder, the sequencer or the book moves it on purpose, the new value
/// is a decision to be made here and said in the commit, not a number to
/// paste.
const RECORDED_DIGEST: &str = "2610f37b6fc0eff55283ddc00436ac2cb07f8fefe9ea876ba00fd4d9513729c2";

#[test]
fn replaying_one_recorded_session_of_venue_bytes_twice_and_from_a_checkpoint_reproduces_every_event_book_fill_and_flag()
-> Result<()> {
    let session = recorded_session();
    let first = replay_from_the_start(&session)?;

    // --- premises: the session exercised what it was built to exercise ------
    let saw = |wanted: fn(&SequenceEvent) -> bool| first.sequence_events.iter().any(wanted);
    assert!(
        saw(|event| matches!(event, SequenceEvent::GapFilled { .. })),
        "premise: the overtaken packet was not reordered: {:?}",
        first.sequence_events
    );
    assert!(
        saw(|event| matches!(event, SequenceEvent::Duplicate { .. })),
        "premise: the repeated packet was not seen as a duplicate"
    );
    assert!(
        saw(|event| matches!(event, SequenceEvent::GapAbandoned { .. })),
        "premise: the hole that never fills was not abandoned"
    );
    let sequences: Vec<u64> = first
        .events
        .iter()
        .map(|event| event.origin.sequence)
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] <= pair[1]),
        "premise: the sequencer released out of order: {sequences:?}"
    );
    assert!(
        first.fills.iter().any(|fill| fill
            .buy
            .is_some_and(|sweep| sweep.levels_consumed > 1 && sweep.filled == order_size())),
        "premise: no simulated fill walked more than one level"
    );
    assert!(
        first.fills.iter().any(|fill| fill.buy.is_none()),
        "premise: the halt never withheld a fill"
    );
    let distinct: std::collections::BTreeSet<&String> = first.book_states.iter().collect();
    assert!(
        distinct.len() > 10,
        "premise: the book barely moved: {} distinct states",
        distinct.len()
    );

    // --- twice ---------------------------------------------------------------
    let second = replay_from_the_start(&recorded_session())?;
    assert_eq!(first.events, second.events, "event order differs");
    assert_eq!(first.book_states, second.book_states, "book states differ");
    assert_eq!(first.fills, second.fills, "simulated fills differ");
    assert_eq!(first.integrity, second.integrity, "integrity flags differ");
    assert_eq!(first, second);
    assert_eq!(first.digest(), second.digest());

    // --- from a checkpoint, in the clean span before the hole ----------------
    // After the first print, not before it. This sat one message earlier, and
    // a restore that forgot the trade count passed: a checkpoint taken before
    // anything traded carries a zero, and a zero survives being forgotten.
    const CHECKPOINT_AFTER: usize = 6;
    {
        let mut at_checkpoint = fresh_book();
        replay(&mut at_checkpoint, &first.events[..CHECKPOINT_AFTER])?;
        assert!(
            at_checkpoint.trade_count() > 0 && !at_checkpoint.session_volume().is_zero(),
            "premise: the checkpoint holds state the levels alone do not carry"
        );
    }
    assert!(
        first.integrity.steps[..=CHECKPOINT_AFTER]
            .iter()
            .all(|step| !step.unreliable),
        "premise: the checkpoint was taken inside a reliable span"
    );
    let (books, fills) = resumed_from_a_checkpoint(&first.events, CHECKPOINT_AFTER)?;
    assert!(books.len() > 5, "premise: the resumed span is not trivial");
    assert_eq!(
        books,
        first.book_states[CHECKPOINT_AFTER..],
        "a replay resumed from a checkpoint reached different books"
    );
    assert_eq!(fills, first.fills[CHECKPOINT_AFTER..]);

    // --- on any host -----------------------------------------------------------
    assert_eq!(
        first.digest(),
        RECORDED_DIGEST,
        "the replay digest moved; see the constant's comment"
    );
    Ok(())
}

/// The lake stage of the replay: the recorded session is sealed through
/// `Lake::write_segment` as the edge node's `LakeSink` writes it, pinned and
/// read back through `read_market_history`, and replayed from what came back.
/// The digest must be `RECORDED_DIGEST`, so the stage is inside the digest and
/// not beside it.
///
/// Until this test, `RECORDED_DIGEST` was computed from decode to book in
/// memory and never touched the lake, so a change to `write_segment` or to
/// `decode_line` moved no digest. `Timestamp` serialises to the millisecond,
/// so a reader that took the message's instants from the serialised payload
/// returned every venue-native tick stamped differently from the tape.
///
/// Mutation: in `qip_storage::lake_history::decode_line`, drop the two
/// `exchange_time_ns` / `capture_time_ns` overrides, or in `read_market_history`
/// skip pushing a tick; the equality of events, or the digest, fails.
#[test]
fn a_recorded_session_sealed_into_the_lake_and_read_back_replays_to_the_recorded_digest()
-> Result<()> {
    let direct = replay_from_the_start(&recorded_session())?;
    assert!(
        direct.events.len() > 10,
        "premise: the session has events to seal"
    );
    assert!(
        direct
            .stamped_nanos
            .iter()
            .any(|(venue, capture)| venue % 1_000_000 != 0 || capture % 1_000_000 != 0),
        "premise: the session is stamped below a millisecond, which is what the lake must keep"
    );

    let licence = "itch-a-licence";
    let store = MemoryBlobStore::new();
    let lake = Lake::new(&store);
    let partition = Partition::new(
        RecordClass::Market,
        "XNAS",
        "2024-01-02",
        "obj-aapl",
        Entitlement {
            id: licence.into(),
            raw_retention_permitted: false,
        },
    )?;
    let mut body = Vec::new();
    for message in &direct.events {
        let hash = MarketEvent::hash_payload(message)?;
        let event = MarketEvent::new(
            message.clone(),
            message.venue_time,
            message.capture_time,
            message.venue_time,
            Duration::ZERO,
            hash,
            Licence::Granted {
                dataset: licence.to_string(),
                usage: Usage::Trade,
                expires_at: Timestamp::MAX,
            },
        )?;
        let line = qip_core::canonical::canonical_json(&serde_json::json!({
            "exchange_time_ns": message.venue_time.as_nanos(),
            "capture_time_ns": message.capture_time.as_nanos(),
            "feed": "itch-a",
            "partition": 0,
            "sequence": message.origin.sequence,
            "event": serde_json::to_value(&event)?,
        }));
        body.extend_from_slice(line.as_bytes());
        body.push(b'\n');
    }
    lake.write_segment(&partition, "seg-1", body, None)?;

    let granted: std::collections::BTreeSet<String> = [licence.to_string()].into();
    let query = HistoryQuery {
        from_date: "2024-01-02".into(),
        to_date: "2024-01-02".into(),
        venues: std::collections::BTreeSet::new(),
        instruments: std::collections::BTreeSet::new(),
        granted: granted.clone(),
        max_segments: 4,
        max_ticks: 1_000,
    };
    let listing = lake.pin_market_history("tick-replay", &query)?;
    assert_eq!(listing.segments.len(), 1, "premise: the segment is pinned");
    let history = lake.read_market_history("tick-replay", &granted, 1_000)?;
    assert_eq!(history.unparsed, 0, "a line of the session was not read back");
    let events: Vec<MarketMessage> = history.ticks.into_iter().map(|tick| tick.message).collect();
    assert_eq!(events, direct.events, "the lake returned a different tape");

    let mut from_lake = direct.clone();
    from_lake.events = events;
    from_lake.stamped_nanos = from_lake
        .events
        .iter()
        .map(|event| (event.venue_time.as_nanos(), event.capture_time.as_nanos()))
        .collect();
    from_lake.book_states.clear();
    from_lake.fills.clear();
    for through in 1..=from_lake.events.len() {
        let mut state = fresh_book();
        replay(&mut state, &from_lake.events[..through])?;
        let (book, fill) = observe(&state);
        from_lake.book_states.push(book);
        from_lake.fills.push(fill);
    }
    let mut state = fresh_book();
    from_lake.integrity = replay(&mut state, &from_lake.events)?;
    assert_eq!(from_lake.digest(), RECORDED_DIGEST);
    Ok(())
}

/// The flags are an output like the fills are. A replay that reproduced every
/// book and dropped the fact that a span of them is not to be believed would
/// hand a training job a clean-looking tape with a hole in it.
///
/// **What this does not show, found while writing it and still open.** The
/// hole in this session is two packets wide, and every book edit after it is
/// flagged. A hole *one* packet wide is not: the sequencer stamps its reset
/// with the first missing sequence, `qip_orderbook::replay` reads any reset as
/// a venue's completed rebuild, and the packet after a one-wide hole is then
/// contiguous with the reset. The tape replays as wholly reliable against a
/// book rebuilt from nothing. Telling the two resets apart needs the canonical
/// schema to say who declared one, which is a contract change and not made
/// here; TICK-017's register row carries it as the gap.
#[test]
fn the_replay_carries_the_abandoned_gap_out_as_an_integrity_flag_and_a_lost_message_count()
-> Result<()> {
    let record = replay_from_the_start(&recorded_session())?;
    let stats: Vec<&StreamStats> = record.stream.iter().map(|(_, stats)| stats).collect();
    assert_eq!(stats.len(), 1, "premise: one stream");
    assert_eq!(stats[0].gaps_abandoned, 1);
    assert_eq!(stats[0].messages_lost, 2, "sequences 11 and 12");
    assert_eq!(stats[0].duplicates, 1);
    assert_eq!(stats[0].gaps_filled, 1);
    assert_eq!(
        record.integrity.steps.len(),
        record.events.len(),
        "one flag per replayed event"
    );

    // The reset the sequencer declared is in the event order, where the hole
    // was, and every book edit after it is flagged and withheld: the book
    // those edits would build is missing everything that rested before.
    let reset_at = record
        .events
        .iter()
        .position(|event| matches!(event.body, qip_contracts::MessageBody::Reset { .. }))
        .expect("premise: the abandoned gap put a reset in the stream");
    assert_eq!(record.events[reset_at - 1].origin.sequence, 10);
    assert_eq!(record.events[reset_at + 1].origin.sequence, 13);
    assert!(
        record.integrity.steps[..=reset_at]
            .iter()
            .all(|step| !step.unreliable),
        "premise: nothing before the hole is flagged"
    );
    assert!(
        record.events.len() > reset_at + 5,
        "premise: the session goes on after the hole"
    );
    assert!(
        record.integrity.steps[reset_at + 1..]
            .iter()
            .all(|step| step.unreliable),
        "a book edit after the unrecovered hole was replayed as reliable"
    );
    assert_eq!(record.integrity.periods.len(), 1);
    assert_eq!(record.integrity.periods[0].until, None, "never recovered");
    let empty = fresh_book().snapshot().book.digest();
    let last = {
        let mut state = fresh_book();
        replay(&mut state, &record.events)?;
        state.snapshot().book.digest()
    };
    assert_eq!(
        last, empty,
        "edits were applied to a book known to be wrong"
    );

    // A clean tape of the same session has none of it, so the flags above are
    // findings about this tape and not constants.
    let clean: Vec<Packet> = recorded_session()
        .into_iter()
        .filter(|packet| packet.sequence <= 5)
        .collect();
    let clean = replay_from_the_start(&clean)?;
    assert!(clean.integrity.is_clean());
    assert_eq!(clean.stream[0].1.gaps_abandoned, 0);
    assert_ne!(clean.digest(), record.digest());
    Ok(())
}

/// A hole one packet wide degrades the replay: the step after it is flagged
/// unreliable and a `Gap { 11, 11 }` period is reported.
///
/// This test was named `a_hole_one_packet_wide_is_replayed_as_wholly_reliable_
/// which_is_the_open_gap_and_not_the_intent` and its doc said it was "meant to
/// fail". It was written while the defect stood: the sequencer's reset for the
/// hole was read by `qip_orderbook::replay` as a completed rebuild, the next
/// packet was contiguous with it, no period opened, and every step read
/// reliable. `6bf4d300` (TICK-040) changed `replay` so a gap is measured
/// against the last *real* sequence rather than the reset's, and the body was
/// updated to assert the fix, but the name still claimed the opposite. The
/// name now says what the body asserts.
///
/// Mutation: in `qip_orderbook::replay::replay`, in the
/// `sequence > previous + 1` arm, replace `open = Some(period);` with
/// `let _ = period;`. No period opens, `is_clean()` is true and the first
/// assertion after the premises fires. Equally: make the reset arm update
/// `last.insert(stream.clone(), sequence)`. The next packet is then
/// contiguous with the reset and the replay reads wholly reliable again,
/// which is the original defect.
///
/// Known oddity, recorded rather than asserted: the period's `until` is the
/// *reset's* venue time, which precedes the period's `from`, and the period is
/// never taken out of `open`, so every later step stays flagged unreliable
/// while `periods[0].until` says it ended. The per-step flags are the
/// conservative half and are what this test pins.
#[test]
fn a_hole_one_packet_wide_is_replayed_as_degraded_with_the_step_after_it_flagged_unreliable()
-> Result<()> {
    let mut session = recorded_session();
    // Sequence twelve arrives after all, so only eleven is lost.
    session.insert(11, packet(12, 120, add(12_000, 20, b'S', 111, 1_002_950)));
    let record = replay_from_the_start(&session)?;

    // Premise: a packet really was lost, and the sequencer really said so.
    let stats = record.stream[0].1;
    assert_eq!(stats.gaps_abandoned, 1);
    assert_eq!(stats.messages_lost, 1, "sequence 11 alone");
    let reset_at = record
        .events
        .iter()
        .position(|event| matches!(event.body, qip_contracts::MessageBody::Reset { .. }))
        .expect("premise: the abandoned hole put a reset in the stream");
    assert_eq!(record.events[reset_at + 1].origin.sequence, 12);
    // Premise: there was a book to lose. Before the hole a buy walks two
    // levels of resting asks that the reset then throws away.
    let before = record.fills[reset_at - 1]
        .buy
        .expect("premise: the book was usable before the hole");
    assert_eq!(before.filled, order_size());

    // Premise: before the hole the replay was clean, so the flags below are
    // about the hole and not a replay that flags everything.
    assert!(
        record.integrity.steps[..reset_at]
            .iter()
            .all(|step| !step.unreliable),
        "premise: nothing before the hole is flagged"
    );

    // Degraded reliability: the hole opens a period.
    assert!(
        !record.integrity.is_clean(),
        "one-packet hole should be flagged, but is_clean() returned true: {:?}",
        record.integrity.periods
    );
    assert_eq!(
        record.integrity.periods.len(),
        1,
        "expected one unreliable period for the one-packet gap"
    );
    let period = &record.integrity.periods[0];
    assert!(
        matches!(period.fault, qip_orderbook::replay::IntegrityFault::Gap { missing_from, missing_to } if missing_from == 11 && missing_to == 11),
        "period fault should be Gap(11..11), got {:?}",
        period.fault
    );
    assert_eq!(
        period.until,
        Some(record.events[reset_at].venue_time),
        "period should close at the reset"
    );

    // The book edit after the hole is still applied (the period is closed at
    // the reset) but carries the unreliable flag downstream.
    let after = record.fills[reset_at + 1]
        .buy
        .expect("the book should still be evaluated after reset");
    assert_eq!(after.filled, Decimal::from_int(111));
    assert!(
        record.integrity.steps[reset_at + 1].unreliable,
        "the first step after a one-packet hole was replayed as reliable"
    );
    assert!(
        record.integrity.steps[reset_at + 1..]
            .iter()
            .all(|step| step.unreliable),
        "a step after the hole was replayed as reliable"
    );
    Ok(())
}

// --- the distillation stage inside the replay's digest ----------------------

/// What the reflex model made of the recorded session: the student's own
/// serialised form and its score at every step of the replay where the touch
/// was two-sided. Digested, so a change to the student moves a number a person
/// can compare.
#[derive(Serialize)]
struct DistilledScores<'a> {
    student: &'a DistilledModel,
    scores: Vec<f64>,
}

/// Book features at every released event that has both sides of the touch:
/// the spread in price units and the size imbalance at the touch. The target
/// is the microprice's offset from the mid, which the book alone determines.
fn touch_features(record: &ReplayRecord) -> Result<(Vec<Vec<f64>>, Vec<f64>)> {
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    for through in 1..=record.events.len() {
        let mut state = fresh_book();
        replay(&mut state, &record.events[..through])?;
        let (Some(bid), Some(ask), Some(mid), Some(micro), Some(spread)) = (
            state.best_bid(),
            state.best_ask(),
            state.mid(),
            state.microprice(),
            state.spread(),
        ) else {
            continue;
        };
        let (b, a) = (bid.size.to_f64(), ask.size.to_f64());
        rows.push(vec![spread.to_f64(), (b - a) / (b + a)]);
        targets.push((micro - mid).to_f64());
    }
    Ok((rows, targets))
}

fn distilled_digest(student: &DistilledModel, rows: &[Vec<f64>]) -> Result<String> {
    let scores = rows
        .iter()
        .map(|row| student.evaluate(row))
        .collect::<Result<Vec<f64>>>()?;
    Ok(sha256_hex(
        &serde_json::to_vec(&DistilledScores { student, scores }).expect("scores serialise"),
    ))
}

fn distil_the_recorded_session() -> Result<(DistilledModel, Vec<Vec<f64>>)> {
    let record = replay_from_the_start(&recorded_session())?;
    let (rows, targets) = touch_features(&record)?;
    let times = (0..rows.len())
        .map(|i| captured(i as i64))
        .collect::<Vec<_>>();
    let data = TrainingDataset::new(
        "recorded-session",
        vec!["spread".to_string(), "imbalance".to_string()],
        rows.clone(),
        targets,
        times,
    )?;
    let spec = TrainingSpec::new(
        "touch-model",
        "v1",
        "the-model-owner",
        "recorded session",
        ModelFamily::Linear { ridge: 1e-6 },
    );
    let teacher = LocalTrainer::new().fit(&spec, &data, captured(1_000))?;
    let distillation = distil(&teacher, &data, StudentForm::Linear { ridge: 1e-6 }, 0.0)?;
    Ok((distillation.student().clone(), rows))
}

/// The digest of the student distilled from the recorded session's touch, as
/// first computed. Moves only when the distillation stage, the trainer or the
/// replay it learns from moves.
const DISTILLED_DIGEST: &str = "4296d2909713b5336fc94a247882adbb338bffc5d7eb65ef6d0816234964363c";

#[test]
fn a_mutated_distillation_stage_changes_the_digest_of_the_replay_it_learned_from() -> Result<()> {
    let (student, rows) = distil_the_recorded_session()?;
    assert!(
        rows.len() >= 8,
        "premise: the session gave {} row(s)",
        rows.len()
    );
    let digest = distilled_digest(&student, &rows)?;

    // Reproducible: a second run of the whole chain, bytes to student.
    let (again, again_rows) = distil_the_recorded_session()?;
    assert_eq!(rows, again_rows, "the features are not reproducible");
    assert_eq!(digest, distilled_digest(&again, &again_rows)?);

    // Sensitive: a student whose one weight is nudged is a different student,
    // and the digest says so. A digest that ignored the student would pass the
    // line above on any build.
    let qip_strategy::model::ModelForm::Linear {
        intercept,
        coefficients,
    } = student.form().clone()
    else {
        panic!("premise: a linear student");
    };
    let mut nudged = coefficients.clone();
    nudged[0] += 1e-6;
    let mutated = DistilledModel::linear(student.name().to_string(), intercept, nudged)?;
    assert_ne!(digest, distilled_digest(&mutated, &rows)?);

    assert_eq!(
        digest, DISTILLED_DIGEST,
        "the distilled digest moved; see the constant"
    );
    Ok(())
}

// --- the fabric's replay/simulation engine over the recorded session ---------

/// The recorded session's decoded, sequenced events as the fabric's replay
/// input: each message knowable at its capture time, which is the only clock a
/// recording has.
fn fabric_ticks(session: &[Packet]) -> Result<Vec<qip_simulation_engine::tick_replay::ReplayTick>> {
    Ok(decode_and_sequence(session)?
        .events
        .into_iter()
        .map(|message| qip_simulation_engine::tick_replay::ReplayTick {
            knowable_at: message.capture_time,
            message,
        })
        .collect())
}

fn fabric_model() -> qip_simulation_engine::tick_replay::ExecutionModel {
    qip_simulation_engine::tick_replay::ExecutionModel {
        latency: Duration::from_millis(10),
        fee_bps: 2.0,
        impact_bps_at_full_depth: 20.0,
        limit_ttl: Duration::from_secs(60),
        message_budget: None,
    }
}

/// `qip_simulation_engine::tick_replay::ReplaySession` over the recorded
/// session reproduces its run digest, and a mutation of the replayed history
/// or of the simulated order moves a digest. The digest is not pinned to a
/// literal here: it was never computed on a host, and a constant written
/// without running is the failure this file's own `RECORDED_DIGEST` comment
/// warns about.
///
/// Mutation: drop one tick from the history; both digests and the step count
/// must move.
#[test]
fn the_fabric_replay_engine_over_the_recorded_session_is_reproducible_and_its_digest_moves_with_the_history()
-> Result<()> {
    use qip_simulation_engine::tick_replay::{
        OrderSide, ReplayOrder, ReplaySession,
    };

    let ticks = fabric_ticks(&recorded_session())?;
    assert!(
        ticks.len() > 2,
        "premise: the recorded session produced events to replay"
    );
    let instrument = ticks[0].message.object_id.as_str().to_string();
    let order = |quantity: i64| ReplayOrder {
        id: "o-1".into(),
        instrument: instrument.clone(),
        side: OrderSide::Buy,
        quantity: Decimal::from_int(quantity),
        limit: None,
        decided_at: ticks[0].knowable_at,
    };

    let run = |ticks: Vec<_>, quantity: i64| -> Result<_> {
        ReplaySession::new(fabric_model(), ticks, vec![order(quantity)], Vec::new())?.run_to_end()
    };
    let first = run(ticks.clone(), 1)?;
    let second = run(ticks.clone(), 1)?;
    assert_eq!(
        first.steps as usize,
        ticks.len(),
        "premise: every recorded tick was stepped"
    );
    assert_eq!(first, second, "one recording replays to one run");
    assert_eq!(first.digest, second.digest);

    // The same bytes decoded again give the same replay input.
    let again = run(fabric_ticks(&recorded_session())?, 1)?;
    assert_eq!(first.input_digest, again.input_digest);

    // Mutate the history: the stage under test must notice.
    let mut shorter = ticks.clone();
    shorter.pop();
    let mutated = run(shorter, 1)?;
    assert_ne!(first.input_digest, mutated.input_digest);
    assert_ne!(first.digest, mutated.digest);

    // Mutate the simulated decision: the input digest must notice.
    assert_ne!(first.input_digest, run(ticks, 2)?.input_digest);
    Ok(())
}

// --- cross-venue alignment inside the replay's digest ------------------------

/// Two venues' stamps for the recorded session, each on its own clock and each
/// with its own published uncertainty, in microseconds. Arrival is the cell's
/// single clock. Venue B's third event sits 30 microseconds after A's with
/// 25 on each clock, so it must come out unordered.
fn venue_streams(uncertainty_b_micros: i64) -> Result<(Vec<Placed>, Vec<Placed>)> {
    let place = |event: i64, err: i64, arrival: i64| {
        Placed::new(
            captured(event),
            Duration::from_micros(err),
            captured(arrival),
        )
    };
    let a = vec![
        place(100, 25, 110)?,
        place(200, 25, 210)?,
        place(300, 25, 310)?,
    ];
    let b = vec![
        place(150, uncertainty_b_micros, 160)?,
        place(330, uncertainty_b_micros, 340)?,
        place(1_000, uncertainty_b_micros, 1_010)?,
    ];
    Ok((a, b))
}

/// Every B event placed as-of the A stream, rendered and hashed, so a change
/// to any alignment answer moves one string a person can compare.
fn alignment_digest(uncertainty_b_micros: i64) -> Result<String> {
    let (a, b) = venue_streams(uncertainty_b_micros)?;
    let mut rendered = String::new();
    for target in &b {
        rendered.push_str(&format!(
            "{:?}|{:?}\n",
            as_of(target, &a),
            align(&a[0], target)
        ));
    }
    Ok(sha256_hex(rendered.as_bytes()))
}

#[test]
fn cross_venue_alignment_of_the_recorded_stamps_is_reproducible_and_its_digest_moves_with_the_stage()
-> Result<()> {
    // Premise: the clock error is wide enough that one pair is unordered, or
    // the digest would be insensitive to the stage under test.
    let (a, b) = venue_streams(25)?;
    let near = as_of(&b[1], &a);
    assert!(
        !near.unordered.is_empty(),
        "the premise failed: no event pair fell inside the combined uncertainty"
    );

    let digest = alignment_digest(25)?;
    assert_eq!(digest, alignment_digest(25)?);

    // Mutation of the stage's input: with venue B's clock exact, the unordered
    // pair becomes ordered and the digest must say so.
    assert_ne!(digest, alignment_digest(0)?);
    Ok(())
}
