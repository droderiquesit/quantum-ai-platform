//! The reflex outbox (ADR 0100 §1, §5, §6, §8; SLICE-24): the mirror that
//! only ever offers, the writer that routes every journal entry to P2 and
//! every outcome also to P1, the recorded inputs, and the heartbeat the
//! writer beats for the pressure gauge.
//!
//! Every test here drives the real `SegmentLog` spool (the fsync-failure
//! test wraps it to inject the failure) and reads the records back through
//! `writer::records`, the same reader the drain and replay use — so what is
//! asserted is what reached disk, not what the writer meant to write.

use qip_contracts::market_event::MarketEvent;
use qip_contracts::reflex::{Decision, JournalEntry, OutcomeRecord};
use qip_contracts::replay::{
    AppliedReadings, ControlPosition, PassMarker, PressureRecord, WireRecord,
};
use qip_contracts::{BookSide, Entitlement, MarketMessage, MessageBody, Origin, Usage, VenueId};
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Decimal, Duration, EventId, ManualClock, ObjectId, Timestamp, sha256_hex};
use qip_edge::journal::{Journal, ship};
use qip_edge::pressure::{Exhaustion, JournalPressure};
use qip_edge_node::event_fabric::inputs::{Recorded, RecordedInputs};
use qip_edge_node::event_fabric::mirror::FabricMirror;
use qip_edge_node::event_fabric::pressure::{PressureGauge, SpoolPublisher, Thresholds};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::event_fabric::writer::{
    self, HandoffReceiver, HandoffSender, PassInputs, Spool, SpoolSizes, SpoolWriter,
    SpooledRecord, Step, WriterConfig,
};
use qip_events::Topic;
use qip_events::event_fabric::codec::Batch;
use qip_events::event_fabric::policy::QosClass;
use qip_observability::metrics::{Metrics, names};
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const CELL: &str = "cell-outbox";

/// Short, so an idle step costs the suite milliseconds; bounded, which is
/// the property under test, not its length.
const POLL: std::time::Duration = std::time::Duration::from_millis(5);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-outbox-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory can be created");
    dir
}

fn heartbeat_bound() -> Duration {
    Duration::from_secs(5)
}

fn telemetry() -> (Arc<Metrics>, Arc<OutboxTelemetry>) {
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    let telemetry = Arc::new(OutboxTelemetry::new(Arc::clone(&metrics)));
    (metrics, telemetry)
}

/// A gauge on a manual clock with a budget far above anything a test
/// writes, so only the heartbeat and the unwritable bit can move it.
fn gauge() -> (Arc<ManualClock>, PressureGauge, SpoolPublisher) {
    let clock = Arc::new(ManualClock::new(t(0)));
    let handed: Arc<dyn Clock> = clock.clone();
    let thresholds = Thresholds::new(
        1 << 40,
        Decimal::parse("0.5").expect("a literal fraction parses"),
        Decimal::parse("0.9").expect("a literal fraction parses"),
    )
    .expect("a large budget with 0.5 below 0.9 is valid");
    let (gauge, spool, _drain) =
        PressureGauge::new(thresholds, handed, heartbeat_bound()).expect("a positive bound");
    (clock, gauge, spool)
}

fn segment_log(dir: &PathBuf) -> SegmentLog {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(0)));
    SegmentLog::open(dir, SegmentLogConfig::new(clock)).expect("the spool opens")
}

/// A writer over a real spool at `dir`, with its gauge and clock.
struct Outbox<S: Spool> {
    writer: SpoolWriter<S>,
    gauge: PressureGauge,
    clock: Arc<ManualClock>,
}

fn open_writer<S: Spool>(spool: S, receiver: HandoffReceiver, started: Timestamp) -> Outbox<S> {
    let (clock, gauge, publisher) = gauge();
    let (_, telemetry) = telemetry();
    let config = WriterConfig::new(CELL, started, POLL, 64).expect("a valid writer config");
    let writer = SpoolWriter::open(spool, receiver, publisher, telemetry, config)
        .expect("the writer opens and commits its session");
    Outbox {
        writer,
        gauge,
        clock,
    }
}

/// Step until the channel has been idle for one poll.
fn drain<S: Spool>(writer: &mut SpoolWriter<S>) {
    for _ in 0..64 {
        if writer.step().expect("the writer beats") == Step::Idle {
            return;
        }
    }
    panic!("the writer never went idle");
}

fn filled(order: &str) -> Decision {
    Decision::Filled {
        order_id: order.to_string(),
        venue: "XLON".to_string(),
        object: "obj-OUTBOX".to_string(),
        quantity: "5".to_string(),
        price: "101.5".to_string(),
        simulated: true,
        shares: vec![("alpha".to_string(), "5".to_string())],
        side: Some(BookSide::Ask),
        quote_unit: Some("GBP".to_string()),
        fee: None,
    }
}

fn ingested() -> Decision {
    Decision::Ingested {
        feed: "test-feed".to_string(),
        decoded: 3,
        skipped: 0,
    }
}

fn refused() -> Decision {
    Decision::Refused {
        gate: "quote_budget".to_string(),
        reason: "the bucket is empty".to_string(),
    }
}

/// The six outcome kinds ADR 0100 §5's P1 row names, by `Decision::kind`.
const OUTCOME_KINDS: [&str; 6] = [
    "order_sent",
    "filled",
    "order_expired",
    "mass_cancelled",
    "crossed_internally",
    "reconciliation_break",
];

/// Two batches of decisions: every outcome kind, runs of non-outcomes
/// between them, and each batch ending on a non-outcome run so a span has
/// to close at a batch boundary as well as before an outcome.
fn two_batches() -> [Vec<Decision>; 2] {
    [
        vec![
            ingested(),
            Decision::OrderSent {
                order_id: "o-1".to_string(),
                venue: "XLON".to_string(),
                quantity: "5".to_string(),
                simulated: true,
                release_at: None,
                equalised: false,
            },
            refused(),
            Decision::HaltChanged {
                halted: false,
                reason: "released".to_string(),
            },
            filled("o-1"),
            Decision::OrderExpired {
                order_id: "o-2".to_string(),
                venue: "XLON".to_string(),
                withdrawn: "3".to_string(),
            },
            ingested(),
        ],
        vec![
            Decision::StrategyWithdrawn {
                strategy: "beta".to_string(),
            },
            Decision::MassCancelled {
                order_id: "o-3".to_string(),
                venue: "XLON".to_string(),
                withdrawn: "2".to_string(),
            },
            Decision::CrossedInternally {
                object: "obj-OUTBOX".to_string(),
                venue: "XLON".to_string(),
                quantity: "1".to_string(),
                price: "101".to_string(),
                bought: vec!["alpha".to_string()],
                sold: vec!["beta".to_string()],
            },
            refused(),
            Decision::ReconciliationBreak {
                detail: "venue holds 5, cell holds 4".to_string(),
            },
            ingested(),
            refused(),
        ],
    ]
}

/// Record both batches into a journal, ship each through a fabric mirror and
/// step the writer after each; return the journal's entries and the spool's
/// records.
fn run_two_batches() -> (Vec<JournalEntry>, Vec<SpooledRecord>, u64) {
    let dir = temp_dir("routing");
    let (sender, receiver) = writer::channel(8).expect("a positive capacity");
    let mut outbox = open_writer(segment_log(&dir), receiver, t(0));
    let mut mirror = FabricMirror::new(CELL, sender).expect("a named cell");
    let mut journal = Journal::new();
    let mut second = 0;
    for batch in two_batches() {
        for decision in batch {
            second += 1;
            journal.record(decision, t(second));
        }
        let shipped = ship(&mut journal, &mut mirror, CELL, Vec::new(), t(second))
            .expect("the writer's channel has room");
        assert!(shipped > 0, "premise: every batch ships something");
        drain(&mut outbox.writer);
    }
    let records = writer::records(outbox.writer.spool()).expect("the spool reads back");
    (journal.entries().to_vec(), records, outbox.writer.session())
}

fn on(records: &[SpooledRecord], topic: Topic) -> Vec<&SpooledRecord> {
    records.iter().filter(|r| r.event.topic == topic).collect()
}

/// FABRIC-002/003 (ADR 0100 §6): a stalled writer must never park the
/// decision thread. `Cell::flush` is the only call in the cell that may
/// block, and in fabric mode its mirror must not: a full channel is a
/// refusal returned at once, with every entry left unshipped in the journal
/// for the next flush — not a wait, and not a loss.
#[test]
fn ship_returns_without_waiting_when_the_writer_is_stalled_and_the_entries_stay_unshipped() {
    let (sender, receiver) = writer::channel(1).expect("a positive capacity");
    let mut mirror = FabricMirror::new(CELL, sender).expect("a named cell");
    let mut journal = Journal::trimmed_on_ship();
    journal.record(ingested(), t(1));

    // Premise: the channel takes one batch, and a trimming journal drops what
    // it shipped — so an entry still held below was held because it was
    // refused, not because this journal keeps everything.
    assert_eq!(
        ship(&mut journal, &mut mirror, CELL, Vec::new(), t(1)).expect("room for one"),
        1
    );
    assert_eq!(mirror.accepted(), 1);
    assert!(journal.unshipped().is_empty());
    assert_eq!(journal.retained(), 0, "premise: a shipped entry is trimmed");

    // The writer is stalled: nothing reads the channel, which is full.
    journal.record(filled("o-1"), t(2));
    journal.record(refused(), t(3));
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = ship(&mut journal, &mut mirror, CELL, Vec::new(), t(3));
        let _ = done.send((result, journal, mirror));
    });
    let (result, mut journal, mut mirror) = finished
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("ship returned within the bound instead of waiting on a stalled writer");

    let refusal = result.expect_err("a full channel is refused, not waited on");
    assert!(
        refusal.message().contains("handoff channel is full"),
        "{}",
        refusal.message()
    );
    assert_eq!(mirror.refused_full(), 1);
    assert_eq!(
        journal.unshipped().len(),
        2,
        "both refused entries stay unshipped"
    );
    assert_eq!(journal.retained(), 2, "and neither was trimmed");

    // Once the writer takes the first batch, the next flush offers the same
    // entries again, chained onto what already went.
    let dir = temp_dir("stalled");
    let mut outbox = open_writer(segment_log(&dir), receiver, t(0));
    drain(&mut outbox.writer);
    assert_eq!(
        ship(&mut journal, &mut mirror, CELL, Vec::new(), t(4)).expect("room again"),
        2
    );
    drain(&mut outbox.writer);
    let records = writer::records(outbox.writer.spool()).expect("the spool reads back");
    let sequences: Vec<u64> = on(&records, Topic::ReflexJournalRecorded)
        .iter()
        .map(|r| {
            writer::journal_entry(&r.event)
                .expect("a journal entry")
                .sequence
        })
        .collect();
    assert_eq!(
        sequences,
        vec![0, 1, 2],
        "nothing was lost across the refusal"
    );
    assert!(outbox.writer.failure().is_none());
}

/// ADR 0100 §5: P2 carries every journal entry and may be shed; P1 carries
/// every outcome and may not. An outcome that reached only P2 is a fill the
/// ledger can lose under overload. Each P1 copy carries the entry's sequence
/// and digest so it can be placed on the chain it came from.
#[test]
fn every_entry_reaches_p2_and_every_outcome_also_reaches_p1_with_its_journal_digest() {
    let (entries, records, session) = run_two_batches();

    // Which entries are outcomes is taken from ADR 0100 §5's list, written
    // out in this file, and never from the classifier under test: a test
    // that asked `is_outcome` which entries needed a P1 copy would agree
    // with any classifier.
    let is_listed = |e: &&JournalEntry| OUTCOME_KINDS.contains(&e.decision.kind());
    // Premise: the journal holds every outcome kind and some that are not.
    let kinds: BTreeSet<&str> = entries
        .iter()
        .filter(is_listed)
        .map(|e| e.decision.kind())
        .collect();
    assert_eq!(kinds, OUTCOME_KINDS.into_iter().collect::<BTreeSet<_>>());
    assert!(entries.iter().any(|e| !is_listed(&e)));

    // Every entry on P2, in order, byte-identical, on the P2 stream.
    let p2 = on(&records, Topic::ReflexJournalRecorded);
    assert!(p2.iter().all(|r| r.stream == QosClass::P2MarketJournal));
    let on_p2: Vec<JournalEntry> = p2
        .iter()
        .map(|r| writer::journal_entry(&r.event).expect("a journal entry"))
        .collect();
    assert_eq!(on_p2, entries);

    // Every outcome, and only outcomes, on P1 with its digest.
    let outcomes: Vec<OutcomeRecord> = on(&records, Topic::ReflexOutcomeRecorded)
        .iter()
        .map(|r| {
            assert_eq!(r.stream, QosClass::P1Outcomes);
            writer::outcome(&r.event).expect("an outcome record")
        })
        .collect();
    let expected: Vec<&JournalEntry> = entries.iter().filter(is_listed).collect();
    for entry in &expected {
        let outcome = outcomes
            .iter()
            .find(|o| o.journal_sequence == entry.sequence)
            .unwrap_or_else(|| {
                panic!(
                    "the {} at {} has no P1 copy",
                    entry.decision.kind(),
                    entry.sequence
                )
            });
        assert_eq!(outcome.journal_digest, entry.digest);
        assert_eq!(&outcome.entry, *entry);
        assert_eq!(outcome.cell, CELL);
        assert_eq!(outcome.session, session);
    }
    assert_eq!(
        outcomes.len(),
        expected.len(),
        "a non-outcome entry was copied to P1"
    );
}

/// Red-team M12: P1 carries outcomes and `ChainSpan`s, not every entry, and
/// must still be chain-verifiable on its own. Walked here as a consumer of
/// P1 alone would: each outcome's entry must chain onto the tail the record
/// before it left, and each span must start where the last record ended. A
/// run of non-outcomes with no span is a hole P1 cannot tell from a loss.
#[test]
fn p1_stays_chain_verifiable_without_traces_because_chain_spans_cover_every_non_outcome_run() {
    let (entries, records, _) = run_two_batches();
    let p1: Vec<&SpooledRecord> = records
        .iter()
        .filter(|r| r.stream == QosClass::P1Outcomes)
        .collect();

    // Premise, read off the journal rather than off what the writer made of
    // it: the chain has non-outcome runs both before outcomes and at the end
    // of each shipped batch, so the walk below has spans to need, and ends on
    // a non-outcome so the last one is needed too.
    let listed = |e: &JournalEntry| OUTCOME_KINDS.contains(&e.decision.kind());
    let runs_before_an_outcome = entries
        .windows(2)
        .filter(|pair| !listed(&pair[0]) && listed(&pair[1]))
        .count();
    assert!(
        runs_before_an_outcome >= 2,
        "premise: runs between outcomes"
    );
    let [first_batch, _] = two_batches();
    assert!(
        !listed(&entries[first_batch.len() - 1]),
        "premise: batch one ends on a run"
    );
    let last = entries.last().expect("premise: entries were recorded");
    assert!(!listed(last), "premise: the chain ends on a run");

    let mut tail = Journal::GENESIS.to_string();
    let mut next: u64 = 0;
    for record in p1 {
        match record.event.topic {
            Topic::ReflexOutcomeRecorded => {
                let outcome = writer::outcome(&record.event).expect("an outcome");
                assert_eq!(
                    outcome.journal_sequence, next,
                    "P1 skipped from {next} to {}: a run was not spanned",
                    outcome.journal_sequence
                );
                let recomputed = outcome
                    .entry
                    .expected_digest(&tail)
                    .expect("a v2 entry recomputes");
                assert_eq!(
                    recomputed, outcome.journal_digest,
                    "the outcome at {next} does not chain onto the P1 tail before it"
                );
                tail = outcome.journal_digest;
                next = outcome.journal_sequence + 1;
            }
            Topic::ReflexChainSpan => {
                let span = writer::chain_span(&record.event).expect("a span");
                assert_eq!(span.first_seq, next, "a span must start where P1 left off");
                assert!(span.last_seq >= span.first_seq);
                tail = span.tail_digest;
                next = span.last_seq + 1;
            }
            other => panic!("unexpected {other} on P1"),
        }
    }
    assert_eq!(next, last.sequence + 1, "P1 covers the whole chain");
    assert_eq!(tail, last.digest, "and ends on the journal's own tail");
}

/// Red-team M1/F6: the existing store mirror keys a session by its start
/// second, so a crash loop restarting twice inside one second writes two
/// sessions under one key and reissues every event id. The outbox's session
/// is a counter committed to the spool before any record.
#[test]
fn two_starts_in_one_second_are_two_sessions_with_distinct_event_ids() {
    let dir = temp_dir("sessions");
    let first_start = t(10).saturating_add(Duration::from_millis(100));
    let second_start = t(10).saturating_add(Duration::from_millis(700));
    assert_eq!(
        first_start.as_secs(),
        second_start.as_secs(),
        "premise: both starts fall in the same second"
    );

    let mut sessions = Vec::new();
    let mut last_records = Vec::new();
    for started in [first_start, second_start] {
        let (sender, receiver) = writer::channel(4).expect("a positive capacity");
        let mut outbox = open_writer(segment_log(&dir), receiver, started);
        let mut mirror = FabricMirror::new(CELL, sender).expect("a named cell");
        // The same decision at the same journal sequence in both sessions,
        // which is exactly what a restarted cell produces.
        let mut journal = Journal::new();
        journal.record(ingested(), t(10));
        ship(&mut journal, &mut mirror, CELL, Vec::new(), t(10)).expect("room");
        drain(&mut outbox.writer);
        sessions.push(outbox.writer.session());
        last_records = writer::records(outbox.writer.spool()).expect("reads back");
        // Dropping the writer closes the spool, as a process exit would.
    }

    assert_ne!(sessions[0], sessions[1], "two starts are two sessions");
    assert!(
        sessions[1] > sessions[0],
        "and the counter only moves forward"
    );
    let ids: Vec<&EventId> = on(&last_records, Topic::ReflexJournalRecorded)
        .iter()
        .map(|r| &r.event.event_id)
        .collect();
    assert_eq!(ids.len(), 2, "premise: both sessions wrote sequence 0");
    assert_ne!(ids[0], ids[1], "the same entry in two sessions has two ids");
}

fn marker(pass: u64) -> PassMarker {
    PassMarker {
        cell: CELL.to_string(),
        session: "1".to_string(),
        pass,
        now_ns: t(20).as_nanos() + pass as i64,
        tape_digest: sha256_hex(b"tape"),
        tape_from: pass,
        tape_to: pass,
        control: Vec::new(),
        readings: AppliedReadings::default(),
        config_digest: sha256_hex(b"config"),
        plan_digest: sha256_hex(b"plan"),
        gateway_seed: 7,
        binary_version: "test".to_string(),
    }
}

/// Red-team M8: an inputs backlog that overflows and drops silently leaves
/// replay re-driving the passes either side and calling the window
/// reproduced. The dropped passes must be declared, on P2 where they would
/// have been and on P1 where the declaration cannot itself be shed.
#[test]
fn an_input_backlog_overflow_writes_a_gap_that_marks_the_window_unreproducible() {
    let (sender, receiver) = writer::channel(1).expect("a positive capacity");
    // A bound that holds exactly two passes' inputs.
    let one = serde_json::to_vec(&marker(10)).expect("serialises").len() as u64;
    let thresholds = Thresholds::new(
        4 * one + 3,
        Decimal::parse("0.25").expect("parses"),
        Decimal::parse("0.5").expect("parses"),
    )
    .expect("valid lines");
    assert!(thresholds.exhaust_bytes() >= 2 * one && thresholds.exhaust_bytes() < 3 * one);
    let (metrics, telemetry) = telemetry();
    let mut inputs = RecordedInputs::new(sender, thresholds, telemetry);

    let record = |inputs: &mut RecordedInputs, pass| {
        inputs
            .record(PassInputs {
                marker: marker(pass),
                applied: Vec::new(),
            })
            .expect("the writer is alive")
    };
    // The writer is stalled: one pass fills the channel, two fill the
    // backlog, and the next two overflow.
    assert_eq!(record(&mut inputs, 10), Recorded::HandedOff);
    assert_eq!(record(&mut inputs, 11), Recorded::Queued);
    assert_eq!(record(&mut inputs, 12), Recorded::Queued);
    assert_eq!(record(&mut inputs, 13), Recorded::Dropped);
    assert_eq!(record(&mut inputs, 14), Recorded::Dropped);

    // The writer recovers; the backlog drains in order, gap included.
    let dir = temp_dir("overflow");
    let mut outbox = open_writer(segment_log(&dir), receiver, t(0));
    for _ in 0..8 {
        outbox.writer.step().expect("beats");
        inputs.pump().expect("the writer is alive");
    }
    assert_eq!(inputs.backlog_len(), 0, "premise: the backlog drained");
    assert_eq!(record(&mut inputs, 15), Recorded::HandedOff);
    drain(&mut outbox.writer);

    let records = writer::records(outbox.writer.spool()).expect("reads back");
    let marked: Vec<u64> = on(&records, Topic::ReflexPassMarked)
        .iter()
        .map(|r| writer::pass_marker(&r.event).expect("a marker").pass)
        .collect();
    assert_eq!(marked, vec![10, 11, 12, 15]);

    let gaps: Vec<(QosClass, u64, u64, String)> = on(&records, Topic::EventFabricGap)
        .iter()
        .map(|r| {
            let gap = writer::gap(&r.event).expect("a gap");
            (r.stream, gap.from_seq, gap.to_seq, gap.stream)
        })
        .collect();
    let streams: BTreeSet<&str> = gaps.iter().map(|g| g.0.as_str()).collect();
    assert_eq!(gaps.len(), 2, "one declaration per lane");
    assert_eq!(
        streams,
        [
            QosClass::P1Outcomes.as_str(),
            QosClass::P2MarketJournal.as_str()
        ]
        .into_iter()
        .collect(),
        "the gap is declared on both lanes"
    );
    for (_, from, to, stream) in &gaps {
        assert_eq!((*from, *to), (13, 14));
        assert_eq!(stream, Topic::ReflexPassMarked.name());
    }
    // Every pass is either recorded or inside a declared window.
    for pass in 10..=15 {
        let covered = marked.contains(&pass) || gaps.iter().any(|g| g.1 <= pass && pass <= g.2);
        assert!(
            covered,
            "pass {pass} is neither recorded nor declared missing"
        );
    }
    // And on P2 the gap sits where the missing passes would have been.
    let p2_order: Vec<String> = records
        .iter()
        .filter(|r| r.stream == QosClass::P2MarketJournal)
        .map(|r| r.event.topic.name().to_string())
        .collect();
    let gap_at = p2_order
        .iter()
        .position(|t| t == Topic::EventFabricGap.name())
        .expect("a P2 gap");
    assert_eq!(gap_at, 3, "after passes 10, 11 and 12 and before 15");
    // Counted once per window, beside the records that declare it.
    assert_eq!(
        metrics
            .snapshot()
            .counter_total(names::EDGE_EVENT_FABRIC_INPUT_GAPS),
        1,
        "one window, one gap"
    );
}

fn market_event() -> MarketEvent {
    let at = t(30);
    let payload = MarketMessage::new(
        ObjectId::from_string("obj-OUTBOX"),
        Origin::new(VenueId::new("XLON"), "test-tape", 0, 7),
        MessageBody::LevelSet {
            side: BookSide::Bid,
            price: Decimal::parse("101.5").expect("parses"),
            quantity: Decimal::parse("12").expect("parses"),
            order_count: None,
        },
        at,
        at,
    );
    let hash = MarketEvent::hash_payload(&payload).expect("hashes");
    MarketEvent::new(
        payload,
        at,
        at,
        at,
        Duration::ZERO,
        hash,
        Entitlement::Granted {
            dataset: "outbox-test".to_string(),
            usage: Usage::Trade,
            expires_at: t(1_000_000),
        },
    )
    .expect("a licensed event")
}

/// Red-team B5: replay reads back the readings a pass took — journal
/// pressure, the halt flag, the region wire — instead of guessing "normal".
/// A writer that dropped or rewrote them would make a halted pass replay as
/// a healthy one. The marker must come off the spool as the bytes it went in
/// as, and the tape events beside it likewise.
#[test]
fn a_pass_marker_and_its_readings_survive_the_spool_byte_for_byte() {
    let mut original = marker(40);
    original.control = vec![ControlPosition::new(
        "control.grants",
        0,
        9,
        EventId::from_string("EVT-OUTBOX-CONTROL"),
    )];
    original.readings = AppliedReadings {
        journal_pressure: Some(PressureRecord::new("narrow", "half")),
        halt_flag: Some(WireRecord::new("engaged", "operator drill")),
        region_wire: Some(WireRecord::new("dark", "eu-west")),
    };
    assert_ne!(
        original.readings,
        AppliedReadings::default(),
        "premise: every reading is present"
    );
    let applied = market_event();

    let (sender, receiver) = writer::channel(2).expect("a positive capacity");
    let (_, telemetry) = telemetry();
    let thresholds = Thresholds::new(
        1 << 30,
        Decimal::parse("0.5").expect("parses"),
        Decimal::parse("0.9").expect("parses"),
    )
    .expect("valid");
    let mut inputs = RecordedInputs::new(sender, thresholds, telemetry);
    assert_eq!(
        inputs
            .record(PassInputs {
                marker: original.clone(),
                applied: vec![applied.clone()],
            })
            .expect("alive"),
        Recorded::HandedOff
    );
    let dir = temp_dir("marker");
    let mut outbox = open_writer(segment_log(&dir), receiver, t(0));
    drain(&mut outbox.writer);

    let records = writer::records(outbox.writer.spool()).expect("reads back");
    let marked = on(&records, Topic::ReflexPassMarked);
    assert_eq!(marked.len(), 1);
    let event = &marked[0].event;
    let expected = canonical_json(&serde_json::to_value(&original).expect("serialises"));
    assert_eq!(
        canonical_json(&event.payload),
        expected,
        "the marker's bytes on the spool differ from the bytes the pass built"
    );
    assert_eq!(event.payload_hash, sha256_hex(expected.as_bytes()));
    assert_eq!(writer::pass_marker(event).expect("a marker"), original);

    let events = on(&records, Topic::MarketEventApplied);
    assert_eq!(events.len(), 1);
    assert_eq!(
        canonical_json(&events[0].event.payload),
        canonical_json(&serde_json::to_value(&applied).expect("serialises"))
    );
    assert_eq!(
        writer::market_event(&events[0].event).expect("an event"),
        applied
    );
}

/// A spool whose appends fail on demand: the fsync failure a real disk
/// produces rarely and at the worst moment.
#[derive(Debug)]
struct FailingSpool {
    inner: SegmentLog,
    fail: Arc<AtomicBool>,
}

impl Spool for FailingSpool {
    fn append(&mut self, batch: &Batch) -> Result<u64> {
        if self.fail.load(Ordering::Acquire) {
            return Err(Error::io("injected: fsync reported an I/O error"));
        }
        Spool::append(&mut self.inner, batch)
    }

    fn sizes(&self) -> Result<SpoolSizes> {
        self.inner.sizes()
    }

    fn manifest_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Spool::manifest_get(&self.inner, key)
    }

    fn manifest_put(&mut self, key: &str, bytes: &[u8]) -> Result<()> {
        Spool::manifest_put(&mut self.inner, key, bytes)
    }
}

/// The gauge's heartbeat is the writer's liveness (SLICE-54). A writer that
/// beat only when it wrote would read stale — and halt the cell — on every
/// quiet stretch; and a writer whose fsync failed must say so rather than
/// keep beating over a spool that no longer takes records.
#[test]
fn the_writer_beats_the_gauge_heartbeat_while_idle_and_marks_it_unwritable_on_an_fsync_error() {
    let dir = temp_dir("heartbeat");
    let fail = Arc::new(AtomicBool::new(false));
    let spool = FailingSpool {
        inner: segment_log(&dir),
        fail: Arc::clone(&fail),
    };
    let (sender, receiver): (HandoffSender, HandoffReceiver) =
        writer::channel(4).expect("a positive capacity");
    let mut outbox = open_writer(spool, receiver, t(0));

    // Premise: an unbeaten gauge reads stale.
    assert_eq!(
        outbox.gauge.read().pressure,
        JournalPressure::Exhausted(Exhaustion::Stale)
    );

    // Idle: nothing arrives, and the writer still beats.
    assert_eq!(outbox.writer.step().expect("beats"), Step::Idle);
    let reading = outbox.gauge.read();
    assert_eq!(reading.pressure, JournalPressure::Normal);
    assert_eq!(reading.generation, 1);

    // Past the bound the last beat is stale (the premise for the next step)…
    outbox.clock.advance(heartbeat_bound() + heartbeat_bound());
    assert_eq!(
        outbox.gauge.read().pressure,
        JournalPressure::Exhausted(Exhaustion::Stale)
    );
    // …and another idle loop makes it fresh again, having written nothing.
    assert_eq!(outbox.writer.step().expect("beats"), Step::Idle);
    assert_eq!(outbox.gauge.read().pressure, JournalPressure::Normal);
    assert_eq!(outbox.gauge.read().generation, 2);

    // An fsync failure: the gauge reads unwritable, and nothing is claimed
    // written.
    fail.store(true, Ordering::Release);
    let mut mirror = FabricMirror::new(CELL, sender).expect("a named cell");
    let mut journal = Journal::new();
    journal.record(filled("o-9"), t(1));
    ship(&mut journal, &mut mirror, CELL, Vec::new(), t(1)).expect("the channel takes it");
    assert_eq!(outbox.writer.step().expect("beats"), Step::Failed);
    assert_eq!(
        outbox.gauge.read().pressure,
        JournalPressure::Exhausted(Exhaustion::Unwritable)
    );
    assert!(outbox.writer.failure().is_some());

    // It keeps beating and keeps reading unwritable; it does not retry a
    // spool that refused, even once the fault clears.
    fail.store(false, Ordering::Release);
    assert_eq!(outbox.writer.step().expect("beats"), Step::Failed);
    let reading = outbox.gauge.read();
    assert_eq!(
        reading.pressure,
        JournalPressure::Exhausted(Exhaustion::Unwritable)
    );
    assert_eq!(reading.generation, 4);
    assert!(
        writer::records(&outbox.writer.spool().inner)
            .expect("reads back")
            .is_empty(),
        "nothing reached the spool"
    );
}
