//! ADR 0129 (R12-CONTRACT-7089, R12-FABRIC-8022, R12-FABRIC-8084): the
//! producing region, partition and authenticated principal travel in the
//! batch, stamped by the broker, and the producing component's kind by the
//! writer — so a consumer reads them off the wire instead of supplying its
//! own. Until this the ledger stamped every envelope "central".
//!
//! Every test encodes to wire bytes and decodes before reading, because the
//! in-memory struct a test just built proves nothing about the codec.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::canonical::canonical_json;
use qip_core::{CorrelationId, EventId, Lineage, Timestamp, sha256_hex};
use qip_events::AnyEvent;
use qip_events::Topic;
use qip_events::event_fabric::bindings;
use qip_events::event_fabric::codec::{
    Batch, BrokerOrigin, DecodeOutcome, FORMAT_VERSION, FORMAT_VERSION_WITHOUT_ORIGIN,
    LogicalTimestamp, MessageType, PayloadCodec, Provenance, Record, stamp_broker,
    stamp_broker_origin, stamp_drain,
};
use qip_events::event_fabric::envelope::{FabricContext, FabricEnvelope};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};

fn event(id: &str) -> AnyEvent {
    let payload = serde_json::json!({ "cell": "cell-a", "n": 7 });
    AnyEvent {
        event_id: EventId::from_string(id),
        topic: Topic::ReflexOutcomeRecorded,
        schema_version: 1,
        occurred_at: Timestamp::from_secs(1_790_000_000),
        recorded_at: Timestamp::from_secs(1_790_000_001),
        sequence: 0,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string("COR00000000000000000000042"),
            causation_id: None,
            trace_id: Some(qip_core::TraceId::new("trace-origin")),
            producer: "producer-under-test".to_string(),
        },
        idempotency_key: Some(format!("idem-{id}")),
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    }
}

fn batch() -> Batch {
    let record = Record::from_any_event(&event("evt-1"), PayloadCodec::CanonicalJson).unwrap();
    let mut batch = Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-a", 5, 40);
    stamp_broker(
        &mut batch,
        17,
        9,
        LogicalTimestamp {
            physical_ns: 1_790_000_002_000_000_000,
            logical: 4,
        },
        None,
    );
    batch
}

fn origin() -> BrokerOrigin {
    BrokerOrigin {
        region: "europe-west2".to_string(),
        partition: 3,
        principal: "qip-edge-node-london-1".to_string(),
    }
}

fn stamped() -> Batch {
    let mut batch = batch().with_provenance(Provenance::EdgeNode);
    stamp_broker_origin(&mut batch, origin());
    batch
}

fn wire(batch: &Batch) -> Vec<u8> {
    batch.encode().unwrap()
}

fn decoded(bytes: &[u8]) -> Batch {
    match Batch::decode(bytes).unwrap() {
        DecodeOutcome::Complete(batch) => batch,
        other => panic!("the batch did not decode whole: {other:?}"),
    }
}

fn version_of(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[4], bytes[5]])
}

/// The ledger's context since ADR 0129: no region of its own.
fn consumer() -> FabricContext {
    FabricContext {
        stream: "reflex-outcomes.local".to_string(),
        region: None,
        partition: 3,
        qos_class: QosClass::P1Outcomes,
        auth_context: "consumer-group:ledger".to_string(),
        provenance: "qip-ledgerd".to_string(),
    }
}

/// Mutation: drop the origin or provenance from `Batch::encode` (or from
/// the decode) and the round trip loses the field.
#[test]
fn a_stamped_batch_round_trips_its_region_partition_principal_and_provenance() {
    let bytes = wire(&stamped());
    assert_eq!(version_of(&bytes), FORMAT_VERSION);
    let back = decoded(&bytes);
    assert_eq!(back, stamped());
    assert_eq!(back.origin, Some(Box::new(origin())));
    assert_eq!(back.provenance, Some(Provenance::EdgeNode));
}

/// A batch carrying neither field keeps version 1's exact bytes, so every
/// batch written before ADR 0129 — and every chain hash over one — is
/// unchanged; it decodes with both explicitly absent, never defaulted.
#[test]
fn a_batch_without_the_fields_keeps_version_ones_bytes_and_decodes_them_as_absent() {
    let bytes = wire(&batch());
    assert_eq!(version_of(&bytes), FORMAT_VERSION_WITHOUT_ORIGIN);
    let back = decoded(&bytes);
    assert_eq!(back.origin, None);
    assert_eq!(back.provenance, None);
    assert_eq!(
        wire(&back),
        bytes,
        "re-encoding an old batch moved its bytes"
    );
}

/// The origin is under the batch CRC: a custodian that rewrites the region
/// after the broker stamped it is caught as corruption.
#[test]
fn a_region_rewritten_after_the_broker_stamped_it_is_refused_as_corruption() {
    let mut bytes = wire(&stamped());
    let at = bytes
        .windows(b"europe-west2".len())
        .position(|w| w == b"europe-west2")
        .expect("premise: the region is on the wire");
    bytes[at] = b'u';
    let refusal = Batch::decode(&bytes).expect_err("a rewritten region decoded");
    assert!(
        refusal.message().contains("batch CRC mismatch"),
        "{refusal:?}"
    );
}

/// The envelope says what the broker stamped, not what the reader thinks.
#[test]
fn a_consumer_reads_region_principal_and_provenance_off_the_wire() {
    let batch = decoded(&wire(&stamped()));
    let envelope = FabricEnvelope::open(&batch, 0, &consumer(), &bindings::ALL).unwrap();
    assert_eq!(envelope.region(), "europe-west2");
    assert_eq!(envelope.partition(), 3);
    assert_eq!(envelope.auth_context(), "qip-edge-node-london-1");
    assert_eq!(envelope.provenance(), "qip-edge-node");
}

/// ADR 0129's "a test that a consumer-fabricated region is refused": a reader
/// that names a region other than the broker's is refused, as is one reading
/// the batch as another partition, and a batch no broker stamped is refused
/// to a reader that states no region rather than given one.
#[test]
fn a_consumer_fabricated_region_a_wrong_partition_and_an_unstamped_batch_are_refused() {
    let batch = decoded(&wire(&stamped()));
    // Premise: the same batch opens for an honest consumer.
    assert!(FabricEnvelope::open(&batch, 0, &consumer(), &bindings::ALL).is_ok());

    let mut fabricating = consumer();
    fabricating.region = Some("central".to_string());
    let refusal = FabricEnvelope::open(&batch, 0, &fabricating, &bindings::ALL)
        .expect_err("a consumer-fabricated region was believed");
    assert!(
        refusal.message().contains("reader claims central"),
        "{refusal:?}"
    );

    let mut elsewhere = consumer();
    elsewhere.partition = 4;
    assert!(FabricEnvelope::open(&batch, 0, &elsewhere, &bindings::ALL).is_err());

    let unstamped = decoded(&wire(&batch_with_provenance_only()));
    let refusal = FabricEnvelope::open(&unstamped, 0, &consumer(), &bindings::ALL)
        .expect_err("an unstamped batch was given a region by its reader");
    assert!(
        refusal.message().contains("no broker-stamped region"),
        "{refusal:?}"
    );
}

fn batch_with_provenance_only() -> Batch {
    batch().with_provenance(Provenance::Desk)
}

/// A provenance byte this build does not know is refused, never read as one
/// it does.
#[test]
fn an_unknown_provenance_byte_is_refused() {
    let good = wire(&batch_with_provenance_only());
    let mut bytes = good.clone();
    // Header: prefix (14) | message type, flags, encoding (3) | schema id,
    // version (8) | producer id (4 + len) | epochs, sequences, offsets (32) |
    // HLC (12) | previous hash tag (1) | provenance (1).
    let producer_len = "cell-a".len();
    let at = 14 + 3 + 8 + 4 + producer_len + 32 + 12 + 1;
    assert_eq!(bytes[at], 2, "premise: this is the desk's provenance byte");
    bytes[at] = 9;
    // Re-seal the batch CRC — over the header through the record count,
    // stored in the four bytes after it — so the provenance check is the
    // one that refuses, not the CRC.
    let header_end = at + 1 + 1 + 4;
    let crc = qip_events::event_fabric::crc32c::crc32c(&bytes[14..header_end]);
    bytes[header_end..header_end + 4].copy_from_slice(&crc.to_le_bytes());
    let refusal = Batch::decode(&bytes).expect_err("an unknown provenance decoded");
    assert!(
        refusal.message().contains("provenance byte 9"),
        "{refusal:?}"
    );
}
