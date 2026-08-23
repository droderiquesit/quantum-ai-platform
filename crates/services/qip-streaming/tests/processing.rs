//! The pipeline between arriving and being trusted.
//!
//! Gap detection is asserted here as *composed* behaviour: the events the
//! outcome carries are `qip_sequencing::SequenceEvent`s produced by the tracker
//! itself, and the watermark is the tracker's. Nothing in `qip-streaming`
//! decides what a hole is.

#![allow(clippy::panic_in_result_fn)]

mod common;

use common::{at, hot_tick, hot_tick_labelled, to_wire, unsequenced_tick, warm_note};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_sequencing::{GapReason, ReorderPolicy, SequenceEvent};
use qip_streaming::{
    ProcessingPolicy, RejectionReason, StreamEnvelope, StreamObservation, StreamProcessor,
};

fn processor() -> StreamProcessor {
    StreamProcessor::new(ProcessingPolicy {
        reorder: ReorderPolicy::new(64, Duration::from_millis(50)),
        ..ProcessingPolicy::default()
    })
}

fn sequences(envelopes: &[StreamEnvelope]) -> Vec<u64> {
    envelopes
        .iter()
        .filter_map(StreamEnvelope::sequence_number)
        .collect()
}

fn watermark_of(outcome: &qip_streaming::BatchOutcome) -> Option<u64> {
    outcome.watermarks.first().map(|w| w.position)
}

#[test]
fn the_same_event_delivered_twice_is_processed_once() -> Result<()> {
    let mut processor = processor();
    // Two deliveries of one fact under two event ids — a reconnecting feed
    // replaying its buffer, which is the case the idempotency key exists for.
    let first = hot_tick_labelled("A", 1, at(0), at(10))?;
    let second = hot_tick_labelled("B", 1, at(0), at(11))?;
    assert_ne!(first.event_id(), second.event_id());
    assert_eq!(
        first.idempotency_key(),
        second.idempotency_key(),
        "the fixture must actually be one fact delivered twice"
    );

    let outcome = processor.admit(vec![first.clone(), second], at(20));
    assert_eq!(sequences(&outcome.accepted), vec![1]);
    assert_eq!(
        outcome.accepted.first().map(StreamEnvelope::event_id),
        Some(first.event_id())
    );

    let duplicates = outcome.rejections_because(RejectionReason::Duplicate);
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates.first().map(|r| r.index), Some(1));
    assert!(
        duplicates
            .first()
            .is_some_and(|r| r.detail.contains("already been processed")),
        "the rejection must name what was already seen"
    );
    assert_eq!(processor.stats().duplicates, 1);
    Ok(())
}

#[test]
fn a_duplicate_is_still_suppressed_across_separate_batches() -> Result<()> {
    let mut processor = processor();
    let first = processor.admit(vec![hot_tick_labelled("A", 1, at(0), at(10))?], at(20));
    assert_eq!(first.accepted.len(), 1);

    let again = processor.admit(vec![hot_tick_labelled("B", 1, at(0), at(11))?], at(21));
    assert!(again.accepted.is_empty());
    assert_eq!(
        again.rejections_because(RejectionReason::Duplicate).len(),
        1
    );
    Ok(())
}

#[test]
fn a_gap_stops_the_watermark_and_the_hole_filling_releases_what_was_held() -> Result<()> {
    let mut processor = processor();

    let start = processor.admit(
        vec![hot_tick(1, at(0), at(10))?, hot_tick(2, at(0), at(11))?],
        at(20),
    );
    assert_eq!(sequences(&start.accepted), vec![1, 2]);
    assert_eq!(watermark_of(&start), Some(2));

    // Four arrives with three still missing.
    let held = processor.admit(vec![hot_tick(4, at(0), at(12))?], at(20));
    assert!(
        held.accepted.is_empty(),
        "nothing past a hole may be released"
    );
    assert_eq!(
        watermark_of(&held),
        Some(2),
        "a watermark past a hole is a promise that was not kept"
    );
    assert!(
        held.sequence_events.iter().any(|event| matches!(
            event,
            SequenceEvent::GapOpened {
                missing_from: 3,
                missing_to: 3,
                ..
            }
        )),
        "the gap must be reported by the tracker, not inferred here: {:?}",
        held.sequence_events
    );

    // Three arrives and unblocks four.
    let filled = processor.admit(vec![hot_tick(3, at(0), at(13))?], at(20));
    assert_eq!(sequences(&filled.accepted), vec![3, 4]);
    assert_eq!(watermark_of(&filled), Some(4));
    assert!(
        filled.sequence_events.iter().any(|event| matches!(
            event,
            SequenceEvent::GapFilled {
                recovered_through: 4,
                ..
            }
        )),
        "{:?}",
        filled.sequence_events
    );
    Ok(())
}

#[test]
fn a_hole_that_will_not_fill_resynchronises_and_only_then_moves_the_watermark() -> Result<()> {
    let mut processor = processor();
    processor.admit(vec![hot_tick(1, at(0), at(10))?], at(10));
    let held = processor.admit(vec![hot_tick(3, at(0), at(11))?], at(11));
    assert!(held.accepted.is_empty());
    assert_eq!(watermark_of(&held), Some(1));

    // The deadline passes with nothing arriving to carry it — a stream going
    // silent right after a hole is exactly what this exists for.
    let expired = processor.poll(at(11).saturating_add(Duration::from_millis(60)));
    assert!(
        expired.sequence_events.iter().any(|event| matches!(
            event,
            SequenceEvent::GapAbandoned {
                reason: GapReason::Deadline,
                ..
            }
        )),
        "{:?}",
        expired.sequence_events
    );
    assert!(
        expired.observations.iter().any(|observation| matches!(
            observation,
            StreamObservation::StreamResynchronised { .. }
        )),
        "consumers must be told to resynchronise before the watermark moves"
    );
    assert_eq!(sequences(&expired.accepted), vec![3]);
    assert_eq!(watermark_of(&expired), Some(3));
    Ok(())
}

#[test]
fn a_malformed_event_is_rejected_with_a_named_reason_and_does_not_abort_the_batch() -> Result<()> {
    let mut processor = processor();

    let tampered = {
        let mut wire = to_wire(&hot_tick(9, at(0), at(14))?)?;
        let payload = wire
            .get_mut("event")
            .and_then(|event| event.get_mut("payload"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| Error::invalid("no payload object"))?;
        payload.insert("price".into(), serde_json::json!("1.0"));
        wire
    };
    let misrouted = {
        let mut wire = to_wire(&warm_note("M", at(0), at(15))?)?;
        if let Some(object) = wire.as_object_mut() {
            object.insert("routing_class".into(), serde_json::json!("hot"));
        }
        wire
    };

    let frames = vec![
        to_wire(&hot_tick(1, at(0), at(10))?)?,
        serde_json::json!({"this is not": "an envelope"}),
        to_wire(&hot_tick(2, at(0), at(11))?)?,
        tampered,
        to_wire(&hot_tick(3, at(0), at(12))?)?,
        misrouted,
    ];

    let outcome = processor.admit_wire(frames, at(20));

    assert_eq!(
        sequences(&outcome.accepted),
        vec![1, 2, 3],
        "a bad event must not take the good ones with it"
    );
    assert_eq!(outcome.rejected.len(), 3);

    let malformed = outcome.rejections_because(RejectionReason::Malformed);
    assert_eq!(
        malformed.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![1, 3],
        "each rejection must point at the frame it came from"
    );
    assert!(
        malformed
            .iter()
            .all(|rejection| !rejection.detail.is_empty()),
        "every rejection carries a reason in words"
    );
    assert!(
        malformed
            .iter()
            .any(|rejection| rejection.detail.contains("payload hash mismatch")),
        "the tampered frame must be named for what was wrong with it"
    );

    let unroutable = outcome.rejections_because(RejectionReason::Unroutable);
    assert_eq!(
        unroutable.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![5]
    );
    assert_eq!(processor.stats().malformed, 2);
    assert_eq!(processor.stats().unroutable, 1);
    assert_eq!(processor.stats().offered, 6);
    Ok(())
}

#[test]
fn an_event_older_than_the_replay_window_is_refused() -> Result<()> {
    let policy = ProcessingPolicy::default();
    let mut processor = StreamProcessor::new(policy);

    let stale = hot_tick(1, at(0), at(0))?;
    let now = at(0).saturating_add(policy.replay_window + Duration::from_secs(1));
    let outcome = processor.admit(vec![stale], now);

    assert!(outcome.accepted.is_empty());
    let replayed = outcome.rejections_because(RejectionReason::Replayed);
    assert_eq!(replayed.len(), 1);
    assert!(
        replayed
            .first()
            .is_some_and(|r| r.detail.contains("replay window")),
        "the refusal must name the window it fell outside"
    );

    // The same event inside the window is accepted, so the refusal is about
    // age rather than about the event.
    let fresh = hot_tick(1, at(0), at(0))?;
    let inside = processor.admit(vec![fresh], at(0).saturating_add(Duration::from_hours(1)));
    assert_eq!(inside.accepted.len(), 1);
    Ok(())
}

#[test]
fn an_event_stamped_ahead_of_the_callers_clock_is_refused() -> Result<()> {
    let mut processor = processor();
    let outcome = processor.admit(vec![hot_tick(1, at(0), at(5_000))?], at(0));
    assert!(outcome.accepted.is_empty());
    assert_eq!(
        outcome
            .rejections_because(RejectionReason::FutureDated)
            .len(),
        1
    );
    Ok(())
}

#[test]
fn clock_skew_is_reported_and_the_event_is_still_accepted() -> Result<()> {
    let mut processor = processor();
    // Ten seconds of ingest lag, against a five-second tolerance.
    let outcome = processor.admit(vec![hot_tick(1, at(0), at(10_000))?], at(10_000));

    assert_eq!(
        outcome.accepted.len(),
        1,
        "dropping the data loses the data and keeps the clock problem"
    );
    let skew = outcome
        .observations
        .iter()
        .find_map(|observation| match observation {
            StreamObservation::ClockSkew { skew, .. } => Some(*skew),
            _ => None,
        });
    assert_eq!(skew, Some(Duration::from_secs(10)));
    assert_eq!(processor.stats().skewed, 1);
    Ok(())
}

#[test]
fn an_ingest_clamp_is_reported_as_an_observation() -> Result<()> {
    let mut processor = processor();
    // The source claims to have delivered the tick 250ms before it happened.
    let outcome = processor.admit(vec![hot_tick(1, at(1_000), at(750))?], at(1_000));

    assert_eq!(outcome.accepted.len(), 1);
    let correction = outcome
        .observations
        .iter()
        .find_map(|observation| match observation {
            StreamObservation::IngestClamped { correction, .. } => Some(*correction),
            _ => None,
        });
    assert_eq!(correction, Some(Duration::from_millis(250)));
    assert_eq!(processor.stats().clamped, 1);
    Ok(())
}

#[test]
fn a_source_that_should_number_its_output_and_did_not_is_refused() -> Result<()> {
    let mut processor = processor();
    let outcome = processor.admit(vec![unsequenced_tick(at(0), at(10))?], at(20));

    assert!(outcome.accepted.is_empty());
    let rejections = outcome.rejections_because(RejectionReason::Unsequenced);
    assert_eq!(rejections.len(), 1);
    assert!(
        rejections
            .first()
            .is_some_and(|r| r.detail.contains("gap-checked")),
        "the refusal must say what cannot be done without a sequence"
    );
    Ok(())
}

#[test]
fn an_unsequenced_source_is_released_without_a_sequence_tracker() -> Result<()> {
    let mut processor = processor();
    // Alternative data is not expected to number anything, so it is released
    // immediately rather than having gaps manufactured out of arrival order.
    let outcome = processor.admit(
        vec![
            warm_note("A", at(0), at(10))?,
            warm_note("B", at(0), at(11))?,
        ],
        at(20),
    );
    assert_eq!(outcome.accepted.len(), 2);
    assert!(outcome.sequence_events.is_empty());
    Ok(())
}

#[test]
fn the_same_input_produces_the_same_output_because_nothing_reads_a_clock() -> Result<()> {
    let frames = vec![
        to_wire(&hot_tick(1, at(0), at(10))?)?,
        to_wire(&hot_tick(3, at(0), at(11))?)?,
        serde_json::json!({"garbage": true}),
        to_wire(&hot_tick(2, at(0), at(12))?)?,
        to_wire(&hot_tick(1, at(0), at(13))?)?,
    ];

    let run = |frames: Vec<serde_json::Value>| {
        let mut processor = processor();
        let first = processor.admit_wire(frames, at(20));
        let second = processor.poll(at(200));
        (first, second, processor.stats())
    };

    let (first_a, second_a, stats_a) = run(frames.clone());
    let (first_b, second_b, stats_b) = run(frames);

    assert_eq!(first_a, first_b);
    assert_eq!(second_a, second_b);
    assert_eq!(stats_a, stats_b);
    Ok(())
}

#[test]
fn every_offered_event_is_accounted_for() -> Result<()> {
    let mut processor = processor();
    let frames = vec![
        to_wire(&hot_tick(1, at(0), at(10))?)?,
        to_wire(&hot_tick(1, at(0), at(10))?)?,
        serde_json::json!({"garbage": true}),
        to_wire(&unsequenced_tick(at(0), at(10))?)?,
    ];
    processor.admit_wire(frames, at(20));

    let stats = processor.stats();
    assert_eq!(stats.offered, 4);
    assert_eq!(
        stats.accepted + stats.malformed + stats.duplicates + stats.unsequenced,
        stats.offered,
        "every event is either accepted or rejected for a named reason: {stats:?}"
    );
    Ok(())
}

#[test]
fn a_quiet_batch_still_publishes_the_current_watermarks() -> Result<()> {
    let mut processor = processor();
    processor.admit(vec![hot_tick(1, at(0), at(10))?], at(20));
    // A batch of nothing but a duplicate: no survivors, but a caller polling
    // needs to see where the streams stand.
    let outcome = processor.admit(vec![hot_tick(1, at(0), at(10))?], at(21));
    assert!(outcome.accepted.is_empty());
    assert_eq!(watermark_of(&outcome), Some(1));
    Ok(())
}

#[test]
fn a_batch_of_nothing_is_harmless() {
    let mut processor = processor();
    let outcome = processor.admit(Vec::new(), Timestamp::EPOCH);
    assert!(outcome.is_empty());
    assert!(outcome.watermarks.is_empty());
}
