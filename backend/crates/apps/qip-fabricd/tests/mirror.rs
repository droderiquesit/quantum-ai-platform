//! The cross-region mirror role on one host (ADR 0119 §3, R12-OBS-9148): two
//! brokers standing in for two regions, the mirror copying between them, and
//! the link cut while the source keeps taking writes.
//!
//! What this proves is the lag series — that it is recorded where the mirror
//! measures it and that it grows, in both records and seconds, while the
//! link is down and the source keeps acknowledging. It proves nothing about
//! a deployment: nothing starts the role, and the second region it would run
//! in is not authorised (ADR 0035).
//!
//! Every test asserts its premise first, so a lag of zero that was never
//! measured cannot pass for one the mirror caught up to.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// integration tests may unwrap: a panic is the failure report
// Exact comparison is the assertion: every gauge here is a whole count or a
// whole number of seconds off a manual clock, so a tolerance would only let a
// value the mirror computed wrongly pass.
#![allow(clippy::float_cmp)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as StreamOrdering, OverloadPolicy, QosClass,
    StreamPolicy, StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_fabricd::mirror::{Mirror, MirrorConfig, MirrorTarget};
use qip_fabricd::telemetry::FabricdTelemetry;
use qip_observability::metrics::{Metrics, labels, names};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;

const REGION: &str = "us-east4";
const STREAM: &str = "outcomes.test";
const TARGET_STREAM: &str = "us-east4.outcomes.test";

static DIRS: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRS.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-fabricd-mirror-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The policy both sides declare. `Mirroring::None` on the copy as on the
/// source: the policy arm that would select a stream for mirroring is still
/// refused (C8), and this suite does not reach for it.
fn policy() -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "cell".to_string(),
        ordering: StreamOrdering::PerPartition,
        retention: qip_events::RetentionClass::DerivedState,
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

fn broker(label: &str, clock: Arc<dyn Clock>, stream: &str) -> Broker {
    let broker = Broker::open(temp_dir(label), clock.clone()).unwrap();
    broker
        .declare_stream(stream, 1, policy(), SegmentLogConfig::new(clock))
        .unwrap();
    broker
        .register_schema(stream, 1, 1, Shape::of(&1u32).unwrap())
        .unwrap();
    broker
}

/// Produce one batch to the source as cell `cell-a` would, returning the
/// offset the source acknowledged it at.
fn produce(source: &Broker, sequence: u64) -> u64 {
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("fill-{sequence}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: vec![7; 16],
        }],
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-a", 1, sequence);
    source
        .produce_to(STREAM, 0, batch)
        .expect("the source broker answered")
        .expect("the source broker acknowledged the produce")
        .base_offset()
}

/// The link to the target region, with a switch a test can throw. A cut
/// link refuses every produce the way an unreachable peer would: with an
/// error, and without the target ever seeing the batch.
struct Link<'a> {
    target: &'a Broker,
    cut: AtomicBool,
}

impl MirrorTarget for Link<'_> {
    fn produce_copy(&self, stream: &str, partition: u32, batch: Batch) -> Result<u64> {
        if self.cut.load(Ordering::SeqCst) {
            return Err(Error::io("the link to the target region is cut"));
        }
        self.target.produce_copy(stream, partition, batch)
    }
}

fn gauge(metrics: &Metrics, name: &str) -> Option<f64> {
    metrics.snapshot().gauge(
        name,
        &labels([("source_region", REGION), ("stream", STREAM)]),
    )
}

/// ADR 0119 §2-§3's game day on one host: with the link up the mirror
/// copies and reports no lag; with it cut, the source keeps acknowledging
/// every write and both lag series grow, in records and in seconds; with it
/// restored the target converges and the lag returns to zero.
///
/// Mutation (run, failed, restored): delete the `telemetry.mirror_lag(..)`
/// call in `Mirror::pass` — the series is never written and the first
/// `gauge(..)` assertion fails on `None`.
#[test]
fn while_the_link_is_cut_the_source_keeps_acknowledging_and_both_mirror_lag_series_grow() {
    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 31)));
    let shared: Arc<dyn Clock> = clock.clone();
    let source = broker("source", shared.clone(), STREAM);
    let target = broker("target", shared.clone(), TARGET_STREAM);
    let link = Link {
        target: &target,
        cut: AtomicBool::new(false),
    };
    let metrics = Arc::new(Metrics::new("qip-fabricd"));
    let telemetry = FabricdTelemetry::new(metrics.clone());
    let config = MirrorConfig::new([REGION.to_string()], [STREAM.to_string()]).unwrap();
    let mut mirror = Mirror::new(
        &config,
        REGION,
        &BTreeMap::from([(STREAM.to_string(), 1)]),
        shared,
    )
    .unwrap();

    // Premise: before any pass, neither series exists, so a value read later
    // is one this mirror wrote.
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS), None);
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS), None);

    // Link up: three committed records are copied and nothing is behind.
    for sequence in 0..3 {
        assert_eq!(produce(&source, sequence), sequence);
    }
    clock.advance(Duration::from_secs(1));
    let up = mirror.pass(&source, &link, &telemetry);
    assert!(up.failures.is_empty(), "{:?}", up.failures);
    assert_eq!(up.copied, 3, "premise: the mirror copied with the link up");
    assert_eq!(
        target.metadata(TARGET_STREAM, 0).unwrap().high_watermark(),
        3,
        "premise: the target holds the three copies"
    );
    assert_eq!(mirror.checkpoint(STREAM, 0).unwrap().next_offset, 3);
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS), Some(0.0));
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS), Some(0.0));

    // Cut the link. The source region's produce path never learns the mirror
    // exists (ADR 0119 §2): every write is still acknowledged, at the offset
    // it would have had anyway.
    link.cut.store(true, Ordering::SeqCst);
    for sequence in 3..5 {
        assert_eq!(produce(&source, sequence), sequence);
    }
    clock.advance(Duration::from_secs(5));
    let cut = mirror.pass(&source, &link, &telemetry);
    assert_eq!(cut.copied, 0, "a copy crossed a cut link");
    assert!(
        cut.failures
            .iter()
            .any(|failure| failure.message.contains("cut")),
        "premise: the pass met the cut link, {:?}",
        cut.failures
    );
    assert_eq!(
        mirror.checkpoint(STREAM, 0).unwrap().next_offset,
        3,
        "the checkpoint advanced without the target's acknowledgement"
    );
    let records_cut = gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS).unwrap();
    let seconds_cut = gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS).unwrap();
    assert_eq!(records_cut, 2.0, "two committed records are uncopied");
    assert_eq!(
        seconds_cut, 5.0,
        "the oldest uncopied record was stamped five seconds ago"
    );

    // Still cut, and still writing: both series keep growing.
    assert_eq!(produce(&source, 5), 5);
    clock.advance(Duration::from_secs(10));
    let still_cut = mirror.pass(&source, &link, &telemetry);
    assert_eq!(still_cut.copied, 0);
    let records_later = gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS).unwrap();
    let seconds_later = gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS).unwrap();
    assert!(
        records_later > records_cut,
        "lag in records did not grow: {records_cut} then {records_later}"
    );
    assert!(
        seconds_later > seconds_cut,
        "lag in seconds did not grow: {seconds_cut} then {seconds_later}"
    );
    assert_eq!((records_later, seconds_later), (3.0, 15.0));
    assert_eq!(
        target.metadata(TARGET_STREAM, 0).unwrap().high_watermark(),
        3,
        "the target took a copy across a cut link"
    );

    // Restore the link: the target converges, each copy at its source
    // offset, and the lag returns to zero.
    link.cut.store(false, Ordering::SeqCst);
    let restored = mirror.pass(&source, &link, &telemetry);
    assert!(restored.failures.is_empty(), "{:?}", restored.failures);
    assert_eq!(restored.copied, 3);
    assert_eq!(
        target.metadata(TARGET_STREAM, 0).unwrap().high_watermark(),
        6
    );
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_RECORDS), Some(0.0));
    assert_eq!(gauge(&metrics, names::FABRIC_MIRROR_LAG_SECONDS), Some(0.0));
}

/// A label value outside the configured region list or the committed
/// stream set is refused when the mirror is built, so neither lag series can
/// ever carry one.
///
/// Mutation (run, failed, restored): delete the `config.regions.contains`
/// refusal in `Mirror::new` — the foreign region is accepted and the
/// `expect_err` fires; likewise the `config.streams.contains` refusal for
/// the foreign stream.
#[test]
fn a_mirror_naming_a_region_or_stream_outside_its_configuration_is_refused_at_construction() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 31)));
    let config = MirrorConfig::new([REGION.to_string()], [STREAM.to_string()]).unwrap();
    let configured = BTreeMap::from([(STREAM.to_string(), 1)]);
    assert!(
        Mirror::new(&config, REGION, &configured, clock.clone()).is_ok(),
        "premise: the configured region and stream are accepted"
    );

    let region = Mirror::new(&config, "europe-west2", &configured, clock.clone())
        .expect_err("a region outside the configured list was accepted");
    assert!(
        region
            .message()
            .contains("`europe-west2` is not in the configured region list"),
        "{}",
        region.message()
    );

    let foreign = BTreeMap::from([("orders.unlisted".to_string(), 1)]);
    let stream = Mirror::new(&config, REGION, &foreign, clock)
        .expect_err("a stream outside the committed set was accepted");
    assert!(
        stream
            .message()
            .contains("`orders.unlisted` is not in the mirror's committed stream set"),
        "{}",
        stream.message()
    );
}
