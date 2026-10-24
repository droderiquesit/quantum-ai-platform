//! The mirror's copy-and-checkpoint logic (ADR 0119 §1, R12-FABRIC-7038):
//! only committed records, idempotent by source position, a missing offset a
//! gap and never a skip, and a broken batch chain produces nothing.
//!
//! Every test asserts its premise first, so a plan that is empty because the
//! rows were empty cannot pass for one that refused a bad chain.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_events::event_fabric::codec::{
    Batch, ContentHash, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_streaming::event_fabric::managed::{CommittedBatch, PartitionRef};
use qip_streaming::event_fabric::mirror::{
    MirrorCheckpoint, MirrorGap, SourcePartition, plan_copy, region_qualified,
};

fn source() -> SourcePartition {
    SourcePartition {
        region: "us-east4".to_string(),
        stream: "reflex-outcomes.local".to_string(),
        partition: 0,
    }
}

/// `count` batches chained the way the source broker chains them: each names
/// the SHA-256 of the previous batch's encoding.
fn chain(count: u64) -> Vec<CommittedBatch> {
    let mut rows: Vec<CommittedBatch> = Vec::new();
    for offset in 0..count {
        let mut batch = Batch::new(
            MessageType::Data,
            1,
            1,
            PayloadCodec::CanonicalJson,
            vec![Record {
                event_id: format!("evt-{offset}"),
                trace_id: None,
                source_timestamp_ns: 1,
                payload: format!("fill-{offset}").into_bytes(),
            }],
        )
        .unwrap();
        stamp_drain(&mut batch, "cell-a", 1, offset);
        batch.base_offset = offset;
        batch.previous_batch_hash = rows
            .last()
            .map(|previous| ContentHash::sha256_of(&previous.bytes));
        let bytes = batch.encode().unwrap();
        rows.push(CommittedBatch {
            partition: PartitionRef::new("reflex-outcomes.local", 0),
            offset,
            lease_epoch: 1,
            batch_hash: qip_core::hash::sha256_hex(&bytes),
            previous_batch_hash: rows.last().map(|previous| previous.batch_hash.clone()),
            bytes,
        });
    }
    rows
}

fn offsets(plan: &qip_streaming::event_fabric::mirror::MirrorPlan) -> Vec<u64> {
    plan.copies.iter().map(|copy| copy.offset).collect()
}

/// A restart that re-fetches from before the checkpoint copies nothing twice.
///
/// Mutation: in `plan_copy`, drop the `row.offset < plan.checkpoint.next_offset`
/// branch — offsets 0 and 1 are verified again against a checkpoint past them and
/// the second plan is refused as a broken chain, so the `unwrap` fires.
#[test]
fn a_refetch_from_before_the_checkpoint_copies_only_what_the_target_has_not_acknowledged() {
    let rows = chain(4);
    let first = plan_copy(
        &source(),
        &MirrorCheckpoint::default(),
        2,
        rows[..2].to_vec(),
    )
    .unwrap();
    assert_eq!(offsets(&first), [0, 1], "premise: the first run copies two");
    assert_eq!(first.checkpoint.next_offset, 2);

    let second = plan_copy(&source(), &first.checkpoint, 4, rows).unwrap();

    assert_eq!(offsets(&second), [2, 3]);
    assert_eq!(second.duplicates, 2);
    assert!(second.gaps.is_empty());
    assert_eq!(second.checkpoint.next_offset, 4);
    assert_eq!(
        second.checkpoint.last_batch_hash.as_deref(),
        Some(chain(4)[3].batch_hash.as_str())
    );
}

/// A record the source has not acknowledged is not a fact the source holds.
///
/// Mutation: delete the `row.offset >= committed_through` refusal — offset 2
/// is copied and the `expect_err` fires.
#[test]
fn a_row_at_or_above_the_committed_bound_is_refused_not_copied() {
    let rows = chain(3);
    assert!(
        plan_copy(&source(), &MirrorCheckpoint::default(), 3, rows.clone()).is_ok(),
        "premise: the same rows are copied when all three are committed"
    );
    let refused = plan_copy(&source(), &MirrorCheckpoint::default(), 2, rows)
        .expect_err("an uncommitted row was copied");
    assert!(
        refused.message().contains("not committed"),
        "{}",
        refused.message()
    );
}

/// A missing source offset is journaled as a gap, the rows around it are
/// copied, and the chain is checked again from the row after the gap.
///
/// Mutation: in `plan_copy`, replace the `gaps.push(..)` with nothing — the
/// copy is made silently and the `gaps` assertion fires.
#[test]
fn a_missing_source_offset_is_a_gap_and_the_rows_around_it_are_still_copied() {
    let mut rows = chain(5);
    rows.remove(2);
    assert_eq!(
        rows.iter().map(|r| r.offset).collect::<Vec<_>>(),
        [0, 1, 3, 4],
        "premise: offset 2 is missing from what was fetched"
    );

    let plan = plan_copy(&source(), &MirrorCheckpoint::default(), 5, rows).unwrap();

    assert_eq!(offsets(&plan), [0, 1, 3, 4]);
    assert_eq!(
        plan.gaps,
        [MirrorGap {
            from_offset: 2,
            to_offset: 3
        }]
    );
    assert_eq!(plan.checkpoint.next_offset, 5);
}

/// A row whose link does not name the batch the mirror last copied is a
/// broken chain: the whole plan is refused and nothing is produced.
///
/// Mutation: in `plan_copy`, delete the `chain_known && embedded != ..` check —
/// the substituted batch is copied and the `expect_err` fires.
#[test]
fn a_row_that_does_not_chain_to_the_last_copy_refuses_the_whole_plan() {
    let honest = chain(3);
    let mut forged = chain(3);
    // Offset 2 re-chained to a batch that never existed, with its hash and
    // embedded link consistent with each other, so only the chain can catch it.
    let mut batch = match Batch::decode(&forged[2].bytes).unwrap() {
        qip_events::event_fabric::codec::DecodeOutcome::Complete(batch) => batch,
        other => panic!("{other:?}"),
    };
    batch.previous_batch_hash = Some(ContentHash::sha256_of(b"a batch nobody committed"));
    let bytes = batch.encode().unwrap();
    forged[2] = CommittedBatch {
        batch_hash: qip_core::hash::sha256_hex(&bytes),
        previous_batch_hash: Some(qip_core::hash::sha256_hex(b"a batch nobody committed")),
        bytes,
        ..forged[2].clone()
    };
    assert!(
        plan_copy(&source(), &MirrorCheckpoint::default(), 3, honest).is_ok(),
        "premise: the honest chain is copied"
    );

    let refused = plan_copy(&source(), &MirrorCheckpoint::default(), 3, forged)
        .expect_err("a broken chain was copied");

    assert!(
        refused.message().contains("does not chain"),
        "{}",
        refused.message()
    );
}

/// Bytes that do not hash to the hash the row carries are a substituted row.
///
/// Mutation: delete `row.verify()?` — the flipped byte is copied and the
/// `expect_err` fires.
#[test]
fn a_row_whose_bytes_do_not_hash_to_its_hash_is_refused() {
    let mut rows = chain(2);
    assert!(
        plan_copy(&source(), &MirrorCheckpoint::default(), 2, rows.clone()).is_ok(),
        "premise: the untouched rows are copied"
    );
    let last = rows[1].bytes.len() - 1;
    rows[1].bytes[last] ^= 1;
    let refused = plan_copy(&source(), &MirrorCheckpoint::default(), 2, rows)
        .expect_err("a substituted row was copied");
    assert!(
        refused.message().contains("does not hash"),
        "{}",
        refused.message()
    );
}

/// Rows must arrive in strictly ascending offset order.
///
/// Mutation: delete the ascending-order refusal — the repeated offset is
/// counted as a duplicate and the `expect_err` fires.
#[test]
fn rows_out_of_order_or_repeated_are_refused() {
    let rows = chain(2);
    let reversed = vec![rows[1].clone(), rows[0].clone()];
    let refused = plan_copy(&source(), &MirrorCheckpoint::default(), 2, reversed)
        .expect_err("descending offsets were accepted");
    assert!(
        refused.message().contains("strictly ascending"),
        "{}",
        refused.message()
    );
}

/// A copy lands in `<source-region>.<stream>`, and a region that could make
/// two sources one target is refused.
///
/// Mutation: let `region_qualified` accept a dot in the region — `a.b`/`c`
/// and `a`/`b.c` both become `a.b.c` and the last assertion fires.
#[test]
fn a_copy_goes_to_the_region_qualified_stream_and_two_sources_never_share_one() {
    assert_eq!(
        source().target_stream().unwrap(),
        "us-east4.reflex-outcomes.local"
    );
    assert!(
        region_qualified("a", "b.c").is_ok(),
        "premise: a clean pair"
    );
    assert!(region_qualified("a.b", "c").is_err(), "a dotted region");
    assert!(region_qualified("", "c").is_err(), "an empty region");
    assert!(region_qualified("us-east4", " ").is_err(), "a blank stream");
}
