//! Framed transports: length-prefixed and newline-delimited JSON.
//!
//! This is the shape a crypto exchange's WebSocket feed reduces to once the
//! socket layer is stripped away — a sequence of JSON documents, delimited
//! either by a length prefix or by a newline, arriving in whatever chunks the
//! network chose. The chunk boundaries have nothing to do with the message
//! boundaries, and a frame will be split across two reads roughly as often as
//! the frame size divides the MTU.
//!
//! Two decisions follow from that.
//!
//! **Reassembly is written once.** [`StreamAssembler`] owns the carry-over
//! buffer and drives any [`Decoder`], so no individual decoder implements the
//! split-frame case, and the one implementation is tested against every possible
//! split offset rather than the two or three a hand-written test would pick.
//!
//! **A malformed frame is skipped, not refused.** Unlike FIX, the frame
//! boundaries here are known independently of the content: a frame that is not
//! valid JSON costs exactly that frame, and the stream stays synchronised. So
//! the batch survives, with a record of what was dropped.

use crate::bytes::ByteOrder;
use crate::decoder::{
    Decoder, Diagnostics, FeedIdentity, InstrumentPartitions, SkipReason, SkipRecord,
    build_message, check_frame_bound,
};
use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

const PROTOCOL: &str = "json-framed";

/// How frames are separated on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Framing {
    /// A binary length prefix of `width` bytes.
    LengthPrefixed {
        width: usize,
        order: ByteOrder,
        /// Whether the declared length counts the prefix itself. Venues differ,
        /// and being wrong shifts every frame by `width` bytes — which decodes
        /// as garbage rather than failing, so it has to be configuration.
        includes_prefix: bool,
    },
    /// Frames end at a newline. A trailing carriage return is tolerated.
    NewlineDelimited,
}

impl Framing {
    /// Find the first whole frame in `bytes`.
    ///
    /// Returns the frame's payload bounds and its total width including the
    /// delimiter. `Ok(None)` means the frame has not finished arriving.
    pub fn next_frame(&self, bytes: &[u8]) -> Result<Option<(usize, usize, usize)>> {
        match *self {
            Self::LengthPrefixed {
                width,
                order,
                includes_prefix,
            } => {
                if width == 0 || width > 8 {
                    return Err(Error::invalid(format!(
                        "{PROTOCOL}: length prefix width {width} is not between 1 and 8"
                    )));
                }
                let Some(prefix) = bytes.get(..width) else {
                    return Ok(None);
                };
                let reader = crate::bytes::Reader::new(prefix, order);
                let declared = reader.uint(0, width)? as usize;
                let payload_length = if includes_prefix {
                    declared.checked_sub(width).ok_or_else(|| {
                        Error::schema(format!(
                            "{PROTOCOL}: declared frame length {declared} is shorter than its own {width}-byte prefix"
                        ))
                    })?
                } else {
                    declared
                };
                check_frame_bound(PROTOCOL, payload_length)?;
                let end = width + payload_length;
                if bytes.len() < end {
                    return Ok(None);
                }
                Ok(Some((width, end, end)))
            }
            Self::NewlineDelimited => {
                let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') else {
                    check_frame_bound(PROTOCOL, bytes.len())?;
                    return Ok(None);
                };
                let mut end = newline;
                if end > 0 && bytes.get(end - 1) == Some(&b'\r') {
                    end -= 1;
                }
                Ok(Some((0, end, newline + 1)))
            }
        }
    }
}

/// One JSON frame as venues on this shape publish it.
///
/// Deliberately permissive about which fields are present and strict about what
/// they mean: every venue names these differently, but they all send the same
/// facts, and the renaming belongs in configuration rather than in a second
/// decoder.
#[derive(Clone, Debug, Deserialize)]
struct WireFrame {
    /// The venue's sequence number for this stream.
    #[serde(alias = "sequence", alias = "u")]
    seq: u64,
    /// Venue timestamp in milliseconds since the Unix epoch.
    #[serde(alias = "timestamp", alias = "E")]
    ts: i64,
    #[serde(alias = "s", alias = "instrument")]
    symbol: String,
    #[serde(rename = "type", alias = "e")]
    kind: String,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default, alias = "size", alias = "quantity")]
    qty: Option<Decimal>,
    #[serde(default)]
    order_id: Option<u64>,
    #[serde(default)]
    orders: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    bid: Option<(Decimal, Decimal)>,
    #[serde(default)]
    ask: Option<(Decimal, Decimal)>,
    #[serde(default)]
    reason: Option<String>,
}

/// Decodes framed JSON into market messages.
#[derive(Debug)]
pub struct JsonFrameDecoder {
    identity: FeedIdentity,
    instruments: InstrumentPartitions,
    framing: Framing,
    diagnostics: Diagnostics,
    consumed: usize,
}

impl JsonFrameDecoder {
    /// A decoder for one venue's framed JSON feed.
    pub fn new(
        venue: VenueId,
        feed: impl Into<String>,
        instruments: InstrumentPartitions,
        framing: Framing,
    ) -> Self {
        Self {
            identity: FeedIdentity::new(venue, feed),
            instruments,
            framing,
            diagnostics: Diagnostics::default(),
            consumed: 0,
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

    fn body_of(&mut self, frame: &WireFrame, offset: usize, at: Timestamp) -> Option<MessageBody> {
        let side = frame.side.as_deref().and_then(parse_side);
        match frame.kind.as_str() {
            "level" => Some(MessageBody::LevelSet {
                side: self.required_side(side, offset, at)?,
                price: self.required(frame.price, "price", offset, at)?,
                quantity: frame.qty.unwrap_or(Decimal::ZERO),
                order_count: frame.orders,
            }),
            "quote" => Some(MessageBody::Quote {
                bid: frame.bid,
                ask: frame.ask,
            }),
            "trade" => Some(MessageBody::Trade {
                price: self.required(frame.price, "price", offset, at)?,
                quantity: self.required(frame.qty, "qty", offset, at)?,
                condition: TradeCondition::Regular,
                // The taker's side is what venues on this shape publish, and it
                // is already the aggressor — no inversion, unlike ITCH, where
                // the side named is the resting order's.
                aggressor: side,
            }),
            "add" => Some(MessageBody::OrderAdded {
                order_ref: self.required_order(frame.order_id, offset, at)?,
                side: self.required_side(side, offset, at)?,
                price: self.required(frame.price, "price", offset, at)?,
                quantity: self.required(frame.qty, "qty", offset, at)?,
            }),
            // An in-place amendment: this venue keeps the order's identity and,
            // by its rules, its queue position — so unlike a Nasdaq replace this
            // really is the identity-preserving change the contract describes.
            "amend" => Some(MessageBody::OrderReplaced {
                order_ref: self.required_order(frame.order_id, offset, at)?,
                price: self.required(frame.price, "price", offset, at)?,
                quantity: self.required(frame.qty, "qty", offset, at)?,
            }),
            "reduce" => Some(MessageBody::OrderReduced {
                order_ref: self.required_order(frame.order_id, offset, at)?,
                remaining: self.required(frame.qty, "qty", offset, at)?,
            }),
            "remove" => Some(MessageBody::OrderRemoved {
                order_ref: self.required_order(frame.order_id, offset, at)?,
            }),
            "status" => {
                let status = match frame.status.as_deref() {
                    Some("open") | Some("trading") => VenueStatus::Open,
                    Some("auction") => VenueStatus::Auction,
                    Some("halted") | Some("paused") => VenueStatus::Halted,
                    Some("closed") => VenueStatus::Closed,
                    other => {
                        self.skip(
                            SkipReason::Malformed {
                                detail: format!("unmapped status {other:?}"),
                            },
                            offset,
                            at,
                        );
                        return None;
                    }
                };
                Some(MessageBody::StatusChange { status })
            }
            "snapshot" | "reset" => Some(MessageBody::Reset {
                reason: frame
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("{PROTOCOL} snapshot replaces the book")),
            }),
            other => {
                self.skip(
                    SkipReason::UnknownMessageType {
                        code: other.to_string(),
                    },
                    offset,
                    at,
                );
                None
            }
        }
    }

    fn required(
        &mut self,
        value: Option<Decimal>,
        field: &str,
        offset: usize,
        at: Timestamp,
    ) -> Option<Decimal> {
        match value {
            Some(value) => Some(value),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: format!("frame has no `{field}`"),
                    },
                    offset,
                    at,
                );
                None
            }
        }
    }

    fn required_side(
        &mut self,
        side: Option<BookSide>,
        offset: usize,
        at: Timestamp,
    ) -> Option<BookSide> {
        match side {
            Some(side) => Some(side),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: "frame has no usable `side`".to_string(),
                    },
                    offset,
                    at,
                );
                None
            }
        }
    }

    fn required_order(&mut self, order: Option<u64>, offset: usize, at: Timestamp) -> Option<u64> {
        match order {
            Some(order) => Some(order),
            None => {
                self.skip(
                    SkipReason::Malformed {
                        detail: "frame has no `order_id`".to_string(),
                    },
                    offset,
                    at,
                );
                None
            }
        }
    }
}

impl Decoder for JsonFrameDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        self.consumed = 0;
        let mut out = Vec::new();
        let mut position = 0usize;

        while let Some(rest) = bytes.get(position..) {
            if rest.is_empty() {
                break;
            }
            let Some((payload_start, payload_end, width)) = self.framing.next_frame(rest)? else {
                break;
            };
            let payload = rest.get(payload_start..payload_end).unwrap_or_default();
            // An empty frame is a keep-alive on most of these feeds.
            if !payload.iter().all(u8::is_ascii_whitespace) {
                match serde_json::from_slice::<WireFrame>(payload) {
                    Ok(frame) => self.decode_frame(&frame, position, captured_at, &mut out),
                    Err(error) => self.skip(
                        SkipReason::Malformed {
                            detail: format!("frame is not a known JSON message: {error}"),
                        },
                        position,
                        captured_at,
                    ),
                }
            }
            position += width;
            self.consumed = position;
            self.diagnostics.bytes_consumed += width as u64;
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

impl JsonFrameDecoder {
    fn decode_frame(
        &mut self,
        frame: &WireFrame,
        offset: usize,
        captured_at: Timestamp,
        out: &mut Vec<MarketMessage>,
    ) {
        let Some(partition) = self.instruments.resolve(&frame.symbol) else {
            self.skip(
                SkipReason::UnmappedInstrument {
                    symbol: frame.symbol.clone(),
                },
                offset,
                captured_at,
            );
            return;
        };
        let Some(body) = self.body_of(frame, offset, captured_at) else {
            return;
        };
        out.push(build_message(
            self.identity.origin(partition, frame.seq),
            0,
            body,
            Timestamp::from_millis(frame.ts),
            captured_at,
        ));
        self.diagnostics.messages_decoded += 1;
    }
}

/// A side written the way venues on this shape write it.
fn parse_side(text: &str) -> Option<BookSide> {
    match text {
        "buy" | "bid" | "b" | "B" => Some(BookSide::Bid),
        "sell" | "ask" | "offer" | "s" | "S" | "a" => Some(BookSide::Ask),
        _ => None,
    }
}

/// Holds the bytes a decoder could not yet use, and re-presents them.
///
/// The invariant this exists to keep: a decoder never sees the tail of a message
/// without its head. Everything a decoder declines to consume is carried into
/// the next call, in front of the new bytes, so a message split across any
/// number of reads is decoded exactly once and identically to the same message
/// delivered whole.
#[derive(Debug)]
pub struct StreamAssembler {
    pending: Vec<u8>,
    /// The point at which a buffer that never yields a frame is called a fault
    /// rather than a slow sender.
    ///
    /// Without it, a stream that has lost synchronisation grows the buffer at
    /// line rate until the process dies — the same unbounded-buffer failure the
    /// sequencing layer guards against downstream.
    max_pending: usize,
}

impl Default for StreamAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamAssembler {
    /// An assembler holding nothing, bounded at [`crate::decoder::MAX_FRAME_BYTES`].
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            max_pending: crate::decoder::MAX_FRAME_BYTES,
        }
    }

    /// Refuse a carry-over larger than `max_pending`.
    pub fn with_capacity_limit(mut self, max_pending: usize) -> Self {
        self.max_pending = max_pending;
        self
    }

    /// Bytes held over from previous reads.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Feed `bytes` to `decoder`, carrying any partial trailing message.
    pub fn push(
        &mut self,
        decoder: &mut dyn Decoder,
        bytes: &[u8],
        captured_at: Timestamp,
    ) -> Result<Vec<MarketMessage>> {
        // The common case is an empty carry, where the input is decoded in place
        // and nothing is copied.
        let messages = if self.pending.is_empty() {
            let decoded = decoder.decode(bytes, captured_at)?;
            let consumed = decoder.consumed().min(bytes.len());
            self.pending
                .extend_from_slice(bytes.get(consumed..).unwrap_or_default());
            decoded
        } else {
            self.pending.extend_from_slice(bytes);
            let decoded = decoder.decode(&self.pending, captured_at)?;
            let consumed = decoder.consumed().min(self.pending.len());
            self.pending.drain(..consumed);
            decoded
        };

        if self.pending.len() > self.max_pending {
            let held = self.pending.len();
            self.pending.clear();
            return Err(Error::schema(format!(
                "{}: {held} bytes accumulated without completing a message, which is a lost frame boundary rather than a slow sender",
                decoder.protocol()
            )));
        }
        Ok(messages)
    }

    /// Discard the carry-over, after a resynchronisation.
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}
