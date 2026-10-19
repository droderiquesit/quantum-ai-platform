//! R12-CONTRACT-7089 at the producer's boundary: the drain's `FabricSink`
//! opens every record as a `FabricEnvelope` before it touches the producer,
//! and what it does send is read back off a real broker as the same
//! envelopes.
//!
//! The broker is the `qip-streaming` library on loopback serving the
//! committed local catalogue (as `fabric_wiring.rs` does), the sink is the
//! one `main` builds (`FabricSettings::sink`), and the read-back goes through
//! the codec's `Batch::decode` and `FabricEnvelope::open_all` — the calls the
//! ledger's consumer makes.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::canonical::canonical_json;
use qip_core::{
    Clock, CorrelationId, EventId, Lineage, SystemClock, Timestamp, TraceId, sha256_hex,
};
use qip_edge_node::event_fabric::drain::{FabricSink, JOURNAL_STREAM, Sink};
use qip_edge_node::outbox::{
    FABRIC_PARTITION_VARIABLE, FABRIC_TOKEN_VARIABLE, FABRIC_URL_VARIABLE, read_fabric,
};
use qip_events::AnyEvent;
use qip_events::Topic;
use qip_events::event_fabric::bindings;
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, DecodeOutcome, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::envelope::{FabricContext, FabricEnvelope};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass, StreamPolicy};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service};
use qip_transport::event_fabric::auth::{self, IdentityTable};
use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};
use qip_transport::tls::LinkSecurity;

const CELL: &str = "cell-local";
const IDENTITY: &str = "reflex:cell-local";
const REGION: &str = "us-east";
/// A fixture string inside the bearer alphabet, not a credential for
/// anything.
const TOKEN: &str = "fixture-token-edge-node-fabric-envelope-00000000000000000";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

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

struct Fabric {
    root: PathBuf,
    service: Arc<Service>,
    address: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a reply is still to be lost; see [`LostFirstProduceReply`].
    armed: Arc<std::sync::atomic::AtomicBool>,
}

/// The data plane with the reply to the first produce lost after the
/// broker appended it: the 503 a load balancer returns when the broker
/// committed and the connection behind it died. The producer cannot tell
/// that from a send that never landed, so it retries the same batch.
struct LostFirstProduceReply {
    inner: DataPlane,
    armed: Arc<std::sync::atomic::AtomicBool>,
}

impl Handler for LostFirstProduceReply {
    fn handle(&self, request: &Request) -> Response {
        let answered = self.inner.handle(request);
        if request.path == "/v1/event-fabric/produce"
            && answered.status == 200
            && self.armed.swap(false, Ordering::SeqCst)
        {
            return Response::json(503, r#"{"error":"upstream connection reset"}"#);
        }
        answered
    }
}

impl Fabric {
    fn start() -> Self {
        Self::start_with(false)
    }

    /// [`Self::start`], losing the first produce reply when `lose` is set.
    fn start_with(lose: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "qip-edge-envelope-{}-{}",
            std::process::id(),
            DIRECTORIES.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
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
        let service = Arc::new(Service::new(broker, identities, &catalogue, 1, segments).unwrap());
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(lose));
        let handler: Arc<dyn Handler> = if lose {
            Arc::new(LostFirstProduceReply {
                inner: DataPlane(service.clone()),
                armed: Arc::clone(&armed),
            })
        } else {
            Arc::new(DataPlane(service.clone()))
        };
        let server = Server::bind("127.0.0.1:0", handler, ServerLimits::default()).unwrap();
        let address = server.local_address().unwrap();
        let stop = server.shutdown_handle();
        std::thread::spawn(move || {
            let _ = server.serve();
        });
        service.broker().metadata(JOURNAL_STREAM, 0).unwrap();
        Self {
            root,
            service,
            address,
            stop,
            armed,
        }
    }

    /// The sink `main` builds, pointed at this broker.
    fn sink(&self) -> FabricSink {
        let url = format!("http://{}", self.address);
        let lookup = move |name: &str| match name {
            FABRIC_URL_VARIABLE => Some(url.clone()),
            FABRIC_TOKEN_VARIABLE => Some(TOKEN.to_string()),
            FABRIC_PARTITION_VARIABLE => Some("0".to_string()),
            _ => None,
        };
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        read_fabric(&lookup)
            .unwrap()
            .expect("configured")
            .sink(CELL, REGION, clock, &LinkSecurity::plaintext_for_tests())
            .unwrap()
    }

    /// Every batch the journal stream's partition 0 holds, decoded.
    fn journal(&self) -> Vec<Batch> {
        let broker = self.service.broker();
        let mut batches = Vec::new();
        let mut offset = 0u64;
        loop {
            let fetched = broker
                .fetch(JOURNAL_STREAM, 0, offset, 1024 * 1024)
                .unwrap();
            let bytes = qip_core::hash::from_hex(fetched.batches()).unwrap();
            if bytes.is_empty() {
                return batches;
            }
            let DecodeOutcome::Complete(batch) = Batch::decode(&bytes).unwrap() else {
                panic!("a fetch never answers with a torn batch");
            };
            batches.push(batch);
            offset += 1;
        }
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(&self.address);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn record(id: &str, version: u32, tamper_payload: bool) -> Record {
    let payload = serde_json::json!({ "entry": id });
    let mut event = AnyEvent {
        event_id: EventId::from_string(id),
        topic: Topic::ReflexJournalRecorded,
        schema_version: version,
        occurred_at: Timestamp::from_secs(1_790_000_000),
        recorded_at: Timestamp::from_secs(1_790_000_000),
        sequence: 0,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000077"),
            causation_id: None,
            trace_id: Some(TraceId::new("trace-77")),
            producer: "qip-edge-node.outbox".to_string(),
        },
        idempotency_key: Some(id.to_string()),
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    };
    if tamper_payload {
        event.payload = serde_json::json!({ "entry": "altered after sealing" });
    }
    Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap()
}

/// A batch as the writer leaves it in the spool: writer fields only.
fn spooled(records: Vec<Record>) -> Batch {
    Batch::new(
        MessageType::Data,
        QosClass::P2MarketJournal.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )
    .unwrap()
}

fn reader() -> FabricContext {
    FabricContext {
        stream: JOURNAL_STREAM.to_string(),
        region: "central".to_string(),
        partition: 0,
        qos_class: QosClass::P2MarketJournal,
        auth_context: "consumer-group:test".to_string(),
        provenance: "fabric_envelope test".to_string(),
    }
}

/// A batch the sink sends reaches the broker and is read back as the same
/// envelopes: identity, correlation id, trace id, the producer's stamp and
/// the broker's offset all intact; a second, later batch continues the
/// producer's dense sequence rather than restarting it.
///
/// Mutation: in `FabricSink::deliver`, send a clone with its records
/// reversed (or drop the `producer.send`) and the read-back differs / is
/// empty.
#[test]
fn a_batch_the_sink_sends_is_read_back_from_the_broker_as_the_same_envelopes() {
    let fabric = Fabric::start();
    let mut sink = fabric.sink();
    assert!(
        fabric.journal().is_empty(),
        "premise: the stream starts empty"
    );

    sink.deliver(
        0,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-1", 1, false), record("evt-2", 1, false)]),
    )
    .unwrap();
    sink.deliver(
        1,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-3", 1, false)]),
    )
    .unwrap();

    let held = fabric.journal();
    assert_eq!(held.len(), 2);
    let mut envelopes = Vec::new();
    for batch in &held {
        envelopes.extend(FabricEnvelope::open_all(batch, &reader(), &bindings::ALL).unwrap());
    }
    let ids: Vec<&str> = envelopes.iter().map(|e| e.event_id().as_str()).collect();
    assert_eq!(ids, ["evt-1", "evt-2", "evt-3"]);
    for envelope in &envelopes {
        assert_eq!(envelope.producer_id(), CELL);
        assert_eq!(
            envelope.lineage().correlation_id.as_str(),
            "COR00000000000000000000077"
        );
        assert_eq!(
            envelope.lineage().trace_id.as_ref().map(TraceId::as_str),
            Some("trace-77")
        );
        assert_eq!(envelope.qos_class(), QosClass::P2MarketJournal);
    }
    let sequences: Vec<u64> = envelopes.iter().map(|e| e.producer_sequence()).collect();
    assert_eq!(sequences, [0, 1, 2], "the producer's sequence is dense");
    let offsets: Vec<u64> = envelopes.iter().map(|e| e.offset()).collect();
    assert_eq!(offsets, [0, 0, 1], "the broker assigns an offset per batch");
    let identities: std::collections::BTreeSet<String> =
        envelopes.iter().map(FabricEnvelope::identity).collect();
    assert_eq!(identities.len(), 3, "every envelope has its own identity");
}

/// A batch any of whose records the consumer would refuse is refused by the
/// sink before anything is sent: the stream stays empty (no partial effect,
/// not even the good record that preceded the bad one), and the same sink
/// then delivers a good batch at offset 0 with sequence 0, so the refusal
/// consumed neither.
///
/// Mutation: delete the `FabricEnvelope::open_all` call in
/// `FabricSink::deliver` and the stream holds the bad batch.
#[test]
fn a_batch_with_a_record_the_consumer_would_refuse_is_never_sent_and_leaves_no_trace() {
    let fabric = Fabric::start();
    let mut sink = fabric.sink();

    let cases = [
        (
            "a schema version the reader cannot read",
            record("evt-v", 2, false),
        ),
        ("a payload altered after sealing", record("evt-t", 1, true)),
    ];
    for (what, bad) in cases {
        let batch = spooled(vec![record("evt-good", 1, false), bad]);
        let err = sink
            .deliver(0, QosClass::P2MarketJournal, &batch)
            .unwrap_err();
        assert_eq!(err.code(), "schema", "{what}: {}", err.message());
        assert!(
            fabric.journal().is_empty(),
            "{what}: the broker holds a batch the sink should have refused"
        );
    }

    sink.deliver(
        0,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-after", 1, false)]),
    )
    .unwrap();
    let held = fabric.journal();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].base_offset, 0);
    assert_eq!(
        held[0].base_sequence, 0,
        "the refusals consumed no sequence"
    );
}

/// Re-reading the same offset yields envelopes with the same identities, so
/// a consumer that sees a batch twice (a restart before its commit) can
/// recognise the repeat by identity rather than by position.
///
/// Mutation: derive `identity()` from the offset alone and a second batch
/// that restarts at the same offset would collide; here, include a random
/// component and the two reads differ.
#[test]
fn a_redelivered_read_of_the_same_offset_yields_the_same_envelope_identities() {
    let fabric = Fabric::start();
    let mut sink = fabric.sink();
    sink.deliver(
        0,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-r", 1, false)]),
    )
    .unwrap();
    let first: Vec<String> = fabric
        .journal()
        .iter()
        .flat_map(|b| FabricEnvelope::open_all(b, &reader(), &bindings::ALL).unwrap())
        .map(|e| e.identity())
        .collect();
    let second: Vec<String> = fabric
        .journal()
        .iter()
        .flat_map(|b| FabricEnvelope::open_all(b, &reader(), &bindings::ALL).unwrap())
        .map(|e| e.identity())
        .collect();
    assert_eq!(first.len(), 1, "premise: one envelope was read");
    assert_eq!(first, second);
}

/// FABRIC-8076: the producer never duplicates on retry. The broker appends
/// the first batch and the reply is lost, so the sink's producer retries the
/// same batch under the same producer sequence; the broker recognises it and
/// the stream holds the batch once. A second batch then continues the dense
/// sequence.
///
/// Mutation: in the broker's produce path, accept a sequence it has already
/// committed as new, and the stream holds the first batch twice.
#[test]
fn a_batch_whose_acknowledgement_was_lost_is_retried_and_held_once() {
    let fabric = Fabric::start_with(true);
    let mut sink = fabric.sink();
    assert!(
        fabric.journal().is_empty(),
        "premise: the stream starts empty"
    );

    let first = sink.deliver(
        0,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-1", 1, false), record("evt-2", 1, false)]),
    );
    // Premise: a reply really was lost after the broker appended.
    assert!(
        !fabric.armed.load(Ordering::SeqCst),
        "premise: the first produce reply was never lost"
    );
    // Whether the retry reports success or the broker's duplicate answer,
    // what matters is what the stream holds.
    let _ = first;
    sink.deliver(
        1,
        QosClass::P2MarketJournal,
        &spooled(vec![record("evt-3", 1, false)]),
    )
    .unwrap();

    let held = fabric.journal();
    let mut envelopes = Vec::new();
    for batch in &held {
        envelopes.extend(FabricEnvelope::open_all(batch, &reader(), &bindings::ALL).unwrap());
    }
    let ids: Vec<&str> = envelopes.iter().map(|e| e.event_id().as_str()).collect();
    assert_eq!(
        ids,
        ["evt-1", "evt-2", "evt-3"],
        "a lost acknowledgement duplicated or dropped a record"
    );
    let sequences: Vec<u64> = envelopes.iter().map(|e| e.producer_sequence()).collect();
    assert_eq!(sequences, [0, 1, 2], "the producer's sequence is dense");
}
