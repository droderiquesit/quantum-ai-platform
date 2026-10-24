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
use qip_execution_engine::creation::{CreationAction, CreationAdmission};
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
    /// One entry per instrument [`MatchingEngine::open`] opened, and no
    /// other: nothing here creates a book on demand.
    books: BTreeMap<String, InstrumentBook>,
    /// Monotonic queue sequence, shared across instruments so an ordering is
    /// total and a replay is unambiguous.
    sequence: u64,
}

/// Refuse `admission` unless it is to `action` exactly `object_id`.
fn admitted(
    admission: &CreationAdmission,
    action: CreationAction,
    object_id: &ObjectId,
) -> Result<()> {
    let request = admission.request();
    if request.action != action || request.product != object_id.as_str() {
        return Err(Error::denied(format!(
            "the matching engine will not {} {}: the admission offered is to {} {} at {}, and \
             this needs an admission to {} exactly this instrument",
            action.as_str(),
            object_id.as_str(),
            request.action.as_str(),
            request.product,
            request.venue,
            action.as_str()
        )));
    }
    Ok(())
}

impl MatchingEngine {
    /// An engine with no books, under which every seed and every order is
    /// refused until [`Self::open`] opens the instrument's book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open an empty book for `object_id`, so it can be seeded and traded.
    ///
    /// **The only way a book comes to exist here.** [`Self::seed`] and
    /// [`Self::execute`] used to create one on demand
    /// (`entry().or_default()`) for whatever object id they were handed, so a
    /// book for an instrument the venue never listed could be conjured by
    /// seeding liquidity into it — a market brought into existence by a call
    /// that never asked whether anybody had permitted it (R12-COVERAGE-5107).
    /// Both now refuse an instrument whose book is not open, and the
    /// simulated exchange opens a book at one place only: when it lists the
    /// instrument, which it does only under a creation admission.
    ///
    /// Refuses a book that is already open. Opening it again would be a
    /// second listing of the same id, and must never quietly discard what
    /// rests in the first.
    ///
    /// **Demands an admission to list exactly this instrument.** The engine
    /// is public and re-exported, so "the exchange opens a book only when it
    /// lists" held for the exchange and not for any other caller: anyone
    /// could build an engine, open a book and seed it with no permission
    /// recorded anywhere (R12-EXEC-5079). The venue the admission names is
    /// the exchange's to check — this engine has no venue of its own.
    pub fn open(&mut self, object_id: &ObjectId, admission: &CreationAdmission) -> Result<()> {
        admitted(admission, CreationAction::List, object_id)?;
        if self.books.contains_key(object_id.as_str()) {
            return Err(Error::invalid(format!(
                "the book for {} is already open, and opening it again would discard what rests \
                 in it; trade the open book, or clear it before reopening",
                object_id.as_str()
            )));
        }
        self.books
            .insert(object_id.as_str().to_string(), InstrumentBook::default());
        Ok(())
    }

    /// Whether `object_id` has a book open.
    pub fn is_open(&self, object_id: &ObjectId) -> bool {
        self.books.contains_key(object_id.as_str())
    }

    /// The open book for `object_id`, or a refusal naming what to do.
    fn opened(&mut self, object_id: &ObjectId) -> Result<&mut InstrumentBook> {
        self.books.get_mut(object_id.as_str()).ok_or_else(|| {
            Error::not_found(format!(
                "no book is open for {}, and this engine seeds and matches only an instrument \
                 whose book was opened; the venue opens one when it lists the instrument, so \
                 list it first",
                object_id.as_str()
            ))
        })
    }

    /// Add resting interest that belongs to somebody else.
    ///
    /// How a simulated venue gets a market to trade against without pretending
    /// the client is trading with itself. Refused for an instrument whose book
    /// is not open, before a queue sequence is spent.
    ///
    /// **Demands an admission to seed exactly this instrument**
    /// (R12-EXEC-5079): seeding is one of the three governed actions, and a
    /// listing admission is not a seeding one.
    pub fn seed(
        &mut self,
        object_id: &ObjectId,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
        admission: &CreationAdmission,
    ) -> Result<()> {
        admitted(admission, CreationAction::Seed, object_id)?;
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid("seeded liquidity needs a positive quantity"));
        }
        if price <= Decimal::ZERO {
            return Err(Error::invalid("seeded liquidity needs a positive price"));
        }
        let sequence = self.sequence.saturating_add(1);
        let order_id = OrderId::from_string(format!(
            "venue-liquidity-{}-{}",
            object_id.as_str(),
            sequence
        ));
        let book = self.opened(object_id)?;
        book.side_mut(side).push(Resting {
            order_id,
            side,
            price,
            quantity,
            filled: Decimal::ZERO,
            sequence,
            rested_at: at,
            owner: Participant::Venue,
        });
        book.sort(side);
        self.sequence = sequence;
        Ok(())
    }

    /// Match an incoming order, resting whatever is left if it may rest.
    ///
    /// `limit` of `None` is an order that takes any price and never rests: what
    /// it cannot fill immediately is cancelled, because a market order that
    /// rested would become a limit order at a price nobody chose.
    ///
    /// Refused, with nothing matched and nothing rested, for an instrument
    /// whose book is not open: an order that rested would otherwise open a
    /// book nobody listed.
    pub fn execute(
        &mut self,
        object_id: &ObjectId,
        order_id: &OrderId,
        side: Side,
        quantity: Decimal,
        limit: Option<Decimal>,
        at: Timestamp,
        owner: Participant,
    ) -> Result<ExecutionOutcome> {
        let book = self.opened(object_id)?;
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
                let sequence = self.sequence.saturating_add(1);
                let book = self.opened(object_id)?;
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
                self.sequence = sequence;
                Ok(ExecutionOutcome {
                    trades,
                    filled,
                    cancelled: Decimal::ZERO,
                    resting: true,
                })
            }
            _ => Ok(ExecutionOutcome {
                trades,
                filled,
                cancelled: remaining,
                resting: false,
            }),
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

    /// Drop an instrument's book, for a venue that delists or reopens. The
    /// instrument is then refused by [`Self::seed`] and [`Self::execute`]
    /// until it is opened again.
    pub fn clear(&mut self, object_id: &ObjectId) {
        self.books.remove(object_id.as_str());
    }
}
