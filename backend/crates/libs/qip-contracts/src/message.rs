//! The protocol-agnostic market message.
//!
//! Every venue protocol — FIX, ITCH, SBE, a WebSocket JSON frame, a chain log
//! — decodes into this. Downstream, nothing knows which wire it arrived on,
//! which is what lets one order book implementation serve every venue class.

use crate::venue::Origin;
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};

/// Which side of the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BookSide {
    Bid,
    Ask,
}

impl BookSide {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Ask => "ask",
        }
    }

    pub const fn opposite(&self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }

    /// Whether `better` is a more aggressive price than `than` on this side.
    ///
    /// The comparison that flips by side and is got wrong at least once in
    /// every order book ever written.
    pub fn is_better(&self, better: Decimal, than: Decimal) -> bool {
        match self {
            Self::Bid => better > than,
            Self::Ask => better < than,
        }
    }
}

/// Why a trade printed the way it did.
///
/// Conditions decide whether a print updates the last price, contributes to
/// volume, or should be ignored entirely. Treating every print as a trade is
/// how a mid-price gets dragged by an odd-lot cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeCondition {
    /// An ordinary continuous-session trade.
    Regular,
    /// Printed in an opening, closing or intraday auction.
    Auction,
    /// Executed away and reported late.
    Reported,
    /// Odd lot; does not update the last sale.
    OddLot,
    /// Corrected or cancelled a previous print.
    Correction,
    /// Negotiated away from the prevailing quote.
    Negotiated,
}

impl TradeCondition {
    /// Whether this print should update the reference last price.
    pub const fn updates_last(&self) -> bool {
        matches!(self, Self::Regular | Self::Auction)
    }

    /// Whether this print counts toward traded volume.
    pub const fn counts_toward_volume(&self) -> bool {
        !matches!(self, Self::Correction)
    }
}

/// What a decoded message says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageBody {
    /// A new order resting on the book. Order-by-order (L3) venues only.
    OrderAdded {
        order_ref: u64,
        side: BookSide,
        price: Decimal,
        quantity: Decimal,
    },
    /// A resting order's remaining quantity fell without trading.
    OrderReduced { order_ref: u64, remaining: Decimal },
    /// A resting order left the book.
    OrderRemoved { order_ref: u64 },
    /// A resting order's price or size changed, keeping its identity.
    OrderReplaced {
        order_ref: u64,
        price: Decimal,
        quantity: Decimal,
    },
    /// An aggregated price level was set to a new size. Level-based (L2)
    /// venues. A size of zero removes the level.
    LevelSet {
        side: BookSide,
        price: Decimal,
        quantity: Decimal,
        /// Resting orders at the level, where the venue publishes it.
        order_count: Option<u32>,
    },
    /// The top of book, for venues that publish only a quote.
    Quote {
        bid: Option<(Decimal, Decimal)>,
        ask: Option<(Decimal, Decimal)>,
    },
    /// A trade printed.
    Trade {
        price: Decimal,
        quantity: Decimal,
        condition: TradeCondition,
        /// The aggressing side where the venue discloses it.
        aggressor: Option<BookSide>,
    },
    /// The venue changed trading state for this instrument.
    StatusChange { status: crate::venue::VenueStatus },
    /// An auction's indicative price and imbalance.
    AuctionUpdate {
        indicative_price: Option<Decimal>,
        paired: Decimal,
        imbalance: Decimal,
        imbalance_side: Option<BookSide>,
    },
    /// The book should be discarded and rebuilt from the next snapshot.
    ///
    /// Emitted on a detected gap. A consumer that ignores this is trading off
    /// a book it knows to be wrong.
    Reset { reason: String },
}

impl MessageBody {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::OrderAdded { .. } => "order_added",
            Self::OrderReduced { .. } => "order_reduced",
            Self::OrderRemoved { .. } => "order_removed",
            Self::OrderReplaced { .. } => "order_replaced",
            Self::LevelSet { .. } => "level_set",
            Self::Quote { .. } => "quote",
            Self::Trade { .. } => "trade",
            Self::StatusChange { .. } => "status_change",
            Self::AuctionUpdate { .. } => "auction_update",
            Self::Reset { .. } => "reset",
        }
    }

    /// Whether applying this message can change the top of book.
    ///
    /// The feature DAG uses this to skip recomputation, so it must never
    /// return false for a message that could move the touch.
    pub const fn may_move_touch(&self) -> bool {
        !matches!(self, Self::Trade { .. } | Self::AuctionUpdate { .. })
    }
}

/// A decoded message, stamped and attributed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketMessage {
    pub object_id: ObjectId,
    pub origin: Origin,
    pub body: MessageBody,
    /// When the venue says it happened.
    pub venue_time: Timestamp,
    /// When this cell's hardware saw the packet.
    pub capture_time: Timestamp,
}

impl MarketMessage {
    pub fn new(
        object_id: ObjectId,
        origin: Origin,
        body: MessageBody,
        venue_time: Timestamp,
        capture_time: Timestamp,
    ) -> Self {
        Self {
            object_id,
            origin,
            body,
            venue_time,
            capture_time,
        }
    }

    /// Wire latency: how long the message took to reach this cell.
    ///
    /// Saturates at zero rather than going negative when the venue's clock
    /// runs ahead of ours, which is common and is a clock-discipline problem
    /// rather than a message problem.
    pub fn transit(&self) -> qip_core::Duration {
        self.capture_time.since(self.venue_time)
    }

    /// Stamp with valid-time from the venue and known-time from capture.
    pub fn stamped(self) -> crate::time::Stamped<Self> {
        let (valid, known) = (self.venue_time, self.capture_time);
        crate::time::Stamped::new(self, valid, known)
    }
}
