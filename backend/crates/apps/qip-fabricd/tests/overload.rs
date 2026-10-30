//! R12-FABRIC-8089, the clause the stream quotas could not hold: topic
//! priority on the one resource every stream shares, the protocol listener's
//! connection slots.
//!
//! `qip_fabricd::data_plane_limits` caps the listener at the transport's 64
//! connections. That cap was class-blind. A telemetry producer that opened
//! enough connections held every slot while every stream quota was honoured
//! — the quota is charged when a batch is admitted, and these connections
//! never got that far — and the next P0 produce was answered `503` at the
//! accept loop. Nothing shed the telemetry; the control traffic was starved
//! instead, which is the inversion the requirement forbids.
//!
//! Since `PRIORITY_RESERVED_CONNECTIONS` the listener holds a quarter of the
//! cap for callers the catalogue grants a P0 or P1 stream. A caller without
//! one is classed bulk when its request head is read and, with the bulk share
//! full, answered `503` before its body is read — so a P4 flood fills three
//! quarters of the pool and no more, and the controller's grant goes through.
//!
//! Nothing here places, amends or cancels an order. The fabric stores and
//! serves batches of bytes other processes wrote.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use qip_core::{Duration, SystemClock};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_fabricd::config::Config;
use qip_fabricd::{PRIORITY_RESERVED_CONNECTIONS, Running, data_plane_limits, start_with_archive};
use qip_storage::MemoryBlobStore;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::Route;
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};

const CONTROL: &str = "control.overload";
const TELEMETRY: &str = "telemetry.overload";
const CONTROLLER: &str = "release-controller";
const AGENT: &str = "agent:telemetry-1";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

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

/// One P0 stream the controller may produce to and one P4 stream the agent
/// may: the agent holds no grant on any P0 or P1 stream, which is what
/// makes it bulk.
fn catalogue_json() -> String {
    let streams = [
        stream_json(
            CONTROL,
            "p0_control",
            "irreplaceable",
            "refuse_producer",
            "archived",
        ),
        stream_json(
            TELEMETRY,
            "p4_telemetry",
            "derived_state",
            "sample_or_shed",
            "none",
        ),
    ]
    .join(",");
    let grants = [
        format!(
            r#"{{"identity":"{CONTROLLER}","stream":"{CONTROL}","permission":"produce","key_scope":"any"}}"#
        ),
        format!(
            r#"{{"identity":"{AGENT}","stream":"{TELEMETRY}","permission":"produce","key_scope":"any"}}"#
        ),
    ]
    .join(",");
    format!(r#"{{"streams":[{streams}],"grants":[{grants}]}}"#)
}

/// A fixture string inside the bearer alphabet, not a credential for
/// anything; different per broker so a port reused across tests cannot be
/// answered by a stranger.
fn token(fabric: u64, identity: &str) -> String {
    format!(
        "fixture-token-{fabric}-for-{}-000000000000000000000000",
        identity.replace(':', "-")
    )
}

struct Fabric {
    number: u64,
    root: PathBuf,
    running: Option<Running>,
}

impl Fabric {
    fn start(label: &str) -> Self {
        let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "qip-fabricd-overload-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("streams.json"), catalogue_json()).unwrap();
        let identities = [CONTROLLER, AGENT]
            .iter()
            .map(|identity| {
                format!(
                    "{identity} {}\n",
                    qip_core::hash::sha256_hex(token(unique, identity).as_bytes())
                )
            })
            .collect::<String>();
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

    fn address(&self) -> String {
        self.running.as_ref().unwrap().address().to_string()
    }

    fn producer(&self, identity: &str, stream: &str, class: QosClass) -> Producer {
        Producer::new(ProducerConfig {
            transport: Box::new(HttpTransport::new(
                format!("http://{}", self.address()),
                BearerToken::new(token(self.number, identity)).unwrap(),
            )),
            stream: stream.to_string(),
            partition: 0,
            producer_id: identity.to_string(),
            qos_class: class,
            ack_profile: class.ack_floor(),
            // Bounded well inside the listener's 15-second read timeout: a
            // producer that only got through once the stalled connections
            // timed out would pass this test with the reserve deleted.
            retry_policy: RetryPolicy {
                max_attempts: 40,
                initial_backoff: Duration::from_millis(100),
                max_backoff: Duration::from_millis(100),
                multiplier: 1,
                jitter_basis_points: 0,
            },
            breaker_policy: BreakerPolicy::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            retry_seed: 7,
            breaker_seed: 7,
            timeouts: Timeouts::new(
                StdDuration::from_secs(5),
                StdDuration::from_secs(20),
                StdDuration::from_secs(20),
            ),
        })
        .unwrap()
    }

    /// A connection that has sent the head of an agent's produce request —
    /// enough for the listener to class it — and none of the body it
    /// declared, so it holds whatever slot it was given until the listener
    /// gives up on the body. Returns the socket and what the listener did
    /// with it within `wait`.
    ///
    /// The listener also caps how many peers may be between accept and the
    /// end of their head at once (`max_handshaking`), and closes the rest
    /// with nothing written. Sixty-four heads sent together trip that cap,
    /// which is a different bound from the one under test, so a connection
    /// closed unanswered is opened again until it is held or shed.
    fn stalled_agent_connection(&self, wait: StdDuration) -> (TcpStream, Stall) {
        for _ in 0..200 {
            let (socket, stall) = self.try_stalled_agent_connection(wait);
            if stall != Stall::Dropped {
                return (socket, stall);
            }
            std::thread::sleep(StdDuration::from_millis(10));
        }
        panic!("the listener closed two hundred head-only connections unanswered");
    }

    fn try_stalled_agent_connection(&self, wait: StdDuration) -> (TcpStream, Stall) {
        let mut socket = TcpStream::connect(self.address()).expect("the listener accepts");
        let head = format!(
            "POST {} HTTP/1.1\r\nhost: fabric\r\nauthorization: Bearer {}\r\n\
             content-type: application/json\r\ncontent-length: 65536\r\n\r\n",
            Route::Produce.path(),
            token(self.number, AGENT)
        );
        socket.write_all(head.as_bytes()).unwrap();
        socket.flush().unwrap();
        socket.set_read_timeout(Some(wait)).unwrap();
        let mut answer = Vec::new();
        let mut chunk = [0u8; 256];
        let stall = loop {
            match socket.read(&mut chunk) {
                Ok(0) if answer.is_empty() => break Stall::Dropped,
                Ok(0) => break Stall::Answered(parse_status(&answer)),
                Ok(n) => {
                    answer.extend_from_slice(&chunk[..n]);
                    if answer.windows(4).any(|w| w == b"\r\n\r\n") {
                        break Stall::Answered(parse_status(&answer));
                    }
                }
                Err(_) => break Stall::Held,
            }
        };
        (socket, stall)
    }
}

/// What the listener did with a head-only connection within the wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stall {
    /// Nothing within the wait: the listener is holding the connection for
    /// its body.
    Held,
    /// A status line came back, 503 for a shed connection.
    Answered(Option<u16>),
    /// Closed with nothing written: the handshake cap, not a slot decision.
    Dropped,
}

impl Drop for Fabric {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn parse_status(answer: &[u8]) -> Option<u16> {
    let text = String::from_utf8_lossy(answer);
    text.split_whitespace().nth(1)?.parse().ok()
}

fn one_record(class: QosClass, tag: &str) -> Batch {
    Batch::new(
        MessageType::Data,
        class.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: tag.to_string(),
            trace_id: None,
            source_timestamp_ns: 1_700_000_000_000_000_000,
            payload: br#"{"kind":"fixture"}"#.to_vec(),
        }],
    )
    .unwrap()
}

/// Mutation: set `reserved_for_priority: 0` in `data_plane_limits` (the cap
/// is class-blind again) and every stalled connection holds a slot, the
/// controller's produce is answered `503` at the accept loop until its
/// bounded retries run out, and this fails at "a P0 produce commits".
/// Mutation: make `Service::admission_class` answer `Priority` for every
/// caller and the flood is never shed, with the same failure.
#[test]
fn p4_producers_holding_every_connection_slot_cannot_starve_a_p0_commit() {
    let limits = data_plane_limits();
    assert_eq!(limits.reserved_for_priority, PRIORITY_RESERVED_CONNECTIONS);
    assert!(
        limits.reserved_for_priority > 0 && limits.reserved_for_priority < limits.max_concurrent,
        "premise: the data plane reserves some of its {} slots, not none and not all",
        limits.max_concurrent
    );
    let fabric = Fabric::start("starve");

    // Premise: with no flood, the agent's own produce is served. The flood
    // below is shed for want of slots, not for want of a grant.
    let mut agent = fabric.producer(AGENT, TELEMETRY, QosClass::P4Telemetry);
    agent.init().expect("premise: the agent initialises");
    agent
        .send(one_record(QosClass::P4Telemetry, "telemetry-quiet"))
        .expect("premise: the agent produces when nothing is flooding");

    // The flood: as many stalled agent connections as the listener has
    // slots, opened together, each classed bulk from its credential and
    // holding its slot until the listener's read timeout, long after this
    // test is over.
    let flood: Vec<(TcpStream, Stall)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..limits.max_concurrent)
            .map(|_| {
                let fabric = &fabric;
                scope.spawn(move || fabric.stalled_agent_connection(StdDuration::from_secs(3)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let shed = flood
        .iter()
        .filter(|(_, stall)| *stall == Stall::Answered(Some(503)))
        .count();
    let held = flood
        .iter()
        .filter(|(_, stall)| *stall == Stall::Held)
        .count();
    assert_eq!(
        held,
        limits.max_concurrent - limits.reserved_for_priority,
        "premise: the flood fills the whole bulk share and holds it"
    );
    assert_eq!(
        shed, limits.reserved_for_priority,
        "every connection past the bulk share is answered 503 before its body is read"
    );

    // The clause: with the bulk share full, the controller's P0 produce —
    // an init and a produce, each a fresh connection — still commits.
    let mut controller = fabric.producer(CONTROLLER, CONTROL, QosClass::P0Control);
    controller
        .init()
        .expect("a P0 producer initialises while P4 holds every bulk slot");
    let ack = controller
        .send(one_record(QosClass::P0Control, "grant-under-flood"))
        .expect("a P0 produce commits while P4 holds every bulk slot");
    assert_eq!(ack.base_offset(), 0);

    // And a further agent connection is still shed: the reserve is the
    // controller's, not the next arrival's.
    let (_socket, stall) = fabric.stalled_agent_connection(StdDuration::from_secs(3));
    assert_eq!(
        stall,
        Stall::Answered(Some(503)),
        "the bulk share stays full while the flood holds it"
    );

    drop(flood);
}
