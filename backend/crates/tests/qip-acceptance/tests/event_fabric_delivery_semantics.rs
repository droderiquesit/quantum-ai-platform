//! Event Fabric delivery semantics: FABRIC-043 through FABRIC-050, driven
//! through the broker library (`qip_streaming::event_fabric::broker::Broker`),
//! its ledger sink, and `qip_events`' stream policy.
//!
//! The previous version of this file built `DeliverySemantics`,
//! `ReplicationConfig` and `LedgerSinkState` structs in each test body and
//! asserted the values it had put in them. It imported nothing from the
//! platform and passed with the broker deleted. Several of the requirements
//! name RF3 quorums the fabric cannot run; those tests pin the refusal that
//! says so, labelled as gaps.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_events::retention::RetentionClass;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::sink::{LedgerSink, OutcomeWriter, SinkStep};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

const STREAM: &str = "fills";

fn spec(class: QosClass) -> StreamPolicySpec {
    StreamPolicySpec {
        qos_class: class,
        partition_key: "account".to_string(),
        ordering: Ordering::PerPartition,
        retention: RetentionClass::Irreplaceable,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1_000_000,
        message_quota_per_producer: 1_000,
        lag_limit: 1_000,
        entitlement: Entitlement::new("ledger", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 20,
    }
}

/// A broker in a fresh directory with one single-partition P1 stream.
fn broker(message_quota: u64) -> Broker {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-acceptance-delivery-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
    let broker = Broker::open(dir, Arc::clone(&clock)).unwrap();
    let policy = StreamPolicy::new(StreamPolicySpec {
        message_quota_per_producer: message_quota,
        ..spec(QosClass::P1Outcomes)
    })
    .unwrap();
    broker
        .declare_stream(
            STREAM,
            1,
            policy,
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
        .unwrap();
    broker
        .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    broker
}

/// A batch of `ids`, stamped as `producer` at `epoch` from `sequence`.
fn batch(producer: &str, epoch: u64, sequence: u64, ids: &[&str]) -> Batch {
    let records = ids
        .iter()
        .map(|id| Record {
            event_id: (*id).to_string(),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: b"{\"qty\":1}".to_vec(),
        })
        .collect();
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        records,
    )
    .expect("a batch");
    stamp_drain(&mut batch, producer, epoch, sequence);
    batch
}

/// FABRIC-043: effectively-once is at-least-once transport plus an
/// idempotent producer and fencing — never a delivery guarantee. A retried
/// batch (same producer, epoch, sequence, bytes) is acknowledged at its first
/// offset without a second append; a producer whose successor has initialised
/// is fenced.
///
/// Mutation: in `ProducerTable::admit`, return `Admission::Appended` for a
/// verified duplicate instead of `Admission::Duplicate` — the retry is
/// appended and the high-watermark assertion fires. Mutation: delete the
/// `epoch < current_epoch` fence — the old incarnation's write is admitted
/// and the fenced assertion fires.
#[test]
fn fabric_043_a_retried_batch_is_appended_once_and_a_superseded_producer_is_fenced() {
    let broker = broker(1_000);
    let epoch = broker.init_producer(STREAM, 0, "ledger-writer").unwrap();
    let first = broker
        .produce(
            STREAM,
            "acct-1",
            batch("ledger-writer", epoch, 0, &["fill-1"]),
        )
        .unwrap();
    let retry = broker
        .produce(
            STREAM,
            "acct-1",
            batch("ledger-writer", epoch, 0, &["fill-1"]),
        )
        .expect("a verified retry is acknowledged, not refused");
    assert_eq!(retry.base_offset(), first.base_offset());
    assert_eq!(
        broker.metadata(STREAM, 0).unwrap().high_watermark(),
        1,
        "the retry was appended a second time"
    );

    let successor = broker.init_producer(STREAM, 0, "ledger-writer").unwrap();
    assert!(successor > epoch, "premise: a new incarnation");
    let fenced = broker
        .produce(
            STREAM,
            "acct-1",
            batch("ledger-writer", epoch, 1, &["fill-2"]),
        )
        .expect_err("the superseded incarnation wrote");
    assert!(fenced.to_string().contains("fenced"), "{fenced}");
    assert_eq!(broker.metadata(STREAM, 0).unwrap().high_watermark(), 1);
}

/// An idempotent-by-event-id writer that can be told to refuse once.
#[derive(Debug, Default)]
struct Ledger {
    committed: BTreeSet<String>,
    /// How many times an apply changed the ledger's state.
    state_changes: u32,
    deliveries: BTreeMap<String, u32>,
    refuse_next: Option<String>,
}

impl OutcomeWriter for Ledger {
    fn apply(&mut self, record: &Record) -> qip_core::error::Result<()> {
        *self.deliveries.entry(record.event_id.clone()).or_default() += 1;
        if self.refuse_next.as_deref() == Some(record.event_id.as_str()) {
            self.refuse_next = None;
            return Err(qip_core::error::Error::io("the ledger store is down"));
        }
        if self.committed.insert(record.event_id.clone()) {
            self.state_changes += 1;
        }
        Ok(())
    }
}

/// FABRIC-044: a financial outcome delivered more than once is committed
/// once in effect. The sink re-delivers a whole batch after a partial
/// failure (the first record twice), and the checkpoint moves only after
/// every record in the batch is applied.
///
/// **What is not here:** "uniqueness enforced transactionally in the same
/// commit as the state change" is the writer's contract (`qip-capital`'s
/// ledger), not the sink's; this test's writer is idempotent by
/// construction. The sink half — checkpoint after write, re-delivery rather
/// than skip — is what is driven.
///
/// Mutation: in `LedgerSink::step`, move `broker.commit_offset(..)` above the
/// record loop — the checkpoint moves before the failed record is applied,
/// the next step finds nothing, and `fill-b` is never committed.
#[test]
fn fabric_044_a_redelivered_outcome_commits_once_and_the_checkpoint_waits_for_every_record() {
    let broker = broker(1_000);
    broker
        .produce(
            STREAM,
            "acct-1",
            batch("venue", 1, 0, &["fill-a", "fill-b"]),
        )
        .unwrap();
    let ledger = Ledger {
        refuse_next: Some("fill-b".to_string()),
        ..Ledger::default()
    };
    let mut sink = LedgerSink::new("ledger", STREAM, 0, ledger, 10, 1_000).unwrap();

    let first = sink.step(&broker).unwrap();
    assert!(
        matches!(first, SinkStep::Retrying { offset: 0, .. }),
        "{first:?}"
    );
    assert_eq!(broker.committed_offset("ledger", STREAM, 0).unwrap(), None);

    assert_eq!(
        sink.step(&broker).unwrap(),
        SinkStep::Committed { offset: 0 }
    );
    assert_eq!(sink.step(&broker).unwrap(), SinkStep::Idle);

    let ledger = sink.writer();
    // Premise: the first outcome really was delivered twice.
    assert_eq!(ledger.deliveries.get("fill-a"), Some(&2));
    let delivered: u32 = ledger.deliveries.values().sum();
    // fill-a twice (applied, then re-delivered with its batch); fill-b twice
    // (refused, then applied).
    assert_eq!(delivered, 4, "{:?}", ledger.deliveries);
    assert_eq!(ledger.deliveries.get("fill-b"), Some(&2));
    assert_eq!(ledger.committed.len(), 2, "{:?}", ledger.committed);
    assert_eq!(
        ledger.state_changes, 2,
        "a redelivery changed the ledger twice"
    );
}

/// Build a policy for `class` at `replication_factor`.
fn at_rf(class: QosClass, replication_factor: u32) -> qip_core::error::Result<StreamPolicy> {
    StreamPolicy::new(StreamPolicySpec {
        replication_factor,
        ..spec(class)
    })
}

/// FABRIC-045, what exists and what does not. A P0 control stream admits
/// only the strongest acknowledgement and, at an irreplaceable retention, must
/// reach the archive before release. **Gap:** RF3 three-zone quorum is
/// refused (C2); the "quorum" acknowledgement is one broker's fsync.
///
/// Mutation: change `QosClass::ack_floor`'s P0 arm to `LeaderOnly` — a
/// leader-only P0 policy is admitted and the first assertion fires.
#[test]
fn fabric_045_a_p0_stream_requires_the_strongest_ack_and_archiving_and_cannot_declare_rf3() {
    let weaker = StreamPolicy::new(StreamPolicySpec {
        ack_profile: AckProfile::LeaderOnly,
        ..spec(QosClass::P0Control)
    });
    assert!(weaker.is_err(), "a P0 stream accepted a leader-only ack");
    let admitted = at_rf(QosClass::P0Control, 1).expect("premise: the floor is admitted");
    assert!(admitted.archive_required());
    let rf3 = at_rf(QosClass::P0Control, 3).expect_err("GAP: RF3 is refused today");
    assert!(rf3.to_string().contains("(C2)"), "{rf3}");
}

/// FABRIC-046: P1 has the same floor as P0, carries deterministic event ids
/// (the producer sequence, verified on retry — see fabric_043), and archives.
/// **Gap:** RF3 refused.
///
/// Mutation: change `QosClass::ack_floor`'s P1 arm to `LeaderOnly` — the
/// equality with P0's floor fires.
#[test]
fn fabric_046_a_p1_stream_shares_p0s_floor_archives_and_cannot_declare_rf3() {
    assert_eq!(
        QosClass::P1Outcomes.ack_floor(),
        QosClass::P0Control.ack_floor()
    );
    let admitted = at_rf(QosClass::P1Outcomes, 1).expect("premise: admitted");
    assert!(admitted.archive_required());
    assert!(
        at_rf(QosClass::P1Outcomes, 3).is_err(),
        "GAP: RF3 is refused today"
    );
}

/// FABRIC-047: P2 commits to the leader before acknowledging, and a P2
/// stream may still be archive-required by its retention class. **Gap:** RF3
/// regional quorum refused.
///
/// Mutation: change `QosClass::ack_floor`'s P2 arm to `None` — the floor
/// assertion fires.
#[test]
fn fabric_047_a_p2_stream_acknowledges_on_the_leader_and_cannot_declare_rf3() {
    assert_eq!(
        QosClass::P2MarketJournal.ack_floor(),
        AckProfile::LeaderOnly
    );
    let none = StreamPolicy::new(StreamPolicySpec {
        ack_profile: AckProfile::None,
        ..spec(QosClass::P2MarketJournal)
    });
    assert!(none.is_err(), "a P2 stream accepted no acknowledgement");
    assert!(
        at_rf(QosClass::P2MarketJournal, 3).is_err(),
        "GAP: RF3 is refused today"
    );
}

/// FABRIC-048: P4 telemetry alone may run with no acknowledgement and a
/// sample-or-shed overload policy. **Gap:** "every drop counted" is not
/// implemented in the policy and is not asserted.
///
/// Mutation: change `QosClass::ack_floor`'s P4 arm to `LeaderOnly` — the
/// best-effort P4 policy is refused and the `expect` fails.
#[test]
fn fabric_048_only_telemetry_may_run_best_effort_and_shed_first() {
    let best_effort = StreamPolicy::new(StreamPolicySpec {
        ack_profile: AckProfile::None,
        overload_policy: OverloadPolicy::SampleOrShed,
        retention: RetentionClass::Transient,
        ..spec(QosClass::P4Telemetry)
    })
    .expect("telemetry may run best-effort");
    assert_eq!(best_effort.ack_profile(), AckProfile::None);
    assert!(!best_effort.archive_required());
    for class in [
        QosClass::P0Control,
        QosClass::P1Outcomes,
        QosClass::P3Research,
    ] {
        let refused = StreamPolicy::new(StreamPolicySpec {
            ack_profile: AckProfile::None,
            ..spec(class)
        });
        assert!(
            refused.is_err(),
            "{} accepted no acknowledgement",
            class.as_str()
        );
    }
}

/// FABRIC-049: quotas are declared on every stream (the spec has no default)
/// and the broker enforces them per producer.
///
/// Mutation: delete `self.charge_quota(stream, &batch)?` in
/// `Broker::produce_to` — the second message is admitted and the
/// `expect_err` fails.
#[test]
fn fabric_049_the_broker_enforces_the_declared_message_quota_per_producer() {
    let broker = broker(1);
    assert_eq!(
        broker
            .stream_policy(STREAM)
            .unwrap()
            .message_quota_per_producer(),
        1,
        "premise: the declared quota is the one the broker holds"
    );
    broker
        .produce(STREAM, "acct-1", batch("venue-a", 1, 0, &["f-1"]))
        .unwrap();
    let refused = broker
        .produce(STREAM, "acct-1", batch("venue-a", 1, 1, &["f-2"]))
        .expect_err("a second message within the window exceeds a quota of one");
    assert!(refused.to_string().contains("quota"), "{refused}");
    broker
        .produce(STREAM, "acct-1", batch("venue-b", 1, 0, &["f-3"]))
        .expect("another producer's budget is its own");
}

/// FABRIC-050: under overload a P1 record is refused explicitly — the
/// producer gets an error it can act on — and nothing already acknowledged is
/// discarded.
///
/// Mutation: make `Broker::produce` return `Ok` with the previous ack when
/// over quota (a silent drop) — the `expect_err` fails.
#[test]
fn fabric_050_an_overloaded_p1_stream_refuses_explicitly_and_keeps_what_it_acknowledged() {
    let broker = broker(1);
    let acknowledged = broker
        .produce(STREAM, "acct-1", batch("venue-a", 1, 0, &["kept"]))
        .unwrap();
    let refused = broker
        .produce(STREAM, "acct-1", batch("venue-a", 1, 1, &["refused"]))
        .expect_err("an over-quota P1 record must be refused, not dropped");
    assert!(!refused.to_string().is_empty());

    let fetched = broker
        .fetch(STREAM, 0, acknowledged.base_offset(), 1_048_576)
        .unwrap();
    assert_eq!(
        fetched.high_watermark(),
        1,
        "exactly the acknowledged record"
    );
    let bytes = qip_core::hash::from_hex(fetched.batches()).expect("hex");
    let batch = match Batch::decode(&bytes).unwrap() {
        qip_events::event_fabric::codec::DecodeOutcome::Complete(batch) => batch,
        qip_events::event_fabric::codec::DecodeOutcome::Torn => panic!("torn"),
    };
    assert_eq!(batch.records[0].event_id, "kept");
}
