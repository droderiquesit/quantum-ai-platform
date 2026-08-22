//! Nasdaq TotalView-ITCH 5.0.
//!
//! Big-endian, fixed width, no length prefix and no delimiter: the type byte
//! implies the length, so a type this decoder does not recognise cannot be
//! stepped over and takes the stream with it. Every ITCH 5.0 type is therefore
//! in [`message_length`] whether or not it is mapped — knowing how long a
//! message is and knowing what it means are separate problems, and conflating
//! them means one unmapped type desynchronises the session.
//!
//! Two mappings deserve explanation.
//!
//! **Executions carry a remaining quantity, not a delta.** ITCH publishes the
//! executed shares; [`MessageBody::OrderReduced`] states what is left. The
//! decoder therefore tracks the resting size of every order it has seen added.
//! The alternative — leaking deltas downstream — would make every order book
//! implementation reconstruct the same table, and they would disagree.
//!
//! **A replace is a removal and an add, not a replace.** Nasdaq's `U` assigns a
//! new order reference and sends the order to the back of the queue. Modelling
//! it as an identity-preserving amendment would leave a book holding a queue
//! position the order no longer has, which is precisely the thing a queue model
//! is consulted about.

use crate::bytes::{ByteOrder, Reader, since_midnight};
use crate::decoder::{
    Decoder, Diagnostics, FeedIdentity, InstrumentPartitions, SkipReason, SkipRecord, build_message,
};
use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use std::collections::BTreeMap;

const PROTOCOL: &str = "itch.5.0";

/// Prices are unsigned 32-bit integers with four implied decimals.
const PRICE_EXPONENT: u32 = 4;
const PRICE_WIDTH: usize = 4;

/// The wire length of an ITCH 5.0 message, including the type byte.
///
/// `None` means the type is not part of ITCH 5.0 at all, which is unrecoverable:
/// without a length there is no next message boundary to find.
pub const fn message_length(message_type: u8) -> Option<usize> {
    Some(match message_type {
        b'S' => 12, // system event
        b'R' => 39, // stock directory
        b'H' => 25, // stock trading action
        b'Y' => 20, // reg SHO restriction
        b'L' => 26, // market participant position
        b'V' => 35, // MWCB decline level
        b'W' => 12, // MWCB status
        b'K' => 28, // IPO quoting period update
        b'J' => 35, // LULD auction collar
        b'h' => 21, // operational halt
        b'A' => 36, // add order, no attribution
        b'F' => 40, // add order with attribution
        b'E' => 31, // order executed
        b'C' => 36, // order executed with price
        b'X' => 23, // order cancel
        b'D' => 19, // order delete
        b'U' => 35, // order replace
        b'P' => 44, // trade, non-cross
        b'Q' => 40, // cross trade
        b'B' => 19, // broken trade
        b'I' => 50, // net order imbalance indicator
        b'N' => 20, // retail price improvement indicator
        _ => return None,
    })
}

/// What the decoder remembers about a resting order.
///
/// Only what a later message cannot supply: `E`, `C`, `X` and `D` name an order
/// reference and nothing else, so the instrument, the side, the display price
/// and the remaining size all have to come from the add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RestingOrder {
    partition: u32,
    side: BookSide,
    shares: u64,
    price: Decimal,
}

/// Decodes Nasdaq TotalView-ITCH 5.0.
#[derive(Debug)]
pub struct ItchDecoder {
    identity: FeedIdentity,
    instruments: InstrumentPartitions,
    /// Midnight of the session being decoded, in UTC.
    session_midnight: Timestamp,
    /// Resting orders, so an execution can be expressed as a remaining size.
    open_orders: BTreeMap<u64, RestingOrder>,
    /// The sequence stamped on the next wire message.
    ///
    /// ITCH payloads carry no sequence number — MoldUDP64 supplies it — so the
    /// decoder counts wire messages and the transport aligns it with
    /// [`ItchDecoder::set_sequence`] at session start and after a retransmission.
    /// Without that call a downstream gap detector cannot see a packet loss,
    /// because the count simply never skips.
    next_sequence: u64,
    diagnostics: Diagnostics,
    consumed: usize,
}

impl ItchDecoder {
    pub fn new(
        venue: VenueId,
        feed: impl Into<String>,
        instruments: InstrumentPartitions,
        session_midnight: Timestamp,
    ) -> Self {
        Self {
            identity: FeedIdentity::new(venue, feed),
            instruments,
            session_midnight,
            open_orders: BTreeMap::new(),
            next_sequence: 1,
            diagnostics: Diagnostics::default(),
            consumed: 0,
        }
    }

    /// Align the decoder with the transport's sequence number.
    pub fn set_sequence(&mut self, sequence: u64) {
        self.next_sequence = sequence;
    }

    /// How many resting orders are being tracked.
    ///
    /// Exposed so an operator can see the table's size: it grows with every add
    /// and shrinks on delete or full execution, and a venue that never deletes
    /// an order would grow it without bound. In practice the table is emptied
    /// when the session ends and the decoder is rebuilt.
    pub fn open_order_count(&self) -> usize {
        self.open_orders.len()
    }

    fn skip(&mut self, reason: SkipReason, offset: usize, at: Timestamp) {
        self.diagnostics.record_skip(SkipRecord {
            protocol: PROTOCOL.to_string(),
            reason,
            offset,
            at,
        });
    }

    fn decode_one(
        &mut self,
        frame: &[u8],
        offset: usize,
        captured_at: Timestamp,
        out: &mut Vec<MarketMessage>,
    ) -> Result<()> {
        let reader = Reader::new(frame, ByteOrder::Big);
        let message_type = reader.u8_at(0)?;
        let venue_time = since_midnight(self.session_midnight, reader.uint(5, 6)?)?;
        let sequence = self.next_sequence;

        match message_type {
            b'A' | b'F' => {
                let order_ref = reader.uint(11, 8)?;
                let side = itch_side(reader.u8_at(19)?)?;
                let shares = reader.uint(20, 4)?;
                let symbol = reader.ascii(24, 8)?;
                let price = reader.fixed(32, PRICE_WIDTH, false, PRICE_EXPONENT)?;
                let Some(partition) = self.instruments.resolve(&symbol) else {
                    self.skip(
                        SkipReason::UnmappedInstrument { symbol },
                        offset,
                        captured_at,
                    );
                    return Ok(());
                };
                self.open_orders.insert(
                    order_ref,
                    RestingOrder {
                        partition,
                        side,
                        shares,
                        price,
                    },
                );
                out.push(build_message(
                    self.identity.origin(partition, sequence),
                    0,
                    MessageBody::OrderAdded {
                        order_ref,
                        side,
                        price,
                        quantity: shares_to_decimal(shares),
                    },
                    venue_time,
                    captured_at,
                ));
            }
            b'E' | b'C' => {
                let order_ref = reader.uint(11, 8)?;
                let executed = reader.uint(19, 4)?;
                // `C` restates the price because the execution happened away
                // from the order's display price; `E` executed at it, and the
                // resting price is the book's, not this message's.
                let (price_override, printable) = if message_type == b'C' {
                    (
                        Some(reader.fixed(32, PRICE_WIDTH, false, PRICE_EXPONENT)?),
                        reader.u8_at(31)? == b'Y',
                    )
                } else {
                    (None, true)
                };
                self.execute(
                    order_ref,
                    executed,
                    price_override,
                    printable,
                    sequence,
                    venue_time,
                    captured_at,
                    offset,
                    out,
                );
            }
            b'X' => {
                let order_ref = reader.uint(11, 8)?;
                let cancelled = reader.uint(19, 4)?;
                self.reduce(
                    order_ref,
                    cancelled,
                    sequence,
                    venue_time,
                    captured_at,
                    offset,
                    out,
                );
            }
            b'D' => {
                let order_ref = reader.uint(11, 8)?;
                let Some(order) = self.open_orders.remove(&order_ref) else {
                    self.skip(
                        SkipReason::UnknownOrderReference { order_ref },
                        offset,
                        captured_at,
                    );
                    return Ok(());
                };
                out.push(build_message(
                    self.identity.origin(order.partition, sequence),
                    0,
                    MessageBody::OrderRemoved { order_ref },
                    venue_time,
                    captured_at,
                ));
            }
            b'U' => {
                let original_ref = reader.uint(11, 8)?;
                let new_ref = reader.uint(19, 8)?;
                let shares = reader.uint(27, 4)?;
                let price = reader.fixed(31, PRICE_WIDTH, false, PRICE_EXPONENT)?;
                let Some(order) = self.open_orders.remove(&original_ref) else {
                    self.skip(
                        SkipReason::UnknownOrderReference {
                            order_ref: original_ref,
                        },
                        offset,
                        captured_at,
                    );
                    return Ok(());
                };
                self.open_orders.insert(
                    new_ref,
                    RestingOrder {
                        partition: order.partition,
                        side: order.side,
                        shares,
                        price,
                    },
                );
                let origin = self.identity.origin(order.partition, sequence);
                out.push(build_message(
                    origin.clone(),
                    0,
                    MessageBody::OrderRemoved {
                        order_ref: original_ref,
                    },
                    venue_time,
                    captured_at,
                ));
                out.push(build_message(
                    origin,
                    1,
                    MessageBody::OrderAdded {
                        order_ref: new_ref,
                        side: order.side,
                        price,
                        quantity: shares_to_decimal(shares),
                    },
                    venue_time,
                    captured_at,
                ));
            }
            b'P' => {
                // A trade against a non-displayed order: it prints, but there is
                // no resting quantity to reduce because none was ever displayed.
                let side = itch_side(reader.u8_at(19)?)?;
                let shares = reader.uint(20, 4)?;
                let symbol = reader.ascii(24, 8)?;
                let price = reader.fixed(32, PRICE_WIDTH, false, PRICE_EXPONENT)?;
                let Some(partition) = self.instruments.resolve(&symbol) else {
                    self.skip(
                        SkipReason::UnmappedInstrument { symbol },
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
                        quantity: shares_to_decimal(shares),
                        condition: TradeCondition::Regular,
                        // The side on the wire is the resting non-displayed
                        // order's, so the aggressor is the other one.
                        aggressor: Some(side.opposite()),
                    },
                    venue_time,
                    captured_at,
                ));
            }
            b'Q' => {
                let shares = reader.uint(11, 8)?;
                let symbol = reader.ascii(19, 8)?;
                let price = reader.fixed(27, PRICE_WIDTH, false, PRICE_EXPONENT)?;
                let Some(partition) = self.instruments.resolve(&symbol) else {
                    self.skip(
                        SkipReason::UnmappedInstrument { symbol },
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
                        quantity: shares_to_decimal(shares),
                        // A cross prints at a single auction price with no
                        // aggressor; treating it as a regular trade would let it
                        // drag a mid-price that no continuous order ever paid.
                        condition: TradeCondition::Auction,
                        aggressor: None,
                    },
                    venue_time,
                    captured_at,
                ));
            }
            b'H' => {
                let symbol = reader.ascii(11, 8)?;
                let state = reader.u8_at(19)?;
                let Some(partition) = self.instruments.resolve(&symbol) else {
                    self.skip(
                        SkipReason::UnmappedInstrument { symbol },
                        offset,
                        captured_at,
                    );
                    return Ok(());
                };
                let status = match state {
                    b'T' => VenueStatus::Open,
                    // Quotation-only: orders are accepted and quotes published,
                    // but nothing trades. `Auction` is the state that describes
                    // that; `Halted` would wrongly stop the router quoting.
                    b'Q' => VenueStatus::Auction,
                    b'H' | b'P' => VenueStatus::Halted,
                    other => {
                        self.skip(
                            SkipReason::Malformed {
                                detail: format!("unknown trading state `{}`", char::from(other)),
                            },
                            offset,
                            captured_at,
                        );
                        return Ok(());
                    }
                };
                out.push(build_message(
                    self.identity.origin(partition, sequence),
                    0,
                    MessageBody::StatusChange { status },
                    venue_time,
                    captured_at,
                ));
            }
            other => {
                self.skip(
                    SkipReason::NoMarketFact {
                        code: char::from(other).to_string(),
                    },
                    offset,
                    captured_at,
                );
            }
        }
        Ok(())
    }

    /// Apply an execution: the resting order shrinks, and unless the venue
    /// marked the print non-displayable, a trade prints too.
    fn execute(
        &mut self,
        order_ref: u64,
        executed: u64,
        price_override: Option<Decimal>,
        printable: bool,
        sequence: u64,
        venue_time: Timestamp,
        captured_at: Timestamp,
        offset: usize,
        out: &mut Vec<MarketMessage>,
    ) {
        let Some(order) = self.open_orders.get_mut(&order_ref) else {
            self.skip(
                SkipReason::UnknownOrderReference { order_ref },
                offset,
                captured_at,
            );
            return;
        };
        let side = order.side;
        let partition = order.partition;
        // `E` prints at the order's display price, which only the add carried.
        let price = price_override.unwrap_or(order.price);
        let remaining = order.shares.saturating_sub(executed);
        order.shares = remaining;
        if remaining == 0 {
            self.open_orders.remove(&order_ref);
        }

        let origin = self.identity.origin(partition, sequence);
        // The reduction is emitted before the print. A consumer that sees the
        // print first would compute a spread against size that has already been
        // taken, and the two orderings are indistinguishable afterwards.
        out.push(build_message(
            origin.clone(),
            0,
            if remaining == 0 {
                MessageBody::OrderRemoved { order_ref }
            } else {
                MessageBody::OrderReduced {
                    order_ref,
                    remaining: shares_to_decimal(remaining),
                }
            },
            venue_time,
            captured_at,
        ));

        // A `C` marked non-printable improved on the display price without
        // printing; the book still shrinks, but the trade never happened as far
        // as the last sale is concerned, and publishing it would drag the mark.
        if printable {
            out.push(build_message(
                origin,
                1,
                MessageBody::Trade {
                    price,
                    quantity: shares_to_decimal(executed),
                    condition: TradeCondition::Regular,
                    aggressor: Some(side.opposite()),
                },
                venue_time,
                captured_at,
            ));
        }
    }

    fn reduce(
        &mut self,
        order_ref: u64,
        cancelled: u64,
        sequence: u64,
        venue_time: Timestamp,
        captured_at: Timestamp,
        offset: usize,
        out: &mut Vec<MarketMessage>,
    ) {
        let Some(order) = self.open_orders.get_mut(&order_ref) else {
            self.skip(
                SkipReason::UnknownOrderReference { order_ref },
                offset,
                captured_at,
            );
            return;
        };
        let partition = order.partition;
        let remaining = order.shares.saturating_sub(cancelled);
        order.shares = remaining;
        if remaining == 0 {
            self.open_orders.remove(&order_ref);
        }
        out.push(build_message(
            self.identity.origin(partition, sequence),
            0,
            if remaining == 0 {
                MessageBody::OrderRemoved { order_ref }
            } else {
                MessageBody::OrderReduced {
                    order_ref,
                    remaining: shares_to_decimal(remaining),
                }
            },
            venue_time,
            captured_at,
        ));
    }
}

impl Decoder for ItchDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        self.consumed = 0;
        let mut out = Vec::new();
        let mut position = 0usize;
        loop {
            let Some(&message_type) = bytes.get(position) else {
                break;
            };
            let Some(length) = message_length(message_type) else {
                return Err(Error::schema(format!(
                    "{PROTOCOL}: message type `{}` at offset {position} is not ITCH 5.0, so the next message boundary is unknowable",
                    char::from(message_type)
                )));
            };
            let Some(frame) = bytes.get(position..position + length) else {
                // The message has not finished arriving. Consuming nothing of it
                // is what lets the caller re-present it whole.
                break;
            };
            let before = out.len();
            self.decode_one(frame, position, captured_at, &mut out)?;
            self.diagnostics.messages_decoded += (out.len() - before) as u64;
            self.next_sequence += 1;
            position += length;
            self.consumed = position;
            self.diagnostics.bytes_consumed += length as u64;
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

fn itch_side(byte: u8) -> Result<BookSide> {
    match byte {
        b'B' => Ok(BookSide::Bid),
        b'S' => Ok(BookSide::Ask),
        other => Err(Error::schema(format!(
            "{PROTOCOL}: `{}` is not a buy/sell indicator",
            char::from(other)
        ))),
    }
}

/// Share counts are whole numbers, so the conversion is exact.
fn shares_to_decimal(shares: u64) -> Decimal {
    Decimal::from_raw(i128::from(shares) * qip_core::decimal::SCALE)
}
