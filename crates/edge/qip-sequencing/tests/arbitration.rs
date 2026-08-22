//! A/B line arbitration: redundancy that is actually used.

use qip_contracts::{BookSide, MarketMessage, MessageBody, Origin, VenueId};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_sequencing::{ArbitrationEvent, LineArbiter};

fn message(line: &str, sequence: u64) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string(format!("{line}-{sequence}")),
        Origin::new(VenueId::new("XNAS"), line, 1, sequence),
        MessageBody::OrderAdded {
            order_ref: sequence,
            side: BookSide::Bid,
            price: Decimal::from_int(100),
            quantity: Decimal::from_int(10),
        },
        Timestamp::from_nanos(sequence as i64 * 1_000),
        Timestamp::from_nanos(sequence as i64 * 1_000 + 500),
    )
}

fn at(step: i64) -> Timestamp {
    Timestamp::from_nanos(1_704_207_845_000_000_000 + step * 1_000_000)
}

fn arbiter() -> LineArbiter {
    LineArbiter::new("itch", &["itch-a", "itch-b"], 64)
}

fn published_sequences(messages: &[MarketMessage]) -> Vec<u64> {
    messages
        .iter()
        .map(|message| message.origin.sequence)
        .collect()
}

#[test]
fn arbitration_over_two_healthy_lines_publishes_exactly_what_one_healthy_line_would() {
    // The property that makes redundancy safe to switch on: adding the B line
    // changes nothing at all while the A line is keeping up.
    let alone = {
        let mut arbiter = arbiter();
        let mut released = Vec::new();
        for sequence in 1..=20u64 {
            released.extend(
                arbiter
                    .accept(
                        "itch-a",
                        vec![message("itch-a", sequence)],
                        at(sequence as i64),
                    )
                    .released,
            );
        }
        released
    };

    let merged = {
        let mut arbiter = arbiter();
        let mut released = Vec::new();
        for sequence in 1..=20u64 {
            let step = sequence as i64;
            released.extend(
                arbiter
                    .accept("itch-a", vec![message("itch-a", sequence)], at(step))
                    .released,
            );
            released.extend(
                arbiter
                    .accept("itch-b", vec![message("itch-b", sequence)], at(step))
                    .released,
            );
        }
        released
    };

    assert_eq!(
        merged, alone,
        "the second copy of every message must be invisible downstream"
    );
}

#[test]
fn the_published_stream_is_labelled_with_one_feed_name_whichever_line_won() {
    let mut arbiter = arbiter();
    let from_a = arbiter.accept("itch-a", vec![message("itch-a", 1)], at(0));
    let from_b = arbiter.accept("itch-b", vec![message("itch-b", 2)], at(1));

    for message in from_a.released.iter().chain(&from_b.released) {
        assert_eq!(
            message.origin.feed, "itch",
            "two physical line names would make one stream look like two half-streams"
        );
    }
    assert_eq!(
        from_a.released[0].origin.stream_key(),
        from_b.released[0].origin.stream_key()
    );
}

#[test]
fn a_line_that_loses_packets_is_covered_by_the_other_and_the_loss_is_reported() {
    let mut arbiter = arbiter();
    // A drops every third sequence; B carries everything, a little later.
    for sequence in 1..=30u64 {
        let step = sequence as i64 * 2;
        if !sequence.is_multiple_of(3) {
            arbiter.accept("itch-a", vec![message("itch-a", sequence)], at(step));
        }
        arbiter.accept("itch-b", vec![message("itch-b", sequence)], at(step + 1));
    }
    // Push the early sequences out of the window: a line's misses are only
    // attributable once the sequence leaves the window without it.
    for sequence in 31..=110u64 {
        arbiter.accept(
            "itch-a",
            vec![message("itch-a", sequence)],
            at(sequence as i64 * 2),
        );
        arbiter.accept(
            "itch-b",
            vec![message("itch-b", sequence)],
            at(sequence as i64 * 2 + 1),
        );
    }

    let health_a = arbiter
        .line_health("itch-a")
        .expect("line a is known")
        .clone();
    let health_b = arbiter
        .line_health("itch-b")
        .expect("line b is known")
        .clone();
    assert!(
        health_a.missed > 0,
        "the lossy line's misses must be visible"
    );
    assert_eq!(health_b.missed, 0);
    assert!(health_a.loss_rate_f64 > 0.0 && health_a.loss_rate_f64 < 0.5);
    assert!(
        health_b.mean_lag_nanos_f64 > 0.0,
        "the line that habitually arrives second should say how far behind it is"
    );
}

#[test]
fn a_sequence_only_one_line_carries_is_still_published_once() {
    let mut arbiter = arbiter();
    arbiter.accept("itch-a", vec![message("itch-a", 1)], at(0));
    let only_b = arbiter.accept("itch-b", vec![message("itch-b", 2)], at(1));
    let late_a = arbiter.accept("itch-a", vec![message("itch-a", 2)], at(2));

    assert_eq!(published_sequences(&only_b.released), vec![2]);
    assert!(late_a.released.is_empty());
    assert!(matches!(
        late_a.events.first(),
        Some(ArbitrationEvent::Duplicate { sequence: 2, .. })
    ));
    assert_eq!(arbiter.winner_of(2), Some("itch-b"));
}

#[test]
fn every_fact_that_shared_a_wire_message_is_published_or_dropped_together() {
    let mut arbiter = arbiter();
    let unit = vec![message("itch-a", 1), message("itch-a", 1)];
    let first = arbiter.accept("itch-a", unit, at(0));
    assert_eq!(first.released.len(), 2);

    let second = arbiter.accept(
        "itch-b",
        vec![message("itch-b", 1), message("itch-b", 1)],
        at(1),
    );
    assert!(
        second.released.is_empty(),
        "half a duplicated packet is worse than the whole of it"
    );
}

#[test]
fn the_window_of_remembered_sequences_stays_bounded() {
    // The structure that would otherwise grow for the whole session.
    let mut arbiter = LineArbiter::new("itch", &["itch-a", "itch-b"], 16);
    for sequence in 1..=1_000u64 {
        arbiter.accept(
            "itch-a",
            vec![message("itch-a", sequence)],
            at(sequence as i64),
        );
        assert!(
            arbiter.tracked() <= 16,
            "remembered {} sequences against a window of 16",
            arbiter.tracked()
        );
    }
}

#[test]
fn a_line_further_behind_than_the_window_is_reported_as_a_fault_not_as_jitter() {
    let mut arbiter = LineArbiter::new("itch", &["itch-a", "itch-b"], 4);
    for sequence in 1..=20u64 {
        arbiter.accept(
            "itch-a",
            vec![message("itch-a", sequence)],
            at(sequence as i64),
        );
    }
    let stale = arbiter.accept("itch-b", vec![message("itch-b", 1)], at(100));

    assert!(
        stale.released.is_empty(),
        "it was published nineteen sequences ago"
    );
    assert!(matches!(
        stale.events.first(),
        Some(ArbitrationEvent::BeyondWindow { sequence: 1, .. })
    ));
}
