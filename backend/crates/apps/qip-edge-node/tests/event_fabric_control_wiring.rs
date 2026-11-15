//! R12-FABRIC-10077 at the composition seam: a signed capital grant the
//! centre publishes to the regional fabric's P0 `control.local` stream is
//! consumed by the cell from there — over the real HTTP data plane, from a
//! real `qip-streaming` broker serving the committed local catalogue — and
//! applied at a pass boundary, through the node's own settings, token and
//! wiring (`FabricSettings::control_consumer`, `ControlWiring::spawn`).
//!
//! `tests/event_fabric_control.rs` proves the consumer against a scripted
//! broker; nothing there proves the node can *build* a consumer from its
//! configuration, that the committed catalogue grants the cell's identity the
//! read, or that a record a producer put on a real P0 stream reaches the cell.
//! A `main` that never constructed the consumer would pass all of it, which
//! is how the row sat with "no shipped cell consumes control.local".
//!
//! The broker is composed from the `qip-streaming` library, not the
//! `qip-fabricd` app, because nothing may depend on an app. Nothing here
//! reaches a venue.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{
    Clock, CorrelationId, Decimal, Duration, Id, Lineage, ManualClock, SystemClock, Timestamp, dec,
    sha256_hex,
};
use qip_edge::TrustRoot;
use qip_edge::cell::{Cell, CellConfig};
use qip_edge::envelope::sign_payload;
use qip_edge::mesh::CapitalGrantTopic;
use qip_edge_node::control::{Applied, Barrier, ControlKind, Outcome, Provenance};
use qip_edge_node::event_fabric::control::{CONTROL_STREAM, ControlWiring};
use qip_edge_node::event_fabric::telemetry::OutboxTelemetry;
use qip_edge_node::outbox::{
    FABRIC_CONTROL_TOKEN_VARIABLE, FABRIC_TOKEN_VARIABLE, FABRIC_URL_VARIABLE, read_fabric,
};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{QosClass, StreamPolicy};
use qip_events::{Envelope, EventBody, Topic};
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::state::MarketState;
use qip_observability::metrics::Metrics;
use qip_storage::MemoryBlobStore;
use qip_storage::segment::archive::Archiver;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service, TopiclessStreams};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::{self, BearerToken, IdentityTable};
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::transport::{HttpTransport, Timeouts};
use qip_transport::retry::{RetryPolicy, ThreadSleeper};
use qip_transport::server::{Handler, Request, Response, Server, ServerLimits};
use qip_transport::tls::LinkSecurity;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

const CELL: &str = "cell-local";
const REGION: &str = "local";
const STRATEGY: &str = "mean-reversion-1";
const KEY: &[u8] = b"control-wiring-test-envelope-key";
/// Fixture strings inside the bearer alphabet; not credentials for anything.
const CELL_CONSUME_TOKEN: &str = "fixture-token-cell-local-consume-0000000000000000000000";
const CELL_PRODUCE_TOKEN: &str = "fixture-token-cell-local-produce-0000000000000000000000";
const CONTROLLER_TOKEN: &str = "fixture-token-release-controller-00000000000000000000";
const DEADLINE: StdDuration = StdDuration::from_secs(30);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

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
    address: String,
    stop: Arc<AtomicBool>,
    stop_server: Arc<AtomicBool>,
}

impl Fabric {
    fn start(test: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "qip-edge-node-control-wiring-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let catalogue = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../infrastructure/event-fabric/streams.local.json"),
        )
        .expect("the committed local catalogue");
        let catalogue = Catalogue::parse(&catalogue).unwrap();
        // The identities the committed catalogue's grants name: the cell's
        // consume identity on control.local, and the release controller that
        // may produce to it. The cell's produce identity is a third token.
        let identities = IdentityTable::parse(&format!(
            "reflex:cell-local {}\nrelease-controller {}\n",
            sha256_hex(CELL_CONSUME_TOKEN.as_bytes()),
            sha256_hex(CONTROLLER_TOKEN.as_bytes()),
        ))
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
        service.broker().metadata(CONTROL_STREAM, 0).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let broker = service.broker().clone();
            let stop = stop.clone();
            let archiver = Archiver::new(Arc::new(MemoryBlobStore::new())).with_manifest_index();
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = broker.seal_aged();
                    let _ = broker.archive_sealed(&archiver);
                    std::thread::sleep(StdDuration::from_millis(50));
                }
            });
        }
        Self {
            root,
            address,
            stop,
            stop_server,
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct GrantBody(CapitalEnvelope);

impl EventBody for GrantBody {
    const TOPIC: Topic = CapitalGrantTopic::TOPIC;
    const SCHEMA_VERSION: u32 = CapitalGrantTopic::SCHEMA_VERSION;
}

fn grant(gross: Decimal, key: Option<&[u8]>) -> CapitalEnvelope {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(STRATEGY),
            CELL,
            gross,
            dec!("400"),
            dec!("50000"),
            vec![VenueId::new("XLON")],
            t(0),
            t(7_200),
            "alice@example.com",
            signature,
        )
        .unwrap()
    };
    let unsigned = build("unsigned");
    match key {
        Some(key) => build(&sign_payload(key, &unsigned.signing_payload())),
        None => unsigned,
    }
}

/// The centre's side: a P0 producer, as the release controller, publishing
/// `envelope` to `control.local` and returning once the broker acknowledged
/// it as archived.
fn publish(fabric: &Fabric, envelope: CapitalEnvelope, event_id: &str) {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let transport = HttpTransport::over(
        &LinkSecurity::plaintext_for_tests(),
        &fabric.address,
        BearerToken::resolve("TEST", Some(CONTROLLER_TOKEN.to_string()), None).unwrap(),
    )
    .unwrap();
    let mut producer = Producer::new(ProducerConfig {
        transport: Box::new(transport),
        stream: CONTROL_STREAM.to_string(),
        partition: 0,
        producer_id: "release-controller".to_string(),
        qos_class: QosClass::P0Control,
        ack_profile: QosClass::P0Control.ack_floor(),
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock,
        sleeper: Arc::new(ThreadSleeper),
        retry_seed: 1,
        breaker_seed: 2,
        timeouts: Timeouts::default(),
    })
    .unwrap();
    let event = Envelope::new(
        Id::from_string(event_id.to_string()),
        t(0),
        t(0),
        Lineage::root(
            CorrelationId::from_string(format!("COR{event_id}")),
            "release-controller",
        ),
        GrantBody(envelope),
    )
    .erase()
    .unwrap();
    let record = Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap();
    let batch = Batch::new(
        MessageType::Data,
        QosClass::P0Control.batch_schema_id(),
        qip_events::event_fabric::policy::BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap();
    producer.init().unwrap();
    producer.send(batch).unwrap();
}

fn settings(fabric: &Fabric, control_token: Option<&str>) -> qip_edge_node::outbox::FabricSettings {
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    vars.insert(FABRIC_URL_VARIABLE.to_string(), fabric.url());
    vars.insert(
        FABRIC_TOKEN_VARIABLE.to_string(),
        CELL_PRODUCE_TOKEN.to_string(),
    );
    if let Some(token) = control_token {
        vars.insert(FABRIC_CONTROL_TOKEN_VARIABLE.to_string(), token.to_string());
    }
    read_fabric(&|name| vars.get(name).cloned())
        .unwrap()
        .expect("a configured fabric")
}

fn funded_cell() -> Cell {
    use qip_contracts::signal::SignalKind;
    use qip_core::ObjectId;
    use qip_edge::envelope::VerifiedEnvelope;
    use qip_strategy::catalogue::FeatureCatalogue;
    use qip_strategy::compile::StrategyCompiler;
    use qip_strategy::ir::{Expr, Rule, StrategySpec};

    let mut compiler = StrategyCompiler::new(FeatureCatalogue::new());
    let spec = StrategySpec::new(
        StrategyId::new(STRATEGY),
        ObjectId::from_string("obj-ACME"),
        Duration::from_secs(30),
    )
    .with_rule(Rule::new(
        "never",
        SignalKind::Enter,
        Expr::Flag(false),
        Expr::Exact(dec!("1")),
        Expr::Statistic(0.5),
        100,
    ));
    let strategy = compiler.compile(&spec).unwrap();
    let program = compiler.into_program();
    let mut cell = Cell::new(
        CellConfig::new(CELL, REGION).with_venue(VenueId::new("XLON")),
        FeatureEngine::new(MarketState::default(), Duration::from_secs(5)),
    )
    .unwrap();
    let first = VerifiedEnvelope::verify(grant(dec!("1000"), Some(KEY)), KEY, CELL, t(10)).unwrap();
    cell.deploy(strategy, program, first).unwrap();
    cell
}

fn wire(fabric: &Fabric, clock: &Arc<ManualClock>) -> ControlWiring {
    let link = LinkSecurity::plaintext_for_tests();
    let consumer = settings(fabric, Some(CELL_CONSUME_TOKEN))
        .control_consumer(CELL, clock.clone(), &link)
        .unwrap()
        .expect("a control token is configured");
    let metrics = Arc::new(Metrics::new("qip-edge-node"));
    ControlWiring::spawn(
        CELL,
        TrustRoot::shared_key(KEY).unwrap(),
        clock.clone(),
        consumer,
        Arc::new(OutboxTelemetry::new(metrics)),
        16,
    )
    .unwrap()
}

/// The row's own check: a publish lands on the P0 stream and the cell
/// consumes it from there. A genuine renewal published by the controller,
/// read over HTTP by the cell's own identity, crosses to the cell once and is
/// applied under a `Fabric` provenance naming `control.local`.
///
/// Mutation: in `ControlWiring::spawn`, hand `ControlConsumer::new_trusted`
/// a `TrustRoot::shared_key` of another key — the grant is refused and the
/// applied list is empty; this fails at the `applied` assertion.
#[test]
fn a_grant_published_to_the_p0_control_stream_is_consumed_by_the_cell_from_there() {
    let fabric = Fabric::start("consumed");
    let clock = Arc::new(ManualClock::new(t(60)));
    publish(&fabric, grant(dec!("2000"), Some(KEY)), "EVT-GRANT");

    let wiring = wire(&fabric, &clock);
    let (mut live, (_sender, mut handoff)) = wiring.into_parts();
    assert_eq!(
        live.barrier.wait(DEADLINE),
        Barrier::CaughtUp,
        "the consumer never read through the start watermark of a one-record stream"
    );

    let mut cell = funded_cell();
    let boundary = handoff.boundary(&mut cell, None, clock.now());
    assert_eq!(boundary.applied.len(), 1, "applied: {:?}", boundary.applied);
    let Applied {
        kind,
        provenance,
        outcome,
    } = &boundary.applied[0];
    assert_eq!(*kind, ControlKind::Envelope);
    assert_eq!(*outcome, Outcome::Applied);
    match provenance {
        Provenance::Fabric(position) => {
            assert_eq!(position.stream, CONTROL_STREAM);
            assert_eq!(position.offset, 0);
        }
        other => panic!("the grant did not arrive from the fabric: {other:?}"),
    }
    assert!(live.refusals.drain().refused.is_empty());
}

/// A record on the stream signed under a key the cell does not hold is
/// refused and the cell keeps what it had: the fabric is not a way round the
/// signature the mesh demands.
///
/// Mutation: in `new_trusted`, build the capital downlink from a trust root
/// that accepts anything and this fails at the empty-`applied` assertion.
#[test]
fn a_grant_on_the_stream_signed_under_another_key_never_reaches_the_cell() {
    let fabric = Fabric::start("forged");
    let clock = Arc::new(ManualClock::new(t(60)));
    publish(
        &fabric,
        grant(dec!("9000"), Some(b"a-key-this-cell-has-never-held")),
        "EVT-FORGED",
    );

    let wiring = wire(&fabric, &clock);
    let (mut live, (_sender, mut handoff)) = wiring.into_parts();
    assert_eq!(live.barrier.wait(DEADLINE), Barrier::CaughtUp);

    let mut cell = funded_cell();
    let boundary = handoff.boundary(&mut cell, None, clock.now());
    assert!(
        boundary.applied.is_empty(),
        "a forged grant crossed: {:?}",
        boundary.applied
    );
    let refused = live.refusals.drain();
    assert_eq!(refused.refused.len(), 1, "{refused:?}");
}

/// No control token, no consumer: the produce token is a different grant
/// (SLICE-53) and is never borrowed to read control, so a cell configured
/// for shipping only reads no control and says so at start.
///
/// Mutation: make the no-token arm of `control_consumer` return an error
/// instead of `None` and this fails at the `unwrap`.
#[test]
fn a_cell_with_no_control_token_reads_no_control() {
    let fabric = Fabric::start("untokened");
    let link = LinkSecurity::plaintext_for_tests();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(t(60)));
    let consumer = settings(&fabric, None)
        .control_consumer(CELL, clock, &link)
        .unwrap();
    assert!(consumer.is_none());
}
