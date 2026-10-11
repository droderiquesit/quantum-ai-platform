//! FABRIC-7044: deterministic fault and partition testing of the fabric's
//! replicated path (ADR 0111), seeded so a failing schedule is a failing
//! seed anyone can replay.
//!
//! Three brokers — three `qip-fabricd` instances in three zones — share one
//! managed log, whose clock is the database's time. Each step of a seeded
//! schedule does one of: produce through whichever broker the producer
//! believes leads, run one broker's lease pass, let database time pass, kill
//! a broker and restart it on its own disk, or cut a broker off from the
//! database and heal it. After every step and at the end the suite checks
//! the three properties ADR 0111 exists for:
//!
//! 1. **No acknowledged record is lost.** Every acknowledgement a producer
//!    received is in the managed log at the offset it was given, with the
//!    payload it was given for.
//! 2. **No two instances commit in one epoch, and epochs never go back.**
//!    The managed log's rows carry non-decreasing lease epochs, and each
//!    epoch was used by exactly one holder.
//! 3. **Every broker's disk is a prefix of the committed log**, byte for
//!    byte — a deposed or partitioned broker never holds a batch the managed
//!    log did not commit.
//!
//! The schedule runs at the broker library, not over sockets, so each step
//! is synchronous and the seed alone decides the interleaving; the socket
//! path is held by `tests/managed.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Clock, Duration, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering as Ord_, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::managed::{ManagedLog, MemoryManagedLog, PartitionRef};

const STREAM: &str = "outcomes.chaos";
const TTL_MS: u64 = 3_000;
const ZONES: [&str; 3] = ["zone-a", "zone-b", "zone-c"];
const STEPS: u32 = 400;
const SEEDS: u64 = 6;

static DIRS: AtomicU64 = AtomicU64::new(0);

fn policy() -> StreamPolicy {
    StreamPolicy::new(StreamPolicySpec {
        qos_class: QosClass::P1Outcomes,
        partition_key: "cell".to_string(),
        ordering: Ord_::PerPartition,
        retention: qip_events::RetentionClass::Irreplaceable,
        replication_factor: 3,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: AckProfile::ManagedMajority,
        byte_quota_per_producer: 1 << 30,
        message_quota_per_producer: 1 << 30,
        lag_limit: 1 << 30,
        entitlement: Entitlement::new("qip-test", "research").unwrap(),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 30,
    })
    .unwrap()
}

struct Zone {
    dir: PathBuf,
    broker: Option<Broker>,
}

struct Cluster {
    time: Arc<ManualClock>,
    log: Arc<MemoryManagedLog>,
    zones: Vec<Zone>,
}

impl Cluster {
    fn new(seed: u64) -> Self {
        let time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 11)));
        let log = Arc::new(MemoryManagedLog::new(time.clone()));
        let mut cluster = Self {
            time,
            log,
            zones: Vec::new(),
        };
        for zone in ZONES {
            let unique = DIRS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "qip-chaos-{seed}-{zone}-{}-{unique}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            cluster.zones.push(Zone { dir, broker: None });
        }
        for index in 0..ZONES.len() {
            cluster.start(index);
        }
        cluster
    }

    /// (Re)start a zone's broker on its own disk: a new process, so a new
    /// local leader epoch and so a new lease-holder name.
    fn start(&mut self, index: usize) {
        let clock: Arc<dyn Clock> = self.time.clone();
        let log: Arc<dyn ManagedLog> = self.log.clone();
        let broker = Broker::open(&self.zones[index].dir, clock.clone())
            .unwrap()
            .with_managed_log(log, ZONES[index])
            .unwrap();
        broker
            .declare_stream(
                STREAM,
                1,
                policy(),
                SegmentLogConfig::new(clock).with_archive_required(true),
            )
            .unwrap();
        broker
            .register_schema(STREAM, 1, 1, Shape::of(&1u32).unwrap())
            .unwrap();
        self.zones[index].broker = Some(broker);
    }

    fn kill(&mut self, index: usize) {
        self.zones[index].broker = None;
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        for zone in &mut self.zones {
            zone.broker = None;
            let _ = std::fs::remove_dir_all(&zone.dir);
        }
    }
}

fn batch(sequence: u64, payload: &[u8]) -> Batch {
    let mut batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("chaos-{sequence}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: payload.to_vec(),
        }],
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-chaos", 1, sequence);
    batch
}

fn decode(bytes: &[u8]) -> Batch {
    match Batch::decode(bytes).unwrap() {
        DecodeOutcome::Complete(batch) => batch,
        DecodeOutcome::Torn => panic!("a committed row decodes complete"),
    }
}

/// The three properties, checked against the managed log as the record.
fn check(
    cluster: &Cluster,
    acknowledged: &BTreeMap<u64, Vec<u8>>,
    seed: u64,
    step: u32,
    disks: bool,
) {
    let partition = PartitionRef::new(STREAM, 0);
    let through = cluster.log.committed_through(&partition).unwrap();
    let mut committed = Vec::new();
    while (committed.len() as u64) < through {
        committed.extend(
            cluster
                .log
                .read(&partition, committed.len() as u64, 64)
                .unwrap(),
        );
    }
    for (offset, payload) in acknowledged {
        let row = committed
            .get(usize::try_from(*offset).unwrap())
            .unwrap_or_else(|| {
                panic!("seed {seed} step {step}: acknowledged offset {offset} lost")
            });
        assert_eq!(
            &decode(&row.bytes).records[0].payload,
            payload,
            "seed {seed} step {step}: offset {offset} holds another record than was acknowledged"
        );
    }
    let epochs = cluster.log.committed_epochs(&partition);
    assert!(
        epochs.windows(2).all(|pair| pair[0] <= pair[1]),
        "seed {seed} step {step}: lease epochs went backwards: {epochs:?}"
    );
    for zone in &cluster.zones {
        let Some(broker) = &zone.broker else { continue };
        let local = broker.metadata(STREAM, 0).unwrap().high_watermark();
        assert!(
            local <= through,
            "seed {seed} step {step}: a broker holds {local} batches, the log committed {through}"
        );
        // A diverged disk stays diverged, so comparing every byte every
        // step buys nothing a periodic comparison misses.
        if !disks {
            continue;
        }
        for offset in 0..local {
            let hex = broker.fetch(STREAM, 0, offset, 1).unwrap();
            let bytes = qip_core::hash::from_hex(hex.batches()).unwrap();
            assert_eq!(
                bytes,
                committed[usize::try_from(offset).unwrap()].bytes,
                "seed {seed} step {step}: a broker's disk diverges at {offset}"
            );
        }
    }
}

/// Mutation: in `MemoryManagedLog::commit`, drop the lease-epoch check (commit
/// whenever the offset is next) — a deposed broker that still believes it
/// leads commits under its stale epoch after a takeover, epochs go backwards
/// and property 2 fires within the first seeds. Mutation: in
/// `Broker::produce_managed`, append locally before committing — a
/// partitioned leader's disk gains a batch the log never committed and
/// property 3 fires.
#[test]
fn seeded_kills_and_partitions_lose_no_acknowledged_record_and_never_split_an_epoch() {
    for seed in 0..SEEDS {
        let mut rng = Xoshiro256::seeded(0xFAB1_C0DE ^ seed);
        let mut cluster = Cluster::new(seed);
        let mut acknowledged: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        let mut sequence = 0u64;
        let mut cut: BTreeSet<String> = BTreeSet::new();
        let mut epochs_by_holder: BTreeMap<u64, String> = BTreeMap::new();
        let mut faults = 0u32;

        for step in 0..STEPS {
            let zone = (rng.next_u64() % ZONES.len() as u64) as usize;
            match rng.next_u64() % 100 {
                // Produce through a broker the producer picked; only the
                // leader can acknowledge, and a retry of a refused sequence
                // reuses it.
                0..=44 => {
                    if let Some(broker) = &cluster.zones[zone].broker {
                        let payload = format!("seed-{seed}-seq-{sequence}").into_bytes();
                        if let Ok(Ok(ack)) = broker.produce_to(STREAM, 0, batch(sequence, &payload))
                        {
                            let epoch = ack.managed_commit_epoch().expect("a managed ack");
                            let holder = broker.managed_holder().unwrap().to_string();
                            let first = epochs_by_holder
                                .entry(epoch)
                                .or_insert_with(|| holder.clone());
                            assert_eq!(
                                first, &holder,
                                "seed {seed} step {step}: two holders acknowledged in epoch {epoch}"
                            );
                            acknowledged.insert(ack.base_offset(), payload);
                            sequence += 1;
                        }
                    }
                }
                45..=69 => {
                    if let Some(broker) = &cluster.zones[zone].broker {
                        let _ = broker.tend_leases(TTL_MS);
                    }
                }
                70..=81 => cluster
                    .time
                    .advance(Duration::from_millis((rng.next_u64() % 2_000) as i64)),
                82..=89 => {
                    faults += 1;
                    if cluster.zones[zone].broker.is_some() {
                        cluster.kill(zone);
                    } else {
                        cluster.start(zone);
                    }
                }
                _ => {
                    faults += 1;
                    if let Some(holder) = cluster.zones[zone]
                        .broker
                        .as_ref()
                        .and_then(|b| b.managed_holder().map(str::to_string))
                    {
                        let isolate = !cut.contains(&holder);
                        cluster.log.partition_from_database(&holder, isolate);
                        if isolate {
                            cut.insert(holder);
                        } else {
                            cut.remove(&holder);
                        }
                    }
                }
            }
            check(&cluster, &acknowledged, seed, step, step % 25 == 0);
        }

        // Heal everything, bring every zone back, and let leases settle: the
        // fabric must still make progress and every disk must converge.
        for holder in &cut {
            cluster.log.partition_from_database(holder, false);
        }
        for index in 0..ZONES.len() {
            if cluster.zones[index].broker.is_none() {
                cluster.start(index);
            }
        }
        cluster
            .time
            .advance(Duration::from_millis(i64::try_from(TTL_MS).unwrap() + 1));
        for zone in &cluster.zones {
            let _ = zone.broker.as_ref().unwrap().tend_leases(TTL_MS);
        }
        for zone in &cluster.zones {
            zone.broker.as_ref().unwrap().tend_leases(TTL_MS).unwrap();
        }
        let leaders: Vec<usize> = (0..ZONES.len())
            .filter(|i| {
                let broker = cluster.zones[*i].broker.as_ref().unwrap();
                broker
                    .produce_to(STREAM, 0, batch(sequence, b"after-healing"))
                    .is_ok_and(|answer| answer.is_ok())
            })
            .collect();
        assert_eq!(
            leaders.len(),
            1,
            "seed {seed}: exactly one leader after healing"
        );
        for zone in &cluster.zones {
            zone.broker.as_ref().unwrap().tend_leases(TTL_MS).unwrap();
        }
        check(&cluster, &acknowledged, seed, STEPS, true);
        assert!(
            !acknowledged.is_empty() && faults > 10,
            "premise: seed {seed} acknowledged {} records through {faults} faults",
            acknowledged.len()
        );
    }
}
