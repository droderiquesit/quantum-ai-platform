//! Sequence tracking: ordering, duplicates, gaps and the watermark's promise.

use qip_contracts::{BookSide, MarketMessage, MessageBody, Origin, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_sequencing::{
    GapReason, ReorderPolicy, SequenceEvent, SequenceTracker, SequencedBatch, Sequencer,
};
use std::collections::BTreeSet;

const FEED: &str = "itch-a";

fn origin(sequence: u64, partition: u32) -> Origin {
    Origin::new(VenueId::new("XNAS"), FEED, partition, sequence)
}

/// One message. `ordinal` distinguishes facts that shared a wire message.
fn message(sequence: u64, ordinal: u32, partition: u32) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string(format!("{partition}-{sequence}-{ordinal}")),
        origin(sequence, partition),
        MessageBody::OrderAdded {
            order_ref: sequence * 10 + u64::from(ordinal),
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(10),
        },
        Timestamp::from_nanos(sequence as i64 * 1_000),
        Timestamp::from_nanos(sequence as i64 * 1_000 + 500),
    )
}

fn unit(sequence: u64) -> Vec<MarketMessage> {
    vec![message(sequence, 0, 1)]
}

fn at(step: i64) -> Timestamp {
    Timestamp::from_nanos(1_704_207_845_000_000_000 + step * 1_000_000)
}

/// The venue sequences a batch released, ignoring any reset the tracker
/// synthesised — a reset occupies the position of the data it replaces, and
/// counting it as delivered data would be exactly the confusion to avoid.
fn sequences(batch: &SequencedBatch) -> Vec<u64> {
    batch
        .released
        .iter()
        .filter(|message| !matches!(message.body, MessageBody::Reset { .. }))
        .map(|message| message.origin.sequence)
        .collect()
}

fn patient_policy() -> ReorderPolicy {
    ReorderPolicy::new(1_024, Duration::from_secs(3_600))
}

#[test]
fn a_watermark_names_the_highest_contiguous_position_and_stops_at_a_hole() {
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
    tracker.accept_unit(1, unit(1), at(0));
    tracker.accept_unit(2, unit(2), at(1));
    let batch = tracker.accept_unit(5, unit(5), at(2));

    assert!(sequences(&batch).is_empty(), "5 cannot be applied before 3");
    let watermark = tracker.watermark().expect("a position exists");
    assert_eq!(
        watermark.position, 2,
        "a watermark past the hole would be a promise the tracker cannot keep"
    );
    assert!(matches!(
        batch.events.first(),
        Some(SequenceEvent::GapOpened {
            missing_from: 3,
            missing_to: 4,
            ..
        })
    ));
}

#[test]
fn a_hole_that_fills_releases_everything_it_was_holding_in_order() {
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
    tracker.accept_unit(1, unit(1), at(0));
    tracker.accept_unit(4, unit(4), at(1));
    tracker.accept_unit(3, unit(3), at(2));
    let batch = tracker.accept_unit(2, unit(2), at(3));

    assert_eq!(sequences(&batch), vec![2, 3, 4]);
    assert_eq!(tracker.watermark().map(|w| w.position), Some(4));
    assert_eq!(tracker.buffered(), 0);
    assert_eq!(tracker.stats().gaps_filled, 1);
}

#[test]
fn a_message_delivered_twice_is_applied_once() {
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
    tracker.accept_unit(1, unit(1), at(0));
    let again = tracker.accept_unit(1, unit(1), at(1));

    assert!(again.released.is_empty());
    assert!(matches!(
        again.events.first(),
        Some(SequenceEvent::Duplicate { sequence: 1, .. })
    ));
    assert_eq!(tracker.stats().duplicates, 1);
}

#[test]
fn a_message_held_out_of_order_and_then_re_delivered_is_still_applied_once() {
    // The case a naive "have I released this?" check misses: the duplicate
    // arrives while the original is still waiting for its predecessors.
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
    tracker.accept_unit(1, unit(1), at(0));
    tracker.accept_unit(3, unit(3), at(1));
    let duplicate = tracker.accept_unit(3, unit(3), at(2));
    assert!(matches!(
        duplicate.events.first(),
        Some(SequenceEvent::Duplicate { sequence: 3, .. })
    ));

    let batch = tracker.accept_unit(2, unit(2), at(3));
    assert_eq!(sequences(&batch), vec![2, 3], "3 is released exactly once");
}

#[test]
fn all_the_facts_that_shared_one_wire_message_are_released_together() {
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
    tracker.accept_unit(1, unit(1), at(0));
    let batch = tracker.accept_unit(2, vec![message(2, 0, 1), message(2, 1, 1)], at(1));

    assert_eq!(batch.released.len(), 2);
    assert_eq!(
        tracker.watermark().map(|w| w.position),
        Some(2),
        "the position is the packet's, not one per fact"
    );

    let duplicate = tracker.accept_unit(2, vec![message(2, 0, 1), message(2, 1, 1)], at(2));
    assert!(
        duplicate.released.is_empty(),
        "a re-delivered packet is dropped whole, not half-applied"
    );
}

#[test]
fn the_reorder_buffer_stays_bounded_under_a_gap_that_never_fills() {
    // The failure this bound exists for: the buffer fills because of a permanent
    // gap, which is the very fault the buffer is meant to help survive.
    let policy = ReorderPolicy::new(8, Duration::from_secs(3_600));
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", policy);
    tracker.accept_unit(1, unit(1), at(0));

    let mut released = Vec::new();
    for sequence in 3..=200u64 {
        let batch = tracker.accept_unit(sequence, unit(sequence), at(sequence as i64));
        released.extend(sequences(&batch));
        assert!(
            tracker.buffered() <= 8,
            "held {} messages against a bound of 8",
            tracker.buffered()
        );
    }

    assert!(tracker.stats().gaps_abandoned >= 1);
    assert!(
        released.contains(&200),
        "the stream must resume rather than stall behind a hole that will never fill"
    );
}

#[test]
fn an_abandoned_gap_tells_the_consumer_to_discard_before_it_hands_over_anything_newer() {
    let policy = ReorderPolicy::new(2, Duration::from_secs(3_600));
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", policy);
    tracker.accept_unit(1, unit(1), at(0));
    tracker.accept_unit(5, unit(5), at(1));
    tracker.accept_unit(6, unit(6), at(2));
    let batch = tracker.accept_unit(7, unit(7), at(3));

    let reset_position = batch
        .released
        .iter()
        .position(|message| matches!(message.body, MessageBody::Reset { .. }))
        .expect("an unrecoverable gap must produce a reset");
    assert_eq!(
        reset_position, 0,
        "a consumer applying this batch in order must learn its book is stale first"
    );
    let MessageBody::Reset { reason } = &batch.released[0].body else {
        panic!("checked above");
    };
    assert!(
        reason.contains("2..=4"),
        "the reason names what was lost: {reason}"
    );
    assert!(matches!(
        batch.events.last(),
        Some(SequenceEvent::GapAbandoned {
            reason: GapReason::BufferFull,
            ..
        })
    ));
    assert_eq!(
        tracker.watermark().map(|w| w.position),
        Some(7),
        "the position only moves past the hole behind the reset"
    );
}

#[test]
fn a_gap_that_outlives_its_deadline_is_abandoned_even_if_nothing_else_arrives() {
    // A stream going quiet is a common sequel to losing packets, so the deadline
    // cannot depend on a later message arriving to carry it.
    let policy = ReorderPolicy::new(1_024, Duration::from_millis(50));
    let mut tracker = SequenceTracker::new("XNAS/itch-a/1", policy);
    tracker.accept_unit(1, unit(1), at(0));
    tracker.accept_unit(4, unit(4), at(0));

    let early = tracker.poll(at(0).saturating_add(Duration::from_millis(10)));
    assert!(early.released.is_empty(), "the deadline has not passed yet");

    let late = tracker.poll(at(0).saturating_add(Duration::from_millis(60)));
    assert!(matches!(
        late.events.last(),
        Some(SequenceEvent::GapAbandoned {
            reason: GapReason::Deadline,
            ..
        })
    ));
    assert_eq!(sequences(&late), vec![4]);
    assert_eq!(tracker.stats().messages_lost, 2, "2 and 3 were lost");
}

#[test]
fn reordered_delivery_produces_exactly_what_in_order_delivery_produces() {
    // The property the whole reorder buffer exists to provide: the network's
    // choices about ordering must not reach the book.
    let in_order: Vec<u64> = (1..=40).collect();
    let expected = {
        let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
        let mut released = Vec::new();
        for sequence in &in_order {
            released.extend(
                tracker
                    .accept_unit(*sequence, unit(*sequence), at(*sequence as i64))
                    .released,
            );
        }
        released
    };

    Property::new("any arrival order within the window releases the same stream")
        .cases(300)
        .for_all(
            |rng: &mut Xoshiro256| {
                // The first arrival defines where the stream starts, so it is
                // held fixed; everything after it is shuffled freely.
                let mut order: Vec<u64> = (2..=40).collect();
                for index in (1..order.len()).rev() {
                    order.swap(index, rng.below(index as u64 + 1) as usize);
                }
                order.insert(0, 1);
                order
            },
            |order| {
                let mut tracker = SequenceTracker::new("XNAS/itch-a/1", patient_policy());
                let mut released = Vec::new();
                for (step, sequence) in order.iter().enumerate() {
                    released.extend(
                        tracker
                            .accept_unit(*sequence, unit(*sequence), at(step as i64))
                            .released,
                    );
                }
                if released == expected {
                    Ok(())
                } else {
                    Err(format!(
                        "released {:?}",
                        released
                            .iter()
                            .map(|message| message.origin.sequence)
                            .collect::<Vec<_>>()
                    ))
                }
            },
        );
}

#[test]
fn a_watermark_only_covers_sequences_that_were_delivered_or_explicitly_disclaimed() {
    // The watermark's promise, stated as an invariant rather than a scenario:
    // for everything at or below it, the consumer has either been handed the
    // message or been told, by a reset it received first, that the message is
    // gone. Nothing in between — that in-between is the silently wrong book.
    Property::new("the watermark covers only delivered or disclaimed sequences")
        .cases(200)
        .for_all(
            |rng: &mut Xoshiro256| {
                (0..60)
                    .map(|_| 1 + rng.below(30))
                    .collect::<Vec<u64>>()
            },
            |arrivals| {
                let policy = ReorderPolicy::new(6, Duration::from_millis(5));
                let mut tracker = SequenceTracker::new("XNAS/itch-a/1", policy);
                let mut covered: BTreeSet<u64> = BTreeSet::new();
                let mut start: Option<u64> = None;

                for (step, sequence) in arrivals.iter().enumerate() {
                    let now = at(step as i64);
                    for batch in [
                        tracker.accept_unit(*sequence, unit(*sequence), now),
                        tracker.poll(now),
                    ] {
                        let abandoned: Vec<(u64, u64)> = batch
                            .events
                            .iter()
                            .filter_map(|event| match event {
                                SequenceEvent::GapAbandoned {
                                    missing_from,
                                    missing_to,
                                    ..
                                } => Some((*missing_from, *missing_to)),
                                _ => None,
                            })
                            .collect();
                        if !abandoned.is_empty()
                            && !matches!(
                                batch.released.first().map(|m| &m.body),
                                Some(MessageBody::Reset { .. })
                            )
                        {
                            return Err(
                                "a gap was abandoned without a reset leading the batch".to_string()
                            );
                        }
                        for (from, to) in abandoned {
                            covered.extend(from..=to);
                        }
                        for message in &batch.released {
                            covered.insert(message.origin.sequence);
                        }
                    }
                    start.get_or_insert(*sequence);

                    if let (Some(watermark), Some(start)) = (tracker.watermark(), start) {
                        for position in start..=watermark.position {
                            if !covered.contains(&position) {
                                return Err(format!(
                                    "watermark at {} claims {position}, which was neither delivered nor disclaimed",
                                    watermark.position
                                ));
                            }
                        }
                    }
                }
                Ok(())
            },
        );
}

#[test]
fn two_streams_never_manufacture_gaps_in_each_other() {
    let mut sequencer = Sequencer::new(patient_policy());
    let batch = sequencer.accept(
        vec![message(1, 0, 1), message(1, 0, 2), message(2, 0, 1)],
        at(0),
    );

    assert_eq!(
        batch.released.len(),
        3,
        "each partition numbers from its own origin"
    );
    assert_eq!(batch.watermarks.len(), 2);
    assert!(
        batch
            .events
            .iter()
            .all(|event| !matches!(event, SequenceEvent::GapOpened { .. }))
    );
}

#[test]
fn a_tracker_resuming_from_a_log_sees_what_was_lost_while_it_was_down() {
    let mut tracker = SequenceTracker::expecting("XNAS/itch-a/1", patient_policy(), 101);
    let batch = tracker.accept_unit(105, unit(105), at(0));

    assert!(batch.released.is_empty());
    assert!(matches!(
        batch.events.first(),
        Some(SequenceEvent::GapOpened {
            missing_from: 101,
            missing_to: 104,
            ..
        })
    ));
}
