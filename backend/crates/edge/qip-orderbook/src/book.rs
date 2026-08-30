//! The book a consumer actually holds.
//!
//! Which resolution a venue publishes is the venue's business. A strategy
//! asking for the microprice, or a router asking what a sweep would cost,
//! should not branch on it — so both flavours live behind this one type and
//! behind [`BookView`], and the only operation that has to know the difference
//! is [`Book::queue_position`], which an aggregated feed genuinely cannot
//! answer and says so rather than approximating.

use crate::l2::L2Book;
use crate::l3::{L3Book, QueuePosition};
use crate::ladder::LevelWalk;
use crate::snapshot::BookKind;
use crate::view::BookView;
use qip_contracts::{BookSide, MessageBody};
use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// A venue's book, at whatever resolution the venue publishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Book {
    /// Every resting order tracked by reference.
    OrderByOrder(L3Book),
    /// Aggregated price levels.
    Aggregated(L2Book),
}

impl Book {
    /// An empty order-by-order book.
    pub fn order_by_order() -> Self {
        Self::OrderByOrder(L3Book::new())
    }

    /// An empty aggregated book.
    pub fn aggregated() -> Self {
        Self::Aggregated(L2Book::new())
    }

    /// An empty book of the given kind.
    pub fn of_kind(kind: BookKind) -> Self {
        match kind {
            BookKind::OrderByOrder => Self::order_by_order(),
            BookKind::Aggregated => Self::aggregated(),
        }
    }

    /// Apply one decoded message, refusing updates at the wrong resolution.
    pub fn apply(&mut self, body: &MessageBody) -> Result<()> {
        match self {
            Self::OrderByOrder(book) => book.apply(body),
            Self::Aggregated(book) => book.apply(body),
        }
    }

    /// Discard every level and order.
    ///
    /// The book's half of a reset. Whether the state that follows is safe to
    /// trade is [`crate::VenueState`]'s to say, not the book's — an empty book
    /// alone cannot distinguish "nothing rests" from "we threw it away".
    pub fn clear(&mut self) {
        match self {
            Self::OrderByOrder(book) => book.clear(),
            Self::Aggregated(book) => book.clear(),
        }
    }

    /// Where an order sits in its level's queue.
    ///
    /// `Unavailable` on an aggregated book: the information is not merely
    /// missing from this implementation, it is absent from the feed, and a
    /// caller must be able to tell that apart from "your order is not there".
    pub fn queue_position(&self, order_ref: u64) -> Result<QueuePosition> {
        match self {
            Self::OrderByOrder(book) => book.queue_position(order_ref),
            Self::Aggregated(_) => Err(Error::unavailable(
                "an aggregated book does not track individual orders, so it cannot report a queue position",
            )),
        }
    }

    /// The order-by-order book, if this is one.
    pub fn as_order_by_order(&self) -> Option<&L3Book> {
        match self {
            Self::OrderByOrder(book) => Some(book),
            Self::Aggregated(_) => None,
        }
    }

    /// The aggregated book, if this is one.
    pub fn as_aggregated(&self) -> Option<&L2Book> {
        match self {
            Self::Aggregated(book) => Some(book),
            Self::OrderByOrder(_) => None,
        }
    }
}

impl BookView for Book {
    fn kind(&self) -> BookKind {
        match self {
            Self::OrderByOrder(_) => BookKind::OrderByOrder,
            Self::Aggregated(_) => BookKind::Aggregated,
        }
    }

    fn walk(&self, side: BookSide) -> LevelWalk<'_> {
        match self {
            Self::OrderByOrder(book) => book.walk(side),
            Self::Aggregated(book) => book.walk(side),
        }
    }

    fn size_at(&self, side: BookSide, price: Decimal) -> Decimal {
        match self {
            Self::OrderByOrder(book) => book.size_at(side, price),
            Self::Aggregated(book) => book.size_at(side, price),
        }
    }

    fn level_count(&self, side: BookSide) -> usize {
        match self {
            Self::OrderByOrder(book) => book.level_count(side),
            Self::Aggregated(book) => book.level_count(side),
        }
    }

    fn resting_orders(&self) -> usize {
        match self {
            Self::OrderByOrder(book) => book.resting_orders(),
            Self::Aggregated(book) => book.resting_orders(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::OrderByOrder(book) => book.is_empty(),
            Self::Aggregated(book) => book.is_empty(),
        }
    }
}
