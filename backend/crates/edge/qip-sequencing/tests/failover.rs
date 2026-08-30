//! Failover: changing source without dropping or double-applying anything.

use qip_contracts::{BookSide, MarketMessage, MessageBody, Origin, VenueId};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::testing::Property;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_sequencing::{FailoverEvent, FailoverReconciler};

fn message(source: &str, sequence: u64) -> MarketMessage {
    MarketMessage::new(
        ObjectId::from_string(format!("{source}-{sequence}")),
        Origin::new(VenueId::new("XNAS"), source, 1, sequence),
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

fn applied_sequences(messages: &[MarketMessage]) -> Vec<u64> {
    messages
        .iter()
        .filter(|message| !matches!(message.body, MessageBody::Reset { .. }))
        .map(|message| message.origin.sequence)
        .collect()
}

#[test]
fn a_backup_that_replays_what_the_primary_already_delivered_applies_none_of_it_twice() {
    let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary");
    for sequence in 1..=10u64 {
        reconciler.admit(
            "primary",
            vec![message("primary", sequence)],
            at(sequence as i64),
        );
    }
    reconciler.begin_switch("backup");

    // The backup restarts its replay five messages back, as a reconnecting
    // session does.
    let mut applied = Vec::new();
    let mut events = Vec::new();
    for sequence in 6..=15u64 {
        let outcome = reconciler.admit("backup", vec![message("backup", sequence)], at(100));
        applied.extend(outcome.applied);
        events.extend(outcome.events);
    }

    assert_eq!(
        applied_sequences(&applied),
        (11..=15).collect::<Vec<u64>>(),
        "everything at or below the applied position must be dropped"
    );
    assert!(
        applied
            .iter()
            .all(|message| !matches!(message.body, MessageBody::Reset { .. }))
    );
    assert_eq!(reconciler.stats().units_already_applied, 5);
    assert!(events.iter().any(|event| matches!(
        event,
        FailoverEvent::SwitchCompleted {
            at_sequence: 11,
            ..
        }
    )));
    assert_eq!(reconciler.active(), "backup");
}

#[test]
fn a_backup_that_resumes_past_the_last_applied_position_says_what_is_missing() {
    let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary");
    for sequence in 1..=10u64 {
        reconciler.admit(
            "primary",
            vec![message("primary", sequence)],
            at(sequence as i64),
        );
    }
    reconciler.begin_switch("backup");
    let outcome = reconciler.admit("backup", vec![message("backup", 14)], at(100));

    let MessageBody::Reset { reason } = &outcome.applied[0].body else {
        panic!("a source resuming past our position must produce a reset first");
    };
    assert!(
        reason.contains("14"),
        "the reason names where it resumed: {reason}"
    );
    assert!(matches!(
        outcome.events.first(),
        Some(FailoverEvent::ResyncRequired {
            missing_from: 11,
            missing_to: 13,
            ..
        })
    ));
    assert_eq!(reconciler.stats().messages_lost, 3);
    assert_eq!(
        outcome.applied[1].origin.sequence, 14,
        "and the newer message follows the warning rather than preceding it"
    );
}

#[test]
fn a_backup_resuming_exactly_where_the_primary_stopped_loses_nothing_and_needs_no_reset() {
    let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary");
    let mut applied = Vec::new();
    for sequence in 1..=10u64 {
        applied.extend(
            reconciler
                .admit(
                    "primary",
                    vec![message("primary", sequence)],
                    at(sequence as i64),
                )
                .applied,
        );
    }
    reconciler.begin_switch("backup");
    for sequence in 11..=20u64 {
        applied.extend(
            reconciler
                .admit(
                    "backup",
                    vec![message("backup", sequence)],
                    at(sequence as i64),
                )
                .applied,
        );
    }

    assert_eq!(applied_sequences(&applied), (1..=20).collect::<Vec<u64>>());
    assert_eq!(reconciler.stats().resyncs, 0);
    assert_eq!(reconciler.stats().messages_lost, 0);
}

#[test]
fn the_old_source_is_still_accepted_until_the_new_one_actually_delivers() {
    // Both sources usually fail for related reasons, so a switch that cut the
    // primary off on request alone would strand a stream whose backup is also
    // down.
    let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary");
    reconciler.admit("primary", vec![message("primary", 1)], at(0));
    reconciler.begin_switch("backup");

    let still_primary = reconciler.admit("primary", vec![message("primary", 2)], at(1));
    assert_eq!(applied_sequences(&still_primary.applied), vec![2]);
    assert!(reconciler.is_switching());
    assert_eq!(reconciler.active(), "primary");

    let cutover = reconciler.admit("backup", vec![message("backup", 3)], at(2));
    assert_eq!(applied_sequences(&cutover.applied), vec![3]);
    assert!(!reconciler.is_switching());
    assert_eq!(reconciler.active(), "backup");

    let stale = reconciler.admit("primary", vec![message("primary", 4)], at(3));
    assert!(
        stale.applied.is_empty(),
        "once the switch has completed the old source is no longer a source"
    );
    assert!(matches!(
        stale.events.first(),
        Some(FailoverEvent::Ignored { .. })
    ));
}

#[test]
fn a_reconciler_resuming_from_a_durable_log_does_not_reapply_what_the_log_holds() {
    let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary").resuming_at(500);
    let outcome = reconciler.admit(
        "primary",
        vec![message("primary", 499), message("primary", 500)],
        at(0),
    );
    assert!(outcome.applied.is_empty());
    assert_eq!(reconciler.stats().units_already_applied, 2);
}

#[test]
fn every_sequence_is_applied_exactly_once_whatever_the_overlap_at_the_switch() {
    // The invariant, over every combination of how far the backup rewinds and
    // when the switch happens: strictly increasing, no repeats, no silent holes.
    Property::new("a switch applies each sequence exactly once")
        .cases(400)
        .for_all(
            |rng: &mut Xoshiro256| {
                let switch_at = 2 + rng.below(18);
                let rewind = rng.below(10);
                (switch_at, rewind)
            },
            |(switch_at, rewind)| {
                let mut reconciler = FailoverReconciler::new("XNAS/md/1", "primary");
                let mut applied = Vec::new();
                let mut resets = 0usize;

                for sequence in 1..=*switch_at {
                    let outcome =
                        reconciler.admit("primary", vec![message("primary", sequence)], at(0));
                    applied.extend(outcome.applied);
                }
                reconciler.begin_switch("backup");
                let resume_from = switch_at.saturating_sub(*rewind).max(1);
                for sequence in resume_from..=30 {
                    let outcome =
                        reconciler.admit("backup", vec![message("backup", sequence)], at(1));
                    resets += outcome
                        .applied
                        .iter()
                        .filter(|message| matches!(message.body, MessageBody::Reset { .. }))
                        .count();
                    applied.extend(outcome.applied);
                }

                let sequences = applied_sequences(&applied);
                if sequences != (1..=30).collect::<Vec<u64>>() {
                    return Err(format!("applied {sequences:?}"));
                }
                if resets != 0 {
                    return Err("a backup that overlaps needs no reset".to_string());
                }
                Ok(())
            },
        );
}
