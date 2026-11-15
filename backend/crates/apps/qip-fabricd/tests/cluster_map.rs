//! R12-FABRIC-8098 through the composition root: two real `qip-fabricd`
//! instances sharing one managed log, and a client that bootstraps from the
//! configured rung (its SRV lookup is refused, as in `local`), fetches the
//! cluster map from a broker that is the follower, produces to the leader the
//! map names, and — after that leader is killed — follows the lease to the
//! survivor with no change to its configuration.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration as StdDuration, Instant};

use qip_core::{Duration, ManualClock, SystemClock, Timestamp};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, BATCH_SCHEMA_VERSION, QosClass};
use qip_fabricd::config::{self, Config};
use qip_fabricd::{Running, start_with_seams};
use qip_storage::MemoryBlobStore;
use qip_streaming::event_fabric::managed::{ManagedLog, MemoryManagedLog, PartitionRef};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::dns::Bootstrap;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::cluster::{ClusterClient, LeaderFollowingTransport};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

const OUTCOMES: &str = "outcomes.managed";
const JOURNAL: &str = "journal.local";
const WRITER: &str = "writer";
const READER: &str = "reader";
const TTL_MS: &str = "1500";

static ROOTS: AtomicU64 = AtomicU64::new(0);

fn stream(name: &str, class: &str, retention: &str, overload: &str, ack: &str, rf: u32) -> String {
    format!(
        r#"{{"name":"{name}","qos_class":"{class}","partition_key":"cell",
            "ordering":"per_partition","retention":"{retention}","replication_factor":{rf},
            "mirroring":"none","overload_policy":"{overload}","ack_profile":"{ack}",
            "byte_quota_per_producer":67108864,"message_quota_per_producer":1000000,
            "lag_limit":1000000,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":20,"peak_bytes_per_second":1048576,"topics":[]}}"#
    )
}

fn catalogue(outcomes_ack: &str, outcomes_rf: u32) -> String {
    let streams = [
        stream(
            OUTCOMES,
            "p1_outcomes",
            "irreplaceable",
            "refuse_producer",
            outcomes_ack,
            outcomes_rf,
        ),
        stream(
            JOURNAL,
            "p2_market_journal",
            "event_anchored",
            "throttle_with_gap",
            "leader_only",
            1,
        ),
    ]
    .join(",");
    format!(
        r#"{{"streams":[{streams}],"grants":[
            {{"identity":"{WRITER}","stream":"{OUTCOMES}","permission":"produce","key_scope":"any"}},
            {{"identity":"{READER}","stream":"{OUTCOMES}","permission":"consume","key_scope":"any"}}]}}"#
    )
}

fn token(identity: &str) -> String {
    format!("managed-fixture-token-for-{identity}-000000000000000000000")
}

/// A started instance and the directory it owns.
struct Instance {
    root: PathBuf,
    running: Option<Running>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn timeouts() -> Timeouts {
    Timeouts::new(
        StdDuration::from_secs(5),
        StdDuration::from_secs(30),
        StdDuration::from_secs(30),
    )
}

fn batch(payload: &str) -> Batch {
    Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: format!("evt-{payload}"),
            trace_id: None,
            source_timestamp_ns: 1,
            payload: payload.as_bytes().to_vec(),
        }],
    )
    .unwrap()
}

/// Reserve a loopback port. The cluster map names an instance by address, and
/// the address must be in configuration before the instance starts, so the
/// port cannot be left to the kernel at bind time.
fn free_address() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

fn start_named(
    name: &str,
    catalogue: &str,
    log: Arc<dyn ManagedLog>,
    listen: &str,
    advertise: &str,
) -> Instance {
    let unique = ROOTS.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "qip-fabricd-cluster-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("streams.json"), catalogue).unwrap();
    let identities = [WRITER, READER]
        .iter()
        .map(|id| {
            format!(
                "{id} {}\n",
                qip_core::hash::sha256_hex(token(id).as_bytes())
            )
        })
        .collect::<String>();
    std::fs::write(root.join("identities"), identities).unwrap();
    let path = |n: &str| root.join(n).display().to_string();
    let variables: BTreeMap<String, String> = [
        (config::DATA_DIR, path("data")),
        (config::CATALOGUE, path("streams.json")),
        (config::IDENTITIES_FILE, path("identities")),
        (config::ARCHIVE_DIR, path("archive")),
        (config::LISTEN, listen.to_string()),
        (config::HEALTH_LISTEN, "127.0.0.1:0".to_string()),
        (config::PARTITIONS, "1".to_string()),
        (config::HOUSEKEEPING_MS, "20".to_string()),
        (config::DEPLOYMENT, "local".to_string()),
        (
            qip_transport::tls::TLS_MODE_VARIABLE,
            "plaintext".to_string(),
        ),
        (config::MANAGED_LOG, "spanner".to_string()),
        (
            config::SPANNER_DATABASE,
            "projects/qip-test/instances/fabric/databases/fabric".to_string(),
        ),
        (config::REGION, "europe-west2".to_string()),
        (config::INSTANCE, name.to_string()),
        (config::INSTANCES, "zone-a,zone-b".to_string()),
        (config::ADVERTISE, advertise.to_string()),
        (config::LEASE_TTL_MS, TTL_MS.to_string()),
        (
            qip_storage::gcp::ENDPOINT_VARIABLE,
            "http://127.0.0.1:9".to_string(),
        ),
        (qip_storage::gcp::METADATA_VARIABLE, "1".to_string()),
    ]
    .into_iter()
    .map(|(n, v)| (n.to_string(), v))
    .collect();
    let running = Config::parse(&variables)
        .and_then(|config| {
            start_with_seams(
                &config,
                Arc::new(SystemClock),
                Some(Arc::new(MemoryBlobStore::new())),
                Some(log),
            )
        })
        .expect("the instance starts");
    Instance {
        root,
        running: Some(running),
    }
}

fn cluster_client(configured: Vec<String>) -> ClusterClient {
    ClusterClient::new(
        Bootstrap::new(configured, Arc::new(SystemClock)).unwrap(),
        Box::new(|| {
            Err(qip_core::error::Error::unavailable(
                "no SRV in a local deployment",
            ))
        }),
        Box::new(|authority| {
            Ok(Box::new(HttpTransport::new(
                format!("http://{authority}"),
                BearerToken::new(token(WRITER))?,
            )))
        }),
        timeouts(),
    )
}

fn leader_name(log: &MemoryManagedLog) -> Option<String> {
    log.lease_of(&PartitionRef::new(OUTCOMES, 0))
        .map(|(holder, _)| holder.split('#').next().unwrap().to_string())
}

/// R12-FABRIC-8098. The client is configured with both addresses, the
/// follower first, and nothing about who leads. It fetches the cluster map
/// before producing and sends to the leader the map names; the leader is
/// killed, database time passes the lease, and the same client finds the
/// survivor and its produce is acknowledged at the next lease epoch, dense.
///
/// Mutation: in `Service::answer`'s `Request::ClusterMap` arm, set every
/// `leader` to `None` — the client keeps the follower it asked and the first
/// produce is refused. Mutation: in `ClusterClient::call`, drop the refresh
/// after a failure — the post-kill produce never leaves the dead leader.
#[test]
fn a_client_fetches_the_map_before_producing_and_follows_a_leader_change() {
    let database_time = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 11, 14)));
    let log = Arc::new(MemoryManagedLog::new(database_time.clone()));
    let (a, b) = (free_address(), free_address());
    let advertise = format!("zone-a={a},zone-b={b}");
    let catalogue = catalogue("managed_majority", 3);
    let mut inst_a = start_named("zone-a", &catalogue, log.clone(), &a, &advertise);
    let mut inst_b = start_named("zone-b", &catalogue, log.clone(), &b, &advertise);

    let deadline = Instant::now() + StdDuration::from_secs(30);
    let leader = loop {
        if let Some(name) = leader_name(&log) {
            break name;
        }
        assert!(Instant::now() < deadline, "no instance took the lease");
        std::thread::sleep(StdDuration::from_millis(20));
    };
    let (leader_addr, follower_addr) = if leader == "zone-a" {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    };
    assert_ne!(leader_addr, follower_addr, "premise: two distinct brokers");

    // Wait until the follower has seen the leader, so its map can name it.
    let mut probe = cluster_client(vec![follower_addr.clone()]);
    let deadline = Instant::now() + StdDuration::from_secs(30);
    loop {
        probe.refresh().unwrap();
        if probe.leader_authority(OUTCOMES, 0).unwrap() == leader_addr {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the follower never named the leader: {:?}",
            probe.map()
        );
        std::thread::sleep(StdDuration::from_millis(20));
    }
    assert_ne!(
        probe.leader_authority(OUTCOMES, 0).unwrap(),
        follower_addr,
        "premise: the broker asked is not the leader, so the map is what routes"
    );

    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(LeaderFollowingTransport::new(
            cluster_client(vec![follower_addr.clone(), leader_addr.clone()]),
            OUTCOMES,
            0,
        )),
        stream: OUTCOMES.to_string(),
        partition: 0,
        producer_id: WRITER.to_string(),
        qos_class: QosClass::P1Outcomes,
        ack_profile: AckProfile::ManagedMajority,
        retry_policy: RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        },
        breaker_policy: BreakerPolicy::default(),
        clock: Arc::new(SystemClock),
        sleeper: Arc::new(ThreadSleeper),
        retry_seed: 3,
        breaker_seed: 3,
        timeouts: timeouts(),
    })
    .unwrap();
    producer
        .init()
        .expect("init reaches the leader the map names");
    for n in 0..3 {
        let ack = producer.send(batch(&format!("fill-{n}"))).unwrap();
        assert_eq!(ack.base_offset(), n);
        assert_eq!(ack.managed_commit_epoch(), Some(1));
    }

    let dead = if leader == "zone-a" {
        &mut inst_a
    } else {
        &mut inst_b
    };
    dead.running.take().unwrap().stop();
    // The lease TTL, plus the placement grace (a third of it) a
    // non-preferred survivor waits after expiry.
    database_time.advance(Duration::from_millis(2_100));

    // Wait for the survivor to take the lease before the client tries again:
    // a producer's backoff is what paces it in production, and a loop that
    // hammered the dead leader would only open the producer's own breaker.
    let survivor = if leader == "zone-a" {
        "zone-b"
    } else {
        "zone-a"
    };
    let deadline = Instant::now() + StdDuration::from_secs(60);
    while leader_name(&log).as_deref() != Some(survivor) {
        assert!(
            Instant::now() < deadline,
            "the survivor never took the lease"
        );
        std::thread::sleep(StdDuration::from_millis(20));
    }
    let deadline = Instant::now() + StdDuration::from_secs(60);
    let ack = loop {
        match producer.send(batch("fill-3")) {
            Ok(ack) => break ack,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(StdDuration::from_millis(50));
                let _ = producer.init();
            }
            Err(error) => panic!("the client never followed the leader change: {error}"),
        }
    };
    assert_eq!(ack.base_offset(), 3, "dense across the leader change");
    assert_eq!(ack.managed_commit_epoch(), Some(2));
    assert_eq!(
        leader_name(&log).as_deref(),
        Some(if leader == "zone-a" {
            "zone-b"
        } else {
            "zone-a"
        })
    );
}
