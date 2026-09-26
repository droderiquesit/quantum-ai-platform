//! The event fabric's record and batch codec (ADR 0100 §1): CRC32C, the
//! by-stage header split CONTRACT-048 names, and the three-outcome decode
//! copied from `qip-storage`'s WAL frame.

use qip_core::{CorrelationId, EventId, Lineage, Timestamp, TraceId};
use qip_events::event_fabric::codec::{
    BATCH_MAGIC, Batch, ContentHash, DecodeOutcome, LogicalTimestamp, MAX_BATCH_LEN, MessageType,
    PayloadCodec, PrefixDecodeOutcome, Record, stamp_broker, stamp_drain,
};
use qip_events::event_fabric::crc32c::crc32c;
use qip_events::{AnyEvent, Envelope, EventBody, Topic};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tick {
    symbol: String,
}

impl EventBody for Tick {
    const TOPIC: Topic = Topic::MarketTick;
    const SCHEMA_VERSION: u32 = 1;
}

/// The documented wire prefix: magic, then a `u16` version, then a `u32`
/// declared body length — the fields the prefix CRC below protects.
const PREFIX_FIELDS_LEN: usize = BATCH_MAGIC.len() + 2 + 4;

/// The prefix fields plus their own trailing CRC32C.
const PREFIX_LEN: usize = PREFIX_FIELDS_LEN + 4;

/// Overwrite the four-byte prefix CRC in a mutated fixture so a corruption
/// made further down the prefix (magic, version or length) is the only thing
/// wrong with it — otherwise the prefix CRC itself would refuse the batch
/// first, and the specific check under test would never run.
fn refresh_prefix_crc(bytes: &mut [u8]) {
    let crc = crc32c(&bytes[..PREFIX_FIELDS_LEN]);
    bytes[PREFIX_FIELDS_LEN..PREFIX_LEN].copy_from_slice(&crc.to_le_bytes());
}

/// Build an `AnyEvent` whose identity, timestamp and (optional) trace id are
/// exactly what a test asserts survives the codec.
fn make_event(id: &str, symbol: &str, occurred_at: Timestamp, trace: Option<&str>) -> AnyEvent {
    let lineage = Lineage {
        correlation_id: CorrelationId::from_string("COR00000000000000000000001"),
        causation_id: None,
        trace_id: trace.map(TraceId::new),
        producer: "codec-test".to_string(),
    };
    let envelope = Envelope::new(
        EventId::from_string(id),
        occurred_at,
        occurred_at,
        lineage,
        Tick {
            symbol: symbol.to_string(),
        },
    );
    envelope.erase().expect("Tick erases to AnyEvent")
}

fn one_record_batch() -> (AnyEvent, Batch) {
    let event = make_event(
        "EVT0000000000000000000009",
        "SOLO",
        Timestamp::from_civil(2026, 3, 1),
        Some("trace-solo"),
    );
    let record =
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes");
    let batch = Batch::new(
        MessageType::Data,
        1,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .expect("one record makes a valid batch");
    (event, batch)
}

/// A one-record batch shaped like [`one_record_batch`]'s but tagged with
/// `schema_id`, so two batches built with different tags are distinguishable
/// after decoding — used only by the concatenated-buffer tests below, where
/// several batches are glued together and a test must tell which one a
/// decode call actually returned.
fn tagged_batch(schema_id: u32, event_id: &str) -> Batch {
    let event = make_event(event_id, "TAG", Timestamp::from_civil(2026, 3, 1), None);
    let record =
        Record::from_any_event(&event, PayloadCodec::CanonicalJson).expect("record encodes");
    Batch::new(
        MessageType::Data,
        schema_id,
        1,
        PayloadCodec::CanonicalJson,
        vec![record],
    )
    .expect("one record makes a valid batch")
}

// --- CRC32C ------------------------------------------------------------

#[test]
fn crc32c_matches_every_rfc_3720_vector() {
    let zeros = [0u8; 32];
    let ones = [0xffu8; 32];
    let ascending: Vec<u8> = (0u8..32).collect();
    let descending: Vec<u8> = (0u8..32).rev().collect();

    // Premise: the four fixtures are genuinely distinct inputs, so a
    // constant-output implementation could not pass every assertion below by
    // accident.
    assert_ne!(zeros.to_vec(), ones.to_vec());
    assert_ne!(ascending, descending);
    assert_ne!(zeros.to_vec(), ascending);

    assert_eq!(
        crc32c(&zeros),
        0x8a91_36aa,
        "RFC 3720 vector: 32 zero bytes"
    );
    assert_eq!(
        crc32c(&ones),
        0x62a8_ab43,
        "RFC 3720 vector: 32 bytes of 0xff"
    );
    assert_eq!(
        crc32c(&ascending),
        0x46dd_794e,
        "RFC 3720 vector: 0x00..0x1f ascending"
    );
    assert_eq!(
        crc32c(&descending),
        0x113f_db5c,
        "RFC 3720 vector: 0x1f..0x00 descending"
    );
}

// --- round trip ----------------------------------------------------------

#[test]
fn a_batch_round_trips_byte_for_byte_with_every_contract_048_header_field() {
    let event_a = make_event(
        "EVT0000000000000000000001",
        "AAA",
        Timestamp::from_civil(2026, 1, 5),
        Some("trace-a"),
    );
    let event_b = make_event(
        "EVT0000000000000000000002",
        "BBB",
        Timestamp::from_civil(2026, 1, 6),
        None,
    );

    let record_a =
        Record::from_any_event(&event_a, PayloadCodec::CanonicalJson).expect("record a encodes");
    let record_b =
        Record::from_any_event(&event_b, PayloadCodec::CanonicalJson).expect("record b encodes");

    let mut batch = Batch::new(
        MessageType::Data,
        7,
        3,
        PayloadCodec::CanonicalJson,
        vec![record_a, record_b],
    )
    .expect("two records make a valid batch");
    stamp_drain(&mut batch, "producer-7", 4, 900);
    stamp_broker(
        &mut batch,
        555,
        2,
        LogicalTimestamp {
            physical_ns: 1_700_000_000_000,
            logical: 3,
        },
        Some(ContentHash::sha256_of(b"previous batch bytes")),
    );

    // Premise: the batch actually carries two records, one with a trace id
    // and one without, before we round-trip it — otherwise a codec that
    // silently drops the trace id field would have nothing to drop.
    assert_eq!(batch.records.len(), 2);
    assert!(batch.records[0].trace_id.is_some());
    assert!(batch.records[1].trace_id.is_none());

    let bytes = batch.encode().expect("a stamped batch encodes");
    let decoded = match Batch::decode(&bytes).expect("a freshly encoded batch decodes") {
        DecodeOutcome::Complete(decoded) => decoded,
        DecodeOutcome::Torn => panic!("a complete batch must not decode as torn"),
    };

    assert_eq!(decoded, batch, "every field must survive the round trip");

    // Byte for byte: re-encoding what came back out reproduces the same wire
    // bytes, which is what "one on-disk format" (ADR 0100 §1) requires.
    let re_encoded = decoded.encode().expect("the decoded batch re-encodes");
    assert_eq!(re_encoded, bytes);

    // Every CONTRACT-048 field named explicitly, so a codec change that
    // dropped exactly one of them would be caught here even if `Batch`'s
    // `PartialEq` above were ever weakened to skip a field.
    assert_eq!(decoded.message_type, MessageType::Data);
    assert_eq!(decoded.schema_id, 7);
    assert_eq!(decoded.schema_version, 3);
    assert_eq!(decoded.encoding, PayloadCodec::CanonicalJson);
    assert_eq!(decoded.producer_id, "producer-7");
    assert_eq!(decoded.producer_epoch, 4);
    assert_eq!(decoded.base_sequence, 900);
    assert_eq!(decoded.base_offset, 555);
    assert_eq!(decoded.leader_epoch, 2);
    assert_eq!(
        decoded.logical_timestamp,
        LogicalTimestamp {
            physical_ns: 1_700_000_000_000,
            logical: 3
        }
    );
    assert_eq!(
        decoded.previous_batch_hash,
        Some(ContentHash::sha256_of(b"previous batch bytes"))
    );

    assert_eq!(decoded.records[0].event_id, "EVT0000000000000000000001");
    assert_eq!(decoded.records[0].trace_id.as_deref(), Some("trace-a"));
    assert_eq!(
        decoded.records[0].source_timestamp_ns,
        Timestamp::from_civil(2026, 1, 5).as_nanos()
    );
    assert_eq!(decoded.records[1].event_id, "EVT0000000000000000000002");
    assert_eq!(decoded.records[1].trace_id, None);

    let recovered_a = decoded.records[0]
        .decode_payload(PayloadCodec::CanonicalJson)
        .expect("payload a decodes");
    assert_eq!(recovered_a, event_a);
    let recovered_b = decoded.records[1]
        .decode_payload(PayloadCodec::CanonicalJson)
        .expect("payload b decodes");
    assert_eq!(recovered_b, event_b);
}

// --- corruption ------------------------------------------------------------

#[test]
fn flipping_any_bit_of_a_complete_batch_is_refused_as_corruption_naming_its_offset() {
    let (_, batch) = one_record_batch();
    let mut bytes = batch.encode().expect("batch encodes");

    // Premise: the unmodified bytes really are a complete, decodable batch.
    match Batch::decode(&bytes) {
        Ok(DecodeOutcome::Complete(_)) => {}
        other => panic!("expected the unmodified batch to decode as complete, got {other:?}"),
    }

    // Flip the last bit of the last byte before the record's own trailing
    // CRC — the final byte of the JSON payload — so only that record's own
    // CRC, not the prefix or the batch-header CRC, can catch it.
    let payload_flip_at = bytes.len() - 5;
    let mut single_flip = bytes.clone();
    single_flip[payload_flip_at] ^= 0x01;
    let err = match Batch::decode(&single_flip) {
        Err(e) => e,
        Ok(outcome) => panic!("a flipped payload bit must be refused, not decoded as {outcome:?}"),
    };
    assert!(
        err.message().contains("byte offset"),
        "error must name the offset of the corruption: {}",
        err.message()
    );
    assert!(
        err.message().contains("record CRC mismatch"),
        "a flipped payload bit must be caught by the record's own CRC: {}",
        err.message()
    );

    // Exhaustive: every single-bit flip of the *entire* encoded batch must be
    // refused as corruption — never silently accepted as a different
    // complete batch, and never misread as an ordinary torn tail. An earlier
    // version of this codec had no CRC over the prefix, so 23 flips inside
    // the declared-length field (byte offsets 6 through 9) decoded as
    // `Torn` instead of being refused: a bit-rotted length looked exactly
    // like a healthy, incomplete write, and a recovery path that truncates a
    // torn tail (as SLICE-16's does) would have silently discarded every
    // batch committed after it.
    for byte_index in 0..bytes.len() {
        let original = bytes[byte_index];
        for bit in 0..8u8 {
            bytes[byte_index] = original ^ (1 << bit);
            match Batch::decode(&bytes) {
                Err(e) => assert!(
                    e.message().contains("byte offset"),
                    "flipping byte {byte_index} bit {bit} must name an offset: {}",
                    e.message()
                ),
                Ok(outcome) => panic!(
                    "flipping byte {byte_index} bit {bit} must be refused, not decoded \
                     as {outcome:?}"
                ),
            }
        }
        bytes[byte_index] = original;
    }

    // The restore above must be exact, not merely "close enough": a bug in
    // it could otherwise hide behind "every flip was refused".
    assert_eq!(
        bytes,
        batch.encode().expect("batch re-encodes"),
        "the fixture must be restored byte for byte after the exhaustive flip"
    );
}

#[test]
fn a_batch_cut_at_every_byte_reads_as_a_torn_tail_and_never_as_data() {
    let (_, batch) = one_record_batch();
    let bytes = batch.encode().expect("batch encodes");

    // Premise: the full-length batch is genuinely complete, so every shorter
    // prefix below is a truncation of something real.
    match Batch::decode(&bytes) {
        Ok(DecodeOutcome::Complete(_)) => {}
        other => panic!("expected the full-length batch to decode as complete, got {other:?}"),
    }

    for cut in 0..bytes.len() {
        let truncated = &bytes[..cut];
        match Batch::decode(truncated) {
            Ok(DecodeOutcome::Torn) => {}
            Ok(DecodeOutcome::Complete(_)) => {
                panic!("a {cut}-byte prefix must never read as a complete batch")
            }
            Err(e) => panic!("a {cut}-byte prefix must read as torn, not as an error: {e}"),
        }
    }
}

#[test]
fn a_wrong_magic_version_or_oversized_length_is_refused_before_the_payload_is_read() {
    let (_, batch) = one_record_batch();
    let bytes = batch.encode().expect("batch encodes");

    // Premise: the well-formed bytes decode fine, so every failure below is
    // caused by the specific corruption made to a copy of them, not by some
    // other defect in the fixture.
    assert!(matches!(
        Batch::decode(&bytes),
        Ok(DecodeOutcome::Complete(_))
    ));

    // Each fixture below corrupts exactly one prefix field and then repairs
    // the prefix CRC over the corrupted bytes, so what is under test is the
    // specific downstream check (magic, version or the length ceiling) —
    // not the prefix CRC, which a stale value would otherwise trip first
    // and mask the check this test means to isolate.
    let mut wrong_magic = bytes.clone();
    wrong_magic[0..4].copy_from_slice(b"XXXX");
    refresh_prefix_crc(&mut wrong_magic);
    let err = Batch::decode(&wrong_magic).expect_err("a wrong magic must be refused");
    assert!(err.message().contains("magic"), "{}", err.message());

    let mut wrong_version = bytes.clone();
    wrong_version[4..6].copy_from_slice(&99u16.to_le_bytes());
    refresh_prefix_crc(&mut wrong_version);
    let err = Batch::decode(&wrong_version).expect_err("an unknown format version must be refused");
    assert!(
        err.message().contains("format version 99"),
        "{}",
        err.message()
    );

    // An oversized declared length, with no matching bytes behind it at all:
    // the buffer this decode call receives stays far smaller than
    // `MAX_BATCH_LEN`, so the refusal can only be coming from the declared
    // length field itself, checked before any attempt to read that much
    // data — never from the reader discovering, by trying, that the bytes
    // are not there.
    let mut oversized = bytes.clone();
    let huge = (MAX_BATCH_LEN as u32) + 1;
    oversized[6..10].copy_from_slice(&huge.to_le_bytes());
    refresh_prefix_crc(&mut oversized);
    assert!(
        oversized.len() < MAX_BATCH_LEN,
        "the fixture buffer must stay small; the refusal must come from the \
         declared length, not from the buffer actually holding that many bytes"
    );
    let err = Batch::decode(&oversized).expect_err("an oversized declared length must be refused");
    assert!(
        err.message().contains(&MAX_BATCH_LEN.to_string()),
        "{}",
        err.message()
    );
}

#[test]
fn a_blake3_content_hash_or_a_non_json_encoding_is_refused_naming_c2() {
    let (_, mut batch) = one_record_batch();

    // A previous-batch hash tagged Blake3 cannot even be encoded: this build
    // has no Blake3 to verify one against.
    stamp_broker(
        &mut batch,
        1,
        1,
        LogicalTimestamp::default(),
        Some(ContentHash::Blake3([0u8; 32])),
    );
    let err = batch
        .encode()
        .expect_err("encoding a Blake3 content hash must be refused");
    assert!(err.message().contains("Blake3"), "{}", err.message());
    assert!(err.message().contains("C2"), "{}", err.message());

    // A wire encoding byte other than canonical JSON (0) is refused on
    // decode. The encoding byte's fixed position — the prefix (magic, a u16
    // version, a u32 length and the prefix's own CRC32C), then one byte
    // each of message type and flags — is the documented wire layout, not
    // an internal that could silently drift under the test.
    let (_, plain_batch) = one_record_batch();
    let bytes = plain_batch.encode().expect("batch encodes");
    let encoding_offset = PREFIX_LEN + 1 + 1;

    // Premise: that offset really does hold the canonical-JSON tag (0)
    // before it is corrupted.
    assert_eq!(
        bytes[encoding_offset], 0,
        "premise: the encoding byte must be canonical JSON (0) before mutation"
    );

    let mut bad_encoding = bytes;
    bad_encoding[encoding_offset] = 1;
    let err = Batch::decode(&bad_encoding).expect_err("encoding byte 1 must be refused");
    assert!(
        err.message().contains("canonical JSON"),
        "{}",
        err.message()
    );
    assert!(err.message().contains("C2"), "{}", err.message());
}

// --- decode_prefix: splitting a multi-batch buffer --------------------------

/// FU-CODEC / SLICE-28: a fetch response may carry more than one encoded
/// batch concatenated in one buffer (`qip_transport::event_fabric::protocol`'s
/// `FetchResponse::batches`), and a caller splitting them apart needs to know
/// exactly where the first one ends without re-deriving this codec's own
/// framing. Before `Batch::decode_prefix` existed, nothing exposed that
/// length, and `qip-transport`'s consumer decoded only the first batch of
/// every fetch and silently dropped the rest.
///
/// Mutation: in `decode_frame`, report `bytes.len()` (the whole input) as the
/// consumed length instead of `end` (the first frame's own length) — fails,
/// because `consumed` then equals the length of the *concatenated* buffer,
/// not the first batch alone, and the assertion on `first_bytes.len()` catches
/// it.
#[test]
fn decoding_two_concatenated_batches_consumes_exactly_the_first() {
    let first = tagged_batch(101, "EVT0000000000000000000101");
    let second = tagged_batch(102, "EVT0000000000000000000102");
    let first_bytes = first.encode().expect("first batch encodes");
    let second_bytes = second.encode().expect("second batch encodes");

    let mut concatenated = first_bytes.clone();
    concatenated.extend_from_slice(&second_bytes);

    // Premise: the two batches really are distinct, differently-sized frames
    // glued together, not one buffer that happens to satisfy the assertions
    // below by coincidence.
    assert_ne!(first_bytes, second_bytes, "premise: the two batches differ");
    assert_eq!(
        concatenated.len(),
        first_bytes.len() + second_bytes.len(),
        "premise: the buffer under test is genuinely both frames concatenated"
    );

    let (decoded_first, consumed) = match Batch::decode_prefix(&concatenated) {
        Ok(PrefixDecodeOutcome::Complete { batch, consumed }) => (batch, consumed),
        other => panic!("expected the first frame to decode as complete, got {other:?}"),
    };
    assert_eq!(
        decoded_first.schema_id, 101,
        "decode_prefix must return the first batch, not the second"
    );
    assert_eq!(
        consumed,
        first_bytes.len(),
        "decode_prefix must report exactly the first batch's own byte length as consumed, \
         not the length of the whole concatenated buffer"
    );

    // The boundary `consumed` names must be exact: what is left over is
    // byte-for-byte the second batch's own encoding, and decoding it on its
    // own must recover it.
    let remainder = &concatenated[consumed..];
    assert_eq!(
        remainder,
        second_bytes.as_slice(),
        "the bytes after `consumed` must be exactly the second batch's own encoding"
    );
    match Batch::decode_prefix(remainder) {
        Ok(PrefixDecodeOutcome::Complete {
            batch,
            consumed: second_consumed,
        }) => {
            assert_eq!(batch.schema_id, 102);
            assert_eq!(second_consumed, second_bytes.len());
        }
        other => panic!("expected the remainder to decode as the second batch, got {other:?}"),
    }
}

// --- per-stage stamping ------------------------------------------------

#[test]
fn a_batch_stamped_by_the_drain_and_then_the_broker_changes_only_the_fields_each_stage_owns() {
    let (_, writer_batch) = one_record_batch();

    // Premise: before any stamp, the drain- and broker-owned fields sit at
    // their writer-time defaults, so a later change to them is genuinely
    // attributable to the stamp that made it, not to `Batch::new`.
    assert_eq!(writer_batch.producer_id, "");
    assert_eq!(writer_batch.producer_epoch, 0);
    assert_eq!(writer_batch.base_offset, 0);
    assert_eq!(writer_batch.previous_batch_hash, None);

    let mut drain_batch = writer_batch.clone();
    stamp_drain(&mut drain_batch, "producer-drain", 5, 42);

    assert_ne!(drain_batch.producer_id, writer_batch.producer_id);
    assert_ne!(drain_batch.producer_epoch, writer_batch.producer_epoch);
    assert_ne!(drain_batch.base_sequence, writer_batch.base_sequence);
    // Every field the drain does not own must read exactly as the writer
    // left it — this is what would catch a drain stamp that also rewrote,
    // say, the first record's event id.
    assert_eq!(drain_batch.message_type, writer_batch.message_type);
    assert_eq!(drain_batch.schema_id, writer_batch.schema_id);
    assert_eq!(drain_batch.schema_version, writer_batch.schema_version);
    assert_eq!(drain_batch.encoding, writer_batch.encoding);
    assert_eq!(drain_batch.records, writer_batch.records);
    assert_eq!(drain_batch.base_offset, writer_batch.base_offset);
    assert_eq!(drain_batch.leader_epoch, writer_batch.leader_epoch);
    assert_eq!(
        drain_batch.logical_timestamp,
        writer_batch.logical_timestamp
    );
    assert_eq!(
        drain_batch.previous_batch_hash,
        writer_batch.previous_batch_hash
    );

    let mut broker_batch = drain_batch.clone();
    stamp_broker(
        &mut broker_batch,
        1234,
        7,
        LogicalTimestamp {
            physical_ns: 42,
            logical: 1,
        },
        Some(ContentHash::sha256_of(b"chain")),
    );

    assert_ne!(broker_batch.base_offset, drain_batch.base_offset);
    assert_ne!(broker_batch.leader_epoch, drain_batch.leader_epoch);
    assert_ne!(
        broker_batch.logical_timestamp,
        drain_batch.logical_timestamp
    );
    assert_ne!(
        broker_batch.previous_batch_hash,
        drain_batch.previous_batch_hash
    );
    // Every field the broker does not own must read exactly as the drain
    // left it.
    assert_eq!(broker_batch.message_type, drain_batch.message_type);
    assert_eq!(broker_batch.schema_id, drain_batch.schema_id);
    assert_eq!(broker_batch.schema_version, drain_batch.schema_version);
    assert_eq!(broker_batch.encoding, drain_batch.encoding);
    assert_eq!(broker_batch.records, drain_batch.records);
    assert_eq!(broker_batch.producer_id, drain_batch.producer_id);
    assert_eq!(broker_batch.producer_epoch, drain_batch.producer_epoch);
    assert_eq!(broker_batch.base_sequence, drain_batch.base_sequence);
}
