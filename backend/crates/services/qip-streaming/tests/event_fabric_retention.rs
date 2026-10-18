//! FABRIC-7034: each stream keeps the hot window its retention class
//! declares, not one window for every stream.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

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
use qip_storage::MemoryBlobStore;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;

const BATCHES: u64 = 24;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-retention-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn policy(retention: qip_events::RetentionClass) -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "cell".to_string(),
        ordering: Ord_::PerPartition,
        retention,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: AckProfile::Archived,
        byte_quota_per_producer: 1 << 24,
        message_quota_per_producer: 1 << 20,
        lag_limit: 1 << 20,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 24,
    })
    .unwrap()
}

fn declare(
    broker: &Broker,
    clock: Arc<dyn Clock>,
    stream: &str,
    retention: qip_events::RetentionClass,
    archived: bool,
) {
    broker
        .declare_stream(
            stream,
            1,
            policy(retention),
            SegmentLogConfig::new(clock)
                .with_archive_required(archived)
                .with_roll_after_bytes(1),
        )
        .unwrap();
    broker
        .register_schema(stream, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    for sequence in 0..BATCHES {
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
        broker.produce_to(stream, 0, batch).unwrap().unwrap();
    }
}

fn local(broker: &Broker, stream: &str, offset: u64) -> bool {
    !broker
        .fetch(stream, 0, offset, 1)
        .map(|response| response.batches().to_string())
        .unwrap_or_default()
        .is_empty()
}

/// A transient stream (§22.1's ring, retained nowhere) keeps only the batch
/// the next produce chains to, while a derived-state stream under the same
/// operator window keeps the whole window.
///
/// Mutation: make `hot_window` return `keep_batches` for every class — the
/// transient stream keeps offsets its class has already let go and the
/// first assertion fails.
#[test]
fn a_stream_with_a_short_retention_class_is_trimmed_while_a_long_one_is_kept() {
    let store = Arc::new(MemoryBlobStore::new());
    let dir = temp_dir("classes");
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
    let broker = Broker::open(&dir, clock.clone())
        .unwrap()
        .with_archive_reader(store);
    declare(
        &broker,
        clock.clone(),
        "ticks",
        qip_events::RetentionClass::Transient,
        false,
    );
    // Derived state is not archived, so nothing below can be hydrated back:
    // what a fetch serves after retention is exactly what is on local disk.
    declare(
        &broker,
        clock,
        "features",
        qip_events::RetentionClass::DerivedState,
        false,
    );
    // Premise: before retention every offset of both streams is served.
    for offset in 0..BATCHES {
        assert!(local(&broker, "ticks", offset) && local(&broker, "features", offset));
    }

    // Segments roll at the log's minimum size, a few batches each, so the
    // window is wide enough for whole segments to fall on either side of it.
    let keep = 12;
    assert!(
        broker.retain_hot(keep).unwrap() > 0,
        "premise: something was trimmed"
    );
    let newest = BATCHES - 1;
    // Offsets inside the long window but in sealed segments the short
    // window lets go of.
    for offset in (BATCHES - keep)..(BATCHES - 4) {
        assert!(
            !local(&broker, "ticks", offset),
            "the transient stream still serves offset {offset}, inside a window its class does not keep"
        );
        assert!(
            local(&broker, "features", offset),
            "the derived-state stream lost offset {offset} inside its window"
        );
    }
    assert!(
        local(&broker, "ticks", newest),
        "the newest transient batch is kept"
    );
    let _ = std::fs::remove_dir_all(dir);
}
