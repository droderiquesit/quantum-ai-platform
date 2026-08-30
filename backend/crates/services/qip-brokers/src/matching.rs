//! A matching engine with price-time priority.
//!
//! This is the part of a simulated venue that is usually faked. A simulator
//! that fills a fixed fraction of every order at the arrival price teaches a
//! strategy that liquidity is free and that queue position does not exist, and
//! both lessons are expensive to unlearn against a real book.
//!
//! So orders actually match here. Resting interest is held in price then time
//! order; an incoming order walks the opposite side taking the best price
//! first and the earliest arrival within a price; what it cannot take either
//! rests behind everything already at its price or goes away. A replace that
//! adds quantity or moves the price goes to the back of the queue, because that
//! is what a venue does and the cost of an amendment is most of why order
//! working is hard.
//!
//! Nothing here is random and nothing reads a clock. The same instructions in
//! the same order produce the same trades, which is what makes a simulated
//! venue usable as a test oracle.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{ObjectId, OrderId};
use qip_core::time::Timestamp;
use qip_execution_engine::order::Side;
use qip_market::book::BookLevel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Who owns a resting order.
///
/// The venue's own seeded liquidity is not booked to anybody's account; the
/// client's is. Keeping them apart is what lets the venue hold a two-sided
/// market without inventing positions for the account under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Participant {
    /// Liquidity the venue was seeded with, standing in for everyone else.
    Venue,
    /// The account this adapter is connected for.
    Client,
}

impl Participant {
    pub const fn is_client(&self) -> bool {
        matches!(self, Self::Client)
    }
}

/// An order resting in the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resting {
    pub order_id: OrderId,
    pub side: Side,
    pub price: Decimal,
    pub quantity: Decimal,
    pub filled: Decimal,
    /// Queue sequence: lower is earlier, and ties within a price go to it.
    pub sequence: u64,
    pub rested_at: Timestamp,
    pub owner: Participant,
}

impl Resting {
    pub fn remaining(&self) -> Decimal {
        self.quantity - self.filled
    }
}

/// One execution between two orders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub object_id: ObjectId,
    /// The resting order's price. The taker crossed the spread; the maker's
    /// price is what it agreed to trade at.
    pub price: Decimal,
    pub quantity: Decimal,
    pub at: Timestamp,
    pub taker: OrderId,
    pub maker: OrderId,
    /// The side the aggressor was on.
    pub taker_side: Side,
    pub taker_owner: Participant,
    pub maker_owner: Participant,
}

/// What happened to an order that reached the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionOutcome {
    pub trades: Vec<Trade>,
    pub filled: Decimal,
    /// Quantity that neither traded nor rests.
    pub cancelled: Decimal,
    /// Whether the remainder is now resting in the book.
    pub resting: bool,
}

impl ExecutionOutcome {
    pub fn traded_value(&self) -> Decimal {
        self.trades
            .iter()
            .map(|trade| trade.price * trade.quantity)
            .sum()
    }
}

/// One instrument's two-sided book.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct InstrumentBook {
    /// Highest price first, then earliest sequence.
    bids: Vec<Resting>,
    /// Lowest price first, then earliest sequence.
    asks: Vec<Resting>,
}

impl InstrumentBook {
    fn side_mut(&mut self, side: Side) -> &mut Vec<Resting> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    fn side(&self, side: Side) -> &[Resting] {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    /// Restore price-time order after an insertion or an amendment.
    fn sort(&mut self, side: Side) {
        match side {
            Side::Buy => self
                .bids
                .sort_by(|a, b| b.price.cmp(&a.price).then(a.sequence.cmp(&b.sequence))),
            Side::Sell => self
                .asks
                .sort_by(|a, b| a.price.cmp(&b.price).then(a.sequence.cmp(&b.sequence))),
        }
    }
}

/// Books for every listed instrument.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MatchingEngine {
    books: BTreeMap<String, InstrumentBook>,
    /// Monotonic queue sequence, shared across instruments so an ordering is
    /// total and a replay is unambiguous.
    sequence: u64,
}

impl MatchingEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add resting interest that belongs to somebody else.
    ///
    /// How a simulated venue gets a market to trade against without pretending
    /// the client is trading with itself.
    pub fn seed(
        &mut self,
        object_id: &ObjectId,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
    ) -> Result<()> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("seeded liquidity needs a positive quantity"));
        }
        if price <= Decimal::ZERO {
            return Err(Error::invalid("seeded liquidity needs a positive price"));
        }
        self.sequence = self.sequence.saturating_add(1);
        let order_id = OrderId::from_string(format!(
            "venue-liquidity-{}-{}",
            object_id.as_str(),
            self.sequence
        ));
        let resting = Resting {
            order_id,
            side,
            price,
            quantity,
            filled: Decimal::ZERO,
            sequence: self.sequence,
            rested_at: at,
            owner: Participant::Venue,
        };
        let book = self
            .books
            .entry(object_id.as_str().to_string())
            .or_default();
        book.side_mut(side).push(resting);
        book.sort(side);
        Ok(())
    }

    /// Match an incoming order, resting whatever is left if it may rest.
    ///
    /// `limit` of `None` is an order that takes any price and never rests: what
    /// it cannot fill immediately is cancelled, because a market order that
    /// rested would become a limit order at a price nobody chose.
    pub fn execute(
        &mut self,
        object_id: &ObjectId,
        order_id: &OrderId,
        side: Side,
        quantity: Decimal,
        limit: Option<Decimal>,
        at: Timestamp,
        owner: Participant,
    ) -> ExecutionOutcome {
        let book = self
            .books
            .entry(object_id.as_str().to_string())
            .or_default();
        let mut remaining = quantity;
        let mut trades = Vec::new();

        {
            let makers = book.side_mut(side.opposite());
            for maker in makers.iter_mut() {
                if remaining <= Decimal::ZERO {
                    break;
                }
                // Both sides are held in book order, so the first unacceptable
                // price ends the walk: nothing behind it can be better.
                let acceptable = match side {
                    Side::Buy => limit.is_none_or(|limit| maker.price <= limit),
                    Side::Sell => limit.is_none_or(|limit| maker.price >= limit),
                };
                if !acceptable {
                    break;
                }
                let available = maker.remaining();
                if available <= Decimal::ZERO {
                    continue;
                }
                let traded = available.min(remaining);
                maker.filled += traded;
                remaining -= traded;
                trades.push(Trade {
                    object_id: object_id.clone(),
                    price: maker.price,
                    quantity: traded,
                    at,
                    taker: order_id.clone(),
                    maker: maker.order_id.clone(),
                    taker_side: side,
                    taker_owner: owner,
                    maker_owner: maker.owner,
                });
            }
            makers.retain(|maker| maker.remaining() > Decimal::ZERO);
        }

        let filled = quantity - remaining;
        match limit {
            Some(price) if remaining > Decimal::ZERO => {
                self.sequence = self.sequence.saturating_add(1);
                let sequence = self.sequence;
                let book = self
                    .books
                    .entry(object_id.as_str().to_string())
                    .or_default();
                book.side_mut(side).push(Resting {
                    order_id: order_id.clone(),
                    side,
                    price,
                    // The book holds what is left to work, so a partially
                    // filled order rests for its residual rather than its
                    // original size.
                    quantity: remaining,
                    filled: Decimal::ZERO,
                    sequence,
                    rested_at: at,
                    owner,
                });
                book.sort(side);
                ExecutionOutcome {
                    trades,
                    filled,
                    cancelled: Decimal::ZERO,
                    resting: true,
                }
            }
            _ => ExecutionOutcome {
                trades,
                filled,
                cancelled: remaining,
                resting: false,
            },
        }
    }

    /// Whether an order is still resting.
    pub fn is_resting(&self, object_id: &ObjectId, order_id: &OrderId) -> bool {
        self.resting(object_id, order_id).is_some()
    }

    /// The resting record for an order, if the book still holds one.
    pub fn resting(&self, object_id: &ObjectId, order_id: &OrderId) -> Option<&Resting> {
        let book = self.books.get(object_id.as_str())?;
        book.bids
            .iter()
            .chain(book.asks.iter())
            .find(|resting| resting.order_id == *order_id)
    }

    /// Pull an order out of the book.
    pub fn cancel(&mut self, object_id: &ObjectId, order_id: &OrderId) -> Result<Resting> {
        let Some(book) = self.books.get_mut(object_id.as_str()) else {
            return Err(Error::not_found(format!(
                "no book for {}",
                object_id.as_str()
            )));
        };
        for side in [Side::Buy, Side::Sell] {
            let orders = book.side_mut(side);
            if let Some(index) = orders
                .iter()
                .position(|resting| resting.order_id == *order_id)
            {
                return Ok(orders.remove(index));
            }
        }
        Err(Error::not_found(format!(
            "nothing resting under {} in {}",
            order_id.as_str(),
            object_id.as_str()
        )))
    }

    /// Amend a resting order, keeping its identity.
    ///
    /// `quantity` is the new *working* quantity — what is left to trade, not
    /// the original size — because that is what the book holds. Reducing it
    /// keeps queue position; adding to it or moving the price goes to the back.
    /// That is the venue convention, and it is the reason an amendment is not
    /// free.
    pub fn replace(
        &mut self,
        object_id: &ObjectId,
        order_id: &OrderId,
        quantity: Decimal,
        price: Option<Decimal>,
        at: Timestamp,
    ) -> Result<Resting> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "replacing {} with a non-positive working quantity is a cancel; say so",
                order_id.as_str()
            )));
        }
        let next_sequence = self.sequence.saturating_add(1);
        let Some(book) = self.books.get_mut(object_id.as_str()) else {
            return Err(Error::not_found(format!(
                "no book for {}",
                object_id.as_str()
            )));
        };
        for side in [Side::Buy, Side::Sell] {
            let orders = book.side_mut(side);
            let Some(index) = orders
                .iter()
                .position(|resting| resting.order_id == *order_id)
            else {
                continue;
            };
            let Some(order) = orders.get_mut(index) else {
                continue;
            };
            let moved_price = price.is_some_and(|price| price != order.price);
            let lost_priority = moved_price || quantity > order.quantity;
            if let Some(price) = price {
                order.price = price;
            }
            order.quantity = quantity;
            // The book holds what is left to work, so an amendment restates
            // the residual rather than adjusting it. Keeping a stale filled
            // figure here would subtract the same shares twice.
            order.filled = Decimal::ZERO;
            order.rested_at = at;
            if lost_priority {
                order.sequence = next_sequence;
            }
            let amended = order.clone();
            book.sort(side);
            if lost_priority {
                self.sequence = next_sequence;
            }
            return Ok(amended);
        }
        Err(Error::not_found(format!(
            "nothing resting under {} in {}",
            order_id.as_str(),
            object_id.as_str()
        )))
    }

    /// Depth by price, best first, aggregated across resting orders.
    pub fn depth(&self, object_id: &ObjectId, side: Side) -> Vec<BookLevel> {
        let Some(book) = self.books.get(object_id.as_str()) else {
            return Vec::new();
        };
        let mut levels: Vec<BookLevel> = Vec::new();
        for resting in book.side(side) {
            let remaining = resting.remaining();
            if remaining <= Decimal::ZERO {
                continue;
            }
            match levels.iter_mut().find(|level| level.price == resting.price) {
                Some(level) => {
                    level.size += remaining;
                    level.order_count = level.order_count.saturating_add(1);
                }
                None => levels.push(BookLevel::new(resting.price, remaining)),
            }
        }
        levels
    }

    /// Best price on a side, if anything is resting.
    pub fn best(&self, object_id: &ObjectId, side: Side) -> Option<BookLevel> {
        self.depth(object_id, side).first().copied()
    }

    /// How many orders are resting across every instrument.
    pub fn resting_count(&self) -> usize {
        self.books
            .values()
            .map(|book| book.bids.len() + book.asks.len())
            .sum()
    }

    /// Drop an instrument's book, for a venue that delists or reopens.
    pub fn clear(&mut self, object_id: &ObjectId) {
        self.books.remove(object_id.as_str());
    }
}
