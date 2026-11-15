//! R12-RES-7114 game day, on one host standing in for two regions (ADR 0119
//! §3): the link is cut while the source keeps committing, lag rises, remote
//! knowledge reads stale, and on return the target converges.
//!
//! Every test asserts its premise first.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_events::event_fabric::codec::{
    Batch, ContentHash, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_observability::metrics::{Metrics, labels, names};
use qip_streaming::event_fabric::managed::{CommittedBatch, PartitionRef};
use qip_streaming::event_fabric::mirror::{MirrorCheckpoint, SourcePartition, plan_copy};
use qip_streaming::event_fabric::mirror_lag::{
    Knowledge, LagBound, ReplayRoute, SourceView, StaleCause, assess, record, replay_route,
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

/// 5 records or 30 s: test figures; production commits its own with the policy.
fn bound() -> LagBound {
    LagBound::new(5, 30).unwrap()
}

/// The source region: it commits whether or not any mirror exists. `commit`
/// has no parameter naming the mirror, which is the property under test.
struct Source {
    log: Vec<CommittedBatch>,
    all: Vec<CommittedBatch>,
}

impl Source {
    fn commit_through(&mut self, n: usize) {
        self.log = self.all[..n].to_vec();
    }
}

fn view(
    source: &Source,
    heard_at: u64,
    checkpoint: &MirrorCheckpoint,
    now_of_oldest: u64,
) -> SourceView {
    SourceView {
        committed_through: source.log.len() as u64,
        oldest_uncopied_at: (source.log.len() as u64 > checkpoint.next_offset)
            .then_some(now_of_oldest),
        heard_at,
    }
}

fn gauge(metrics: &Metrics, name: &str) -> f64 {
    metrics
        .snapshot()
        .gauge(
            name,
            &labels([
                ("source_region", "us-east4"),
                ("stream", "reflex-outcomes.local"),
            ]),
        )
        .unwrap()
}

/// The link is cut: the source keeps committing, the mirror's published lag
/// rises, remote knowledge goes stale, and on return the target converges with
/// no duplicate and no gap.
///
/// Mutation: in `assess`, compute `records` as `0` — the lag gauge never rises
/// and the `lag_records > 0` assertion fails. Mutation 2: delete the
/// `SourceSilent` branch — the cut-link reading is Fresh and the stale
/// assertion fails.
#[test]
fn a_cut_link_lets_the_source_commit_raises_lag_marks_stale_and_converges_on_return() {
    let all = chain(12);
    let mut source = Source {
        log: Vec::new(),
        all,
    };
    let metrics = Metrics::new("mirror-test");
    let mut target: Vec<u64> = Vec::new();
    let mut checkpoint = MirrorCheckpoint::default();

    // Link up: 3 committed, all copied.
    source.commit_through(3);
    let plan = plan_copy(&source_ref(), &checkpoint, 3, source.log.clone()).unwrap();
    target.extend(plan.copies.iter().map(|c| c.offset));
    checkpoint = plan.checkpoint;
    assert_eq!(
        target,
        [0, 1, 2],
        "premise: the mirror copied what was committed"
    );
    let heard_at = 100;
    let (lag, knowledge) = assess(
        &checkpoint,
        &view(&source, heard_at, &checkpoint, 100),
        bound(),
        100,
    )
    .unwrap();
    record(&metrics, "us-east4", "reflex-outcomes.local", &lag);
    assert_eq!(
        (lag.records, knowledge),
        (0, Knowledge::Fresh),
        "premise: caught up reads fresh"
    );

    // Link cut: the source commits nine more. The mirror's last view is
    // unchanged (it hears nothing), so its own reading would be fresh and
    // wrong; the age since contact is what marks it stale.
    source.commit_through(12);
    let (_, silent) = assess(
        &checkpoint,
        &view(&source, heard_at, &checkpoint, 101),
        bound(),
        100 + 31,
    )
    .unwrap();
    assert_eq!(silent, Knowledge::Stale(StaleCause::SourceSilent));

    // A late partial contact reveals the head: lag is past the bound.
    let (lag, knowledge) = assess(
        &checkpoint,
        &view(&source, 120, &checkpoint, 101),
        bound(),
        125,
    )
    .unwrap();
    record(&metrics, "us-east4", "reflex-outcomes.local", &lag);
    assert_eq!(lag.records, 9);
    assert_eq!(knowledge, Knowledge::Stale(StaleCause::LagPastBound));
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS) as u64, 9);
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS) as u64, 24);
    assert_eq!(
        source.log.len(),
        12,
        "the source acknowledged every local commit meanwhile"
    );

    // Return: fabric still retains offset 3, so the fabric route; replay.
    assert_eq!(replay_route(&checkpoint, 0, Some(0)), ReplayRoute::Fabric);
    let plan = plan_copy(&source_ref(), &checkpoint, 12, source.log.clone()).unwrap();
    target.extend(plan.copies.iter().map(|c| c.offset));
    checkpoint = plan.checkpoint;
    assert_eq!(
        target,
        (0..12).collect::<Vec<_>>(),
        "converged: no duplicate, no gap"
    );
    assert!(plan.gaps.is_empty());
    let (lag, knowledge) = assess(
        &checkpoint,
        &view(&source, 200, &checkpoint, 200),
        bound(),
        200,
    )
    .unwrap();
    assert_eq!((lag.records, knowledge), (0, Knowledge::Fresh));
}

fn source_ref() -> SourcePartition {
    source()
}

/// When the source fabric trimmed past the checkpoint while the link was down,
/// the archive is the route; when neither holds it the loss is a named range.
///
/// Mutation: make `replay_route` return `Fabric` unconditionally — the
/// archive and lost assertions fail.
#[test]
fn a_checkpoint_the_fabric_trimmed_replays_from_the_archive_and_a_lost_range_is_named() {
    let at_three = MirrorCheckpoint {
        next_offset: 3,
        last_batch_hash: Some("h".into()),
    };
    assert_eq!(
        replay_route(&at_three, 3, None),
        ReplayRoute::Fabric,
        "premise"
    );
    assert_eq!(replay_route(&at_three, 8, Some(0)), ReplayRoute::Archive);
    assert_eq!(replay_route(&at_three, 8, Some(3)), ReplayRoute::Archive);
    assert_eq!(
        replay_route(&at_three, 8, Some(5)),
        ReplayRoute::Lost { from: 3, to: 5 }
    );
    assert_eq!(
        replay_route(&at_three, 8, None),
        ReplayRoute::Lost { from: 3, to: 8 }
    );
}

/// A mirror holding a record the source never committed, a zero bound and a
/// guess at lag seconds are refused, not reported.
///
/// Mutation: delete the checkpoint-past-committed refusal in `assess`.
#[test]
fn an_impossible_reading_is_refused_rather_than_reported() {
    assert!(LagBound::new(0, 30).is_err());
    assert!(LagBound::new(5, 0).is_err());
    let ahead = MirrorCheckpoint {
        next_offset: 5,
        last_batch_hash: Some("h".into()),
    };
    let v = SourceView {
        committed_through: 4,
        oldest_uncopied_at: None,
        heard_at: 1,
    };
    assert!(assess(&ahead, &v, bound(), 2).is_err());
    let behind = MirrorCheckpoint {
        next_offset: 1,
        last_batch_hash: Some("h".into()),
    };
    let unaged = SourceView {
        committed_through: 4,
        oldest_uncopied_at: None,
        heard_at: 1,
    };
    assert!(assess(&behind, &unaged, bound(), 2).is_err());
    assert!(
        assess(
            &behind,
            &SourceView {
                oldest_uncopied_at: Some(1),
                ..unaged
            },
            bound(),
            0
        )
        .is_err()
    );
}
