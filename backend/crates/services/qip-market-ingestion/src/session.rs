//! Typed canonical records for a venue's session facts, and the one
//! normaliser from a decoded [`MarketMessage`] to a canonical record
//! (R12-TICK-2053).
//!
//! Every venue protocol decodes into [`MessageBody`], which already names a
//! trade, a quote, five kinds of depth update, a status change, an auction
//! update and a reset. What did not exist until this module was the other
//! half: the *canonical record* each kind becomes on the central side. A
//! trade, a quote and a book had one ([`SensedRecord::Trade`],
//! [`SensedRecord::Quote`], [`SensedRecord::Book`]); a status change, an
//! auction's imbalance and a reset had none, so the depth adapter applied
//! them to its book state and they went no further. A desk could not learn
//! that a venue had halted except by the absence of a book.
//!
//! [`normalise`] is exhaustive over [`MessageBody`] with no wildcard arm, so
//! a kind added to the wire contract is a compile error here until somebody
//! says which typed record it becomes — the property the requirement's
//! "fails if any kind lands in a generic bucket" asks for, held by the
//! compiler rather than by a test that lists today's kinds.

use crate::adapter::SensedRecord;
use qip_contracts::message::{
    BookSide, MarketMessage, MessageBody, TradeCondition as WireCondition,
};
use qip_contracts::venue::VenueStatus;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use qip_financial::quality::DataQuality;
use qip_market::book::Side;
use qip_market::quote::{Quote, Trade, TradeCondition, check_capture_order};
use serde::{Deserialize, Serialize};

/// The venue changed trading state for one instrument.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueStatusUpdate {
    pub object_id: ObjectId,
    pub venue: String,
    /// When the venue says the state changed.
    pub at: Timestamp,
    /// When this platform received it.
    pub capture_time: Timestamp,
    /// The venue's own sequence for the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    pub status: VenueStatus,
}

/// An auction's indicative price and imbalance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuctionIndication {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub capture_time: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// `None` where the venue has published no indicative price yet.
    pub indicative_price: Option<Decimal>,
    pub paired: Decimal,
    pub imbalance: Decimal,
    /// `None` where the auction does not lean, or the venue cannot say.
    pub imbalance_side: Option<BookSide>,
}

/// The book's lifecycle ended: it is to be discarded and rebuilt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookReset {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub capture_time: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    pub reason: String,
}

impl VenueStatusUpdate {
    /// Structural problems; empty means publishable.
    pub fn validate(&self) -> Vec<String> {
        check_capture_order(self.at, self.capture_time)
            .into_iter()
            .collect()
    }
}

impl AuctionIndication {
    /// Structural problems; empty means publishable. A negative paired or
    /// unpaired quantity is a decoding error, and a non-positive indicative
    /// price is not a price — the venue's "no price yet" is `None`.
    pub fn validate(&self) -> Vec<String> {
        let mut issues: Vec<String> = check_capture_order(self.at, self.capture_time)
            .into_iter()
            .collect();
        if self.paired.is_negative() || self.imbalance.is_negative() {
            issues.push(format!(
                "negative auction quantity: paired {} imbalance {}",
                self.paired, self.imbalance
            ));
        }
        if let Some(price) = self.indicative_price
            && !price.is_positive()
        {
            issues.push(format!("non-positive indicative price {price}"));
        }
        if self.imbalance.is_zero() && self.imbalance_side.is_some() {
            issues.push(
                "an imbalance side with no imbalance: the auction cannot lean on nothing".into(),
            );
        }
        issues
    }
}

impl BookReset {
    /// Structural problems; empty means publishable. A reset that cannot say
    /// why is one an operator cannot act on.
    pub fn validate(&self) -> Vec<String> {
        let mut issues: Vec<String> = check_capture_order(self.at, self.capture_time)
            .into_iter()
            .collect();
        if self.reason.trim().is_empty() {
            issues.push("a book reset with no reason".into());
        }
        issues
    }
}

/// The one body [`Topic::MarketSession`] carries, tagged by kind, so a
/// subscriber decodes a session fact by what it says it is rather than by
/// trying each shape until one fits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "session", rename_all = "snake_case")]
pub enum SessionRecord {
    Status(VenueStatusUpdate),
    Auction(AuctionIndication),
    Reset(BookReset),
}

impl EventBody for SessionRecord {
    const TOPIC: Topic = Topic::MarketSession;
    const SCHEMA_VERSION: u32 = 1;
}

impl SessionRecord {
    /// The typed sensed record this body is.
    pub fn into_sensed(self) -> SensedRecord {
        match self {
            Self::Status(status) => SensedRecord::Status(Box::new(status)),
            Self::Auction(auction) => SensedRecord::Auction(Box::new(auction)),
            Self::Reset(reset) => SensedRecord::Reset(Box::new(reset)),
        }
    }
}

/// What a decoded message becomes on the central side.
#[derive(Clone, Debug, PartialEq)]
pub enum Canonical {
    /// A typed record of its own. Boxed because a depth update carries
    /// nothing and most messages a depth feed sends are depth updates.
    Record(Box<SensedRecord>),
    /// A depth update (an order added, reduced, removed or replaced, or a
    /// level set). Its canonical record is the [`SensedRecord::Book`] the
    /// book state publishes after applying it, not a record per increment:
    /// an increment means nothing without every increment before it, which
    /// is why `qip_orderbook` holds the state and the depth adapter
    /// publishes the book.
    DepthUpdate,
}

/// Normalise one decoded message for `object_id`, judged `quality`.
///
/// Refuses a one-sided quote: the canonical [`Quote`] is a top of book with
/// both sides, and filling the missing side with zero would publish a
/// spread nobody quoted. A venue publishing one side is a level update, and
/// the depth path is where it belongs.
pub fn normalise(
    message: &MarketMessage,
    object_id: &ObjectId,
    quality: DataQuality,
) -> Result<Canonical> {
    let venue = message.origin.venue.as_str().to_string();
    let at = message.venue_time;
    let capture_time = message.capture_time;
    let sequence = Some(message.origin.sequence);
    let object_id = object_id.clone();
    let record = match &message.body {
        MessageBody::OrderAdded { .. }
        | MessageBody::OrderReduced { .. }
        | MessageBody::OrderRemoved { .. }
        | MessageBody::OrderReplaced { .. }
        | MessageBody::LevelSet { .. } => return Ok(Canonical::DepthUpdate),
        MessageBody::Quote { bid, ask } => {
            let (Some((bid, bid_size)), Some((ask, ask_size))) = (bid, ask) else {
                return Err(Error::invalid(format!(
                    "a one-sided quote from {venue} is not a top of book; it is a level \
                     update for the depth path, and a missing side is not zero"
                )));
            };
            SensedRecord::Quote(Quote {
                object_id,
                venue,
                at,
                capture_time,
                sequence,
                bid: *bid,
                ask: *ask,
                bid_size: *bid_size,
                ask_size: *ask_size,
                quality,
            })
        }
        MessageBody::Trade {
            price,
            quantity,
            condition,
            aggressor,
        } => SensedRecord::Trade(Trade {
            object_id,
            venue,
            at,
            capture_time,
            sequence,
            price: *price,
            size: *quantity,
            aggressor: aggressor.map(|side| match side {
                BookSide::Bid => Side::Buy,
                BookSide::Ask => Side::Sell,
            }),
            condition: trade_condition(*condition),
            trade_id: None,
            quality,
        }),
        MessageBody::StatusChange { status } => SensedRecord::Status(Box::new(VenueStatusUpdate {
            object_id,
            venue,
            at,
            capture_time,
            sequence,
            status: *status,
        })),
        MessageBody::AuctionUpdate {
            indicative_price,
            paired,
            imbalance,
            imbalance_side,
        } => SensedRecord::Auction(Box::new(AuctionIndication {
            object_id,
            venue,
            at,
            capture_time,
            sequence,
            indicative_price: *indicative_price,
            paired: *paired,
            imbalance: *imbalance,
            imbalance_side: *imbalance_side,
        })),
        MessageBody::Reset { reason } => SensedRecord::Reset(Box::new(BookReset {
            object_id,
            venue,
            at,
            capture_time,
            sequence,
            reason: reason.clone(),
        })),
    };
    Ok(Canonical::Record(Box::new(record)))
}

/// The wire's trade condition in the canonical vocabulary. Exhaustive: a
/// condition the wire adds must be placed here before anything compiles.
const fn trade_condition(condition: WireCondition) -> TradeCondition {
    match condition {
        WireCondition::Regular => TradeCondition::Regular,
        WireCondition::Auction => TradeCondition::Auction,
        WireCondition::Reported => TradeCondition::LateReport,
        // The canonical vocabulary has no odd lot. What the platform reads
        // from a condition is whether the print may move the mark, and an
        // odd lot may not (`qip_contracts::TradeCondition::updates_last`);
        // `OffExchange` is the one non-price-forming condition that claims
        // neither lateness nor a correction the venue never made.
        WireCondition::OddLot | WireCondition::Negotiated => TradeCondition::OffExchange,
        WireCondition::Correction => TradeCondition::Corrected,
    }
}
