//! FABRIC-10076 and FABRIC-10102 at the composition root: the real
//! `qip-edge-node` binary, started with `QIP_FABRIC_URL`, ships its journal
//! to the regional fabric — every entry to the P2 journal stream and the
//! outcome stream's chain coverage to P1 — through the writer, relay, spool
//! and drain `main` builds.
//!
//! `tests/event_fabric_outbox.rs` proves the outbox's parts: that `ship`
//! never waits on a stalled writer, and that every entry reaches P2 and
//! every outcome P1 with its digest. Neither proves `main` constructs them
//! with a fabric sink. A `main` that built the spool and drained it only to
//! the tick lake would pass every test there while no deployed cell's
//! outcome ever reached the ledger. So this starts the process, against a
//! broker on loopback serving the committed local catalogue
//! (`infrastructure/event-fabric/streams.local.json`), and reads what the
//! broker holds.
//!
//! The broker is composed from the `qip-streaming` library rather than the
//! `qip-fabricd` app, because nothing may depend on an app
//! (`.claude/rules/architecture/00-boundaries.md`). Its housekeeping is the
//! seal-and-archive pass `qip-fabricd` runs, because P1 is acknowledged only
//! once archived and a producer that is never acknowledged ships nothing.
//!
//! Nothing here reaches a venue. The node runs the simulated feed, which is
//! the only feed under which it runs a pass (ADR 0003).

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use qip_core::{Clock, SystemClock, sha256_hex};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
use qip_events::event_fabric::policy::StreamPolicy;
use qip_storage::MemoryBlobStore;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service, TopiclessStreams};
use qip_transport::event_fabric::auth::{self, IdentityTable};
use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};

const OUTCOMES: &str = "reflex-outcomes.local";
const JOURNAL: &str = "reflex-journal.local";
/// The cell identity the committed local catalogue grants.
const IDENTITY: &str = "reflex:cell-local";
const CELL: &str = "cell-local";
const EVENTUALLY: Duration = Duration::from_secs(90);

/// A fixture string inside the bearer alphabet, not a credential for
/// anything.
const TOKEN: &str = "fixture-token-edge-node-fabric-wiring-000000000000000000";

struct DataPlane(Arc<Service>);

impl Handler for DataPlane {
    fn handle(&self, request: &Request) -> Response {
        let handled = self.0.handle(
            request.header(auth::HEADER),
            None,
            &request.path,
            &request.body,
        );
        Response::new(handled.status, "application/json", handled.body)
    }
}

/// A one-partition broker on loopback serving the committed local
/// catalogue, with `qip-fabricd`'s seal-and-archive pass on a thread.
struct Fabric {
    root: PathBuf,
    service: Arc<Service>,
    address: String,
    stop: Arc<AtomicBool>,
    stop_server: Arc<AtomicBool>,
}

impl Fabric {
    fn start(root: PathBuf) -> Self {
        let catalogue = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../infrastructure/event-fabric/streams.local.json"),
        )
        .expect("the committed local catalogue");
        let catalogue = Catalogue::parse(&catalogue).unwrap();
        let identities =
            IdentityTable::parse(&format!("{IDENTITY} {}\n", sha256_hex(TOKEN.as_bytes())))
                .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let broker = Arc::new(Broker::open(root.join("broker"), clock.clone()).unwrap());
        let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
            SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required())
        });
        let service = Arc::new(
            Service::new(
                broker,
                identities,
                &catalogue,
                1,
                segments,
                TopiclessStreams::AdmitAll,
            )
            .unwrap(),
        );
        let server = Server::bind(
            "127.0.0.1:0",
            Arc::new(DataPlane(service.clone())),
            ServerLimits::default(),
        )
        .unwrap();
        let address = server.local_address().unwrap();
        let stop_server = server.shutdown_handle();
        std::thread::spawn(move || {
            let _ = server.serve();
        });
        // Open both partitions here, once, before two threads can race to
        // open one lazily: a segment log admits one writer per directory.
        for stream in [OUTCOMES, JOURNAL] {
            service.broker().metadata(stream, 0).unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let broker = service.broker().clone();
            let stop = stop.clone();
            let archiver = Archiver::new(Arc::new(MemoryBlobStore::new())).with_manifest_index();
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = broker.seal_aged();
                    let _ = broker.archive_sealed(&archiver);
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
        }
        Self {
            root,
            service,
            address,
            stop,
            stop_server,
        }
    }

    /// The topic of every record `stream` holds on partition 0.
    fn topics(&self, stream: &str) -> BTreeSet<String> {
        let broker = self.service.broker();
        let mut topics = BTreeSet::new();
        let mut offset = 0u64;
        loop {
            let fetched = broker.fetch(stream, 0, offset, 1024 * 1024).unwrap();
            let bytes = qip_core::hash::from_hex(fetched.batches()).unwrap();
            if bytes.is_empty() {
                return topics;
            }
            let mut rest = bytes.as_slice();
            while !rest.is_empty() {
                let DecodeOutcome::Complete(batch) = Batch::decode(rest).unwrap() else {
                    panic!("a fetch never answers with a torn batch");
                };
                rest = &rest[batch.encode().unwrap().len()..];
                for record in &batch.records {
                    let event = record.decode_payload(batch.encoding).unwrap();
                    topics.insert(event.topic.to_string());
                }
                offset += 1;
            }
        }
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stop_server.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.address);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "qip-edge-node-fabric-wiring-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// FABRIC-10076 / FABRIC-10102: the binary started with a fabric endpoint
/// produces its journal to the P2 journal stream and its outcome-stream
/// chain coverage to P1, under the cell's own producer identity, while it
/// keeps deciding.
///
/// Mutation: in `main`, drop the `sinks.push(... fabric.sink(...))` arm
/// (the spool drains to the tick lake only, as it does with no endpoint) —
/// neither stream receives a record and the wait times out.
#[test]
fn a_node_given_a_fabric_endpoint_ships_its_journal_to_p2_and_its_chain_to_p1() {
    let dir = scratch("ships");
    let fabric = Fabric::start(dir.join("fabric"));
    let key = dir.join("capital-envelope-key");
    std::fs::write(&key, "a-cell-envelope-key-for-tests").unwrap();
    let journal = dir.join("journal");
    std::fs::create_dir_all(&journal).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_qip-edge-node"))
        .env_clear()
        .env("QIP_CELL_ID", CELL)
        .env("QIP_CELL_REGION", "europe-west2")
        // `local`, because the test owns both ends of a loopback socket and
        // only a local deployment may choose a plaintext fabric link.
        .env("QIP_DEPLOYMENT", "local")
        .env(qip_transport::tls::TLS_MODE_VARIABLE, "plaintext")
        .env("QIP_VENUES", "CX")
        .env("QIP_REGION_ALLOCATION", "1000000")
        .env("QIP_CAPITAL_ENVELOPE_KEY_FILE", &key)
        .env("QIP_STORAGE_TARGET", "engine")
        .env("QIP_STORAGE_ROOT", &journal)
        .env("QIP_HEALTH_PORT", "0")
        // A flag whose directory does not exist reads as engaged (fail
        // closed), so the cell journals a halt: a decision entry, which a
        // cell with no signed policy would otherwise never write.
        .env(
            qip_edge_node::halt::FLAG_VARIABLE,
            dir.join("no-such-mount").join("halt"),
        )
        .env(qip_edge_node::feed::FEED_VARIABLE, "simulated")
        .env(
            qip_edge_node::outbox::FABRIC_URL_VARIABLE,
            format!("http://{}", fabric.address),
        )
        .env(qip_edge_node::outbox::FABRIC_TOKEN_VARIABLE, TOKEN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");

    let deadline = Instant::now() + EVENTUALLY;
    let shipped = loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "the node exited ({status}) before shipping: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let journal = fabric.topics(JOURNAL);
        let outcomes = fabric.topics(OUTCOMES);
        if journal.contains("reflex.journal_recorded") && !outcomes.is_empty() {
            break (journal, outcomes);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "timed out waiting for the node to ship to the fabric; journal topics \
                 {journal:?}, outcome topics {outcomes:?}; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        alive,
        "the node was still deciding when the fabric held its journal"
    );

    let (journal_topics, outcome_topics) = shipped;
    // P1 carries outcomes and the chain spans that cover every run between
    // them, and nothing a P2 stream carries.
    assert!(
        outcome_topics.iter().all(|topic| [
            "reflex.outcome_recorded",
            "reflex.chain_span",
            "reflex.fabric_gap"
        ]
        .contains(&topic.as_str())),
        "{outcome_topics:?}"
    );
    assert!(
        !journal_topics.contains("reflex.chain_span"),
        "{journal_topics:?}"
    );
    // The producer the broker recorded is the cell, so a second incarnation
    // of it is fenced rather than interleaved.
    let next = fabric
        .service
        .broker()
        .next_sequence(JOURNAL, 0, CELL)
        .unwrap();
    assert!(next > 0, "the journal was produced under the cell's own id");
}
