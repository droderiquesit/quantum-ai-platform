//! R12-FABRIC-7034: a broker applies each stream's retention policy with no
//! operator figure at all.
//!
//! `archiver::retain` returned before calling the broker whenever
//! `QIP_EVENT_FABRIC_HOT_BATCHES` was unset, and no committed deployment sets
//! it. So the broker every deployment would start applied no stream's
//! policy: a transient stream's raw ticks and an event-anchored stream's
//! ninety-day-old book state stayed on its disk for as long as the disk had
//! room, which is the unbounded retention the product direction forbids.
//!
//! The first test goes through the composition root, `start_with_archive`,
//! the function `main` reaches through `start`, with that variable unset. The
//! second calls the two functions the housekeeping thread calls, in the
//! thread's order, because the thread itself cannot show an unarchived
//! segment surviving: it skips retention on any pass whose archive step
//! failed, so the broker's own guard would never be the thing under test.
//!
//! Nothing here places, amends or cancels an order. The fabric stores and
//! serves batches of bytes other processes wrote.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::RetentionClass;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{
    AckProfile, BATCH_SCHEMA_VERSION, Entitlement, Mirroring, Ordering as StreamOrdering,
    OverloadPolicy, QosClass, StreamPolicy, StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_fabricd::archiver::{self, SealCounts};
use qip_fabricd::config::Config;
use qip_fabricd::start_with_archive;
use qip_fabricd::telemetry::FabricdTelemetry;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_storage::{BlobStore, MemoryBlobStore};
use qip_streaming::event_fabric::broker::Broker;

const JOURNAL: &str = "journal.retention";
const TICKS: &str = "ticks.retention";
const FEATURES: &str = "features.retention";
const CONTROL: &str = "control.retention";

const EVENTUALLY: StdDuration = StdDuration::from_secs(60);
/// Long enough for many housekeeping passes at the 20 ms cadence below.
const MANY_PASSES: StdDuration = StdDuration::from_millis(600);

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn temp_root(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "qip-fabricd-retention-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn stream_json(name: &str, class: &str, retention: &str, overload: &str, ack: &str) -> String {
    format!(
        r#"{{"name":"{name}","qos_class":"{class}","partition_key":"cell",
            "ordering":"per_partition","retention":"{retention}","replication_factor":1,
            "mirroring":"none","overload_policy":"{overload}","ack_profile":"{ack}",
            "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
            "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}"#
    )
}

/// One stream of each kind of bound: a span (`event_anchored`, rolling
/// ninety days), a ring retained nowhere (`transient`), a fixed in-memory
/// size (`derived_state`) and permanent (`irreplaceable`).
fn catalogue_json() -> String {
    let streams = [
        stream_json(
            JOURNAL,
            "p2_market_journal",
            "event_anchored",
            "throttle_with_gap",
            "leader_only",
        ),
        stream_json(TICKS, "p4_telemetry", "transient", "sample_or_shed", "none"),
        stream_json(
            FEATURES,
            "p4_telemetry",
            "derived_state",
            "sample_or_shed",
            "none",
        ),
        stream_json(
            CONTROL,
            "p0_control",
            "irreplaceable",
            "refuse_producer",
            "archived",
        ),
    ]
    .join(",");
    format!(
        r#"{{"streams":[{streams}],"grants":[{{"identity":"operator","stream":"{JOURNAL}","permission":"admin","key_scope":"any"}}]}}"#
    )
}

fn send(broker: &Broker, stream: &str, class: QosClass, sequences: std::ops::Range<u64>) {
    for sequence in sequences {
        let mut batch = Batch::new(
            MessageType::Data,
            class.batch_schema_id(),
            BATCH_SCHEMA_VERSION,
            PayloadCodec::CanonicalJson,
            vec![Record {
                event_id: format!("{stream}-{sequence}"),
                trace_id: None,
                source_timestamp_ns: 1,
                payload: vec![u8::try_from(sequence).unwrap(); 1500],
            }],
        )
        .unwrap();
        stamp_drain(&mut batch, "cell-a", 1, sequence);
        broker.produce_to(stream, 0, batch).unwrap().unwrap();
    }
}

fn starts(broker: &Broker, stream: &str) -> Vec<u64> {
    broker.sealed_segment_starts(stream, 0).unwrap()
}

fn served(broker: &Broker, stream: &str, offset: u64) -> bool {
    !broker
        .fetch(stream, 0, offset, 1)
        .map(|response| response.batches().to_string())
        .unwrap_or_default()
        .is_empty()
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(StdDuration::from_millis(10));
    }
}

/// With no hot window configured, the housekeeping thread the composition
/// root spawns still applies each stream's own policy: the transient stream
/// is cut to the segment the next produce chains to, and the event-anchored
/// stream loses exactly the archived segments older than its ninety days —
/// not at eighty-nine days, and not the segments sealed since. Streams whose
/// class names no bound a broker can apply keep everything.
///
/// Mutation: in `archiver::retain`, return `Ok(0)` when `hot_batches` is
/// `None` (the early return this replaces) — the transient stream is never
/// trimmed and the first wait times out.
/// Mutation: in `Broker::retain`, skip the `if let Some(age) = age` block —
/// the journal keeps its ninety-one-day-old segments and its wait times out.
/// Mutation: in `Broker::retain`, compare the segment's age with eighty-nine
/// days instead of the class's span — the segments go a day early and the
/// eighty-nine-day assertion fails.
#[test]
fn with_no_hot_window_configured_each_stream_is_still_trimmed_by_its_own_retention_class() {
    let root = temp_root("root");
    std::fs::write(root.join("streams.json"), catalogue_json()).unwrap();
    std::fs::write(
        root.join("identities"),
        format!(
            "operator {}\n",
            qip_core::hash::sha256_hex(b"fixture-token-for-operator-000000000000000000000000")
        ),
    )
    .unwrap();
    let variables: BTreeMap<String, String> = [
        (qip_fabricd::config::DATA_DIR, root.join("data")),
        (qip_fabricd::config::CATALOGUE, root.join("streams.json")),
        (
            qip_fabricd::config::IDENTITIES_FILE,
            root.join("identities"),
        ),
        (qip_fabricd::config::ARCHIVE_DIR, root.join("archive")),
    ]
    .into_iter()
    .map(|(name, path)| (name.to_string(), path.display().to_string()))
    .chain(
        [
            (qip_fabricd::config::LISTEN, "127.0.0.1:0"),
            (qip_fabricd::config::HEALTH_LISTEN, "127.0.0.1:0"),
            (qip_fabricd::config::PARTITIONS, "1"),
            (qip_fabricd::config::SEGMENT_BYTES, "4096"),
            (qip_fabricd::config::HOUSEKEEPING_MS, "20"),
            (qip_fabricd::config::DEPLOYMENT, "local"),
            (qip_transport::tls::TLS_MODE_VARIABLE, "plaintext"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string())),
    )
    .collect();
    assert!(
        !variables.contains_key(qip_fabricd::config::HOT_BATCHES),
        "premise: the operator configured no hot window"
    );
    let config = Config::parse(&variables).unwrap();
    assert_eq!(config.hot_batches, None, "premise: no hot window");

    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 1, 1)));
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let running = start_with_archive(&config, clock.clone(), Some(store)).unwrap();
    let broker = running.service().broker().clone();

    // ~1.5 kB a batch into 4 kB segments: a sealed segment every few
    // appends, so twelve batches is several sealed segments a stream.
    send(&broker, JOURNAL, QosClass::P2MarketJournal, 0..12);
    send(&broker, TICKS, QosClass::P4Telemetry, 0..12);
    send(&broker, FEATURES, QosClass::P4Telemetry, 0..12);
    send(&broker, CONTROL, QosClass::P0Control, 0..12);

    // The transient stream: only the segment holding the newest batch may
    // stay. The derived-state stream took the same twelve batches and is
    // the premise that there were several segments to cut.
    wait_until("the transient stream is cut to one segment", || {
        starts(&broker, TICKS).len() <= 1 && !served(&broker, TICKS, 0)
    });
    let features = starts(&broker, FEATURES);
    assert!(
        features.len() >= 3 && features[0] == 0,
        "premise: the same batches sealed several segments: {features:?}"
    );
    assert!(served(&broker, TICKS, 11), "the newest batch is kept");

    // The journal is archived and young: nothing of it may go.
    let old = starts(&broker, JOURNAL);
    assert!(old.len() >= 3 && old[0] == 0, "premise: {old:?}");
    wait_until("every sealed journal segment is archived", || {
        broker.metadata(JOURNAL, 0).unwrap().archived_through() > *old.last().unwrap()
    });
    std::thread::sleep(MANY_PASSES);
    assert_eq!(
        starts(&broker, JOURNAL),
        old,
        "an archived segment inside its stream's span stays"
    );

    // One day short of the span: still nothing.
    clock.advance(Duration::from_days(89));
    send(&broker, JOURNAL, QosClass::P2MarketJournal, 12..20);
    // Counted as segments that were not there before, so this wait does
    // not depend on the old ones still being present: that is the next
    // assertion's finding, not this premise's.
    wait_until("the journal seals segments at day 89", || {
        starts(&broker, JOURNAL)
            .iter()
            .filter(|start| !old.contains(start))
            .count()
            >= 2
    });
    let young: Vec<u64> = starts(&broker, JOURNAL)
        .into_iter()
        .filter(|start| !old.contains(start))
        .collect();
    assert!(
        young.len() >= 2,
        "premise: segments sealed at day 89: {young:?}"
    );
    std::thread::sleep(MANY_PASSES);
    let at_89 = starts(&broker, JOURNAL);
    assert!(
        old.iter().all(|start| at_89.contains(start)),
        "eighty-nine days is inside a ninety-day span: {at_89:?}"
    );

    // Day 91: the first segments are past the span, the later ones two days
    // old.
    clock.advance(Duration::from_days(2));
    wait_until(
        "the journal's segments past ninety days are removed",
        || {
            let now = starts(&broker, JOURNAL);
            old.iter().all(|start| !now.contains(start))
        },
    );
    std::thread::sleep(MANY_PASSES);
    let after = starts(&broker, JOURNAL);
    assert!(
        old.iter().all(|start| !after.contains(start)),
        "every segment older than the span is gone: {after:?}"
    );
    assert!(
        young.iter().all(|start| after.contains(start)),
        "every segment sealed inside the span stays: {young:?} in {after:?}"
    );
    assert_eq!(
        after.first(),
        young.first(),
        "the cut is exactly at the first segment inside the span"
    );
    // Removed after archival, not instead of it: the oldest batch is still
    // served, from the archive.
    assert!(served(&broker, JOURNAL, 0), "archived history is served");

    // A class with no bound a broker can apply keeps everything, at any age.
    assert_eq!(starts(&broker, FEATURES), features);
    assert_eq!(starts(&broker, CONTROL).first(), Some(&0));
    assert!(starts(&broker, CONTROL).len() >= 3);

    running.stop();
    let _ = std::fs::remove_dir_all(&root);
}

fn anchored_policy() -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P2MarketJournal,
        partition_key: "cell".to_string(),
        ordering: StreamOrdering::PerPartition,
        retention: RetentionClass::EventAnchored,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::ThrottleWithGap,
        ack_profile: AckProfile::LeaderOnly,
        byte_quota_per_producer: 1 << 26,
        message_quota_per_producer: 1 << 20,
        lag_limit: 1 << 20,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 24,
    })
    .unwrap()
}

/// A segment older than its stream's span that has not been archived stays
/// on disk, and goes on the first retention after the pass that archives it.
/// Age alone would otherwise delete the only copy of ninety-one-day-old
/// history the moment the archive fell behind.
///
/// Two layers hold this and either alone is enough, so one mutation does not
/// fire: `Broker::retain` pulls its boundary back to the first unarchived or
/// unindexed segment, and `SegmentLog::retain_before` withholds a segment
/// `mark_archived` has not recorded.
/// Mutation: in `Broker::retain`, replace `if archive_required` with
/// `if false`, **and** in `SegmentLog::retain_before` pass `false` for
/// `self.config.archive_required` — the first `retain` deletes the
/// unarchived segments and the assertion that it removed none fails. With
/// only the first of the two, this test passes.
#[test]
fn a_segment_past_its_streams_span_stays_until_it_is_archived_and_goes_after() {
    let root = temp_root("guard");
    let clock = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 1, 1)));
    let store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let shared: Arc<dyn Clock> = clock.clone();
    let broker = Broker::open(root.join("data"), shared.clone())
        .unwrap()
        .with_archive_reader(store.clone());
    let policy = anchored_policy();
    assert!(policy.archive_required(), "premise: the class is archived");
    broker
        .declare_stream(
            JOURNAL,
            1,
            policy,
            SegmentLogConfig::new(shared)
                .with_archive_required(true)
                .with_roll_after_bytes(1),
        )
        .unwrap();
    broker
        .register_schema(
            JOURNAL,
            QosClass::P2MarketJournal.batch_schema_id(),
            BATCH_SCHEMA_VERSION,
            Shape::of(&1u32).unwrap(),
        )
        .unwrap();
    send(&broker, JOURNAL, QosClass::P2MarketJournal, 0..12);
    let telemetry = FabricdTelemetry::new(Arc::new(qip_observability::Metrics::new(
        "qip-fabricd-test",
    )));

    let sealed = starts(&broker, JOURNAL);
    assert!(sealed.len() >= 3 && sealed[0] == 0, "premise: {sealed:?}");
    clock.advance(Duration::from_days(91));
    assert_eq!(
        broker.metadata(JOURNAL, 0).unwrap().archived_through(),
        0,
        "premise: nothing is archived yet"
    );

    assert_eq!(
        archiver::retain(&broker, None, &telemetry).unwrap(),
        0,
        "an unarchived segment is not removed, however old"
    );
    assert_eq!(starts(&broker, JOURNAL), sealed);

    let archived = archiver::pass(
        &broker,
        &Archiver::new(store).with_manifest_index(),
        &telemetry,
        &SealCounts::default(),
    )
    .unwrap();
    assert!(archived >= sealed.len() as u64, "premise: {archived}");
    let removed = archiver::retain(&broker, None, &telemetry).unwrap();
    assert_eq!(
        removed,
        sealed.len() as u64,
        "each segment past the span is removed once archived"
    );
    let after = starts(&broker, JOURNAL);
    assert!(
        sealed.iter().all(|start| !after.contains(start)),
        "{after:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
