//! The record and batch codec for the event fabric.
//!
//! ADR 0100 §1 fixes this as **one on-disk format, used by the spool, the
//! wire and broker segments. A batch is written once by the drain and
//! appended as-is.** One format serving three custodians only works if it is
//! unambiguous who may touch which byte, so this module's header is a table,
//! not a blob: every field belongs to exactly one stage, and that stage's
//! stamping function is the only code in the tree allowed to set it.
//!
//! ## Which stage sets each field
//!
//! | Field | Stage | Why that stage |
//! |---|---|---|
//! | magic, format version | writer | fixed at the format's definition, carried by every batch |
//! | message type | writer | chosen when the batch is built, from [`MessageType`] |
//! | schema id, schema version | writer | the schema the records were built against |
//! | flags | writer | reserved; this build defines no bit and refuses any set one rather than guess what it meant |
//! | encoding | writer | the [`PayloadCodec`] every record's payload was written under |
//! | event ids, source timestamps, trace ids | writer | copied from each [`AnyEvent`] so a consumer can read them without decoding JSON |
//! | payload lengths, record CRCs | writer | computed once from record content that no later stage touches |
//! | producer id, producer epoch, base sequence | drain | ADR 0100 §4: the sequence is dense per `(stream, partition)` and is assigned **at drain time**, not by the writer |
//! | base offset, leader epoch | broker | the partition log position and the epoch of the broker that accepted the batch |
//! | logical (HLC) timestamp | broker | assigned on admission to the partition; carried as the two plain integers below, since [`Hlc`](super::hlc)'s own causality rules are SLICE-14's, not this codec's |
//! | previous-batch hash | broker | chains this batch to the one before it in the same partition |
//!
//! Three fields are **not** in this table because they are framing, not
//! CONTRACT-048 content: the total body length (so a reader can tell a torn
//! tail from a corrupt one before touching a payload, exactly as
//! `qip-storage`'s WAL frame does), the record count (so the reader knows how
//! many records to walk) and the batch CRC (recomputed by every stamp, over
//! everything the stamp just wrote — see below). No field in CONTRACT-048's
//! list needed a home this split could not give it.
//!
//! ## Three stamps, one CRC discipline
//!
//! [`Batch`] holds no stored CRC field. [`Batch::encode`] always computes the
//! batch CRC fresh from whatever the struct currently holds, so "the stamp
//! recomputes the batch CRC" is not a step a stamping function can forget —
//! there is no stale value it could instead leave in place. [`Batch::new`] is
//! the writer's stamp; [`stamp_drain`] and [`stamp_broker`] are the other two,
//! and each touches only the fields its own row of the table above names.
//!
//! ## Decoding has exactly three outcomes
//!
//! Copied from `qip-storage`'s `read_frame` (`backend/crates/libs/qip-storage/src/engine/frame.rs`),
//! because a batch that outlives one process and travels through three
//! custodians needs the same discipline a WAL frame does:
//!
//! * **Complete** — magic, version and length checked, every record's CRC and
//!   the batch CRC matched. [`DecodeOutcome::Complete`] carries the [`Batch`].
//! * **Torn tail** — the buffer ends before the declared length, which is
//!   what a crash mid-write or a partial read looks like. [`DecodeOutcome::Torn`],
//!   never an error: a torn tail is an ordinary, expected shape at the end of
//!   a live segment.
//! * **Corrupt** — the bytes are all present but a CRC does not match, or the
//!   magic, version or a tag byte is a value nothing in this build wrote.
//!   [`Batch::decode`] returns `Err`, naming the byte offset, and never
//!   returns the mismatched payload as if it were data.
//!
//! Magic, format version and the declared length are checked **before** any
//! record is parsed, so a wrong magic, an unknown version or a length past
//! [`MAX_BATCH_LEN`] is refused without the reader ever indexing into a
//! payload it has not yet proven is there.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256;

use crate::envelope::{AnyEvent, canonical_json};

use super::crc32c::crc32c;

/// Marks the start of an event-fabric batch. Distinct from `qip-storage`'s
/// `QWAL` frame magic and from `qip-capital-fabric`'s framing, so a reader
/// handed the wrong stream refuses immediately rather than misparsing it.
pub const BATCH_MAGIC: [u8; 4] = *b"QEVB";

/// Bumped only for an incompatible layout change; an older or newer batch is
/// refused rather than guessed at.
pub const FORMAT_VERSION: u16 = 1;

/// Refuse to trust a declared body length before the buffer is known to hold
/// that many bytes. 32 MiB comfortably covers a fast brain's decision batch
/// with room to spare, while still bounding the arithmetic below against a
/// corrupt or malicious length field.
pub const MAX_BATCH_LEN: usize = 32 * 1024 * 1024;

/// magic (4) + format version (2) + body length (4).
const FIXED_PREFIX_LEN: usize = 4 + 2 + 4;

/// No flag bit is defined yet. Any other value is refused on decode rather
/// than silently accepted and ignored, so a future flag a reader does not
/// understand cannot be mistaken for one it does.
const FLAGS_NONE: u8 = 0;

/// The wire tag for [`ContentHash::Sha256`] in the previous-batch-hash field.
const HASH_TAG_NONE: u8 = 0;
const HASH_TAG_SHA256: u8 = 1;
const HASH_TAG_BLAKE3: u8 = 2;

/// The wire tag for [`PayloadCodec::CanonicalJson`].
const ENCODING_TAG_CANONICAL_JSON: u8 = 0;

/// The wire tag for [`MessageType::Data`].
const MESSAGE_TYPE_TAG_DATA: u8 = 0;

/// How a record's payload bytes are encoded.
///
/// The swap seam ADR 0100's alternatives-rejected section keeps from the
/// fidelity-first proposal: a batch always names its own encoding, so a
/// future codec (prost, once ADR 0099 conflict C2 is resolved) can be added
/// as a new tag without breaking a reader of the old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadCodec {
    /// Canonical JSON of an [`AnyEvent`], via [`canonical_json`]. The only
    /// codec this build can write or read.
    CanonicalJson,
}

impl PayloadCodec {
    fn tag(self) -> u8 {
        match self {
            PayloadCodec::CanonicalJson => ENCODING_TAG_CANONICAL_JSON,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            ENCODING_TAG_CANONICAL_JSON => Ok(PayloadCodec::CanonicalJson),
            other => Err(Error::schema(format!(
                "payload encoding byte {other} is not canonical JSON (0); every \
                 other codec (prost included) is blocked pending ADR 0099 conflict C2"
            ))),
        }
    }
}

/// Which digest algorithm produced a content hash.
///
/// Defined here because this batch codec is the first caller; `SLICE-16`'s
/// seals and `SLICE-21`'s manifests carry the same tag so a hash's algorithm
/// travels with it rather than being assumed by whoever reads it later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentHash {
    /// The in-tree SHA-256 (ADR 0043), the only algorithm this build may
    /// produce or accept.
    Sha256([u8; 32]),
    /// Named so the type is ready for ADR 0099 conflict C2's resolution, but
    /// refused everywhere this codec touches it: encoding and decoding both
    /// name the conflict rather than carry bytes nothing in this build can
    /// verify.
    Blake3([u8; 32]),
}

impl ContentHash {
    /// Hash `bytes` with the in-tree SHA-256 — the one algorithm a caller of
    /// this codec may actually produce today.
    pub fn sha256_of(bytes: &[u8]) -> Self {
        ContentHash::Sha256(sha256(bytes))
    }
}

/// The message kind a batch carries.
///
/// This build's vertical slice (ADR 0100 §8) only ever produces ordinary
/// event data; a future control-plane batch shape is a new tag, not a
/// reinterpretation of this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageType {
    /// Ordinary event records: the only value this build writes.
    Data,
}

impl MessageType {
    fn tag(self) -> u8 {
        match self {
            MessageType::Data => MESSAGE_TYPE_TAG_DATA,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            MESSAGE_TYPE_TAG_DATA => Ok(MessageType::Data),
            other => Err(Error::schema(format!(
                "unknown batch message-type byte {other}"
            ))),
        }
    }
}

/// The broker-assigned logical timestamp, carried as the two plain integers
/// a hybrid logical clock is made of.
///
/// `event_fabric::hlc` (SLICE-14) owns the `Hlc` type and the causality rules
/// that make those two integers meaningful together (a physical clock read
/// and a logical counter that only advances when physical time does not).
/// This codec only needs to carry them across the wire, so it does not wait
/// on SLICE-14 to exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LogicalTimestamp {
    pub physical_ns: i64,
    pub logical: u32,
}

/// One event, framed for the wire: the writer-owned fields of CONTRACT-048's
/// per-record split.
///
/// `event_id`, `trace_id` and `source_timestamp_ns` duplicate what
/// [`AnyEvent`] already carries in its payload. That is deliberate, not
/// redundancy for its own sake: it lets a consumer (or a broker enforcing a
/// per-key produce ACL) read a record's identity, trace and timestamp
/// without decoding and parsing its JSON payload.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub event_id: String,
    pub trace_id: Option<String>,
    pub source_timestamp_ns: i64,
    pub payload: Vec<u8>,
}

impl Record {
    /// Build a record from `event`, encoding its payload under `encoding`.
    pub fn from_any_event(event: &AnyEvent, encoding: PayloadCodec) -> Result<Self> {
        let payload = encode_payload(event, encoding)?;
        Ok(Self {
            event_id: event.event_id.as_str().to_string(),
            trace_id: event
                .lineage
                .trace_id
                .as_ref()
                .map(|trace_id| trace_id.as_str().to_string()),
            source_timestamp_ns: event.occurred_at.as_nanos(),
            payload,
        })
    }

    /// Recover the [`AnyEvent`] this record's payload carries.
    pub fn decode_payload(&self, encoding: PayloadCodec) -> Result<AnyEvent> {
        decode_payload(&self.payload, encoding)
    }
}

fn encode_payload(event: &AnyEvent, encoding: PayloadCodec) -> Result<Vec<u8>> {
    match encoding {
        PayloadCodec::CanonicalJson => {
            let value = serde_json::to_value(event)
                .map_err(|e| Error::invalid(format!("event does not serialise to JSON: {e}")))?;
            Ok(canonical_json(&value).into_bytes())
        }
    }
}

fn decode_payload(bytes: &[u8], encoding: PayloadCodec) -> Result<AnyEvent> {
    match encoding {
        PayloadCodec::CanonicalJson => {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| Error::schema(format!("record payload is not UTF-8: {e}")))?;
            let value: serde_json::Value = serde_json::from_str(text)
                .map_err(|e| Error::schema(format!("record payload is not valid JSON: {e}")))?;
            serde_json::from_value(value).map_err(|e| {
                Error::schema(format!("record payload does not decode as an event: {e}"))
            })
        }
    }
}

/// A batch: the writer's records plus the drain's and broker's header fields,
/// exactly as CONTRACT-048 splits them. See the module documentation's table
/// for which stage owns which field.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    // --- writer-owned ---
    pub message_type: MessageType,
    pub schema_id: u32,
    pub schema_version: u32,
    pub encoding: PayloadCodec,
    pub records: Vec<Record>,
    // --- drain-owned ---
    pub producer_id: String,
    pub producer_epoch: u64,
    pub base_sequence: u64,
    // --- broker-owned ---
    pub base_offset: u64,
    pub leader_epoch: u64,
    pub logical_timestamp: LogicalTimestamp,
    pub previous_batch_hash: Option<ContentHash>,
}

impl Batch {
    /// The writer's stamp: build a batch from its records, before the drain
    /// or the broker have touched anything they own. Refuses an empty batch,
    /// since a batch with nothing in it carries no CONTRACT-048 record fields
    /// for anyone downstream to read.
    pub fn new(
        message_type: MessageType,
        schema_id: u32,
        schema_version: u32,
        encoding: PayloadCodec,
        records: Vec<Record>,
    ) -> Result<Self> {
        if records.is_empty() {
            return Err(Error::invalid(
                "a batch must carry at least one record".to_string(),
            ));
        }
        Ok(Self {
            message_type,
            schema_id,
            schema_version,
            encoding,
            records,
            producer_id: String::new(),
            producer_epoch: 0,
            base_sequence: 0,
            base_offset: 0,
            leader_epoch: 0,
            logical_timestamp: LogicalTimestamp::default(),
            previous_batch_hash: None,
        })
    }

    /// Encode this batch to its wire bytes, recomputing the batch CRC (over
    /// the header fields) and every record's own CRC from whatever the
    /// struct currently holds. The two protect disjoint byte ranges: the
    /// batch CRC never has to be recomputed just because a record's payload
    /// changed, and a record's own CRC never has to be recomputed just
    /// because the drain or the broker stamped the header.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut header = Vec::new();
        header.push(self.message_type.tag());
        header.push(FLAGS_NONE);
        header.push(self.encoding.tag());
        write_u32(&mut header, self.schema_id);
        write_u32(&mut header, self.schema_version);
        write_string(&mut header, &self.producer_id)?;
        write_u64(&mut header, self.producer_epoch);
        write_u64(&mut header, self.base_sequence);
        write_u64(&mut header, self.base_offset);
        write_u64(&mut header, self.leader_epoch);
        write_i64(&mut header, self.logical_timestamp.physical_ns);
        write_u32(&mut header, self.logical_timestamp.logical);
        encode_previous_hash(&mut header, &self.previous_batch_hash)?;

        let record_count = u32::try_from(self.records.len()).map_err(|_| {
            Error::invalid(format!(
                "batch has {} records, more than a u32 record count can carry",
                self.records.len()
            ))
        })?;
        write_u32(&mut header, record_count);

        let batch_crc = crc32c(&header);
        let mut body = header;
        body.extend_from_slice(&batch_crc.to_le_bytes());
        for record in &self.records {
            encode_record(&mut body, record)?;
        }

        if body.len() > MAX_BATCH_LEN {
            return Err(Error::invalid(format!(
                "encoded batch is {} bytes, over the {MAX_BATCH_LEN}-byte ceiling",
                body.len()
            )));
        }
        let body_len = u32::try_from(body.len()).map_err(|_| {
            Error::invalid(format!(
                "encoded batch body is {} bytes, more than a u32 length can carry",
                body.len()
            ))
        })?;

        let mut out = Vec::with_capacity(FIXED_PREFIX_LEN + body.len());
        out.extend_from_slice(&BATCH_MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&body_len.to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decode a batch from `bytes`. See the module documentation for the
    /// three outcomes and the order refusals happen in.
    pub fn decode(bytes: &[u8]) -> Result<DecodeOutcome> {
        if bytes.len() < FIXED_PREFIX_LEN {
            return Ok(DecodeOutcome::Torn);
        }
        if bytes[..4] != BATCH_MAGIC {
            if bytes.iter().all(|b| *b == 0) {
                // A crash can leave a tail of zeroes where a block was
                // allocated but never written; that is a torn tail, not
                // damage.
                return Ok(DecodeOutcome::Torn);
            }
            return Err(Error::schema(format!(
                "corrupt batch at byte offset 0: expected the event-fabric batch \
                 magic, found {:02x?}",
                &bytes[..4]
            )));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != FORMAT_VERSION {
            return Err(Error::schema(format!(
                "batch at byte offset 0 is format version {version}, this build reads \
                 version {FORMAT_VERSION}"
            )));
        }
        let declared_len = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
        // Checked before anything past the header is read: an oversized
        // declared length is refused outright rather than treated as a torn
        // tail, which is what skipping this check would otherwise disguise
        // it as.
        if declared_len > MAX_BATCH_LEN {
            return Err(Error::invalid(format!(
                "batch at byte offset 0 declares a body of {declared_len} bytes, over \
                 the {MAX_BATCH_LEN}-byte ceiling"
            )));
        }
        let end = FIXED_PREFIX_LEN + declared_len;
        if bytes.len() < end {
            return Ok(DecodeOutcome::Torn);
        }
        // From here on `body` is exactly the declared length: any field that
        // does not fit inside it is corruption, not a torn write, because a
        // torn write is precisely what the length check above already ruled
        // out.
        let body = &bytes[FIXED_PREFIX_LEN..end];

        let mut cursor = Cursor::new(body, FIXED_PREFIX_LEN);
        let header_start = cursor.pos;
        let message_type = MessageType::from_tag(cursor.read_u8()?)?;
        let flags = cursor.read_u8()?;
        if flags != FLAGS_NONE {
            return Err(Error::schema(format!(
                "batch at byte offset 0 sets flags byte {flags:#04x}, and this build \
                 defines no bit; refusing rather than guessing what it means"
            )));
        }
        let encoding = PayloadCodec::from_tag(cursor.read_u8()?)?;
        let schema_id = cursor.read_u32()?;
        let schema_version = cursor.read_u32()?;
        let producer_id = cursor.read_string()?;
        let producer_epoch = cursor.read_u64()?;
        let base_sequence = cursor.read_u64()?;
        let base_offset = cursor.read_u64()?;
        let leader_epoch = cursor.read_u64()?;
        let physical_ns = cursor.read_i64()?;
        let logical = cursor.read_u32()?;
        let previous_batch_hash = decode_previous_hash(&mut cursor)?;
        let record_count = cursor.read_u32()?;
        let header_end = cursor.pos;
        let header_bytes = cursor.slice(header_start, header_end);
        let expected_batch_crc = cursor.read_u32()?;
        let actual_batch_crc = crc32c(header_bytes);
        if actual_batch_crc != expected_batch_crc {
            return Err(Error::schema(format!(
                "corrupt batch at byte offset {}: batch CRC mismatch over {} header \
                 bytes (recorded {expected_batch_crc:#010x}, computed {actual_batch_crc:#010x})",
                cursor.absolute(header_start),
                header_bytes.len()
            )));
        }

        let mut records = Vec::new();
        for _ in 0..record_count {
            records.push(decode_record(&mut cursor)?);
        }
        if cursor.remaining() != 0 {
            return Err(Error::schema(format!(
                "corrupt batch at byte offset {}: {} trailing bytes after {record_count} \
                 declared records",
                cursor.absolute(cursor.pos),
                cursor.remaining()
            )));
        }

        Ok(DecodeOutcome::Complete(Batch {
            message_type,
            schema_id,
            schema_version,
            encoding,
            records,
            producer_id,
            producer_epoch,
            base_sequence,
            base_offset,
            leader_epoch,
            logical_timestamp: LogicalTimestamp {
                physical_ns,
                logical,
            },
            previous_batch_hash,
        }))
    }
}

/// What decoding a batch produced. See the module documentation for what
/// distinguishes the two outcomes here from the `Err` case decoding also
/// returns.
#[derive(Debug)]
pub enum DecodeOutcome {
    /// Every check passed; the batch is exactly what its writer, drain and
    /// broker stamped.
    Complete(Batch),
    /// The buffer ends before the batch's declared length. Discard it and
    /// wait for the rest, exactly as a torn WAL frame is handled.
    Torn,
}

/// The drain's stamp (ADR 0100 §4): assigns producer identity and the dense
/// per-`(stream, partition)` sequence, both decided at drain time rather than
/// by the writer. Touches no other field.
pub fn stamp_drain(batch: &mut Batch, producer_id: &str, producer_epoch: u64, base_sequence: u64) {
    batch.producer_id = producer_id.to_string();
    batch.producer_epoch = producer_epoch;
    batch.base_sequence = base_sequence;
}

/// The broker's stamp: the partition-log position, the accepting broker's
/// epoch, the logical timestamp assigned on admission, and the hash chaining
/// this batch to the previous one in the same partition. Touches no other
/// field.
pub fn stamp_broker(
    batch: &mut Batch,
    base_offset: u64,
    leader_epoch: u64,
    logical_timestamp: LogicalTimestamp,
    previous_batch_hash: Option<ContentHash>,
) {
    batch.base_offset = base_offset;
    batch.leader_epoch = leader_epoch;
    batch.logical_timestamp = logical_timestamp;
    batch.previous_batch_hash = previous_batch_hash;
}

fn encode_previous_hash(out: &mut Vec<u8>, hash: &Option<ContentHash>) -> Result<()> {
    match hash {
        None => out.push(HASH_TAG_NONE),
        Some(ContentHash::Sha256(digest)) => {
            out.push(HASH_TAG_SHA256);
            out.extend_from_slice(digest);
        }
        Some(ContentHash::Blake3(_)) => {
            return Err(Error::schema(
                "ContentHash::Blake3 is refused pending ADR 0099 conflict C2; a batch \
                 cannot be encoded carrying a previous-batch hash this build cannot verify"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn decode_previous_hash(cursor: &mut Cursor) -> Result<Option<ContentHash>> {
    match cursor.read_u8()? {
        HASH_TAG_NONE => Ok(None),
        HASH_TAG_SHA256 => {
            let bytes = cursor.take(32)?;
            let mut digest = [0u8; 32];
            digest.copy_from_slice(bytes);
            Ok(Some(ContentHash::Sha256(digest)))
        }
        HASH_TAG_BLAKE3 => Err(Error::schema(
            "ContentHash::Blake3 is refused pending ADR 0099 conflict C2; a batch \
             carrying one cannot be decoded"
                .to_string(),
        )),
        other => Err(Error::schema(format!("unknown content-hash tag {other}"))),
    }
}

fn encode_record(out: &mut Vec<u8>, record: &Record) -> Result<()> {
    let start = out.len();
    write_string(out, &record.event_id)?;
    match &record.trace_id {
        Some(trace_id) => {
            out.push(1);
            write_string(out, trace_id)?;
        }
        None => out.push(0),
    }
    write_i64(out, record.source_timestamp_ns);
    write_bytes(out, &record.payload)?;
    let record_crc = crc32c(&out[start..]);
    out.extend_from_slice(&record_crc.to_le_bytes());
    Ok(())
}

fn decode_record(cursor: &mut Cursor) -> Result<Record> {
    let start = cursor.pos;
    let event_id = cursor.read_string()?;
    let has_trace_id = cursor.read_u8()?;
    let trace_id = match has_trace_id {
        0 => None,
        1 => Some(cursor.read_string()?),
        other => {
            return Err(Error::schema(format!(
                "corrupt batch at byte offset {}: trace-id presence byte {other} is \
                 neither 0 nor 1",
                cursor.absolute(start)
            )));
        }
    };
    let source_timestamp_ns = cursor.read_i64()?;
    let payload = cursor.read_bytes()?;
    let end = cursor.pos;
    let record_bytes = cursor.slice(start, end);
    let expected_crc = cursor.read_u32()?;
    let actual_crc = crc32c(record_bytes);
    if actual_crc != expected_crc {
        return Err(Error::schema(format!(
            "corrupt batch at byte offset {}: record CRC mismatch over {} bytes \
             (recorded {expected_crc:#010x}, computed {actual_crc:#010x})",
            cursor.absolute(start),
            record_bytes.len()
        )));
    }
    Ok(Record {
        event_id,
        trace_id,
        source_timestamp_ns,
        payload,
    })
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    write_bytes(out, value.as_bytes())
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        Error::invalid(format!(
            "field is {} bytes, more than a u32 length can carry",
            bytes.len()
        ))
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

/// A cursor over one batch's already-CRC-verified body, so every fixed-width
/// and length-prefixed read is a bounds check plus an advance rather than
/// hand-indexed arithmetic repeated at every call site.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Offset of `bytes[0]` within the original encoded batch, so error
    /// messages name a position a caller can find with a hex dump of the
    /// wire bytes, not just a position inside this function's own slice.
    base: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], base: usize) -> Self {
        Self {
            bytes,
            pos: 0,
            base,
        }
    }

    fn absolute(&self, pos: usize) -> usize {
        self.base + pos
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn slice(&self, start: usize, end: usize) -> &'a [u8] {
        &self.bytes[start..end]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| {
            Error::schema(format!(
                "corrupt batch at byte offset {}: field length overflowed",
                self.absolute(self.pos)
            ))
        })?;
        if end > self.bytes.len() {
            return Err(Error::schema(format!(
                "corrupt batch at byte offset {}: expected {n} more bytes, found {}",
                self.absolute(self.pos),
                self.bytes.len() - self.pos
            )));
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self) -> Result<String> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes).map_err(|e| {
            Error::schema(format!(
                "corrupt batch at byte offset {}: field is not UTF-8: {e}",
                self.absolute(self.pos)
            ))
        })
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>> {
        let len = self.read_u32()? as usize;
        Ok(self.take(len)?.to_vec())
    }
}
