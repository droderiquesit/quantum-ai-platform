//! ADR 0215 (R12-FABRIC-8090): a catalogue stream that names no topic admits
//! no record on a deployed broker, and only a `local` broker may run one
//! open. ADR 0156 decision 4 refused such a stream outright; the committed
//! `telemetry.local` names none because no body in the lock can ride P4, so
//! the refusal is the deployment's to make, and this suite proves both arms
//! against the same catalogue.
//!
//! Nothing here places, amends or cancels an order.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use qip_core::canonical::canonical_json;
use qip_core::{Clock, CorrelationId, EventId, Lineage, SystemClock, Timestamp, sha256_hex};
use qip_events::event_fabric::catalogue::Catalogue;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record, stamp_drain};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass, StreamPolicy};
use qip_events::{AnyEvent, Topic};
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use qip_streaming::event_fabric::service::{SegmentConfigFor, Service, TopiclessStreams};
use qip_transport::event_fabric::auth::IdentityTable;
use qip_transport::event_fabric::protocol::Refusal;

const STREAM: &str = "telemetry.topicless";
const AGENT: &str = "agent:telemetry-1";

static DIRECTORIES: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let unique = DIRECTORIES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-topicless-{label}-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One P4 stream naming no topic, as the committed `telemetry.local` does,
/// and one agent granted produce on it.
fn catalogue() -> Catalogue {
    let json = format!(
        r#"{{"streams":[{{"name":"{STREAM}","qos_class":"p4_telemetry","partition_key":"cell",
            "ordering":"per_partition","retention":"derived_state","replication_factor":1,
            "mirroring":"none","overload_policy":"sample_or_shed","ack_profile":"none",
            "byte_quota_per_producer":262144,"message_quota_per_producer":4096,
            "lag_limit":4096,"entitlement_dataset":"qip-test","entitlement_usage":"research",
            "seal_age_ms":5000,"peak_bytes_per_second":262144,"topics":[]}}],
            "grants":[{{"identity":"{AGENT}","stream":"{STREAM}","permission":"produce","key_scope":"any"}}]}}"#
    );
    Catalogue::parse(json.as_bytes())
        .expect("a topic-less P4 stream parses, as the committed one does")
}

fn service(label: &str, topicless: TopiclessStreams) -> Service {
    let identities = IdentityTable::parse(&format!(
        "{AGENT} {}\n",
        sha256_hex(b"fixture-token-for-the-agent-000000000000000000000000")
    ))
    .unwrap();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let broker = Arc::new(Broker::open(temp_dir(label), clock.clone()).unwrap());
    let segments: SegmentConfigFor = Arc::new(move |policy: &StreamPolicy| {
        SegmentLogConfig::new(clock.clone()).with_archive_required(policy.archive_required())
    });
    Service::new(broker, identities, &catalogue(), 1, segments, topicless).unwrap()
}

/// A real record — a decodable `AnyEvent` under a real topic — so the
/// refusal under test is the stream's and not "body cannot be read".
fn batch(service: &Service) -> Batch {
    let payload = serde_json::json!({ "cell": "cell-a", "n": 7 });
    let event = AnyEvent {
        event_id: EventId::from_string("evt-topicless-1"),
        topic: Topic::ReflexOutcomeRecorded,
        schema_version: 1,
        occurred_at: Timestamp::from_secs(1_790_000_000),
        recorded_at: Timestamp::from_secs(1_790_000_001),
        sequence: 0,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000042"),
            causation_id: None,
            trace_id: None,
            producer: AGENT.to_string(),
        },
        idempotency_key: None,
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    };
    let record = Record::from_any_event(&event, PayloadCodec::CanonicalJson).unwrap();
    let mut batch = Batch::new(
        MessageType::Data,
        QosClass::P4Telemetry.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap();
    let epoch = service.broker().init_producer(STREAM, 0, AGENT).unwrap();
    stamp_drain(&mut batch, AGENT, epoch, 0);
    batch
}

/// Mutation: in `Service::apply_catalogue`, drop the `AdmitNone` arm's
/// `close_body_topics` call — the deployed service appends the record and
/// the `expect_err` fires. Mutation: make `topicless_streams` in qip-fabricd
/// answer `AdmitAll` everywhere — `a_deployed_configuration_closes_...` in
/// qip-fabricd fires; this suite proves the service, that one the mapping.
#[test]
fn a_deployed_broker_admits_nothing_on_a_stream_that_names_no_topic() {
    let closed = service("closed", TopiclessStreams::AdmitNone);
    let high_water = closed
        .broker()
        .metadata(STREAM, 0)
        .unwrap()
        .high_watermark();
    let refused = closed
        .broker()
        .produce_from(STREAM, 0, batch(&closed), Some(AGENT))
        .unwrap()
        .expect_err("a deployed broker appended to a stream that names no topic");
    assert_eq!(refused.refusal, Refusal::SchemaRefused);
    assert!(
        refused.error.message().contains("names no topic"),
        "{:?}",
        refused.error
    );
    assert!(
        refused.error.message().contains("ADR 0215"),
        "{:?}",
        refused.error
    );
    assert_eq!(
        closed
            .broker()
            .metadata(STREAM, 0)
            .unwrap()
            .high_watermark(),
        high_water,
        "nothing was appended"
    );

    // The same catalogue and the same record, on a local broker: admitted,
    // which is the premise that the refusal above is the policy's.
    let open = service("open", TopiclessStreams::AdmitAll);
    open.broker()
        .produce_from(STREAM, 0, batch(&open), Some(AGENT))
        .unwrap()
        .expect("a local broker still runs a topic-less stream open");
}
