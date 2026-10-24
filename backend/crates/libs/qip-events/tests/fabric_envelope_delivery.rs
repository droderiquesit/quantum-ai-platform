//! R12-CONTRACT-7089: `FabricEnvelope` is the unit a consumer (and the
//! producer's own pre-flight) reads off a batch that really went through the
//! fabric codec, not a type only its own tests construct.
//!
//! Every test encodes a batch to wire bytes and decodes it again before
//! opening an envelope from it, because opening from the in-memory struct a
//! test just built would prove nothing about what survives the codec.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use std::collections::BTreeSet;

use qip_core::canonical::canonical_json;
use qip_core::{CorrelationId, EventId, Lineage, Timestamp, TraceId, sha256_hex};
use qip_events::AnyEvent;
use qip_events::Topic;
use qip_events::event_fabric::bindings::{self, TopicBinding};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, LogicalTimestamp, MessageType, PayloadCodec, Record, stamp_broker,
    stamp_drain,
};
use qip_events::event_fabric::envelope::{FabricContext, FabricEnvelope};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};

const CORRELATION: &str = "COR00000000000000000000042";
const TRACE: &str = "trace-0042";

fn context(class: QosClass) -> FabricContext {
    FabricContext {
        stream: "reflex-outcomes.local".to_string(),
        region: "us-east".to_string(),
        partition: 3,
        qos_class: class,
        auth_context: "consumer:ledger".to_string(),
        provenance: "test-reader".to_string(),
    }
}

fn event(id: &str, topic: Topic, version: u32) -> AnyEvent {
    let payload = serde_json::json!({ "cell": "cell-a", "n": 7 });
    AnyEvent {
        event_id: EventId::from_string(id),
        topic,
        schema_version: version,
        occurred_at: Timestamp::from_secs(1_790_000_000),
        recorded_at: Timestamp::from_secs(1_790_000_001),
        sequence: 0,
        lineage: Lineage {
            correlation_id: CorrelationId::from_string(CORRELATION),
            causation_id: None,
            trace_id: Some(TraceId::new(TRACE)),
            producer: "producer-under-test".to_string(),
        },
        idempotency_key: Some(format!("idem-{id}")),
        payload_hash: sha256_hex(canonical_json(&payload).as_bytes()),
        payload,
    }
}

fn record(event: &AnyEvent) -> Record {
    Record::from_any_event(event, PayloadCodec::CanonicalJson).unwrap()
}

/// A batch as a drain and a broker stamp it, sent through `encode`/`decode`.
fn over_the_wire(records: Vec<Record>) -> Batch {
    let mut batch = Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )
    .unwrap();
    stamp_drain(&mut batch, "cell-a", 5, 40);
    stamp_broker(
        &mut batch,
        0,
        17,
        9,
        LogicalTimestamp {
            physical_ns: 1_790_000_002_000_000_000,
            logical: 4,
        },
        None,
    );
    let bytes = batch.encode().unwrap();
    match Batch::decode(&bytes).unwrap() {
        DecodeOutcome::Complete(decoded) => decoded,
        other => panic!("the batch did not decode whole: {other:?}"),
    }
}

fn reflex() -> &'static [TopicBinding] {
    &bindings::ALL
}

/// Every field the requirement lists survives producer -> codec -> consumer,
/// including identity (event id, idempotency key) and the correlation and
/// trace ids.
///
/// Mutation: in `FabricEnvelope::open`, take `offset` from `leader_epoch`
/// (or drop the `+ index` on the sequence) and the field assertions fail.
#[test]
fn an_envelope_opened_from_a_batch_that_crossed_the_codec_carries_every_field() {
    let events = [
        event("evt-1", Topic::ReflexOutcomeRecorded, 1),
        event("evt-2", Topic::ReflexChainSpan, 1),
    ];
    let batch = over_the_wire(events.iter().map(record).collect());
    assert_eq!(batch.records.len(), 2, "premise: both records crossed");

    let opened =
        FabricEnvelope::open_all(&batch, &context(QosClass::P1Outcomes), reflex()).unwrap();
    assert_eq!(opened.len(), 2);
    for (index, (envelope, original)) in opened.iter().zip(&events).enumerate() {
        assert_eq!(envelope.event(), original, "the event is not restated");
        assert_eq!(envelope.event_id().as_str(), original.event_id.as_str());
        assert_eq!(
            envelope.idempotency_key(),
            original.idempotency_key.as_deref()
        );
        assert_eq!(envelope.lineage().correlation_id.as_str(), CORRELATION);
        assert_eq!(
            envelope.lineage().trace_id.as_ref().map(TraceId::as_str),
            Some(TRACE)
        );
        assert_eq!(envelope.payload_hash(), original.payload_hash);
        assert_eq!(envelope.producer_id(), "cell-a");
        assert_eq!(envelope.ordering_key(), "cell-a");
        assert_eq!(envelope.producer_epoch(), 5);
        assert_eq!(envelope.producer_sequence(), 40 + index as u64);
        assert_eq!(envelope.leader_epoch(), 9);
        assert_eq!(envelope.offset(), 17);
        assert_eq!(envelope.logical_timestamp().logical, 4);
        assert_eq!(
            envelope.logical_timestamp().physical,
            Timestamp::from_nanos(1_790_000_002_000_000_000)
        );
        assert_eq!(envelope.qos_class(), QosClass::P1Outcomes);
        assert_eq!(envelope.stream(), "reflex-outcomes.local");
        assert_eq!(envelope.region(), "us-east");
        assert_eq!(envelope.partition(), 3);
        assert_eq!(envelope.auth_context(), "consumer:ledger");
        assert_eq!(envelope.occurred_at(), original.occurred_at);
        assert_eq!(envelope.recorded_at(), original.recorded_at);
        assert_eq!(envelope.schema_hash().as_str().len(), 64);
    }
}

/// The producer-side half: an envelope's record is the same record the
/// consumer-side `open` reads back, through the one codec.
///
/// Mutation: make `to_record` encode a different event (e.g. a clone with its
/// id replaced) and the opened id differs.
#[test]
fn an_envelope_survives_the_serde_wire_and_its_record_round_trips_through_the_codec() {
    let batch = over_the_wire(vec![record(&event(
        "evt-9",
        Topic::ReflexOutcomeRecorded,
        1,
    ))]);
    let envelope =
        FabricEnvelope::open(&batch, 0, &context(QosClass::P1Outcomes), reflex()).unwrap();

    let json = serde_json::to_string(&envelope).unwrap();
    let back: FabricEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(back, envelope, "serde round trip preserves every field");

    let again = over_the_wire(vec![
        envelope.to_record(PayloadCodec::CanonicalJson).unwrap(),
    ]);
    let reopened =
        FabricEnvelope::open(&again, 0, &context(QosClass::P1Outcomes), reflex()).unwrap();
    assert_eq!(reopened.event(), envelope.event());
    assert_eq!(reopened.schema_hash(), envelope.schema_hash());
}

/// A reader refuses a version it was not built for: newer than its binding,
/// zero, and a topic with no binding at all. Refused, not clamped to the
/// highest version it knows.
///
/// Mutation: change `>` to `>=` ... no: delete the version check in `open`
/// and the newer/zero cases are opened.
#[test]
fn an_unsupported_schema_version_or_unknown_topic_is_refused_not_clamped() {
    let ctx = context(QosClass::P1Outcomes);
    let supported = over_the_wire(vec![record(&event(
        "evt-ok",
        Topic::ReflexOutcomeRecorded,
        1,
    ))]);
    assert!(
        FabricEnvelope::open(&supported, 0, &ctx, reflex()).is_ok(),
        "premise: the same event at the supported version opens"
    );

    for version in [0u32, 2, u32::MAX] {
        let batch = over_the_wire(vec![record(&event(
            "evt-v",
            Topic::ReflexOutcomeRecorded,
            version,
        ))]);
        let err = FabricEnvelope::open(&batch, 0, &ctx, reflex()).unwrap_err();
        assert_eq!(err.code(), "schema", "version {version}: {}", err.message());
        assert!(
            err.message().contains("schema version"),
            "{}",
            err.message()
        );
    }

    let unknown = over_the_wire(vec![record(&event("evt-t", Topic::MarketTick, 1))]);
    let err = FabricEnvelope::open(&unknown, 0, &ctx, reflex()).unwrap_err();
    assert!(
        err.message().contains("no reader is bound"),
        "{}",
        err.message()
    );
}

/// A P2 topic arriving on a P1 stream is a routing fault.
///
/// Mutation: delete the class comparison in `open`.
#[test]
fn a_record_on_a_stream_of_the_wrong_class_is_refused() {
    let batch = over_the_wire(vec![record(&event(
        "evt-p2",
        Topic::ReflexJournalRecorded,
        1,
    ))]);
    assert!(
        FabricEnvelope::open(&batch, 0, &context(QosClass::P2MarketJournal), reflex()).is_ok(),
        "premise: on its own class it opens"
    );
    let err =
        FabricEnvelope::open(&batch, 0, &context(QosClass::P1Outcomes), reflex()).unwrap_err();
    assert!(
        err.message().contains("bound to class"),
        "{}",
        err.message()
    );
}

/// A payload altered after sealing, and a record header that disagrees with
/// its payload, are each refused. The payload tamper keeps a valid CRC
/// because the record is re-framed after the edit — the CRC guards the wire,
/// the hash guards the meaning.
///
/// Mutation: drop the payload-hash comparison in `assemble` (first case) or
/// the `record.event_id` comparison in `open` (second) and the matching case
/// opens.
#[test]
fn a_tampered_payload_or_a_header_that_contradicts_its_payload_is_refused() {
    let ctx = context(QosClass::P1Outcomes);

    let mut tampered = event("evt-t", Topic::ReflexOutcomeRecorded, 1);
    tampered.payload = serde_json::json!({ "cell": "cell-a", "n": 8_000_000 });
    let batch = over_the_wire(vec![record(&tampered)]);
    let err = FabricEnvelope::open(&batch, 0, &ctx, reflex()).unwrap_err();
    assert!(err.message().contains("payload hash"), "{}", err.message());

    let genuine = event("evt-real", Topic::ReflexOutcomeRecorded, 1);
    let mut lying = record(&genuine);
    lying.event_id = "evt-other".to_string();
    let err = FabricEnvelope::open(&over_the_wire(vec![lying]), 0, &ctx, reflex()).unwrap_err();
    assert!(err.message().contains("disagree"), "{}", err.message());

    let mut wrong_trace = record(&genuine);
    wrong_trace.trace_id = Some("trace-elsewhere".to_string());
    let err =
        FabricEnvelope::open(&over_the_wire(vec![wrong_trace]), 0, &ctx, reflex()).unwrap_err();
    assert!(err.message().contains("trace id"), "{}", err.message());

    let mut wrong_time = record(&genuine);
    wrong_time.source_timestamp_ns += 1;
    let err =
        FabricEnvelope::open(&over_the_wire(vec![wrong_time]), 0, &ctx, reflex()).unwrap_err();
    assert!(
        err.message().contains("source timestamp"),
        "{}",
        err.message()
    );

    // Malformed payload bytes inside an otherwise well-framed record.
    let mut garbage = record(&genuine);
    garbage.payload = b"{not json".to_vec();
    let err = FabricEnvelope::open(&over_the_wire(vec![garbage]), 0, &ctx, reflex()).unwrap_err();
    assert_eq!(err.code(), "schema", "{}", err.message());
}

/// A batch no drain has stamped has no producer, so no envelope can be made
/// from it; and one refused record refuses the whole batch (`open_all`), so a
/// consumer cannot apply a prefix.
///
/// Mutation: make `open_all` skip refused records (`filter_map(..ok())`) and
/// the second assertion fails.
#[test]
fn an_unstamped_batch_has_no_envelope_and_one_bad_record_refuses_the_whole_batch() {
    let ctx = context(QosClass::P1Outcomes);
    let unstamped = Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        vec![record(&event("evt-u", Topic::ReflexOutcomeRecorded, 1))],
    )
    .unwrap();
    let err = FabricEnvelope::open(&unstamped, 0, &ctx, reflex()).unwrap_err();
    assert!(err.message().contains("producer id"), "{}", err.message());

    let good = record(&event("evt-g", Topic::ReflexOutcomeRecorded, 1));
    let bad = record(&event("evt-b", Topic::ReflexOutcomeRecorded, 2));
    let batch = over_the_wire(vec![good, bad]);
    assert!(
        FabricEnvelope::open(&batch, 0, &ctx, reflex()).is_ok(),
        "premise: record 0 is fine"
    );
    assert!(FabricEnvelope::open_all(&batch, &ctx, reflex()).is_err());
    assert!(
        FabricEnvelope::open(&batch, 2, &ctx, reflex()).is_err(),
        "no record 2"
    );
}

/// The schema/version hash binds the version: a wire envelope whose
/// `schema_version` was edited (or whose hash was) is refused. An integer
/// alone could not be.
///
/// Mutation: pass `None` instead of `Some(wire.schema_hash)` in the wire
/// `TryFrom` and both edits are accepted.
#[test]
fn a_wire_envelope_whose_version_or_schema_hash_was_edited_is_refused() {
    let batch = over_the_wire(vec![record(&event(
        "evt-h",
        Topic::ReflexOutcomeRecorded,
        1,
    ))]);
    let envelope =
        FabricEnvelope::open(&batch, 0, &context(QosClass::P1Outcomes), reflex()).unwrap();
    let genuine = serde_json::to_value(&envelope).unwrap();
    assert!(
        serde_json::from_value::<FabricEnvelope>(genuine.clone()).is_ok(),
        "premise"
    );

    let mut version_edited = genuine.clone();
    version_edited["event"]["schema_version"] = serde_json::json!(2);
    let err = serde_json::from_value::<FabricEnvelope>(version_edited).unwrap_err();
    assert!(err.to_string().contains("schema hash"), "{err}");

    let mut hash_edited = genuine.clone();
    hash_edited["schema_hash"] = serde_json::json!("0".repeat(64));
    assert!(serde_json::from_value::<FabricEnvelope>(hash_edited).is_err());

    let mut hash_removed = genuine;
    hash_removed.as_object_mut().unwrap().remove("schema_hash");
    assert!(serde_json::from_value::<FabricEnvelope>(hash_removed).is_err());
}

/// A redelivery is recognised by identity, a different sequence, epoch or
/// partition is not.
///
/// Mutation: drop `producer_epoch` from `identity()` and the epoch case
/// collides.
#[test]
fn a_redelivered_envelope_has_the_same_identity_and_a_different_one_does_not() {
    let ctx = context(QosClass::P1Outcomes);
    let records = vec![
        record(&event("evt-a", Topic::ReflexOutcomeRecorded, 1)),
        record(&event("evt-b", Topic::ReflexOutcomeRecorded, 1)),
    ];
    let first = over_the_wire(records.clone());
    let redelivered = over_the_wire(records);

    let mut seen = BTreeSet::new();
    for envelope in FabricEnvelope::open_all(&first, &ctx, reflex()).unwrap() {
        assert!(seen.insert(envelope.identity()));
    }
    assert_eq!(seen.len(), 2, "premise: two records, two identities");
    for envelope in FabricEnvelope::open_all(&redelivered, &ctx, reflex()).unwrap() {
        assert!(
            !seen.insert(envelope.identity()),
            "a redelivery is the same identity"
        );
    }
    let mut next_epoch = first.clone();
    next_epoch.producer_epoch += 1;
    let other = FabricEnvelope::open(&next_epoch, 0, &ctx, reflex()).unwrap();
    assert!(
        seen.insert(other.identity()),
        "a new epoch is a new identity"
    );
    let mut other_partition = ctx.clone();
    other_partition.partition += 1;
    let other = FabricEnvelope::open(&first, 0, &other_partition, reflex()).unwrap();
    assert!(
        seen.insert(other.identity()),
        "another partition is a new identity"
    );
}

/// `Timestamp` serialises at millisecond precision, so a record framed from a
/// nanosecond-precise event must carry the instant its payload decodes to.
/// This happened: the real edge node's first spooled batch carried a header
/// 359,542 ns ahead of its payload and its own drain refused it.
///
/// Mutation: in `Record::from_any_event`, set `source_timestamp_ns` back to
/// `event.occurred_at.as_nanos()` and this fails with "source timestamp".
#[test]
fn a_nanosecond_precise_event_time_still_opens_because_the_header_carries_what_the_payload_decodes_to()
 {
    let mut precise = event("evt-ns", Topic::ReflexOutcomeRecorded, 1);
    precise.occurred_at = Timestamp::from_nanos(1_792_217_633_819_359_542);
    assert_ne!(
        precise.occurred_at.as_nanos() % 1_000_000,
        0,
        "premise: the instant has sub-millisecond digits"
    );
    let batch = over_the_wire(vec![record(&precise)]);
    let envelope =
        FabricEnvelope::open(&batch, 0, &context(QosClass::P1Outcomes), reflex()).unwrap();
    assert_eq!(
        envelope.occurred_at(),
        Timestamp::from_nanos(1_792_217_633_819_000_000),
        "the envelope reports the instant the wire preserved"
    );
}
