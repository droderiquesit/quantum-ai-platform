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
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "account".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: overload,
        ack_profile: AckProfile::Quorum,
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
