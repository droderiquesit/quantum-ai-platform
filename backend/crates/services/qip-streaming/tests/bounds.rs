//! Every bound this crate configures, and proof that each one fires.
//!
//! The defect class these guard against is a control that is configured,
//! documented and unreachable: `MaxExpectedShortfall` shipped in every default
//! limit set of the risk engine and could never trigger, because the state it
//! read was always empty. A bound with no test that observes it breaching is
//! that same shape waiting to happen, so each test here drives the crate past
//! the bound and asserts the consequence — and, where a bound is a threshold
//! rather than a refusal, asserts its own premise by showing the same input
//! under a wider bound behaving differently.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, hot_tick_priced, warm_note};
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_events::EventFilter;
use qip_sequencing::{GapReason, ReorderPolicy, SequenceEvent};
use qip_streaming::durable::DurableLogTransport;
use qip_streaming::ports::Publisher;
use qip_streaming::{
    ProcessingPolicy, RejectionReason, StreamEnvelope, StreamObservation, StreamProcessor,
};

fn policy(dedup_capacity: usize) -> ProcessingPolicy {
    ProcessingPolicy {
        dedup_capacity,
        reorder: ReorderPolicy::new(64, Duration::from_millis(50)),
        ..ProcessingPolicy::default()
    }
}

#[test]
fn the_dedup_window_forgets_its_oldest_key_once_the_capacity_is_reached() -> Result<()> {
    // Two keys remembered, which is the whole reason the replay window is
    // documented as the second line of defence rather than a nicety: past this
    // many distinct events a redelivery is no longer caught here.
    let mut processor = StreamProcessor::new(policy(2))?;

    let accepted = processor.admit(
        vec![
            warm_note("A", at(0), at(10))?,
            warm_note("B", at(0), at(11))?,
        ],
        at(20),
    );
    assert_eq!(accepted.accepted.len(), 2);

    // The premise: while A is still remembered, a second delivery of it is
    // refused. Without this the test below would pass against a window that
    // never remembered anything at all.
    let while_remembered = processor.admit(vec![warm_note("A", at(0), at(12))?], at(20));
    assert_eq!(
        while_remembered
            .rejections_because(RejectionReason::Duplicate)
            .len(),
        1,
        "a key inside the window must still be caught"
    );

    // A third distinct key evicts A, which is the bound firing.
    let evicting = processor.admit(vec![warm_note("C", at(0), at(13))?], at(20));
    assert_eq!(evicting.accepted.len(), 1);

    // B is younger than A and must survive the same eviction: the window drops
    // its oldest key, it is not simply cleared when it fills.
    let still_remembered = processor.admit(vec![warm_note("B", at(0), at(14))?], at(20));
    assert_eq!(
        still_remembered
            .rejections_because(RejectionReason::Duplicate)
            .len(),
        1,
        "only the oldest key is evicted"
    );

    let after_eviction = processor.admit(vec![warm_note("A", at(0), at(15))?], at(20));
    assert_eq!(
        after_eviction.accepted.len(),
        1,
        "past the capacity the oldest key is forgotten, and this is the observable proof of it: a \
         redelivery beyond the window is not caught here, which is why the replay window is the \
         second line of defence"
    );
    Ok(())
}

#[test]
fn a_dedup_capacity_of_zero_is_refused_rather_than_widened_to_one() {
    // Refuse, never clamp. A window silently widened from the configured zero
    // to one is a caller bug that survives: the operator reads the policy they
    // wrote, and the process behaves as though they had written something else.
    // `qip_market_ingestion::DedupWindow::new` and `EventBus::dedup_capacity`
    // both already refuse this value; this is the third window agreeing.
    let refused = StreamProcessor::new(policy(0));
    assert!(
        refused.is_err(),
        "a dedup window remembering nothing must be refused at construction"
    );

    // And the gate admits a good value, which is what separates a working
    // refusal from one that refuses everything.
    assert!(StreamProcessor::new(policy(1)).is_ok());
}

#[test]
fn the_reorder_buffer_bound_abandons_the_gap_and_a_wider_bound_does_not() -> Result<()> {
    let run = |max_buffered: usize| -> Result<Vec<SequenceEvent>> {
        let mut processor = StreamProcessor::new(ProcessingPolicy {
            reorder: ReorderPolicy::new(max_buffered, Duration::from_millis(50)),
            ..ProcessingPolicy::default()
        })?;
        processor.admit(vec![hot_tick(1, at(0), at(10))?], at(20));
        // Three messages held behind the hole at 2.
        let held = processor.admit(
            vec![
                hot_tick(3, at(0), at(11))?,
                hot_tick(4, at(0), at(12))?,
                hot_tick(5, at(0), at(13))?,
            ],
            at(20),
        );
        Ok(held.sequence_events)
    };

    let narrow = run(2)?;
    assert!(
        narrow.iter().any(|event| matches!(
            event,
            SequenceEvent::GapOpened {
                missing_from: 2,
                missing_to: 2,
                ..
            }
        )),
        "the premise: a hole must actually have opened: {narrow:?}"
    );
    assert!(
        narrow.iter().any(|event| matches!(
            event,
            SequenceEvent::GapAbandoned {
                reason: GapReason::BufferFull,
                ..
            }
        )),
        "holding more than the bound must abandon the gap rather than grow: {narrow:?}"
    );

    let wide = run(8)?;
    assert!(
        wide.iter().any(|event| matches!(
            event,
            SequenceEvent::GapOpened {
                missing_from: 2,
                ..
            }
        )),
        "{wide:?}"
    );
    assert!(
        !wide
            .iter()
            .any(|event| matches!(event, SequenceEvent::GapAbandoned { .. })),
        "the same three messages under a wider bound are held, so the abandonment above is the \
         bound firing and not the messages: {wide:?}"
    );
    Ok(())
}

#[test]
fn a_duplicate_sharing_a_held_sequence_does_not_evict_the_envelope_being_held() -> Result<()> {
    let mut processor = StreamProcessor::new(policy(65_536))?;
    processor.admit(vec![hot_tick(1, at(0), at(10))?], at(20));

    // A redundant line re-delivers packet 5 with a different fact in it, after
    // packet 7 has already arrived. The two carry different payloads, so the
    // idempotency window does not catch them and the sequencer's own duplicate
    // rule is what fires — which is the path being tested.
    let held = hot_tick_priced("H", 5, 500, at(0), at(11))?;
    let redelivered = hot_tick_priced("R", 5, 501, at(0), at(13))?;
    let outcome = processor.admit(
        vec![
            held.clone(),
            hot_tick_priced("X", 7, 700, at(0), at(12))?,
            redelivered,
        ],
        at(20),
    );
    assert!(
        outcome
            .sequence_events
            .iter()
            .any(|event| matches!(event, SequenceEvent::Duplicate { sequence: 5, .. })),
        "the premise: the tracker must have called the second copy a duplicate: {:?}",
        outcome.sequence_events
    );
    assert!(outcome.accepted.is_empty(), "nothing past the hole yet");

    // The hole fills. The envelope the tracker was holding must come back, and
    // it must come back as itself: if the wrong copy was dropped from the
    // coordinator's pending map, the tracker releases a carrier nobody can
    // match and the carrier's `Reset` body is reported to consumers as an
    // order to resynchronise a stream that never lost anything.
    let filled = processor.admit(
        vec![
            hot_tick(2, at(0), at(14))?,
            hot_tick(3, at(0), at(15))?,
            hot_tick(4, at(0), at(16))?,
        ],
        at(20),
    );
    let released: Vec<&str> = filled
        .accepted
        .iter()
        .map(|envelope| envelope.event_id().as_str())
        .collect();
    assert!(
        released.contains(&held.event_id().as_str()),
        "the held envelope must be released once the hole fills, not dropped: {released:?}"
    );
    assert!(
        !filled.observations.iter().any(|observation| matches!(
            observation,
            StreamObservation::StreamResynchronised { .. }
        )),
        "no hole was abandoned, so no consumer may be told to resynchronise: {:?}",
        filled.observations
    );
    Ok(())
}

#[test]
fn a_replay_as_of_an_instant_omits_a_record_that_was_not_yet_knowable() -> Result<()> {
    let mut transport = DurableLogTransport::in_memory("durable");
    // A fact true at t=0 that the platform only learned at t=1000.
    let late = warm_note("L", at(0), at(1_000))?;
    transport.publish(late.clone(), at(1_000))?;

    // The premise: the record is in the log and a replay that is allowed to see
    // it does see it. Without this the assertion below passes on an empty log.
    let afterwards = transport.replay(&EventFilter::new().as_of(at(2_000)))?;
    assert_eq!(
        afterwards
            .iter()
            .map(|envelope| envelope.event_id().as_str())
            .collect::<Vec<_>>(),
        vec![late.event_id().as_str()],
        "a replay after the ingest instant must return the record"
    );

    let before_it_was_knowable = transport.replay(&EventFilter::new().as_of(at(500)))?;
    assert!(
        before_it_was_knowable.is_empty(),
        "a record ingested at t=1000 was not knowable at t=500; serving it is the point-in-time \
         leak that makes a backtest profitable and a live run not: {:?}",
        before_it_was_knowable
            .iter()
            .map(StreamEnvelope::ingest_timestamp)
            .collect::<Vec<Timestamp>>()
    );
    Ok(())
}
