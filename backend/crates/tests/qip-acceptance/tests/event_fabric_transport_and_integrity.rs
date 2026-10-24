//! Event Fabric transport and integrity: FABRIC-036, FABRIC-037, FABRIC-038, FABRIC-039, FABRIC-040, FABRIC-041.
//!
//! FABRIC-036 is pinned as a gap (there is no QUIC); 037-041 drive the batch
//! codec, the broker and the stream policy.

#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[test]
fn fabric_036_gap_every_fabric_record_crosses_the_one_tcp_http_transport_and_no_datagram_path_exists()
 {
    // GAP PIN. FABRIC-036 asks for QUIC streams for durable records and
    // datagrams only for lossy telemetry. The previous test paired strings
    // with a local `TransportMode` enum and asserted the pairs it had made.
    // There is no QUIC in this build: the `FabricTransport` seam has exactly
    // one production implementation, `HttpTransport`, which speaks HTTP/1.1
    // over TCP — ordered and reliable for every class, including telemetry —
    // and no fabric code opens a UDP socket. That is the truthful half of the
    // property (no durable record can ride a lossy path, because none exists)
    // and the gap (no stream/datagram split at all). Both are asserted.
    //
    // Mutation: add `impl FabricTransport for UdpTransport` in any `src/` file
    // — the implementation list stops being `["HttpTransport"]` and fires.
    // Mutation: open a `std::net::UdpSocket` in
    // `qip-transport/src/event_fabric/` — the datagram assertion fires.
    let mut implementations = Vec::new();
    let mut fabric_sources = 0usize;
    for path in qip_acceptance::files_with_extension("backend/crates", "rs") {
        let display = path.display().to_string();
        if !display.contains("/src/") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = qip_acceptance::production_text(&raw);
        for line in text.lines() {
            let line = line.trim_start();
            if line.starts_with("impl")
                && let Some(rest) = line.split("FabricTransport for ").nth(1)
            {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                implementations.push(name);
            }
        }
        if display.contains("qip-transport/src/event_fabric/") {
            fabric_sources += 1;
            assert!(
                !text.contains("UdpSocket"),
                "{display} opens a datagram socket"
            );
        }
    }
    assert!(
        fabric_sources >= 5,
        "premise: the fabric client sources were scanned"
    );
    assert_eq!(implementations, ["HttpTransport"]);
}

// --- FABRIC-037 to FABRIC-041, through the codec, the broker and the policy ----
//
// Each of these was a struct built in the test body and read back: a
// `RecordWithIntegrity` holding the strings "crc32:abcd1234" and
// "blake3:xyz789", a `RecordLayout::Unversioned` that was asserted to be
// unversioned, and so on. They now drive `qip_events`' batch codec and the
// `qip-streaming` broker.

use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::event_fabric::codec::{
    Batch, DecodeOutcome, FORMAT_VERSION, MessageType, PayloadCodec, Record, stamp_drain,
};
use qip_events::event_fabric::crc32c::crc32c;
use qip_events::event_fabric::policy::{
    AckProfile, Entitlement, Mirroring, Ordering, OverloadPolicy, QosClass, StreamPolicy,
    StreamPolicySpec,
};
use qip_events::event_fabric::schema_id::Shape;
use qip_storage::segment::log::SegmentLogConfig;
use qip_streaming::event_fabric::broker::Broker;
use std::sync::Arc;

fn one_record_batch(event_id: &str, payload: &[u8]) -> Batch {
    Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![Record {
            event_id: event_id.to_string(),
            trace_id: None,
            source_timestamp_ns: 42,
            payload: payload.to_vec(),
        }],
    )
    .expect("a batch")
}

fn spec(class: QosClass) -> StreamPolicySpec {
    StreamPolicySpec {
        qos_class: class,
        partition_key: "account".to_string(),
        ordering: Ordering::PerPartition,
        retention: qip_events::retention::RetentionClass::Irreplaceable,
        replication_factor: 1,
        mirroring: Mirroring::None,
        overload_policy: OverloadPolicy::RefuseProducer,
        ack_profile: class.ack_floor(),
        byte_quota_per_producer: 1_000_000,
        message_quota_per_producer: 1_000,
        lag_limit: 1_000,
        entitlement: Entitlement::new("ledger", "trade").expect("valid"),
        seal_age_ms: 60_000,
        peak_bytes_per_second: 1 << 20,
    }
}

fn broker() -> Broker {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "qip-acceptance-transport-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_civil(2026, 10, 4)));
    let broker = Broker::open(dir, Arc::clone(&clock)).expect("opens");
    broker
        .declare_stream(
            "fills",
            1,
            StreamPolicy::new(spec(QosClass::P1Outcomes)).expect("admitted"),
            SegmentLogConfig::new(clock).with_roll_after_bytes(10_000_000),
        )
        .expect("declared");
    broker
        .register_schema("fills", 1, 1, Shape::of(&1u32).expect("a shape"))
        .expect("registered");
    broker
}

/// FABRIC-037: a batch on the wire carries a checksum over its body, and a
/// single flipped byte anywhere in a record's payload is refused rather than
/// decoded as different data. (Segment-level content hashing is driven by
/// `event_fabric_infrastructure.rs`'s fabric_090.)
///
/// Mutation: delete the per-record CRC comparison in the record decoder
/// (`actual_crc` in `codec.rs`) — the flipped payload decodes `Complete`
/// with altered bytes and the assertion fires.
#[test]
fn fabric_037_one_flipped_payload_byte_is_refused_rather_than_decoded_as_different_data() {
    let payload = b"{\"qty\":17,\"px\":\"101.25\"}";
    let encoded = one_record_batch("evt-37", payload)
        .encode()
        .expect("encodes");
    // Premise: the untouched frame decodes to exactly the payload.
    match Batch::decode(&encoded).expect("decodes") {
        DecodeOutcome::Complete(batch) => assert_eq!(batch.records[0].payload, payload),
        DecodeOutcome::Torn => panic!("premise: a whole frame"),
    }
    let at = encoded
        .windows(payload.len())
        .position(|window| window == payload)
        .expect("premise: the payload is carried verbatim in the frame");
    for offset in [at, at + payload.len() / 2, at + payload.len() - 1] {
        let mut flipped = encoded.clone();
        flipped[offset] ^= 0x01;
        let decoded = Batch::decode(&flipped);
        assert!(
            decoded.is_err(),
            "a flipped byte at {offset} decoded as {decoded:?}"
        );
    }
}

/// FABRIC-038: nothing reaches the wire without a format version, and a
/// frame declaring any version this build does not write is refused — even
/// with a valid prefix checksum, so only the version check can refuse it.
///
/// Mutation: delete the `version != FORMAT_VERSION` refusal in
/// `Batch::decode` — the re-versioned frame decodes and the assertion fires.
#[test]
fn fabric_038_a_frame_declaring_an_unknown_format_version_is_refused() {
    let encoded = one_record_batch("evt-38", b"{}").encode().expect("encodes");
    // A batch carrying neither field ADR 0129 added keeps version 1's
    // layout; one past the newest version this build reads is still unknown.
    assert_eq!(
        u16::from_le_bytes([encoded[4], encoded[5]]),
        qip_events::event_fabric::codec::FORMAT_VERSION_WITHOUT_ORIGIN,
        "premise: the version sits at bytes 4..6"
    );
    let mut future = encoded;
    future[4..6].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    let crc = crc32c(&future[..10]);
    future[10..14].copy_from_slice(&crc.to_le_bytes());
    let refused = Batch::decode(&future).expect_err("an unknown format version decoded");
    assert!(refused.to_string().contains("format version"), "{refused}");
}

/// FABRIC-039: order is strict within a partition — a producer's batches are
/// admitted only in sequence, and offsets follow admission order. A batch
/// that skips ahead is refused naming the sequence expected.
///
/// Mutation: in `ProducerTable::admit`, treat `base_sequence > expected` as
/// an append — the skipped-ahead batch is admitted and the `expect_err`
/// fails.
#[test]
fn fabric_039_a_producer_batch_out_of_sequence_is_refused_and_offsets_follow_admission_order() {
    let broker = broker();
    let mut offsets = Vec::new();
    for sequence in 0..3u64 {
        let mut batch = one_record_batch(&format!("evt-{sequence}"), b"{}");
        stamp_drain(&mut batch, "venue", 1, sequence);
        offsets.push(
            broker
                .produce("fills", "acct", batch)
                .expect("in order")
                .base_offset(),
        );
    }
    assert_eq!(offsets, [0, 1, 2]);
    let mut ahead = one_record_batch("evt-5", b"{}");
    stamp_drain(&mut ahead, "venue", 1, 5);
    let refused = broker
        .produce("fills", "acct", ahead)
        .expect_err("a batch that skipped sequences 3 and 4 was admitted");
    assert!(!refused.to_string().is_empty());
    assert_eq!(
        broker
            .metadata("fills", 0)
            .expect("metadata")
            .high_watermark(),
        3
    );
}

/// FABRIC-040: a record as served carries its placement (offset), the
/// broker's leader epoch, the producer's epoch, a hybrid logical timestamp
/// the broker stamped, and the writer's own source time — none of them
/// supplied by the test.
///
/// Mutation: in `Broker::produce_to`, stamp `LogicalTimestamp::default()`
/// instead of the partition clock's reading — the timestamp assertion fires.
#[test]
fn fabric_040_a_served_batch_carries_offset_epochs_logical_time_and_source_time() {
    let broker = broker();
    let mut batch = one_record_batch("evt-40", b"{}");
    // Premise: the broker-owned fields start unset.
    assert_eq!(batch.logical_timestamp, Default::default());
    stamp_drain(&mut batch, "venue", 3, 0);
    let ack = broker.produce("fills", "acct", batch).expect("admitted");

    let fetched = broker
        .fetch("fills", 0, ack.base_offset(), 1 << 20)
        .expect("fetch");
    let bytes = qip_core::hash::from_hex(fetched.batches()).expect("hex");
    let served = match Batch::decode(&bytes).expect("decodes") {
        DecodeOutcome::Complete(batch) => batch,
        DecodeOutcome::Torn => panic!("torn"),
    };
    assert_eq!(served.base_offset, ack.base_offset());
    assert_eq!(served.leader_epoch, broker.leader_epoch());
    assert!(served.leader_epoch > 0, "premise: an epoch was assigned");
    assert_eq!(served.producer_epoch, 3);
    assert_ne!(served.logical_timestamp, Default::default());
    assert!(served.logical_timestamp.physical_ns > 0);
    assert_eq!(served.records[0].source_timestamp_ns, 42);
}

/// FABRIC-041: durable classes deliver at least once — they refuse to run
/// with no acknowledgement — and at-most-once is telemetry's alone. A
/// consumer that has not committed reads the same batch again.
///
/// Mutation: change `QosClass::ack_floor`'s P3 arm to `None` — a P3 stream
/// with no acknowledgement is admitted and the loop fires.
#[test]
fn fabric_041_only_telemetry_may_be_at_most_once_and_an_uncommitted_read_is_served_again() {
    for class in QosClass::ALL {
        let none = StreamPolicy::new(StreamPolicySpec {
            ack_profile: AckProfile::None,
            ..spec(class)
        });
        assert_eq!(
            none.is_ok(),
            class == QosClass::P4Telemetry,
            "{} with no acknowledgement",
            class.as_str()
        );
    }
    let broker = broker();
    let mut batch = one_record_batch("evt-41", b"{}");
    stamp_drain(&mut batch, "venue", 1, 0);
    broker.produce("fills", "acct", batch).expect("admitted");
    let first = broker.fetch("fills", 0, 0, 1 << 20).expect("fetch");
    assert_eq!(
        broker.committed_offset("risk", "fills", 0).expect("read"),
        None
    );
    let again = broker.fetch("fills", 0, 0, 1 << 20).expect("fetch");
    assert!(!first.batches().is_empty(), "premise: something was served");
    assert_eq!(first.batches(), again.batches());
}
