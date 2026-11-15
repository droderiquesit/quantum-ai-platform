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

use qip_contracts::{BookSide, MarketMessage, VenueId, VenueStatus};
use qip_core::error::Result;
use qip_core::hash::sha256_hex;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_orderbook::replay::{ReplayOutput, replay};
use qip_orderbook::{Sweep, VenueCheckpoint, VenueState};
use qip_protocols::decoder::Diagnostics;
use qip_protocols::{Decoder, InstrumentPartitions, ItchDecoder};
use qip_sequencing::{ReorderPolicy, SequenceEvent, Sequencer, StreamStats};
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

// --- the stages RECORDED_DIGEST did not reach ---------------------------------
//
// R12-TICK-1121 (the lake), 1124 (cross-venue alignment), 1125 (the fabric's
// replay engine) and 1127 (world and market fusion). `RECORDED_DIGEST` is
// computed in memory from decode to book, so a mutation of any of these stages
// moved no digest the suite held. Each test below sends the recorded session
// through the stage it names and asserts a digest a mutation of that stage
// would move.
//
// **None of these tests was compiled or run when it was written** (owner
// instruction of 2026-11-15; ADR 0679), so none pins a literal digest it has
// not seen: it asserts reproducibility, equality with the independent path,
// and sensitivity, and the lake test asserts the existing `RECORDED_DIGEST`.
// Mutation: not run, for each.

use qip_contracts::captured_event::{CapturedMarketEvent, ClockCorrectionLineage};
use qip_core::error::Error;
use qip_kernel::fusion::{FusionConfig, MarketTrace, TracePoint, fuse};
use qip_sequencing::{CrossVenueAligner, Order, VenueAlignment, VenueTick};
use qip_simulation_engine::tick_replay::{
    ExecutionModel, OrderSide, ReplayOrder, ReplayRun, ReplaySession, ReplayTick,
};
use qip_storage::MemoryBlobStore;
use qip_storage::lake::{Entitlement, Lake, Partition, RecordClass};
use qip_storage::lake_history::{HistoryQuery, RecordedHistory, RecordedTick};
use qip_world_model::graph::{EntityKind, Fact, KnowledgeGraph, Node, NodeKind};
use qip_world_model::relationship::{Relationship, RelationshipKind};
use std::collections::BTreeSet;

/// The entitlement the session's lake partitions are written under.
const ENTITLEMENT: &str = "itch-research-test";

/// Decode the session's packets and stop there: the decoder's messages in
/// packet order, and the decoder's own diagnostics.
fn decode_only(session: &[Packet]) -> Result<(Vec<MarketMessage>, Diagnostics)> {
    let mut decoder = ItchDecoder::new(
        VenueId::new("XNAS"),
        "itch-a",
        InstrumentPartitions::new().with("AAPL", 3),
        session_midnight(),
    );
    let mut decoded = Vec::new();
    for packet in session {
        decoder.set_sequence(packet.sequence);
        decoded.extend(decoder.decode(&packet.bytes, captured(packet.captured_micros))?);
    }
    Ok((decoded, decoder.diagnostics().clone()))
}

/// One decoded message as the capture path stores it.
///
/// The canonical record carries both instants as exact integer nanoseconds:
/// `Timestamp`'s own serialised form is RFC 3339 and truncates to the
/// millisecond, which is the limit `resumed_from_a_checkpoint` documents, and
/// a session stamped in microseconds would come back sharing instants.
///
/// `sequence` is the message's position in decode order, not the feed's
/// number. The lake orders a segment by receive time, then this field, then
/// canonical text, and one wire packet can decode into two messages (an
/// execution is a trade and a reduction) that share the first two keys; text
/// order would then decide which a replay applies first. The feed's own
/// sequence is in the canonical record, where the sequencer reads it.
fn captured_event(
    venue: &str,
    position: u64,
    message: &MarketMessage,
    shift_nanos: i64,
    uncertainty_nanos: i64,
) -> CapturedMarketEvent {
    let mut canonical = serde_json::to_value(message).expect("a message serialises");
    canonical["venue_time"] = serde_json::json!(message.venue_time.as_nanos());
    canonical["capture_time"] = serde_json::json!(message.capture_time.as_nanos());
    let corrections = if shift_nanos == 0 {
        Vec::new()
    } else {
        vec![ClockCorrectionLineage {
            kind: "offset_estimate".to_string(),
            delta_nanos: shift_nanos,
            applied_at: message.capture_time,
            samples: 8,
            uncertainty_nanos: uncertainty_nanos as f64,
        }]
    };
    CapturedMarketEvent {
        venue: venue.to_string(),
        feed: "itch-a".to_string(),
        instrument: "AAPL".to_string(),
        sequence: Some(position),
        event_time: message.venue_time,
        receive_time: message.capture_time,
        normalized_time: Timestamp::from_nanos(message.venue_time.as_nanos() + shift_nanos),
        uncertainty: Duration::from_nanos(uncertainty_nanos),
        corrections,
        arrival_order: None,
        entitlement: ENTITLEMENT.to_string(),
        canonical,
        raw_sha256: None,
        raw: None,
    }
}

/// Seal `decoded` as one market segment of `venue` on 2024-01-02.
fn into_the_lake(
    store: &MemoryBlobStore,
    venue: &str,
    decoded: &[MarketMessage],
    shift_nanos: i64,
    uncertainty_nanos: i64,
) -> Result<()> {
    let partition = Partition::new(
        RecordClass::Market,
        venue,
        "2024-01-02",
        "AAPL",
        Entitlement {
            id: ENTITLEMENT.to_string(),
            raw_retention_permitted: false,
        },
    )?;
    let events: Vec<CapturedMarketEvent> = decoded
        .iter()
        .enumerate()
        .map(|(position, message)| {
            captured_event(
                venue,
                position as u64,
                message,
                shift_nanos,
                uncertainty_nanos,
            )
        })
        .collect();
    Lake::new(store).write_market_segment(&partition, &events)?;
    Ok(())
}

/// Pin the day's market history as `manifest` and read it back through the
/// manifest, as a replay job does.
fn out_of_the_lake(store: &MemoryBlobStore, manifest: &str) -> Result<RecordedHistory> {
    let granted: BTreeSet<String> = BTreeSet::from([ENTITLEMENT.to_string()]);
    let query = HistoryQuery {
        from_date: "2024-01-02".to_string(),
        to_date: "2024-01-02".to_string(),
        venues: BTreeSet::new(),
        instruments: BTreeSet::new(),
        granted: granted.clone(),
        max_segments: 16,
        max_ticks: 10_000,
    };
    let lake = Lake::new(store);
    let listing = lake.pin_market_history(manifest, &query)?;
    assert!(
        !listing.segments.is_empty(),
        "premise: the pin found the sealed segment"
    );
    assert!(listing.withheld.is_empty(), "premise: nothing was withheld");
    lake.read_market_history(manifest, &granted, query.max_ticks)
}

/// Sequence what the lake returned, as the live path sequences what the
/// decoder returned: in capture order, the deadline advanced on the next
/// tick's capture instant, and messages of one wire packet offered together.
/// `decoder` is the decode stage's own diagnostics, which the lake does not
/// hold. The book half of the record is left empty.
fn sequence_the_lake(history: &RecordedHistory, decoder: Diagnostics) -> ReplayRecord {
    let mut sequencer = Sequencer::new(ReorderPolicy::new(
        64,
        Duration::from_micros(GAP_TIMEOUT_MICROS),
    ));
    let mut ticks: Vec<&RecordedTick> = history.ticks.iter().collect();
    // Stable, so ticks sharing an instant keep the lake's line order.
    ticks.sort_by_key(|tick| tick.knowable_at);
    let packet_of = |tick: &RecordedTick| {
        (
            tick.knowable_at,
            tick.message.origin.stream_key().to_string(),
            tick.message.origin.sequence,
        )
    };
    let mut events = Vec::new();
    let mut sequence_events = Vec::new();
    let mut index = 0;
    while index < ticks.len() {
        let packet = packet_of(ticks[index]);
        let at = packet.0;
        let mut unit = Vec::new();
        while index < ticks.len() && packet_of(ticks[index]) == packet {
            unit.push(ticks[index].message.clone());
            index += 1;
        }
        let expired = sequencer.poll(at);
        events.extend(expired.released);
        sequence_events.extend(expired.events);
        let batch = sequencer.accept(unit, at);
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
    ReplayRecord {
        events,
        stamped_nanos,
        sequence_events,
        book_states: Vec::new(),
        fills: Vec::new(),
        integrity: ReplayOutput::default(),
        stream,
        decoder,
    }
}

/// Fill in the book half of a record from its events, as
/// `replay_from_the_start` does.
fn with_the_book_half(mut record: ReplayRecord) -> Result<ReplayRecord> {
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

/// R12-TICK-1121. The session is written through `Lake::write_market_segment`,
/// pinned, read back with `read_market_history`, sequenced and replayed, and
/// the result is the digest the in-memory path was pinned to.
///
/// What each half catches. A `write_market_segment` or `decode_line` that
/// loses, reorders or re-stamps a tick moves `events`, `stamped_nanos` or a
/// book, and so the digest. The lossy session at the end shows the digest is
/// a finding about the lake's contents and not a constant: the same chain
/// with one message missing from the sealed segment does not reach it.
///
/// Mutation: not run. Intended: in `Lake::write_market_segment`, drop the
/// last line of `canonical`, or in `lake_history::decode_line` replace
/// `knowable_at: event.receive_time()` / `captured.receive_time` with the
/// event time; the first assertion after the premises fires.
#[test]
fn the_recorded_session_sealed_in_the_tick_lake_and_read_back_replays_to_the_pinned_digest()
-> Result<()> {
    let session = recorded_session();
    let direct = replay_from_the_start(&session)?;
    let (decoded, diagnostics) = decode_only(&session)?;
    let store = MemoryBlobStore::new();
    into_the_lake(&store, "XNAS", &decoded, 0, 100)?;
    let history = out_of_the_lake(&store, "recorded-session")?;

    // --- premises ------------------------------------------------------------
    assert!(
        decoded.len() >= 15,
        "premise: the session decoded to {} message(s)",
        decoded.len()
    );
    assert_eq!(history.unparsed, 0, "premise: every lake line decoded");
    assert_eq!(history.withheld, Vec::<String>::new());
    assert!(
        history.entitlements_read.contains(ENTITLEMENT),
        "premise: the lineage names the entitlement read"
    );
    assert!(
        history
            .ticks
            .iter()
            .map(|tick| &tick.message)
            .eq(decoded.iter()),
        "the lake returned different messages, or the same in another order"
    );
    assert!(
        history.ticks.iter().all(|tick| tick.clock.is_some()),
        "a lake line lost its clock block"
    );
    assert!(
        history.ticks.iter().all(|tick| {
            tick.event_time == tick.message.venue_time
                && tick.knowable_at == tick.message.capture_time
        }),
        "a tick's event or knowable instant differs from its message's"
    );

    // --- the replay, from the lake ---------------------------------------------
    let from_lake = with_the_book_half(sequence_the_lake(&history, diagnostics.clone()))?;
    assert_eq!(from_lake.events, direct.events, "event order differs");
    assert_eq!(from_lake.stamped_nanos, direct.stamped_nanos);
    assert_eq!(from_lake.book_states, direct.book_states);
    assert_eq!(from_lake, direct);
    assert_eq!(
        from_lake.digest(),
        RECORDED_DIGEST,
        "the replay digest of the session read back from the lake moved"
    );

    // --- reproducible: a second lake, a second read -----------------------------
    let again_store = MemoryBlobStore::new();
    into_the_lake(&again_store, "XNAS", &decoded, 0, 100)?;
    let again = out_of_the_lake(&again_store, "recorded-session")?;
    assert_eq!(history.ticks, again.ticks);
    assert_eq!(
        with_the_book_half(sequence_the_lake(&again, diagnostics.clone()))?.digest(),
        from_lake.digest()
    );

    // --- sensitive: one message missing from the sealed segment -----------------
    let mut lossy = decoded.clone();
    lossy.retain(|message| message.origin.sequence != 5);
    assert!(
        lossy.len() < decoded.len(),
        "premise: sequence 5 was in the session"
    );
    let lossy_store = MemoryBlobStore::new();
    into_the_lake(&lossy_store, "XNAS", &lossy, 0, 100)?;
    let lossy_history = out_of_the_lake(&lossy_store, "recorded-session")?;
    let lossy_record = with_the_book_half(sequence_the_lake(&lossy_history, diagnostics))?;
    assert_ne!(
        lossy_record.digest(),
        RECORDED_DIGEST,
        "a lake missing a printed message reached the same replay digest"
    );
    Ok(())
}

// --- cross-venue alignment (R12-TICK-1124) ----------------------------------------

/// The venue a market segment key names.
fn venue_of(segment: &str) -> String {
    segment
        .split('/')
        .find_map(|part| part.strip_prefix("venue="))
        .expect("a market segment key names its venue")
        .to_string()
}

/// Place every tick of a history on the common timeline, in the order a
/// reader wants "latest" to mean: capture instant, then venue, then feed
/// sequence. A tick whose line carried no clock block is refused, not placed
/// at its venue's own instant as if that clock were known.
fn venue_ticks_of(history: &RecordedHistory) -> Result<Vec<VenueTick>> {
    let mut ticks: Vec<&RecordedTick> = history.ticks.iter().collect();
    ticks.sort_by(|a, b| {
        a.knowable_at
            .cmp(&b.knowable_at)
            .then_with(|| venue_of(&a.segment).cmp(&venue_of(&b.segment)))
            .then_with(|| a.message.origin.sequence.cmp(&b.message.origin.sequence))
    });
    ticks
        .into_iter()
        .map(|tick| {
            let clock = tick.clock.ok_or_else(|| {
                Error::invalid(format!(
                    "a tick of {} carries no clock block; its clock is unknown and it cannot be aligned",
                    tick.segment
                ))
            })?;
            Ok(VenueTick {
                venue: venue_of(&tick.segment),
                instrument: "AAPL".to_string(),
                sequence: tick.message.origin.sequence,
                normalized_time: clock.normalized_time,
                uncertainty: clock.uncertainty,
                arrival: tick.knowable_at,
            })
        })
        .collect()
}

/// The recorded session as two venues' history. XNAS is stamped exactly as
/// decoded with a 100 ns clock; XBATS carries the same prints 250 ns later on
/// a clock known to `b_uncertainty`. Sealed, pinned, read back, aligned.
fn two_venue_alignments(b_uncertainty: i64) -> Result<Vec<VenueAlignment>> {
    let (decoded, _) = decode_only(&recorded_session())?;
    let store = MemoryBlobStore::new();
    into_the_lake(&store, "XNAS", &decoded, 0, 100)?;
    into_the_lake(&store, "XBATS", &decoded, 250, b_uncertainty)?;
    let history = out_of_the_lake(&store, "two-venues")?;
    assert_eq!(
        history.ticks.len(),
        2 * decoded.len(),
        "premise: both venues' ticks came back from the lake"
    );
    let mut aligner = CrossVenueAligner::new();
    let mut alignments = Vec::new();
    for tick in venue_ticks_of(&history)? {
        alignments.extend(aligner.relate(tick)?);
    }
    Ok(alignments)
}

fn alignment_digest(alignments: &[VenueAlignment]) -> String {
    sha256_hex(&serde_json::to_vec(alignments).expect("alignments serialise"))
}

/// R12-TICK-1124. Two venues' recorded ticks come back from the lake with
/// their clock blocks, are placed, and are related by `CrossVenueAligner`,
/// which states an order only beyond both recorded uncertainties.
///
/// The session is built so both answers occur: a tick and the other venue's
/// print of it are 250 ns apart, inside the 300 ns their uncertainties sum to
/// (unordered), while a tick and the other venue's previous print are about a
/// microsecond apart (ordered). Reading the same history with the second
/// venue's clock known to 100 ns instead of 200 ns turns those unordered
/// pairs into ordered ones, so the digest depends on the recorded
/// uncertainty: an aligner that ignored it, or a reader that dropped the
/// clock block, would not move it.
///
/// Mutation: not run. Intended: in `qip_sequencing::align::align`, replace
/// `lag.abs() <= lag_uncertainty` with `lag.abs() < Duration::ZERO`; every
/// pair is then ordered and the first `Unordered` assertion fires.
#[test]
fn two_venues_recorded_in_the_lake_are_aligned_by_their_clocks_and_a_mutated_stage_moves_the_digest()
-> Result<()> {
    let alignments = two_venue_alignments(200)?;
    assert!(
        alignments
            .iter()
            .any(|alignment| alignment.order_of_other == Order::Unordered),
        "premise: some pair sits inside its combined uncertainty"
    );
    assert!(
        alignments
            .iter()
            .any(|alignment| alignment.order_of_other != Order::Unordered),
        "premise: some pair is far enough apart to order"
    );
    assert!(
        alignments
            .iter()
            .all(|alignment| alignment.venue != alignment.other_venue),
        "a tick was related to its own venue"
    );

    let again = two_venue_alignments(200)?;
    assert_eq!(alignments, again, "the alignment is not reproducible");
    assert_eq!(alignment_digest(&alignments), alignment_digest(&again));

    let tighter = two_venue_alignments(100)?;
    assert_ne!(
        alignment_digest(&alignments),
        alignment_digest(&tighter),
        "narrowing a clock's recorded uncertainty did not change the alignment digest"
    );
    assert!(
        tighter
            .iter()
            .filter(|alignment| alignment.order_of_other == Order::Unordered)
            .count()
            < alignments
                .iter()
                .filter(|alignment| alignment.order_of_other == Order::Unordered)
                .count(),
        "a tighter clock did not order any pair it could not before"
    );
    Ok(())
}

// --- the fabric's replay engine (R12-TICK-1125) ------------------------------------

fn run_digest(run: &ReplayRun) -> String {
    sha256_hex(&serde_json::to_vec(run).expect("a replay run serialises"))
}

fn replay_ticks_of(history: &RecordedHistory) -> Vec<ReplayTick> {
    history
        .ticks
        .iter()
        .map(|tick| ReplayTick {
            message: tick.message.clone(),
            knowable_at: tick.knowable_at,
        })
        .collect()
}

fn execution_model() -> ExecutionModel {
    ExecutionModel {
        latency: Duration::from_micros(1),
        fee_bps: 1.0,
        impact_bps_at_full_depth: 10.0,
        limit_ttl: Duration::from_secs(60),
        message_budget: None,
    }
}

/// R12-TICK-1125. The lake's ticks, as `qip-deepbrain`'s tick desk hands
/// them, run through `qip_simulation_engine::tick_replay::ReplaySession` with
/// one market order, twice and with each input varied.
///
/// A literal digest cannot be pinned without running it, so this asserts what
/// a mutation of the engine would break: the run is reproducible; the run
/// carries the session's own faults (the hole is a `gap` span, the order
/// really filled); two inputs one tick or one fee apart do not share a
/// digest; and on the clean first five packets the engine's rebuilt book is
/// the book `qip_orderbook::replay` builds from the same messages, an
/// independent path, so an engine that applied a message differently
/// diverges.
///
/// Mutation: not run. Intended: in `ReplaySession::step`, skip
/// `state.apply(&tick.message)?` for `OrderReduced` messages; the book
/// comparison below fires.
#[test]
fn the_fabric_replay_engine_run_over_the_recorded_session_is_reproducible_and_agrees_with_the_book_replay()
-> Result<()> {
    let session = recorded_session();
    let (decoded, _) = decode_only(&session)?;
    let store = MemoryBlobStore::new();
    into_the_lake(&store, "XNAS", &decoded, 0, 100)?;
    let history = out_of_the_lake(&store, "recorded-session")?;
    let ticks = replay_ticks_of(&history);
    let instrument = decoded[0].object_id.as_str().to_string();
    let order = ReplayOrder {
        id: "probe-buy".to_string(),
        instrument: instrument.clone(),
        side: OrderSide::Buy,
        quantity: Decimal::from_int(50),
        limit: None,
        decided_at: captured(45),
    };
    let run = ReplaySession::new(
        execution_model(),
        ticks.clone(),
        vec![order.clone()],
        Vec::new(),
    )?
    .run_to_end()?;

    // --- premises --------------------------------------------------------------
    assert_eq!(run.steps, ticks.len() as u64, "premise: every tick stepped");
    assert!(
        run.spans.iter().any(|span| span.fault == "gap"),
        "premise: the hole in the session was flagged: {:?}",
        run.spans
    );
    assert!(
        !run.unreliable_steps.is_empty(),
        "premise: some step was replayed inside an unreliable span"
    );
    assert_eq!(run.executions.len(), 1, "premise: the probe order resolved");
    assert!(
        run.executions[0].filled.is_positive(),
        "premise: the probe order filled against the replayed book: {:?}",
        run.executions[0]
    );

    // --- reproducible -----------------------------------------------------------
    let again = ReplaySession::new(
        execution_model(),
        ticks.clone(),
        vec![order.clone()],
        Vec::new(),
    )?
    .run_to_end()?;
    assert_eq!(run, again, "the engine's run is not reproducible");
    assert_eq!(run_digest(&run), run_digest(&again));

    // --- sensitive to its inputs --------------------------------------------------
    let mut shorter = ticks.clone();
    let _ = shorter.pop();
    let shorter_run =
        ReplaySession::new(execution_model(), shorter, vec![order.clone()], Vec::new())?
            .run_to_end()?;
    assert_ne!(run_digest(&run), run_digest(&shorter_run));
    let dearer = ExecutionModel {
        fee_bps: 2.0,
        ..execution_model()
    };
    let dearer_run =
        ReplaySession::new(dearer, ticks.clone(), vec![order], Vec::new())?.run_to_end()?;
    assert_ne!(run_digest(&run), run_digest(&dearer_run));
    assert_ne!(
        run.executions[0].fees, dearer_run.executions[0].fees,
        "a dearer fee left the simulated fee unchanged"
    );

    // --- agrees with the independent book replay on the clean span ------------------
    let clean: Vec<ReplayTick> = ticks
        .iter()
        .filter(|tick| tick.message.origin.sequence <= 5)
        .cloned()
        .collect();
    assert!(clean.len() >= 5, "premise: the clean span has its messages");
    let mut engine = ReplaySession::new(execution_model(), clean.clone(), Vec::new(), Vec::new())?;
    while engine.step()? {}
    let engine_book = engine
        .book(&instrument)
        .expect("the engine rebuilt the instrument's book")
        .snapshot()
        .book
        .digest();
    let messages: Vec<MarketMessage> = clean.iter().map(|tick| tick.message.clone()).collect();
    let mut direct = fresh_book();
    replay(&mut direct, &messages)?;
    assert_eq!(
        engine_book,
        direct.snapshot().book.digest(),
        "the fabric's replay engine rebuilt a different book from the same messages"
    );
    assert_ne!(
        engine_book,
        fresh_book().snapshot().book.digest(),
        "premise: the clean span left a book to compare"
    );
    Ok(())
}

// --- world and market fusion (R12-TICK-1127) ------------------------------------------

/// The instrument's replayed market: one point per released event at which
/// both sides of the touch stood, stamped with the instant it became knowable.
fn market_trace_of(events: &[MarketMessage]) -> Result<MarketTrace> {
    let mut points = Vec::new();
    for through in 1..=events.len() {
        let mut state = fresh_book();
        replay(&mut state, &events[..through])?;
        let (Some(bid), Some(ask), Some(mid), Some(spread)) = (
            state.best_bid(),
            state.best_ask(),
            state.mid(),
            state.spread(),
        ) else {
            continue;
        };
        let (b, a) = (bid.size.to_f64(), ask.size.to_f64());
        let mid = mid.to_f64();
        points.push(TracePoint {
            at: events[through - 1].capture_time,
            mid,
            spread_bps: spread.to_f64() / mid * 10_000.0,
            depth: Some(b + a),
            imbalance: Some((b - a) / (b + a)),
        });
    }
    Ok(MarketTrace {
        instrument: "AAPL".to_string(),
        venue: "XNAS".to_string(),
        points,
    })
}

/// One world event, on the session's own clock, concerning the instrument's
/// issuer: expected to move it up, on sound evidence.
fn world_graph() -> Result<KnowledgeGraph> {
    let occurred = captured(25);
    let recorded = captured(45);
    let mut graph = KnowledgeGraph::new();
    graph.add_node(
        Node::event("evt-guidance", "evt-guidance", occurred, recorded)
            .with_attribute("expected_direction", "up")
            .with_attribute("evidence_quality", "0.9"),
    )?;
    graph.add_node(Node::entity(
        "aapl-co",
        EntityKind::Company,
        "aapl-co",
        captured(0),
    ))?;
    graph.add_node(
        Node::new(
            "inst-aapl",
            NodeKind::FinancialObject,
            "inst-aapl",
            captured(0),
        )
        .with_attribute("object_id", "AAPL"),
    )?;
    graph.assert_fact(Fact::new(
        Relationship::new(
            "evt-guidance",
            "aapl-co",
            RelationshipKind::ConcernsEntity,
            1.0,
            "wire:1",
        ),
        occurred,
        recorded,
        0.9,
    )?);
    graph.assert_fact(Fact::new(
        Relationship::new(
            "aapl-co",
            "inst-aapl",
            RelationshipKind::Issues,
            1.0,
            "registry:1",
        ),
        captured(0),
        captured(0),
        1.0,
    )?);
    Ok(graph)
}

fn fusion_config() -> FusionConfig {
    FusionConfig {
        max_hops: 2,
        reaction_threshold_bps: 20.0,
        reaction_window: Duration::from_micros(500),
        expect_reaction_above: 0.15,
        weak_evidence_below: 0.4,
        max_events: 16,
    }
}

/// R12-TICK-1127. The recorded session, sealed in the lake, read back and
/// replayed, becomes the instrument's market trace and is fused with a world
/// event through `qip_kernel::fusion::fuse`.
///
/// The fused digest covers the measured reaction, including the depth ratio
/// and flow imbalance the replayed books gave, so a replay that rebuilt a
/// different book, or a lake that lost a print, moves it: the second half
/// removes the execution and the digest follows. Equal digests from the
/// in-memory replay and the lake path show the two agree before the fusion.
///
/// Mutation: not run. Intended: in `fusion::fuse`, replace
/// `depth_ratio: match (base.depth, latest.depth) { .. }` with `None`; the
/// `depth_ratio` premise fires.
#[test]
fn the_recorded_session_replayed_through_world_and_market_fusion_reproduces_a_digest_the_replay_moves()
-> Result<()> {
    let session = recorded_session();
    let direct = replay_from_the_start(&session)?;
    let (decoded, diagnostics) = decode_only(&session)?;
    let store = MemoryBlobStore::new();
    into_the_lake(&store, "XNAS", &decoded, 0, 100)?;
    let history = out_of_the_lake(&store, "recorded-session")?;
    let from_lake = with_the_book_half(sequence_the_lake(&history, diagnostics.clone()))?;

    let graph = world_graph()?;
    let as_of = captured(2_100);
    let trace = market_trace_of(&from_lake.events)?;
    assert!(
        trace.points.len() >= 5,
        "premise: the replay gave the fusion {} point(s)",
        trace.points.len()
    );
    let fused = fuse(
        &graph,
        std::slice::from_ref(&trace),
        as_of,
        &fusion_config(),
    )?;

    // --- premises: the fusion read the replayed market --------------------------------
    assert_eq!(fused.events.len(), 1, "premise: the world event was fused");
    let event = &fused.events[0];
    assert_eq!(
        event.reactions.len(),
        1,
        "premise: the instrument was watched and measured"
    );
    assert!(
        event.reactions[0].depth_ratio.is_some(),
        "premise: the replayed depth reached the measured reaction"
    );
    assert!(!fused.digest.is_empty(), "premise: a digest was computed");

    // --- reproducible, and the lake path agrees with the in-memory one -------------------
    let again = fuse(
        &graph,
        std::slice::from_ref(&trace),
        as_of,
        &fusion_config(),
    )?;
    assert_eq!(fused, again);
    let direct_trace = market_trace_of(&direct.events)?;
    let from_direct = fuse(
        &graph,
        std::slice::from_ref(&direct_trace),
        as_of,
        &fusion_config(),
    )?;
    assert_eq!(
        fused.digest, from_direct.digest,
        "the fusion of the lake's replay differs from the fusion of the in-memory replay"
    );

    // --- sensitive: the same session with its execution missing ---------------------------
    let without: Vec<MarketMessage> = from_lake
        .events
        .iter()
        .filter(|message| message.origin.sequence != 5)
        .cloned()
        .collect();
    assert!(
        without.len() < from_lake.events.len(),
        "premise: sequence 5 was in the replay"
    );
    let lossy = fuse(
        &graph,
        &[market_trace_of(&without)?],
        as_of,
        &fusion_config(),
    )?;
    assert_ne!(
        fused.digest, lossy.digest,
        "a replay missing a printed execution fused to the same digest"
    );
    Ok(())
}
