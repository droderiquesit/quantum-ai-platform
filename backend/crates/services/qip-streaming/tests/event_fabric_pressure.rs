//! FABRIC-8095: under lag pressure a telemetry stream drops its oldest
//! batches and admits the newest, where every other class keeps its backlog.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;

const BATCHES: u64 = 24;
const LAG_LIMIT: u64 = 4;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-pressure-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn policy(class: QosClass, overload: OverloadPolicy) -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: class,
        partition_key: "cell".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::DerivedState,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: overload,
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1 << 24,
        message_quota_per_producer: 1 << 20,
        lag_limit: LAG_LIMIT,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 24,
    })
    .unwrap()
}

fn batch(stream: &str, sequence: u64) -> Batch {
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("{stream}-{sequence}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: vec![u8::try_from(sequence).unwrap(); 1024],
        }],
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-a", 1, sequence);
    batch
}

/// Declare `stream`, produce one batch, have group `slow` read and commit it,
/// then produce the rest while `slow` reads nothing more.
fn pressed(broker: &Broker, clock: Arc<dyn Clock>, stream: &str, policy: StreamPolicy) {
    broker
        .declare_stream(
            stream,
            1,
            policy,
            SegmentLogConfig::new(clock).with_roll_after_bytes(1),
        )
        .unwrap();
    broker
        .register_schema(stream, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    broker
        .produce_to(stream, 0, batch(stream, 0))
        .unwrap()
        .unwrap();
    broker.commit_offset("slow", stream, 0, 0).unwrap();
    for sequence in 1..BATCHES {
        broker
            .produce_to(stream, 0, batch(stream, sequence))
            .unwrap()
            .expect("a produce under lag pressure is admitted on both classes");
    }
}

fn served(broker: &Broker, stream: &str, offset: u64) -> bool {
    !broker
        .fetch(stream, 0, offset, 1)
        .map(|response| response.batches().to_string())
        .unwrap_or_default()
        .is_empty()
}

/// A P4 telemetry stream whose group trails past the lag limit keeps
/// admitting the newest batch and spends its oldest; a P3 research stream
/// under the same lag keeps every batch, its declared backlog.
///
/// Mutation: make `admission::drops_oldest_for_lag` return `false` — the
/// telemetry stream keeps offset 0 and the first assertion fails.
#[test]
fn a_lagging_p4_stream_drops_its_oldest_batches_and_admits_the_newest() {
    let dir = temp_dir("p4");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let broker = Broker::open(&dir, clock.clone()).unwrap();
    pressed(
        &broker,
        clock.clone(),
        "telemetry",
        policy(QosClass::P4Telemetry, OverloadPolicy::SampleOrShed),
    );
    pressed(
        &broker,
        clock,
        "research",
        policy(QosClass::P3Research, OverloadPolicy::AllowBacklog),
    );

    assert!(
        !served(&broker, "telemetry", 0),
        "the oldest telemetry batch outlived a lag past the limit"
    );
    assert!(
        served(&broker, "telemetry", BATCHES - 1),
        "the newest batch was not admitted"
    );
    // Premise and contrast: the backlog class keeps everything.
    for offset in 0..BATCHES {
        assert!(
            served(&broker, "research", offset),
            "research lost offset {offset}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
