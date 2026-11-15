//! R12-FABRIC-8078: the mirror worker over two real listeners on one host —
//! the networked source, the producer into the target region's
//! region-qualified stream, and the checkpoint file — each standing in for a
//! region (ADR 0119 §3; nothing here is deployed, and a second region is not
//! authorised by ADR 0035).
//!
//! The paper boundary is not near this file: the fabric stores and serves
//! batches of bytes other processes wrote, and a mirrored fill is still the
//! source region's simulated fill.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use qip_core::{Duration, SystemClock};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::QosClass;
use qip_fabricd::config::Config;
use qip_fabricd::mirror::{Mirror, MirrorConfig, MirrorSource};
use qip_fabricd::mirror_link::{FileCheckpoints, MirrorWorker, RemoteSource, RemoteTarget};
use qip_fabricd::telemetry::FabricdTelemetry;
use qip_fabricd::{Running, start_with_archive};
use qip_observability::metrics::Metrics;
use qip_storage::MemoryBlobStore;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

const REGION: &str = "us-east4";
const STREAM: &str = "outcomes.mirrored";
const TARGET_STREAM: &str = "us-east4.outcomes.mirrored";
const CELL: &str = "cell-a";
const READER: &str = "mirror-reader";
const WRITER: &str = "mirror-writer";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn stream_json(name: &str) -> String {
    format!(
        r#"{{"name":"{name}","qos_class":"p1_outcomes","partition_key":"cell",
            "ordering":"per_partition","retention":"irreplaceable","replication_factor":1,
            "mirroring":"none","overload_policy":"refuse_producer","ack_profile":"archived",
            "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
            "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}"#
    )
}

fn grant(identity: &str, stream: &str, permission: &str) -> String {
    format!(
        r#"{{"identity":"{identity}","stream":"{stream}","permission":"{permission}","key_scope":"any"}}"#
    )
}

fn token(fabric: u64, identity: &str) -> String {
    format!("fixture-token-{fabric}-for-{identity}-0000000000000000000000000000")
}

struct Fabric {
    number: u64,
    root: PathBuf,
    running: Option<Running>,
}

impl Fabric {
    /// A fabric declaring `stream`, with `producers` allowed to write it and
    /// `readers` allowed to read it.
    fn start(label: &str, stream: &str, producers: &[&str], readers: &[&str]) -> Self {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "qip-fabricd-mirror-link-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let grants: Vec<String> = producers
            .iter()
            .map(|who| grant(who, stream, "produce"))
            .chain(readers.iter().map(|who| grant(who, stream, "consume")))
            .collect();
        std::fs::write(
            root.join("streams.json"),
            format!(
                r#"{{"streams":[{}],"grants":[{}]}}"#,
                stream_json(stream),
                grants.join(",")
            ),
        )
        .unwrap();
        let identities: String = producers
            .iter()
            .chain(readers)
            .map(|identity| {
                format!(
                    "{identity} {}\n",
                    qip_core::hash::sha256_hex(token(unique, identity).as_bytes())
                )
            })
            .collect();
        std::fs::write(root.join("identities"), identities).unwrap();
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
        let config = Config::parse(&variables).expect("the suite's configuration parses");
        let running = start_with_archive(
            &config,
            Arc::new(SystemClock),
            Some(Arc::new(MemoryBlobStore::new())),
        )
        .expect("the suite's own catalogue starts a broker");
        Self {
            number: unique,
            root,
            running: Some(running),
        }
    }

    fn transport(&self, identity: &str) -> Box<HttpTransport> {
        Box::new(HttpTransport::new(
            format!("http://{}", self.running.as_ref().unwrap().address()),
            BearerToken::new(token(self.number, identity)).unwrap(),
        ))
    }

    fn timeouts() -> Timeouts {
        Timeouts::new(
            StdDuration::from_secs(1),
            StdDuration::from_secs(5),
            StdDuration::from_secs(5),
        )
    }

    fn source(&self, identity: &str) -> RemoteSource {
        RemoteSource::new(self.transport(identity), Self::timeouts())
    }

    fn producer(&self, identity: &str, stream: &str) -> Producer {
        let mut producer = Producer::new(ProducerConfig {
            transport: self.transport(identity),
            stream: stream.to_string(),
            partition: 0,
            producer_id: identity.to_string(),
            qos_class: QosClass::P1Outcomes,
            ack_profile: QosClass::P1Outcomes.ack_floor(),
            retry_policy: RetryPolicy {
                max_attempts: 40,
                initial_backoff: Duration::from_millis(50),
                max_backoff: Duration::from_millis(50),
                multiplier: 1,
                jitter_basis_points: 0,
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 7,
            breaker_seed: 7,
            timeouts: Self::timeouts(),
        })
        .unwrap();
        producer.init().unwrap();
        producer
    }
}

fn batch(sequence: u64) -> Batch {
    Batch::new(
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
    .unwrap()
}

/// A worker copying `STREAM` from `source` into `target`, checkpointing to a
/// file under `source.root`.
fn worker(source: &Fabric, target: &Fabric) -> (MirrorWorker, FileCheckpoints, Arc<Metrics>) {
    let clock: Arc<dyn qip_core::Clock> = Arc::new(SystemClock);
    let config = MirrorConfig::new([REGION.to_string()], [STREAM.to_string()]).unwrap();
    let mirror = Mirror::new(
        &config,
        REGION,
        &BTreeMap::from([(STREAM.to_string(), 1)]),
        clock,
    )
    .unwrap();
    let target_for_factory = target.number;
    let address = target.running.as_ref().unwrap().address().to_string();
    let factory = Box::new(move |stream: &str, partition: u32| {
        assert_eq!(partition, 0);
        Producer::new(ProducerConfig {
            transport: Box::new(HttpTransport::new(
                format!("http://{address}"),
                BearerToken::new(token(target_for_factory, WRITER)).unwrap(),
            )),
            stream: stream.to_string(),
            partition,
            producer_id: WRITER.to_string(),
            qos_class: QosClass::P1Outcomes,
            ack_profile: QosClass::P1Outcomes.ack_floor(),
            // One attempt: a peer that is down must fail the lane, not hold
            // the pass on a retry ladder.
            retry_policy: RetryPolicy {
                max_attempts: 40,
                initial_backoff: Duration::from_millis(50),
                max_backoff: Duration::from_millis(50),
                multiplier: 1,
                jitter_basis_points: 0,
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 7,
            breaker_seed: 7,
            timeouts: Fabric::timeouts(),
        })
    });
    let store = FileCheckpoints::new(source.root.join("mirror.checkpoint"));
    let metrics = Arc::new(Metrics::new("qip-fabricd"));
    let worker = MirrorWorker::new(
        mirror,
        Box::new(source.source(READER)),
        Box::new(RemoteTarget::new(factory, target.source(READER))),
        store.clone(),
        FabricdTelemetry::new(metrics.clone()),
    )
    .unwrap();
    (worker, store, metrics)
}

fn held(target: &Fabric) -> u64 {
    target
        .source(READER)
        .committed_through(TARGET_STREAM, 0)
        .unwrap()
}

/// A partition produced in the source region appears in the target region's
/// region-qualified stream, record for record at the source's offsets; then
/// the peer is stopped and the source keeps acknowledging while the mirror
/// reports the lane failed and the lag grown.
///
/// Mutation (run, failed, restored): in `RemoteTarget::produce_copy`, return
/// `Ok(offset)` without calling `producer.send` — the target holds nothing,
/// and the first `held(&target) == 3` assertion fails.
#[test]
fn a_mirrored_partition_appears_in_the_peer_and_stopping_the_peer_never_stops_local_produce() {
    let source = Fabric::start("src", STREAM, &[CELL], &[READER]);
    let mut target = Fabric::start("dst", TARGET_STREAM, &[WRITER], &[READER]);
    let (mut worker, _, _) = worker(&source, &target);
    let mut cell = source.producer(CELL, STREAM);

    for sequence in 0..3 {
        cell.send(batch(sequence)).unwrap();
    }
    assert_eq!(held(&target), 0, "premise: the target holds nothing yet");

    let report = worker.step().unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.copied, 3);
    assert_eq!(held(&target), 3);
    let copies = target
        .source(READER)
        .fetch_committed(TARGET_STREAM, 0, 0, 1 << 20)
        .unwrap();
    let ids: Vec<String> = copies
        .iter()
        .map(|row| match Batch::decode(&row.bytes).unwrap() {
            qip_events::event_fabric::codec::DecodeOutcome::Complete(b) => {
                b.records[0].event_id.clone()
            }
            qip_events::event_fabric::codec::DecodeOutcome::Torn => panic!("torn"),
        })
        .collect();
    assert_eq!(ids, ["fill-0", "fill-1", "fill-2"]);

    // Stop the peer. The cell's produce path never learns the mirror exists.
    target.running.as_mut().unwrap().stop_data_plane();
    let started = std::time::Instant::now();
    for sequence in 3..6 {
        let ack = cell
            .send(batch(sequence))
            .expect("local produce survived the peer's stop");
        assert_eq!(ack.base_offset(), sequence);
    }
    assert!(
        started.elapsed() < StdDuration::from_secs(10),
        "local produce waited on the stopped peer: {:?}",
        started.elapsed()
    );
    let down = worker.step().unwrap();
    assert_eq!(down.copied, 0, "a copy crossed to a stopped peer");
    assert!(
        !down.failures.is_empty(),
        "premise: the pass met the stopped peer"
    );
    assert_eq!(
        worker.mirror().checkpoint(STREAM, 0).unwrap().next_offset,
        3
    );
}

/// A restart resumes from the file; a checkpoint lost between the target's
/// acknowledgement and the save does not duplicate a record.
///
/// Mutation (run, failed, restored): delete the `committed_through` probe
/// block at the top of `RemoteTarget::produce_copy` — the stale-checkpoint
/// pass produces three duplicates, the target's high watermark reads 6, and
/// the `held(&target) == 3` assertion fails.
#[test]
fn a_restart_resumes_from_its_checkpoint_and_a_stale_one_duplicates_nothing() {
    let source = Fabric::start("src", STREAM, &[CELL], &[READER]);
    let target = Fabric::start("dst", TARGET_STREAM, &[WRITER], &[READER]);
    let mut cell = source.producer(CELL, STREAM);
    let (mut first, store, _) = worker(&source, &target);
    for sequence in 0..3 {
        cell.send(batch(sequence)).unwrap();
    }
    assert_eq!(first.step().unwrap().copied, 3);
    assert!(store.path().exists(), "premise: the checkpoint was written");
    drop(first);

    // Restart: two more records, and a new worker over the same file copies
    // only those.
    for sequence in 3..5 {
        cell.send(batch(sequence)).unwrap();
    }
    let (mut second, _, _) = worker(&source, &target);
    assert_eq!(
        second.mirror().checkpoint(STREAM, 0).unwrap().next_offset,
        3
    );
    let resumed = second.step().unwrap();
    assert!(resumed.failures.is_empty(), "{:?}", resumed.failures);
    assert_eq!(resumed.copied, 2);
    assert_eq!(held(&target), 5);

    // Lose the checkpoint after the target took the copies: the new worker
    // starts at zero, and the target is asked before anything is produced.
    std::fs::remove_file(store.path()).unwrap();
    let (mut third, _, _) = worker(&source, &target);
    assert_eq!(third.mirror().checkpoint(STREAM, 0).unwrap().next_offset, 0);
    let stale = third.step().unwrap();
    assert!(stale.failures.is_empty(), "{:?}", stale.failures);
    assert_eq!(
        held(&target),
        5,
        "a stale checkpoint duplicated records in the target"
    );
    assert_eq!(third.mirror().checkpoint(STREAM, 0).unwrap().next_offset, 5);
}

/// A checkpoint file that does not parse stops the worker rather than
/// restarting a region's history from zero.
///
/// Mutation (run, failed, restored): make `FileCheckpoints::load` skip a
/// line that does not parse — `load` answers an empty first run and the
/// `expect_err` fires.
#[test]
fn a_corrupt_checkpoint_file_is_refused_not_read_as_a_first_run() {
    let source = Fabric::start("src", STREAM, &[CELL], &[READER]);
    let target = Fabric::start("dst", TARGET_STREAM, &[WRITER], &[READER]);
    let path = source.root.join("mirror.checkpoint");
    std::fs::write(&path, "outcomes.mirrored 0 three -\n").unwrap();
    let store = FileCheckpoints::new(&path);
    let refusal = store.load().expect_err("a corrupt checkpoint was read");
    assert!(
        refusal.message().contains("fix or remove the file"),
        "{}",
        refusal.message()
    );
    let _ = target;
}
