//! FIX 4.4 tag=value decoding.
//!
//! FIX frames itself out of its own content: the body length says where the
//! checksum sits and the checksum says whether the body length was right. That
//! makes the two checks inseparable — a decoder that trusts the length without
//! verifying the checksum will happily resynchronise onto the middle of a
//! message and emit plausible garbage for the rest of the session. Both are
//! enforced here before a single field is interpreted, and either failing
//! refuses the frame by name.
//!
//! Refusal, not skipping, is deliberate for these two. A message this decoder
//! does not understand can be stepped over because its boundaries are known; a
//! message whose boundaries are in doubt takes the rest of the stream with it,
//! so the caller has to be told rather than handed a shorter batch.

use crate::decoder::{
    Decoder, Diagnostics, FeedIdentity, InstrumentPartitions, SkipReason, SkipRecord,
    build_message, check_frame_bound,
};
use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueId};
use qip_core::error::{Error, Result};
use qip_core::time::days_from_civil;
use qip_core::{Decimal, Timestamp};

/// The field separator. Never appears inside a value.
pub const SOH: u8 = 0x01;

const PROTOCOL: &str = "fix.4.4";

// Tags this decoder reads. Named because `fields.first(268)` at a call site is
// unreadable six months later and mis-typed digits are invisible in review.
const TAG_MSG_TYPE: u32 = 35;
const TAG_MSG_SEQ_NUM: u32 = 34;
const TAG_SENDING_TIME: u32 = 52;
const TAG_SYMBOL: u32 = 55;
const TAG_LAST_PX: u32 = 31;
const TAG_LAST_QTY: u32 = 32;
const TAG_SIDE: u32 = 54;
const TAG_EXEC_TYPE: u32 = 150;
const TAG_NO_MD_ENTRIES: u32 = 268;
const TAG_MD_ENTRY_TYPE: u32 = 269;
const TAG_MD_ENTRY_PX: u32 = 270;
const TAG_MD_ENTRY_SIZE: u32 = 271;
const TAG_MD_UPDATE_ACTION: u32 = 279;
const TAG_NUMBER_OF_ORDERS: u32 = 346;

/// Decodes SOH-delimited FIX 4.4 into market messages.
#[derive(Debug)]
pub struct FixDecoder {
    identity: FeedIdentity,
    instruments: InstrumentPartitions,
    begin_string: String,
    diagnostics: Diagnostics,
    consumed: usize,
}

impl FixDecoder {
    /// A decoder for one venue's FIX market data feed.
    pub fn new(venue: VenueId, feed: impl Into<String>, instruments: InstrumentPartitions) -> Self {
        Self {
            identity: FeedIdentity::new(venue, feed),
            instruments,
            begin_string: "FIX.4.4".to_string(),
            diagnostics: Diagnostics::default(),
            consumed: 0,
        }
    }

    /// Accept a different `BeginString`, for a venue on 4.2 semantics that is
    /// otherwise tag-compatible.
    pub fn with_begin_string(mut self, begin_string: impl Into<String>) -> Self {
        self.begin_string = begin_string.into();
        self
    }

    /// Locate one whole message starting at `at`.
    ///
    /// `Ok(None)` means the buffer holds only part of a message; the caller must
    /// present more bytes. It never means "give up".
    fn frame<'a>(&self, bytes: &'a [u8], at: usize) -> Result<Option<Frame<'a>>> {
        let rest = match bytes.get(at..) {
            Some(rest) if !rest.is_empty() => rest,
            _ => return Ok(None),
        };
        if rest.len() < 2 {
            return Ok(None);
        }
        if &rest[..2] != b"8=" {
            return Err(Error::schema(format!(
                "{PROTOCOL}: the stream is not aligned to a message start at offset {at}; the next bytes are not `8=`"
            )));
        }

        let begin_end = match find(rest, SOH, 0) {
            Some(idx) => idx,
            None => return Ok(None),
        };
        let begin_value = as_text(&rest[2..begin_end])?;
        if begin_value != self.begin_string {
            return Err(Error::schema(format!(
                "{PROTOCOL}: begin string `{begin_value}` is not the configured `{}`",
                self.begin_string
            )));
        }

        let length_start = begin_end + 1;
        let length_tag = rest.get(length_start..length_start + 2);
        match length_tag {
            None => return Ok(None),
            Some(tag) if tag != b"9=" => {
                return Err(Error::schema(format!(
                    "{PROTOCOL}: BodyLength (9) must directly follow BeginString (8)"
                )));
            }
            Some(_) => {}
        }
        let length_end = match find(rest, SOH, length_start) {
            Some(idx) => idx,
            None => return Ok(None),
        };
        let declared_length = parse_usize(as_text(&rest[length_start + 2..length_end])?)
            .ok_or_else(|| Error::schema(format!("{PROTOCOL}: BodyLength (9) is not a number")))?;
        check_frame_bound(PROTOCOL, declared_length)?;

        let body_start = length_end + 1;
        // The checksum field starts exactly BodyLength bytes after the body
        // begins. That equality *is* the length check: nothing else in a FIX
        // message says where the message ends.
        let checksum_start = body_start + declared_length;
        let trailer = match rest.get(checksum_start..checksum_start + 7) {
            Some(trailer) => trailer,
            None => return Ok(None),
        };
        if &trailer[..3] != b"10=" || trailer[6] != SOH {
            return Err(Error::schema(format!(
                "{PROTOCOL}: BodyLength {declared_length} does not locate the CheckSum (10) field"
            )));
        }
        let declared_checksum = parse_usize(as_text(&trailer[3..6])?)
            .ok_or_else(|| Error::schema(format!("{PROTOCOL}: CheckSum (10) is not a number")))?;

        let computed = rest[..checksum_start]
            .iter()
            .fold(0u32, |acc, byte| acc.wrapping_add(u32::from(*byte)))
            % 256;
        if computed as usize != declared_checksum {
            return Err(Error::schema(format!(
                "{PROTOCOL}: checksum mismatch — computed {computed:03}, message declares {declared_checksum:03}"
            )));
        }

        Ok(Some(Frame {
            body: &rest[body_start..checksum_start],
            length: checksum_start + 7,
        }))
    }

    fn decode_message(
        &mut self,
        fields: &FixFields<'_>,
        offset: usize,
        captured_at: Timestamp,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        let msg_type = fields
            .first(TAG_MSG_TYPE)
            .ok_or_else(|| Error::schema(format!("{PROTOCOL}: message has no MsgType (35)")))?;
        let sequence = fields
            .first(TAG_MSG_SEQ_NUM)
            .and_then(parse_u64)
            .ok_or_else(|| {
                Error::schema(format!("{PROTOCOL}: message has no usable MsgSeqNum (34)"))
            })?;
        // SendingTime is not optional in the header, and a message without it
        // cannot be placed in time. Guessing with the capture time would put a
        // fabricated venue timestamp into the bitemporal record, which is the
        // one thing that must never be fabricated.
        let venue_time = fields
            .first(TAG_SENDING_TIME)
            .and_then(parse_sending_time)
            .ok_or_else(|| {
                Error::schema(format!(
                    "{PROTOCOL}: message {sequence} has no usable SendingTime (52)"
                ))
            })?;

        match msg_type {
            "W" => self.decode_snapshot(fields, sequence, venue_time, captured_at, offset, out),
            "X" => self.decode_incremental(fields, sequence, venue_time, captured_at, offset, out),
            "8" => {
                self.decode_execution_report(fields, sequence, venue_time, captured_at, offset, out)
            }
            other => {
                self.skip(
                    SkipReason::UnknownMessageType {
                        code: other.to_string(),
                    },
                    offset,
                    captured_at,
                );
                Ok(())
            }
        }
    }

    fn decode_snapshot(
        &mut self,
        fields: &FixFields<'_>,
        sequence: u64,
        venue_time: Timestamp,
        captured_at: Timestamp,
        offset: usize,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        let symbol = match fields.first(TAG_SYMBOL) {
            Some(symbol) => symbol.to_string(),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: "snapshot has no Symbol (55)".to_string(),
                    },
                    offset,
                    captured_at,
                );
                return Ok(());
            }
        };
        let Some(partition) = self.instruments.resolve(&symbol) else {
            self.skip(
                SkipReason::UnmappedInstrument { symbol },
                offset,
                captured_at,
            );
            return Ok(());
        };
        let origin = self.identity.origin(partition, sequence);

        // A full refresh replaces the book rather than adding to it. Emitting
        // the levels without the reset would leave every level the venue has
        // since removed sitting in the book, permanently, with nothing to
        // remove it — the failure looks like a stale quote hours later.
        let mut ordinal = 0u32;
        out.push(build_message(
            origin.clone(),
            ordinal,
            MessageBody::Reset {
                reason: format!("{PROTOCOL} snapshot (35=W) for {symbol} replaces the book"),
            },
            venue_time,
            captured_at,
        ));
        ordinal += 1;

        for entry in fields.group(TAG_NO_MD_ENTRIES, TAG_MD_ENTRY_TYPE) {
            let Some(body) = self.entry_body(&entry, None, offset, captured_at) else {
                continue;
            };
            out.push(build_message(
                origin.clone(),
                ordinal,
                body,
                venue_time,
                captured_at,
            ));
            ordinal += 1;
        }
        Ok(())
    }

    fn decode_incremental(
        &mut self,
        fields: &FixFields<'_>,
        sequence: u64,
        venue_time: Timestamp,
        captured_at: Timestamp,
        offset: usize,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        // In an incremental refresh the symbol sits inside each entry, because
        // one message may carry updates for several instruments.
        let message_symbol = fields.first(TAG_SYMBOL).map(str::to_string);
        let mut ordinal = 0u32;
        for entry in fields.group(TAG_NO_MD_ENTRIES, TAG_MD_UPDATE_ACTION) {
            let symbol = match entry
                .first(TAG_SYMBOL)
                .map(str::to_string)
                .or_else(|| message_symbol.clone())
            {
                Some(symbol) => symbol,
                None => {
                    self.skip(
                        SkipReason::Malformed {
                            detail: "incremental entry has no Symbol (55)".to_string(),
                        },
                        offset,
                        captured_at,
                    );
                    continue;
                }
            };
            let Some(partition) = self.instruments.resolve(&symbol) else {
                self.skip(
                    SkipReason::UnmappedInstrument { symbol },
                    offset,
                    captured_at,
                );
                continue;
            };
            let action = entry.first(TAG_MD_UPDATE_ACTION).unwrap_or("0");
            let Some(body) = self.entry_body(&entry, Some(action), offset, captured_at) else {
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
        }
        Ok(())
    }

    /// One `NoMDEntries` entry as a message body.
    ///
    /// `action` is `None` for a snapshot, where every entry is a level being
    /// stated rather than changed.
    fn entry_body(
        &mut self,
        entry: &FixFields<'_>,
        action: Option<&str>,
        offset: usize,
        captured_at: Timestamp,
    ) -> Option<MessageBody> {
        let entry_type = entry.first(TAG_MD_ENTRY_TYPE).unwrap_or("");
        let side = match entry_type {
            "0" => Some(BookSide::Bid),
            "1" => Some(BookSide::Ask),
            "2" => None,
            other => {
                self.skip(
                    SkipReason::NoMarketFact {
                        code: format!("MDEntryType={other}"),
                    },
                    offset,
                    captured_at,
                );
                return None;
            }
        };

        let price = entry.first(TAG_MD_ENTRY_PX).and_then(Decimal::parse);
        let size = entry.first(TAG_MD_ENTRY_SIZE).and_then(Decimal::parse);

        match side {
            Some(side) => {
                let price = self.require(price, "MDEntryPx (270)", offset, captured_at)?;
                // A delete states the level is gone. The contract expresses that
                // as a level set to zero, so the size on a delete is ignored
                // rather than trusted — venues send stale or absent sizes there.
                let quantity = if action == Some("2") {
                    Decimal::ZERO
                } else {
                    self.require(size, "MDEntrySize (271)", offset, captured_at)?
                };
                Some(MessageBody::LevelSet {
                    side,
                    price,
                    quantity,
                    order_count: entry.first(TAG_NUMBER_OF_ORDERS).and_then(parse_u32),
                })
            }
            None => {
                let price = self.require(price, "MDEntryPx (270)", offset, captured_at)?;
                let quantity = self.require(size, "MDEntrySize (271)", offset, captured_at)?;
                Some(MessageBody::Trade {
                    price,
                    quantity,
                    condition: TradeCondition::Regular,
                    aggressor: entry.first(TAG_SIDE).and_then(fix_side),
                })
            }
        }
    }

    fn decode_execution_report(
        &mut self,
        fields: &FixFields<'_>,
        sequence: u64,
        venue_time: Timestamp,
        captured_at: Timestamp,
        offset: usize,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        let exec_type = fields.first(TAG_EXEC_TYPE).unwrap_or("");
        // Only the fill types carry a market fact. A new, cancelled or rejected
        // order is an order-management event; it belongs to the OMS, and this
        // crate deliberately has no vocabulary for it.
        if !matches!(exec_type, "F" | "1" | "2") {
            self.skip(
                SkipReason::NoMarketFact {
                    code: format!("ExecType={exec_type}"),
                },
                offset,
                captured_at,
            );
            return Ok(());
        }
        let symbol = match fields.first(TAG_SYMBOL) {
            Some(symbol) => symbol.to_string(),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: "execution report has no Symbol (55)".to_string(),
                    },
                    offset,
                    captured_at,
                );
                return Ok(());
            }
        };
        let Some(partition) = self.instruments.resolve(&symbol) else {
            self.skip(
                SkipReason::UnmappedInstrument { symbol },
                offset,
                captured_at,
            );
            return Ok(());
        };

        let price = fields.first(TAG_LAST_PX).and_then(Decimal::parse);
        let quantity = fields.first(TAG_LAST_QTY).and_then(Decimal::parse);
        let (Some(price), Some(quantity)) = (price, quantity) else {
            self.skip(
                SkipReason::Malformed {
                    detail: "fill without LastPx (31) or LastQty (32)".to_string(),
                },
                offset,
                captured_at,
            );
            return Ok(());
        };

        out.push(build_message(
            self.identity.origin(partition, sequence),
            0,
            MessageBody::Trade {
                price,
                quantity,
                condition: TradeCondition::Regular,
                aggressor: fields.first(TAG_SIDE).and_then(fix_side),
            },
            venue_time,
            captured_at,
        ));
        Ok(())
    }

    fn require(
        &mut self,
        value: Option<Decimal>,
        field: &str,
        offset: usize,
        captured_at: Timestamp,
    ) -> Option<Decimal> {
        match value {
            Some(value) => Some(value),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: format!("missing or unparsable {field}"),
                    },
                    offset,
                    captured_at,
                );
                None
            }
        }
    }

    fn skip(&mut self, reason: SkipReason, offset: usize, at: Timestamp) {
        self.diagnostics.record_skip(SkipRecord {
            protocol: PROTOCOL.to_string(),
            reason,
            offset,
            at,
        });
    }
}

impl Decoder for FixDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        self.consumed = 0;
        let mut out = Vec::new();
        let mut position = 0usize;
        while let Some(frame) = self.frame(bytes, position)? {
            let fields = FixFields::parse(frame.body)?;
            let before = out.len();
            self.decode_message(&fields, position, captured_at, &mut out)?;
            self.diagnostics.messages_decoded += (out.len() - before) as u64;
            position += frame.length;
            self.consumed = position;
            self.diagnostics.bytes_consumed += frame.length as u64;
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

#[derive(Debug)]
struct Frame<'a> {
    body: &'a [u8],
    /// Total bytes of the framed message, header and trailer included.
    length: usize,
}

/// The tag/value pairs of one message or one repeating-group entry, in wire
/// order.
///
/// Order is preserved rather than collapsed into a map because a repeating group
/// is defined entirely by position: the same tag appears once per entry, and a
/// map would keep exactly one of them.
#[derive(Debug, Clone)]
pub struct FixFields<'a> {
    entries: Vec<(u32, &'a str)>,
}

impl<'a> FixFields<'a> {
    /// Split a body into tag/value pairs.
    pub fn parse(body: &'a [u8]) -> Result<Self> {
        let mut entries = Vec::new();
        for chunk in body.split(|byte| *byte == SOH) {
            if chunk.is_empty() {
                continue;
            }
            let text = as_text(chunk)?;
            let (tag, value) = text
                .split_once('=')
                .ok_or_else(|| Error::schema(format!("{PROTOCOL}: field `{text}` has no `=`")))?;
            let tag = parse_u32(tag)
                .ok_or_else(|| Error::schema(format!("{PROTOCOL}: `{tag}` is not a tag number")))?;
            entries.push((tag, value));
        }
        Ok(Self { entries })
    }

    /// The first value for `tag`.
    pub fn first(&self, tag: u32) -> Option<&'a str> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == tag)
            .map(|(_, value)| *value)
    }

    /// Split the repeating group counted by `count_tag` into entries.
    ///
    /// FIX marks an entry boundary by the reappearance of the group's first
    /// tag, so the caller has to say which tag that is — it differs per group
    /// and getting it wrong silently merges entries.
    pub fn group(&self, count_tag: u32, delimiter_tag: u32) -> Vec<FixFields<'a>> {
        let Some(start) = self.entries.iter().position(|(tag, _)| *tag == count_tag) else {
            return Vec::new();
        };
        let declared = self
            .entries
            .get(start)
            .and_then(|(_, value)| parse_usize(value))
            .unwrap_or(0);

        let mut groups: Vec<FixFields<'a>> = Vec::new();
        for &(tag, value) in self.entries.iter().skip(start + 1) {
            if tag == delimiter_tag {
                if groups.len() == declared {
                    break;
                }
                groups.push(FixFields {
                    entries: Vec::new(),
                });
            }
            match groups.last_mut() {
                Some(current) => current.entries.push((tag, value)),
                // Fields before the first delimiter belong to the message, not
                // to the group; a trailing header field after the count is not
                // an entry.
                None => continue,
            }
        }
        groups
    }
}

fn find(haystack: &[u8], needle: u8, from: usize) -> Option<usize> {
    haystack
        .iter()
        .skip(from)
        .position(|byte| *byte == needle)
        .map(|index| index + from)
}

fn as_text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes)
        .map_err(|_| Error::schema(format!("{PROTOCOL}: field is not valid UTF-8")))
}

fn parse_usize(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_u64(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn fix_side(value: &str) -> Option<BookSide> {
    match value {
        "1" => Some(BookSide::Bid),
        "2" | "5" => Some(BookSide::Ask),
        _ => None,
    }
}

/// Parse `YYYYMMDD-HH:MM:SS[.fff[fff[fff]]]` in UTC.
///
/// FIX times are always UTC by specification, so there is no zone to interpret
/// and no place for a local-time assumption to hide.
pub fn parse_sending_time(text: &str) -> Option<Timestamp> {
    let (date, time) = text.split_once('-')?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = date.get(0..4)?.parse().ok()?;
    let month: u32 = date.get(4..6)?.parse().ok()?;
    let day: u32 = date.get(6..8)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut parts = time.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds_field = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (seconds_text, fraction_text) = match seconds_field.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (seconds_field, ""),
    };
    let seconds: i64 = seconds_text.parse().ok()?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..=60).contains(&seconds) {
        return None;
    }
    if !fraction_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut fraction_nanos: i64 = 0;
    for index in 0..9 {
        let digit = fraction_text
            .as_bytes()
            .get(index)
            .map_or(0, |byte| i64::from(byte - b'0'));
        fraction_nanos = fraction_nanos * 10 + digit;
    }

    let days = days_from_civil(year, month, day);
    let nanos = days.checked_mul(qip_core::time::NANOS_PER_DAY)?
        + hours * qip_core::time::NANOS_PER_HOUR
        + minutes * qip_core::time::NANOS_PER_MIN
        + seconds * qip_core::time::NANOS_PER_SEC
        + fraction_nanos;
    Some(Timestamp::from_nanos(nanos))
}

/// Append a well-formed BodyLength and CheckSum to a body.
///
/// Lives beside the decoder rather than in the tests so that the encoder and the
/// checker cannot drift apart: if the framing rule changes, both sides change
/// together and the tests keep exercising the real rule.
pub fn frame_message(begin_string: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("8={begin_string}\x01").as_bytes());
    out.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    out.extend_from_slice(body);
    let checksum = out
        .iter()
        .fold(0u32, |acc, byte| acc.wrapping_add(u32::from(*byte)))
        % 256;
    out.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    out
}
