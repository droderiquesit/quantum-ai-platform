//! Releasing a whole range of an in-memory log's oldest records, and
//! verifying the chain from the checkpoint that range leaves (ADR 0748).
//!
//! The fast brain's in-memory log outgrew its 2 GiB container in minutes on
//! committed configuration. The bound that fixes it releases the oldest
//! records as one range and holds the retained chain to the range's last
//! hash, so the log no longer replays from genesis. These tests pin what the
//! release may and may not do at the library seam; the platform's use of it
//! is proven in `qip-fastbrain/tests/memory_log.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Context, CorrelationId, Lineage, Timestamp};
use qip_events::log::{EventLog, record_footprint};
use qip_events::{Envelope, EventBody, Topic};
use serde::{Deserialize, Serialize};

/// A permanent record, so neither the count ceiling nor the snapshot window
/// could have taken one: whatever leaves the log here left by the release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Verdict {
    subject: String,
    reasons: Vec<String>,
}

impl EventBody for Verdict {
    const TOPIC: Topic = Topic::RiskApproved;
    const SCHEMA_VERSION: u32 = 1;
}

fn log_of(count: usize) -> (EventLog, Context, Timestamp) {
    let now = Timestamp::from_civil(2026, 11, 28);
    let (ctx, _clock) = Context::deterministic(now, 11);
    let mut log = EventLog::in_memory();
    for index in 0..count {
        append(&mut log, &ctx, now, index);
    }
    (log, ctx, now)
}

fn append(log: &mut EventLog, ctx: &Context, now: Timestamp, index: usize) -> u64 {
    let event = Envelope::new(
        ctx.ids().generate(now),
        now,
        now,
        Lineage::root(
            CorrelationId::from_string("COR00000000000000000000001"),
            "risk",
        ),
        Verdict {
            subject: format!("instrument-{index}"),
            reasons: vec!["within limits".to_string(); index % 4 + 1],
        },
    )
    .erase()
    .unwrap();
    log.append(&event).unwrap()
}

/// The release takes exactly the head through the cut, hands back that
/// range's last hash and count, charges its bytes off the log, counts it in
/// what the log dropped, and leaves a chain that verifies from the
/// checkpoint — and not from genesis, which is the change ADR 0748 says out
/// loud. The next append still chains to the newest record.
///
/// Mutations (each fired): `release_through` drains one record fewer than
/// its prefix (the range ends at 3, not 4); the drained footprints are not
/// subtracted from `retained_bytes` (the bytes no longer sum); `dropped`
/// leaves out `released` (the anchor's budget is 0); `verify_from_checkpoint`
/// holds the head to genesis instead of the range's hash (Err(5)).
#[test]
fn a_released_range_is_checkpointed_and_the_chain_verifies_from_it_not_from_genesis() {
    let (mut log, ctx, now) = log_of(10);
    // Premises: ten permanent records, contiguous from genesis, and the
    // byte count is the sum of what each record holds.
    assert_eq!(log.len(), 10);
    assert_eq!(log.verify_chain(), Ok(()));
    let summed: u64 = log.records().iter().map(record_footprint).sum();
    assert!(summed > 0, "premise: the records weigh something");
    assert_eq!(log.retained_bytes(), summed);
    let fourth_hash = log.records()[3].record_hash.clone();
    let head_bytes: u64 = log.records()[..4].iter().map(record_footprint).sum();
    assert!(log.checkpoint().is_none(), "premise: nothing released yet");

    let range = log.release_through(4).unwrap();

    assert_eq!(range.first_sequence, 1);
    assert_eq!(range.through, 4);
    assert_eq!(range.records, 4);
    assert_eq!(range.last_hash, fourth_hash);
    assert_eq!(range.bytes, head_bytes);
    assert_eq!(log.len(), 6);
    assert_eq!(log.records()[0].sequence, 5);
    assert_eq!(log.retained_bytes(), summed - head_bytes);
    assert_eq!(
        log.retained_bytes(),
        log.records().iter().map(record_footprint).sum::<u64>()
    );
    assert_eq!(log.released(), 4);
    assert_eq!(log.retained_anchor().dropped(), 4);
    assert_eq!(log.checkpoint(), Some(&range));
    assert_eq!(
        log.verify_chain(),
        Err(5),
        "from genesis the released head is a broken link: this log no longer replays from genesis"
    );
    assert_eq!(log.verify_from_checkpoint(), Ok(()));

    // The chain carries on from the newest record, not from the release.
    let next = append(&mut log, &ctx, now, 10);
    assert_eq!(next, 11);
    assert_eq!(log.verify_from_checkpoint(), Ok(()));

    // A second release moves the checkpoint forward to its own range.
    let again = log.release_through(7).unwrap();
    assert_eq!(
        (again.first_sequence, again.through, again.records),
        (5, 7, 3)
    );
    assert_eq!(log.released(), 7);
    assert_eq!(log.verify_from_checkpoint(), Ok(()));
}

/// The cut is the end of the shortest head whose release brings the log to
/// at most the bytes asked for, and nothing at all when it is already there.
///
/// Mutation (fired): `release_cut` returns the record *before* the one that
/// crossed (the log would still hold more than it was asked to).
#[test]
fn the_release_cut_is_the_shortest_head_that_brings_the_log_under_the_bytes_asked_for() {
    let (log, _, _) = log_of(12);
    let total = log.retained_bytes();
    assert_eq!(
        log.release_cut(total),
        None,
        "a log at its bytes releases nothing"
    );
    let keep = total / 2;
    let cut = log
        .release_cut(keep)
        .expect("half the bytes needs a release");
    let after: u64 = log
        .records()
        .iter()
        .filter(|record| record.sequence > cut)
        .map(record_footprint)
        .sum();
    assert!(
        after <= keep,
        "releasing through {cut} leaves {after} > {keep}"
    );
    let one_less: u64 = log
        .records()
        .iter()
        .filter(|record| record.sequence >= cut)
        .map(record_footprint)
        .sum();
    assert!(
        one_less > keep,
        "releasing through {} would already have sufficed; the cut is not the shortest",
        cut - 1
    );
}

/// What the release refuses, each with nothing released: the whole log
/// (the newest record is what the next append chains to), a cut below the
/// retained head, and any log written to a file, whose file is the durable
/// copy its index must not contradict.
///
/// Mutations (each fired): the refusal of a release that takes every record
/// removed (the log empties); the file-backed refusal removed (the file log
/// releases).
#[test]
fn a_release_of_the_whole_log_a_cut_below_the_head_and_a_file_backed_log_are_refused() {
    let (mut log, ctx, now) = log_of(5);
    assert!(log.release_through(5).is_err());
    assert!(log.release_through(99).is_err());
    assert_eq!(log.len(), 5, "a refused release took records");
    log.release_through(2).unwrap();
    assert!(
        log.release_through(1).is_err(),
        "nothing at or below 1 is retained"
    );
    assert_eq!(log.len(), 3);

    let dir = std::env::temp_dir().join(format!("qip-memory-release-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut file = EventLog::open(dir.join("events.jsonl")).unwrap();
    for index in 0..3 {
        append(&mut file, &ctx, now, index);
    }
    assert!(file.release_through(1).is_err());
    assert_eq!(file.len(), 3, "a file-backed log released records");
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
}
