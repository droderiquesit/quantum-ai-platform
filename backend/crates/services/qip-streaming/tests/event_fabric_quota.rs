//! FABRIC-049: per-producer byte and message quotas are enforced by the
//! broker, not merely declared on the policy.
//!
//! Before this, `Broker::declare_stream` stored the quotas and
//! `Refusal::Quota` existed in the protocol, and nothing in `produce` ever
//! read either, so a producer could write at any rate and the policy's
//! numbers were documentation.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::Duration;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir() -> PathBuf {
    let unique = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("qip-quota-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn policy(bytes: u64, messages: u64, lag: u64, overload: OverloadPolicy) -> StreamPolicy {
    // A backlog is P3's declared overload behaviour; since FABRIC-7056 a P1
    // stream may only refuse the producer, so the class follows the policy
    // under test rather than the policy being one P1 may not have.
    let (qos_class, ack_profile) = match overload {
        OverloadPolicy::RefuseProducer => (QosClass::P1Outcomes, AckProfile::Archived),
        OverloadPolicy::AllowBacklog => (QosClass::P3Research, AckProfile::LeaderOnly),
        OverloadPolicy::ThrottleWithGap => (QosClass::P2MarketJournal, AckProfile::LeaderOnly),
        OverloadPolicy::SampleOrShed => (QosClass::P4Telemetry, AckProfile::None),
    };
    StreamPolicy::new(StreamPolicySpec {
        qos_class,
        partition_key: "account".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: overload,
        ack_profile,
        byte_quota_per_producer: bytes,
        message_quota_per_producer: messages,
        lag_limit: lag,
        entitlement: Entitlement::new("internal-reflex", "trade").unwrap(),
        seal_age_ms: 500,
        peak_bytes_per_second: 5_000_000,
    })
    .unwrap()
}

fn batch(producer: &str, sequence: u64, payload: &[u8]) -> Batch {
    let record = Record {
        event_id: format!("evt-{producer}-{sequence}"),
        trace_id: None,
        source_timestamp_ns: sequence as i64,
        payload: payload.to_vec(),
    };
    let mut b = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap();
    stamp_drain(&mut b, producer, 1, sequence);
    b
}

fn broker(bytes: u64, messages: u64) -> (Broker, Arc<ManualClock>) {
    broker_with(bytes, messages, 1_000, OverloadPolicy::RefuseProducer)
}

fn broker_with(
    bytes: u64,
    messages: u64,
    lag: u64,
    overload: OverloadPolicy,
) -> (Broker, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)));
    let dyn_clock: Arc<dyn Clock> = clock.clone();
    let b = Broker::open(temp_dir(), dyn_clock.clone()).unwrap();
    b.declare_stream(
        "orders",
        1,
        policy(bytes, messages, lag, overload),
        SegmentLogConfig::new(dyn_clock).with_roll_after_bytes(10_000_000),
    )
    .unwrap();
    b.register_schema("orders", 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    (b, clock)
}

/// Mutation: delete the `self.charge_quota(stream, &batch)?;` line in
/// `Broker::produce` — the third produce is acknowledged and this fails on
/// the `expect_err`.
#[test]
fn a_producer_past_its_message_quota_is_refused_with_the_wait_and_others_are_not() {
    let (b, _clock) = broker(1_000_000, 2);
    b.produce("orders", "k", batch("p1", 0, b"a")).unwrap();
    b.produce("orders", "k", batch("p1", 1, b"b")).unwrap();
    let refusal = b
        .produce("orders", "k", batch("p1", 2, b"c"))
        .expect_err("a third message in one second exceeds a quota of two");
    let text = refusal.to_string();
    assert!(text.contains("over its quota"), "{text}");
    assert!(text.contains("retry after"), "{text}");
    // The quota is per producer: another producer's budget is untouched.
    b.produce("orders", "k", batch("p2", 0, b"x")).unwrap();
}

/// R12-FABRIC-8089: the per-producer window is charged to the authenticated
/// principal where the session has one, not to the producer id the client
/// chose, so one identity cannot multiply its quota by minting ids. Two
/// producer ids under one principal share a window of ten bytes; the second
/// id's first batch is refused for the first id's spend.
///
/// Mutation: key the window on `batch.producer_id` alone again — the second
/// id has a fresh window, its batch is acknowledged, and the `expect_err`
/// fires.
#[test]
fn two_producer_ids_under_one_principal_share_one_quota_window() {
    let (b, _clock) = broker(10, 1_000);
    b.produce_from(
        "orders",
        0,
        batch("p1", 0, b"123456"),
        Some("qip-edge-node-london-1"),
    )
    .unwrap()
    .expect("the first producer id's six bytes fit the window");
    let refused = b
        .produce_from(
            "orders",
            0,
            batch("p2", 0, b"123456"),
            Some("qip-edge-node-london-1"),
        )
        .unwrap()
        .expect_err("a second producer id under the same principal was given its own window");
    assert!(
        refused.error.message().contains("over its quota"),
        "{:?}",
        refused.error
    );
    // Another principal's window is untouched, so the key is the principal.
    b.produce_from(
        "orders",
        0,
        batch("p3", 0, b"123456"),
        Some("qip-edge-node-paris-1"),
    )
    .unwrap()
    .expect("a different principal has its own window");
}

/// Mutation: change `over_bytes` to `false` — the oversized second batch is
/// acknowledged and this fails.
#[test]
fn a_producer_past_its_byte_quota_is_refused_even_within_its_message_quota() {
    let (b, _clock) = broker(10, 1_000);
    b.produce("orders", "k", batch("p1", 0, b"123456")).unwrap();
    let refusal = b
        .produce("orders", "k", batch("p1", 1, b"123456"))
        .expect_err("twelve bytes in one second exceeds a quota of ten");
    assert!(refusal.to_string().contains("bytes"), "{refusal}");
}

/// R12-FABRIC-8089's credit window, as the broker grants it: every ack
/// says how many payload bytes the producer may still send this second and
/// when the window refills, so the producer can hold a batch it cannot fund
/// before the wire. Mutation: report `policy.byte_quota_per_producer()` as
/// the credit instead of what is left — the second ack reads 10, not 0.
#[test]
fn an_ack_grants_the_credit_left_in_the_window_and_when_it_refills() {
    let (b, clock) = broker(10, 1_000);
    let first = b.produce("orders", "k", batch("p1", 0, b"123456")).unwrap();
    let credit = first.credit().expect("an ack carries a credit window");
    assert_eq!(credit.bytes, 4, "ten bytes of quota less the six just sent");
    assert_eq!(
        credit.refills_in_ms, 1_000,
        "premise: the clock sits at a whole second, so the window is fresh"
    );
    clock.advance(Duration::from_millis(250));
    let second = b.produce("orders", "k", batch("p1", 1, b"1234")).unwrap();
    let credit = second.credit().expect("an ack carries a credit window");
    assert_eq!(credit.bytes, 0, "the window is spent");
    assert_eq!(
        credit.refills_in_ms, 750,
        "the refill is the rest of the second"
    );
    // And a broker on this build never grants more than the quota holds.
    assert!(credit.bytes <= 10);
}

/// A refused batch must not consume its sequence number, and the budget
/// returns with the next window. Mutation: delete the `spent.retain(..)`
/// line in `charge_quota` — the old window's spend is never discarded and
/// the retry after the clock advances is refused (fails at the last
/// `unwrap`).
#[test]
fn a_refused_batch_is_not_charged_and_the_next_window_restores_the_budget() {
    let (b, clock) = broker(1_000_000, 1);
    b.produce("orders", "k", batch("p1", 0, b"a")).unwrap();
    b.produce("orders", "k", batch("p1", 1, b"b"))
        .expect_err("over quota within the window");
    clock.advance(Duration::from_secs(1));
    // The same sequence the refusal rejected is accepted now: the producer
    // table never saw it, so the dense sequence has no hole.
    b.produce("orders", "k", batch("p1", 1, b"b")).unwrap();
}

/// FABRIC-049's lag limit. Mutation: change `lag > policy.lag_limit()` in
/// `refuse_when_a_group_lags` to `false` — the produce past the limit is
/// acknowledged and this fails at the `expect_err`.
#[test]
fn a_refuse_producer_stream_stops_accepting_while_a_checkpointed_group_trails_its_lag_limit() {
    let (b, _clock) = broker_with(1_000_000, 1_000, 2, OverloadPolicy::RefuseProducer);
    for seq in 0..4 {
        b.produce("orders", "k", batch("p1", seq, b"a")).unwrap();
    }
    // Premise: the group has checkpointed, and trails offsets 1..=3 by 3.
    b.commit_offset("risk", "orders", 0, 0).unwrap();
    assert_eq!(b.committed_offset("risk", "orders", 0).unwrap(), Some(0));
    let refusal = b
        .produce("orders", "k", batch("p1", 4, b"a"))
        .expect_err("three batches behind exceeds a lag limit of two");
    assert!(refusal.to_string().contains("lag limit of 2"), "{refusal}");
    // Catching up lifts the refusal, and the refused sequence was not spent.
    b.commit_offset("risk", "orders", 0, 3).unwrap();
    b.produce("orders", "k", batch("p1", 4, b"a")).unwrap();
}

/// A stream whose declared overload behaviour is a backlog is not refused
/// for lag. Mutation: drop the `overload_policy() != RefuseProducer` early
/// return — the backlog stream is refused and this fails.
#[test]
fn a_backlog_tolerant_stream_is_not_refused_for_lag() {
    let (b, _clock) = broker_with(1_000_000, 1_000, 1, OverloadPolicy::AllowBacklog);
    for seq in 0..4 {
        b.produce("orders", "k", batch("p1", seq, b"a")).unwrap();
    }
    b.commit_offset("risk", "orders", 0, 0).unwrap();
    b.produce("orders", "k", batch("p1", 4, b"a")).unwrap();
}

/// FABRIC-7056 at the broker: over quota, a P4 telemetry stream is answered
/// `Shed` — the record is gone and the producer must not retry — while a P1
/// outcomes stream is answered `Quota` with the wait, and in neither case is
/// anything appended. Mutation: in `admission::over_quota`, answer `Quota`
/// for every overload policy — the telemetry refusal is `Quota` and this
/// fails; answer `Shed` for every one — the outcomes refusal is `Shed` and
/// this fails.
#[test]
fn over_quota_telemetry_is_shed_visibly_and_an_outcome_is_only_ever_told_to_wait() {
    use qip_transport::event_fabric::protocol::Refusal;

    for (overload, expect_shed) in [
        (OverloadPolicy::SampleOrShed, true),
        (OverloadPolicy::RefuseProducer, false),
    ] {
        let (b, _clock) = broker_with(1_000_000, 1, 1_000, overload);
        let class = b.stream_policy("orders").unwrap().qos_class();
        b.produce("orders", "k", batch("p1", 0, b"a")).unwrap();
        let refused = b
            .produce_to("orders", 0, batch("p1", 1, b"b"))
            .unwrap()
            .expect_err("a second message in one second exceeds a quota of one");
        match refused.refusal {
            Refusal::Shed { class: shed } => {
                assert!(expect_shed, "{class:?} was shed");
                assert_eq!(shed, QosClass::P4Telemetry);
            }
            Refusal::Quota { .. } => assert!(!expect_shed, "{class:?} was not shed"),
            other => panic!("unexpected refusal {other:?}"),
        }
        // Nothing was appended by the refused produce.
        assert_eq!(b.metadata("orders", 0).unwrap().high_watermark(), 1);
    }
}

fn class_policy(
    qos_class: QosClass,
    ack_profile: AckProfile,
    overload: OverloadPolicy,
    messages: u64,
) -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class,
        partition_key: "account".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: overload,
        ack_profile,
        byte_quota_per_producer: 1_000_000,
        message_quota_per_producer: messages,
        lag_limit: 1_000,
        entitlement: Entitlement::new("internal-reflex", "trade").unwrap(),
        seal_age_ms: 500,
        peak_bytes_per_second: 5_000_000,
    })
    .unwrap()
}

/// R12-FABRIC-8089's integration check, on one broker under concurrent
/// load: while four threads flood a P4 telemetry stream far past its quota,
/// a P0 control stream and a P1 outcome stream written by the *same*
/// producer ids still commit every batch, the telemetry past its quota is
/// shed visibly (never acknowledged, never told to retry), and a P1
/// producer that does exceed its own quota is refused with a back-off
/// rather than shed.
///
/// Until this test the class answers were proven one stream at a time, so
/// nothing showed that telemetry overload leaves control and outcome
/// traffic standing. The producer ids are shared across streams on purpose:
/// a node writes all three, and a quota keyed by producer alone would let
/// its telemetry starve its fills.
///
/// Mutations that fail it: key `charge_quota`'s window by producer alone
/// (control and outcomes are refused behind the telemetry flood); answer
/// `SampleOrShed` with `Quota` in `admission::over_quota` (the flood is told
/// to retry instead of shed).
#[test]
fn under_a_telemetry_flood_p4_is_shed_while_p0_and_p1_still_commit_and_p1_excess_backs_off() {
    use qip_transport::event_fabric::protocol::Refusal;
    use std::thread;

    const TELEMETRY_QUOTA: u64 = 10;
    const FLOOD_PER_THREAD: u64 = 200;
    const CRITICAL_BATCHES: u64 = 200;
    // Each of four nodes writes a quarter of the critical batches, and its
    // control and outcome quota is exactly that: the flood must not cost a
    // node one message of it, and one more is over.
    const PER_NODE: u64 = CRITICAL_BATCHES / 4;

    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 9, 26)));
    let dyn_clock: Arc<dyn Clock> = clock.clone();
    let b = Arc::new(Broker::open(temp_dir(), dyn_clock.clone()).unwrap());
    for (stream, policy) in [
        (
            "control",
            class_policy(
                QosClass::P0Control,
                AckProfile::Archived,
                OverloadPolicy::RefuseProducer,
                PER_NODE,
            ),
        ),
        (
            "outcomes",
            class_policy(
                QosClass::P1Outcomes,
                AckProfile::Archived,
                OverloadPolicy::RefuseProducer,
                PER_NODE,
            ),
        ),
        (
            "telemetry",
            class_policy(
                QosClass::P4Telemetry,
                AckProfile::None,
                OverloadPolicy::SampleOrShed,
                TELEMETRY_QUOTA,
            ),
        ),
    ] {
        b.declare_stream(
            stream,
            1,
            policy,
            SegmentLogConfig::new(dyn_clock.clone()).with_roll_after_bytes(10_000_000),
        )
        .unwrap();
        b.register_schema(stream, 1, 1, Shape::of(&1u32).unwrap())
            .unwrap();
    }

    // The flood: each thread is one node's telemetry producer, advancing its
    // sequence only on an acknowledgement, as a producer told its record is
    // gone must.
    let flooders: Vec<_> = (0..4)
        .map(|node| {
            let b = Arc::clone(&b);
            thread::spawn(move || {
                let producer = format!("node-{node}");
                let (mut committed, mut shed, mut other) = (0u64, 0u64, Vec::new());
                for _ in 0..FLOOD_PER_THREAD {
                    match b
                        .produce_to("telemetry", 0, batch(&producer, committed, b"t"))
                        .unwrap()
                    {
                        Ok(_) => committed += 1,
                        Err(refused) => match refused.refusal {
                            Refusal::Shed { class } => {
                                assert_eq!(class, QosClass::P4Telemetry);
                                shed += 1;
                            }
                            unexpected => other.push(format!("{unexpected:?}")),
                        },
                    }
                }
                (committed, shed, other)
            })
        })
        .collect();

    // Meanwhile the same node ids write control and outcomes, interleaved.
    let mut critical_refusals = Vec::new();
    for sequence in 0..CRITICAL_BATCHES {
        for stream in ["control", "outcomes"] {
            let producer = format!("node-{}", sequence % 4);
            let per_producer = sequence / 4;
            if let Err(refused) = b
                .produce_to(stream, 0, batch(&producer, per_producer, b"c"))
                .unwrap()
            {
                critical_refusals.push(format!("{stream}: {:?}", refused.refusal));
            }
        }
    }

    let mut flood_committed = 0;
    let mut flood_shed = 0;
    for flooder in flooders {
        let (committed, shed, other) = flooder.join().unwrap();
        assert!(
            other.is_empty(),
            "telemetry was answered other than shed: {other:?}"
        );
        assert!(
            committed <= TELEMETRY_QUOTA,
            "a telemetry producer committed {committed}, past its quota of {TELEMETRY_QUOTA}"
        );
        flood_committed += committed;
        flood_shed += shed;
    }
    // The premise: there was an overload, and it was shed.
    assert!(
        flood_shed >= 4 * (FLOOD_PER_THREAD - TELEMETRY_QUOTA),
        "the flood was not shed: {flood_shed} shed"
    );
    assert!(
        critical_refusals.is_empty(),
        "control or outcome traffic was refused under a telemetry flood: {critical_refusals:?}"
    );
    for stream in ["control", "outcomes"] {
        assert_eq!(
            b.metadata(stream, 0).unwrap().high_watermark(),
            CRITICAL_BATCHES,
            "{stream} did not commit every batch"
        );
    }
    assert_eq!(
        b.metadata("telemetry", 0).unwrap().high_watermark(),
        flood_committed
    );

    // P1 past its own quota is told to wait, with a wait, and nothing lands.
    let excess = b
        .produce_to("outcomes", 0, batch("node-0", PER_NODE, b"x"))
        .unwrap()
        .expect_err("node-0 has spent its outcome quota for this window");
    match excess.refusal {
        Refusal::Quota { retry_after_ms } => assert!(
            retry_after_ms > 0,
            "a quota refusal must say how long to back off"
        ),
        other => panic!("an outcome over quota was answered {other:?}, not a back-off"),
    }
    assert_eq!(
        b.metadata("outcomes", 0).unwrap().high_watermark(),
        CRITICAL_BATCHES
    );
}
