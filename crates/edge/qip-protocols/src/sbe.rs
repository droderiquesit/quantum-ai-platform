//! Simple Binary Encoding, driven by an in-tree schema description.
//!
//! SBE's whole value is that the layout is known ahead of time, so decoding is
//! arithmetic rather than parsing. The corollary is that the layout has to come
//! from somewhere, and the usual somewhere — an XML schema compiled into
//! generated code — would put a code generator and a build step between this
//! platform and its data. The schema is therefore a value: a [`SbeSchema`] built
//! at startup, checked once, and read at decode time.
//!
//! **Version tolerance is the point of the block length.** Every advance through
//! the buffer uses the length the *message* declares, never the length the
//! schema expects. A publisher that upgrades to a schema version with three new
//! trailing fields keeps decoding here without a redeploy: the extra bytes are
//! stepped over. A publisher still on an older version keeps decoding too — the
//! fields it does not have simply fall outside its block and read as absent. A
//! decoder that used its own expected block length would break in both
//! directions, and would break at the moment the venue changed something,
//! which is the worst possible moment.
//!
//! Framing is the Simple Open Framing Header: a 4-byte big-endian message length
//! covering the whole message, then a 2-byte encoding identifier. Without it an
//! unknown template could not be skipped, because nothing else in an SBE message
//! says how long it is.

use crate::bytes::{ByteOrder, Reader};
use crate::decoder::{
    Decoder, Diagnostics, FeedIdentity, InstrumentPartitions, SkipReason, SkipRecord,
    build_message, check_frame_bound,
};
use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PROTOCOL: &str = "sbe";

/// Bytes of the Simple Open Framing Header.
pub const SOFH_LENGTH: usize = 6;
/// Bytes of the SBE message header.
pub const HEADER_LENGTH: usize = 8;
/// Bytes of a repeating-group header: block length then entry count.
pub const GROUP_HEADER_LENGTH: usize = 4;

/// How a field is laid out on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SbeEncoding {
    /// Unsigned integer of `width` bytes.
    Uint { width: usize },
    /// Two's-complement signed integer of `width` bytes.
    Int { width: usize },
    /// Fixed-width ASCII, space padded.
    Ascii { width: usize },
    /// An integer mantissa with a constant implied exponent — SBE's usual price
    /// encoding, and the only one that stays exact.
    Fixed {
        width: usize,
        signed: bool,
        exponent: u32,
    },
}

impl SbeEncoding {
    pub const fn width(&self) -> usize {
        match self {
            Self::Uint { width }
            | Self::Int { width }
            | Self::Ascii { width }
            | Self::Fixed { width, .. } => *width,
        }
    }
}

/// What a field means, independent of what the venue calls it.
///
/// Mapping by role rather than by name keeps the schema data and the decoder
/// logic apart: a venue that names its price field `MDEntryPx` and one that
/// names it `Px` need two schema values, not two decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    /// The venue's sequence number for this message.
    Sequence,
    /// Venue timestamp, nanoseconds since the Unix epoch.
    VenueTimeNanos,
    Symbol,
    Side,
    Price,
    Quantity,
    OrderCount,
    /// 0 new, 1 change, 2 delete — the FIX MDUpdateAction convention.
    UpdateAction,
    /// The aggressing side of a trade.
    Aggressor,
    /// Trading state.
    Status,
    /// Present in the schema but not interpreted; decoded so it can be logged.
    Informational,
}

/// One field of a fixed block or a group entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbeField {
    pub name: String,
    pub offset: usize,
    pub encoding: SbeEncoding,
    pub role: FieldRole,
}

impl SbeField {
    pub fn new(
        name: impl Into<String>,
        offset: usize,
        encoding: SbeEncoding,
        role: FieldRole,
    ) -> Self {
        Self {
            name: name.into(),
            offset,
            encoding,
            role,
        }
    }

    fn end(&self) -> usize {
        self.offset + self.encoding.width()
    }
}

/// A repeating group: a header with its own block length, then entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbeGroup {
    pub name: String,
    pub fields: Vec<SbeField>,
}

/// What a template says about the market.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SbeMessageKind {
    /// Aggregated price levels.
    BookUpdate,
    /// Prints.
    Trade,
    /// A trading-state change.
    Status,
}

/// One template in the schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbeMessage {
    pub template_id: u16,
    pub name: String,
    pub kind: SbeMessageKind,
    pub fields: Vec<SbeField>,
    pub group: Option<SbeGroup>,
}

/// The layout of one venue's binary feed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SbeSchema {
    pub id: u16,
    pub version: u16,
    pub byte_order: ByteOrder,
    messages: BTreeMap<u16, SbeMessage>,
}

impl SbeSchema {
    pub fn new(id: u16, version: u16, byte_order: ByteOrder) -> Self {
        Self {
            id,
            version,
            byte_order,
            messages: BTreeMap::new(),
        }
    }

    pub fn with_message(mut self, message: SbeMessage) -> Self {
        self.messages.insert(message.template_id, message);
        self
    }

    pub fn message(&self, template_id: u16) -> Option<&SbeMessage> {
        self.messages.get(&template_id)
    }

    pub fn template_ids(&self) -> Vec<u16> {
        self.messages.keys().copied().collect()
    }

    /// Reject a schema that cannot produce well-formed messages.
    ///
    /// Checked once at construction rather than per message. A schema missing a
    /// venue timestamp would otherwise decode for hours and then be discovered
    /// by a downstream consumer wondering why every message claims the epoch.
    pub fn validate(&self) -> Result<()> {
        if self.messages.is_empty() {
            return Err(Error::invalid("sbe schema declares no templates"));
        }
        for message in self.messages.values() {
            if !message
                .fields
                .iter()
                .any(|field| field.role == FieldRole::VenueTimeNanos)
            {
                return Err(Error::invalid(format!(
                    "sbe template {} ({}) declares no venue timestamp field",
                    message.template_id, message.name
                )));
            }
            let entry_fields = message
                .group
                .as_ref()
                .map_or(&message.fields, |group| &group.fields);
            let needs_price = matches!(
                message.kind,
                SbeMessageKind::BookUpdate | SbeMessageKind::Trade
            );
            if needs_price
                && !entry_fields
                    .iter()
                    .any(|field| field.role == FieldRole::Price)
            {
                return Err(Error::invalid(format!(
                    "sbe template {} ({}) carries prices but declares no price field",
                    message.template_id, message.name
                )));
            }
        }
        Ok(())
    }
}

/// A decoded field value.
#[derive(Clone, Debug, PartialEq)]
pub enum SbeValue {
    Uint(u64),
    Int(i64),
    Text(String),
    Number(Decimal),
}

impl SbeValue {
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Uint(value) => Some(*value),
            Self::Int(value) => u64::try_from(*value).ok(),
            Self::Number(value) => u64::try_from(value.raw() / qip_core::decimal::SCALE).ok(),
            Self::Text(_) => None,
        }
    }

    pub fn as_decimal(&self) -> Option<Decimal> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Uint(value) => i64::try_from(*value).ok().map(Decimal::from_int),
            Self::Int(value) => Some(Decimal::from_int(*value)),
            Self::Text(text) => Decimal::parse(text),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// A side written either as an ASCII `B`/`S` or as a 0/1 enumeration.
    fn as_side(&self) -> Option<BookSide> {
        match self {
            Self::Text(text) => match text.chars().next() {
                Some('B') | Some('b') | Some('0') => Some(BookSide::Bid),
                Some('S') | Some('s') | Some('A') | Some('a') | Some('1') => Some(BookSide::Ask),
                _ => None,
            },
            Self::Uint(0) => Some(BookSide::Bid),
            Self::Uint(1) => Some(BookSide::Ask),
            _ => None,
        }
    }
}

/// The fields of one block, keyed by role.
#[derive(Clone, Debug, Default)]
struct RoleValues {
    values: BTreeMap<FieldRole, SbeValue>,
}

impl RoleValues {
    fn get(&self, role: FieldRole) -> Option<&SbeValue> {
        self.values.get(&role)
    }
}

/// Decodes SBE messages against a schema.
#[derive(Debug)]
pub struct SbeDecoder {
    identity: FeedIdentity,
    instruments: InstrumentPartitions,
    schema: SbeSchema,
    /// Used when a template declares no sequence field, so that messages from
    /// this feed still carry a strictly increasing position.
    next_sequence: u64,
    diagnostics: Diagnostics,
    consumed: usize,
}

impl SbeDecoder {
    pub fn new(
        venue: VenueId,
        feed: impl Into<String>,
        instruments: InstrumentPartitions,
        schema: SbeSchema,
    ) -> Result<Self> {
        schema.validate()?;
        Ok(Self {
            identity: FeedIdentity::new(venue, feed),
            instruments,
            schema,
            next_sequence: 1,
            diagnostics: Diagnostics::default(),
            consumed: 0,
        })
    }

    pub fn set_sequence(&mut self, sequence: u64) {
        self.next_sequence = sequence;
    }

    pub fn schema(&self) -> &SbeSchema {
        &self.schema
    }

    fn skip(&mut self, reason: SkipReason, offset: usize, at: Timestamp) {
        self.diagnostics.record_skip(SkipRecord {
            protocol: PROTOCOL.to_string(),
            reason,
            offset,
            at,
        });
    }

    /// Read the fields that fit inside `block`, ignoring those that do not.
    ///
    /// A field beyond the wire block length belongs to a schema version the
    /// publisher has not reached yet. That is not an error — it is exactly the
    /// case version tolerance exists for.
    fn read_block(&self, block: &[u8], fields: &[SbeField]) -> Result<RoleValues> {
        let reader = Reader::new(block, self.schema.byte_order);
        let mut values = RoleValues::default();
        for field in fields {
            if field.end() > block.len() {
                continue;
            }
            let value = match field.encoding {
                SbeEncoding::Uint { width } => SbeValue::Uint(reader.uint(field.offset, width)?),
                SbeEncoding::Int { width } => SbeValue::Int(reader.int(field.offset, width)?),
                SbeEncoding::Ascii { width } => SbeValue::Text(reader.ascii(field.offset, width)?),
                SbeEncoding::Fixed {
                    width,
                    signed,
                    exponent,
                } => SbeValue::Number(reader.fixed(field.offset, width, signed, exponent)?),
            };
            if field.role != FieldRole::Informational {
                values.values.insert(field.role, value);
            }
        }
        Ok(values)
    }

    fn body_for(
        &mut self,
        kind: SbeMessageKind,
        header: &RoleValues,
        entry: &RoleValues,
        offset: usize,
        captured_at: Timestamp,
    ) -> Option<MessageBody> {
        match kind {
            SbeMessageKind::BookUpdate => {
                let side = entry
                    .get(FieldRole::Side)
                    .or_else(|| header.get(FieldRole::Side))
                    .and_then(SbeValue::as_side)?;
                let price = entry.get(FieldRole::Price)?.as_decimal()?;
                let action = entry
                    .get(FieldRole::UpdateAction)
                    .and_then(SbeValue::as_u64)
                    .unwrap_or(0);
                let quantity = if action == 2 {
                    Decimal::ZERO
                } else {
                    entry.get(FieldRole::Quantity)?.as_decimal()?
                };
                Some(MessageBody::LevelSet {
                    side,
                    price,
                    quantity,
                    order_count: entry
                        .get(FieldRole::OrderCount)
                        .and_then(SbeValue::as_u64)
                        .and_then(|count| u32::try_from(count).ok()),
                })
            }
            SbeMessageKind::Trade => Some(MessageBody::Trade {
                price: entry.get(FieldRole::Price)?.as_decimal()?,
                quantity: entry.get(FieldRole::Quantity)?.as_decimal()?,
                condition: TradeCondition::Regular,
                aggressor: entry
                    .get(FieldRole::Aggressor)
                    .or_else(|| entry.get(FieldRole::Side))
                    .and_then(SbeValue::as_side),
            }),
            SbeMessageKind::Status => {
                let raw = entry
                    .get(FieldRole::Status)
                    .or_else(|| header.get(FieldRole::Status))?;
                let status = match raw {
                    SbeValue::Uint(0) => VenueStatus::Closed,
                    SbeValue::Uint(1) => VenueStatus::Auction,
                    SbeValue::Uint(2) => VenueStatus::Open,
                    SbeValue::Uint(3) => VenueStatus::Halted,
                    other => {
                        let detail = format!("unmapped trading state {other:?}");
                        self.skip(SkipReason::Malformed { detail }, offset, captured_at);
                        return None;
                    }
                };
                Some(MessageBody::StatusChange { status })
            }
        }
    }
}

impl Decoder for SbeDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        self.consumed = 0;
        let mut out = Vec::new();
        let mut position = 0usize;

        loop {
            let Some(sofh) = bytes.get(position..position + SOFH_LENGTH) else {
                break;
            };
            let framing = Reader::new(sofh, ByteOrder::Big);
            let declared = framing.uint(0, 4)? as usize;
            check_frame_bound(PROTOCOL, declared)?;
            if declared < SOFH_LENGTH + HEADER_LENGTH {
                return Err(Error::schema(format!(
                    "{PROTOCOL}: framed length {declared} at offset {position} is shorter than a header"
                )));
            }
            let Some(message) = bytes.get(position + SOFH_LENGTH..position + declared) else {
                break;
            };

            let reader = Reader::new(message, self.schema.byte_order);
            let block_length = reader.uint(0, 2)? as usize;
            let template_id = reader.uint(2, 2)? as u16;
            let schema_id = reader.uint(4, 2)? as u16;
            let version = reader.uint(6, 2)? as u16;
            if schema_id != self.schema.id {
                return Err(Error::schema(format!(
                    "{PROTOCOL}: message declares schema {schema_id}, decoder is configured for {}",
                    self.schema.id
                )));
            }

            let sequence = self.next_sequence;
            self.decode_message(
                message,
                block_length,
                template_id,
                version,
                sequence,
                position,
                captured_at,
                &mut out,
            )?;

            self.next_sequence += 1;
            position += declared;
            self.consumed = position;
            self.diagnostics.bytes_consumed += declared as u64;
        }
        Ok(out)
    }

    fn protocol(&self) -> &str {
        PROTOCOL
    }

    fn consumed(&self) -> usize {
        self.consumed
    }

    fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

impl SbeDecoder {
    fn decode_message(
        &mut self,
        message: &[u8],
        block_length: usize,
        template_id: u16,
        version: u16,
        sequence: u64,
        offset: usize,
        captured_at: Timestamp,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        let Some(template) = self.schema.message(template_id).cloned() else {
            // Skippable precisely because the framing header said how long the
            // message is. This is why the framing header is mandatory here.
            self.skip(
                SkipReason::UnknownMessageType {
                    code: format!("template {template_id} v{version}"),
                },
                offset,
                captured_at,
            );
            return Ok(());
        };

        let block = message
            .get(HEADER_LENGTH..HEADER_LENGTH + block_length)
            .ok_or_else(|| {
                Error::schema(format!(
                    "{PROTOCOL}: template {template_id} declares a {block_length}-byte block that does not fit the framed message"
                ))
            })?;
        let header_values = self.read_block(block, &template.fields)?;

        let Some(venue_nanos) = header_values
            .get(FieldRole::VenueTimeNanos)
            .and_then(SbeValue::as_u64)
        else {
            self.skip(
                SkipReason::Malformed {
                    detail: format!("template {template_id} carried no venue timestamp"),
                },
                offset,
                captured_at,
            );
            return Ok(());
        };
        let venue_time = Timestamp::from_nanos(i64::try_from(venue_nanos).map_err(|_| {
            Error::schema(format!("{PROTOCOL}: venue timestamp {venue_nanos} is out of range"))
        })?);

        let sequence = header_values
            .get(FieldRole::Sequence)
            .and_then(SbeValue::as_u64)
            .unwrap_or(sequence);

        // Entries come from the repeating group where the template has one, and
        // from the fixed block itself where it does not.
        let mut entries: Vec<RoleValues> = Vec::new();
        if let Some(group) = &template.group {
            let group_start = HEADER_LENGTH + block_length;
            let group_header = message.get(group_start..group_start + GROUP_HEADER_LENGTH);
            let Some(group_header) = group_header else {
                self.skip(
                    SkipReason::Malformed {
                        detail: format!("template {template_id} declares group `{}` with no header", group.name),
                    },
                    offset,
                    captured_at,
                );
                return Ok(());
            };
            let header_reader = Reader::new(group_header, self.schema.byte_order);
            let entry_length = header_reader.uint(0, 2)? as usize;
            let count = header_reader.uint(2, 2)? as usize;
            if entry_length == 0 {
                return Err(Error::schema(format!(
                    "{PROTOCOL}: template {template_id} group `{}` declares a zero-length entry",
                    group.name
                )));
            }
            let mut cursor = group_start + GROUP_HEADER_LENGTH;
            for _ in 0..count {
                let Some(entry_bytes) = message.get(cursor..cursor + entry_length) else {
                    return Err(Error::schema(format!(
                        "{PROTOCOL}: template {template_id} group `{}` claims {count} entries that do not fit the framed message",
                        group.name
                    )));
                };
                entries.push(self.read_block(entry_bytes, &group.fields)?);
                cursor += entry_length;
            }
        } else {
            entries.push(header_values.clone());
        }

        let symbol_from = |values: &RoleValues| -> Option<String> {
            values
                .get(FieldRole::Symbol)
                .and_then(|value| value.as_text().map(str::to_string))
        };
        let message_symbol = symbol_from(&header_values);

        let mut ordinal = 0u32;
        for entry in &entries {
            let Some(symbol) = symbol_from(entry).or_else(|| message_symbol.clone()) else {
                self.skip(
                    SkipReason::Malformed {
                        detail: format!("template {template_id} entry carries no symbol"),
                    },
                    offset,
                    captured_at,
                );
                continue;
            };
            let Some(partition) = self.instruments.resolve(&symbol) else {
                self.skip(SkipReason::UnmappedInstrument { symbol }, offset, captured_at);
                continue;
            };
            let Some(body) =
                self.body_for(template.kind, &header_values, entry, offset, captured_at)
            else {
                continue;
            };
            out.push(build_message(
                self.identity.origin(partition, sequence),
                ordinal,
                body,
                venue_time,
                captured_at,
            ));
            ordinal += 1;
            self.diagnostics.messages_decoded += 1;
        }
        Ok(())
    }
}

/// Write an unsigned integer in `order` — the inverse of [`Reader::uint`].
pub fn put_uint(buffer: &mut Vec<u8>, order: ByteOrder, width: usize, value: u64) {
    let bytes: Vec<u8> = (0..width)
        .map(|index| {
            let shift = match order {
                ByteOrder::Big => 8 * (width - 1 - index),
                ByteOrder::Little => 8 * index,
            };
            ((value >> shift) & 0xFF) as u8
        })
        .collect();
    buffer.extend_from_slice(&bytes);
}

/// Write fixed-width ASCII, space padded.
pub fn put_ascii(buffer: &mut Vec<u8>, width: usize, text: &str) {
    let bytes = text.as_bytes();
    for index in 0..width {
        buffer.push(bytes.get(index).copied().unwrap_or(b' '));
    }
}

/// Wrap a block and its optional group in the framing and message headers.
///
/// Kept beside the decoder so that a change to the framing rule cannot be made
/// on one side only; every test that produces SBE bytes goes through here.
pub fn frame_message(
    schema: &SbeSchema,
    template_id: u16,
    version: u16,
    block: &[u8],
    group: Option<(usize, usize, &[u8])>,
) -> Vec<u8> {
    let mut body = Vec::new();
    put_uint(&mut body, schema.byte_order, 2, block.len() as u64);
    put_uint(&mut body, schema.byte_order, 2, u64::from(template_id));
    put_uint(&mut body, schema.byte_order, 2, u64::from(schema.id));
    put_uint(&mut body, schema.byte_order, 2, u64::from(version));
    body.extend_from_slice(block);
    if let Some((entry_length, count, entries)) = group {
        put_uint(&mut body, schema.byte_order, 2, entry_length as u64);
        put_uint(&mut body, schema.byte_order, 2, count as u64);
        body.extend_from_slice(entries);
    }

    let mut out = Vec::new();
    put_uint(&mut out, ByteOrder::Big, 4, (SOFH_LENGTH + body.len()) as u64);
    // 0x5BE0 is the SBE little-endian encoding identifier; the big-endian one is
    // 0xEB50. Recorded for completeness — the schema, not the frame, decides.
    put_uint(
        &mut out,
        ByteOrder::Big,
        2,
        match schema.byte_order {
            ByteOrder::Little => 0x5BE0,
            ByteOrder::Big => 0xEB50,
        },
    );
    out.extend_from_slice(&body);
    out
}
