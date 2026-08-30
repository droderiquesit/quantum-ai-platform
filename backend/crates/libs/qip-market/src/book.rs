//! Limit order books.
//!
//! Depth beyond the touch is what execution algorithms size against and what
//! the microstructure agent reads for pressure. The book keeps both sides
//! sorted so the touch is always the first element.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(&self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }

    /// +1 for a buy, -1 for a sell. Multiplying a quantity by this gives the
    /// signed position change.
    pub fn sign(&self) -> i32 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

/// One price level.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookLevel {
    pub price: Decimal,
    pub size: Decimal,
    /// Number of resting orders, where the venue publishes it.
    #[serde(default)]
    pub order_count: u32,
}

impl BookLevel {
    pub fn new(price: Decimal, size: Decimal) -> Self {
        Self {
            price,
            size,
            order_count: 1,
        }
    }

    pub fn notional(&self) -> Decimal {
        self.price * self.size
    }
}

/// A depth snapshot.
///
/// Bids are held descending and asks ascending, so index 0 of each is the
/// touch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderBook {
    pub object_id: ObjectId,
    pub venue: String,
    pub at: Timestamp,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    /// Venue sequence number, for gap detection.
    #[serde(default)]
    pub sequence: u64,
}

impl OrderBook {
    pub fn new(object_id: ObjectId, venue: impl Into<String>, at: Timestamp) -> Self {
        Self {
            object_id,
            venue: venue.into(),
            at,
            bids: Vec::new(),
            asks: Vec::new(),
            sequence: 0,
        }
    }

    /// Build from level vectors, sorting each side into book order.
    pub fn from_levels(
        object_id: ObjectId,
        venue: impl Into<String>,
        at: Timestamp,
        mut bids: Vec<BookLevel>,
        mut asks: Vec<BookLevel>,
    ) -> Self {
        bids.sort_by(|a, b| b.price.cmp(&a.price));
        asks.sort_by(|a, b| a.price.cmp(&b.price));
        Self {
            object_id,
            venue: venue.into(),
            at,
            bids,
            asks,
            sequence: 0,
        }
    }

    pub fn best_bid(&self) -> Option<BookLevel> {
        self.bids.first().copied()
    }

    pub fn best_ask(&self) -> Option<BookLevel> {
        self.asks.first().copied()
    }

    pub fn mid(&self) -> Option<Decimal> {
        let bid = self.best_bid()?.price;
        let ask = self.best_ask()?.price;
        (bid + ask).checked_div(Decimal::from_int(2))
    }

    pub fn spread(&self) -> Option<Decimal> {
        Some(self.best_ask()?.price - self.best_bid()?.price)
    }

    /// Total size available within `levels` of the touch on one side.
    pub fn depth(&self, side: Side, levels: usize) -> Decimal {
        let source = match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        source.iter().take(levels).map(|l| l.size).sum()
    }

    /// Order-book imbalance over `levels`, in `[-1, 1]`.
    pub fn imbalance(&self, levels: usize) -> f64 {
        let bid = self.depth(Side::Buy, levels).to_f64();
        let ask = self.depth(Side::Sell, levels).to_f64();
        let total = bid + ask;
        if total <= 0.0 {
            return 0.0;
        }
        (bid - ask) / total
    }

    /// Size-weighted mid using the touch sizes.
    pub fn microprice(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        let total = bid.size + ask.size;
        if !total.is_positive() {
            return self.mid();
        }
        (bid.price * ask.size + ask.price * bid.size).checked_div(total)
    }

    /// Walk the book to fill `quantity`, returning the volume-weighted price
    /// and the quantity actually available.
    ///
    /// This is what an execution algorithm consults before crossing the spread:
    /// the touch price is what you see, and this is what you would pay.
    pub fn sweep(&self, side: Side, quantity: Decimal) -> Option<(Decimal, Decimal)> {
        if !quantity.is_positive() {
            return None;
        }
        // Buying lifts offers; selling hits bids.
        let levels = match side {
            Side::Buy => &self.asks,
            Side::Sell => &self.bids,
        };
        let mut remaining = quantity;
        let mut notional = Decimal::ZERO;
        let mut filled = Decimal::ZERO;
        for level in levels {
            if !remaining.is_positive() {
                break;
            }
            let take = level.size.min(remaining);
            notional += take * level.price;
            filled += take;
            remaining -= take;
        }
        if !filled.is_positive() {
            return None;
        }
        Some((notional.checked_div(filled)?, filled))
    }

    /// Cost of crossing to fill `quantity`, in basis points of the mid.
    ///
    /// `None` when the book cannot fill it — a liquidity constraint the caller
    /// must respect rather than a number to extrapolate.
    pub fn sweep_cost_bps(&self, side: Side, quantity: Decimal) -> Option<f64> {
        let mid = self.mid()?;
        if !mid.is_positive() {
            return None;
        }
        let (average, filled) = self.sweep(side, quantity)?;
        if filled < quantity {
            return None;
        }
        let signed = match side {
            Side::Buy => average - mid,
            Side::Sell => mid - average,
        };
        Some(signed.to_f64() / mid.to_f64() * 10_000.0)
    }

    /// Total size resting on both sides.
    pub fn total_depth(&self) -> Decimal {
        self.bids.iter().map(|l| l.size).sum::<Decimal>()
            + self.asks.iter().map(|l| l.size).sum::<Decimal>()
    }

    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => ask.price < bid.price,
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// Structural validation. A book that fails this must not reach a decision.
    pub fn validate(&self) -> Result<()> {
        if self.is_crossed() {
            return Err(Error::invalid(format!(
                "crossed book for {}: best bid {} above best ask {}",
                self.object_id,
                self.best_bid().map(|l| l.price).unwrap_or_default(),
                self.best_ask().map(|l| l.price).unwrap_or_default()
            )));
        }
        if self.bids.windows(2).any(|w| w[0].price <= w[1].price) {
            return Err(Error::invalid("bid levels are not strictly descending"));
        }
        if self.asks.windows(2).any(|w| w[0].price >= w[1].price) {
            return Err(Error::invalid("ask levels are not strictly ascending"));
        }
        if self
            .bids
            .iter()
            .chain(&self.asks)
            .any(|l| l.size.is_negative() || l.price.is_negative())
        {
            return Err(Error::invalid("book contains a negative price or size"));
        }
        Ok(())
    }

    /// Collapse to a top-of-book quote.
    pub fn to_quote(&self) -> Option<crate::quote::Quote> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some(crate::quote::Quote {
            object_id: self.object_id.clone(),
            venue: self.venue.clone(),
            at: self.at,
            bid: bid.price,
            ask: ask.price,
            bid_size: bid.size,
            ask_size: ask.size,
            quality: Default::default(),
        })
    }
}

impl EventBody for OrderBook {
    const TOPIC: Topic = Topic::MarketOrderBook;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        if self.sequence == 0 {
            return None;
        }
        Some(format!(
            "{}:{}:{}",
            self.object_id, self.venue, self.sequence
        ))
    }
}
