//! The aggregated book.
//!
//! Price levels with a total size and, where the venue publishes one, an order
//! count. Most feeds outside the US equity ITCH family give you only this, and
//! most of them publish it sparsely: a level arrives when it changes, in no
//! particular order, with no promise that the levels between two updates were
//! ever mentioned. Holding levels in a map keyed by price rather than a fixed
//! depth array is what makes that a non-event — there is no "slot 3" to keep
//! consistent, and a level nobody has mentioned simply is not there.
//!
//! A size of zero removes a level. Venues repeat those deletes, including for
//! levels that were never populated, so a delete of an absent level is not an
//! error.

use crate::ladder::Ladder;
use crate::snapshot::BookKind;
use crate::view::BookView;
use qip_contracts::{BookSide, MessageBody};
use qip_core::Decimal;
use qip_core::error::{Error, Result};

/// A book of aggregated price levels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L2Book {
    bids: Ladder,
    asks: Ladder,
}

impl Default for L2Book {
    fn default() -> Self {
        Self::new()
    }
}

impl L2Book {
    pub fn new() -> Self {
        Self {
            bids: Ladder::new(BookSide::Bid),
            asks: Ladder::new(BookSide::Ask),
        }
    }

    /// Apply one decoded message.
    ///
    /// Order-by-order messages are refused: an aggregated book has nowhere to
    /// put an order reference, and pretending an add is a level set would
    /// double-count every subsequent update to it.
    pub fn apply(&mut self, body: &MessageBody) -> Result<()> {
        match body {
            MessageBody::LevelSet {
                side,
                price,
                quantity,
                order_count,
            } => self.set_level(*side, *price, *quantity, *order_count),
            MessageBody::Quote { bid, ask } => self.apply_quote(*bid, *ask),
            MessageBody::Reset { .. } => {
                self.clear();
                Ok(())
            }
            MessageBody::OrderAdded { .. }
            | MessageBody::OrderReduced { .. }
            | MessageBody::OrderRemoved { .. }
            | MessageBody::OrderReplaced { .. } => Err(Error::invalid(format!(
                "an aggregated book cannot apply {}: the feed is publishing order-by-order depth",
                body.kind()
            ))),
            MessageBody::Trade { .. }
            | MessageBody::StatusChange { .. }
            | MessageBody::AuctionUpdate { .. } => Ok(()),
        }
    }

    /// Set a level's total size. Zero removes it.
    pub fn set_level(
        &mut self,
        side: BookSide,
        price: Decimal,
        quantity: Decimal,
        order_count: Option<u32>,
    ) -> Result<()> {
        if quantity.is_negative() {
            return Err(Error::invalid(format!(
                "level {price} on the {} was set to a negative size {quantity}",
                side.as_str()
            )));
        }
        // An unpublished order count is recorded as zero rather than guessed at
        // one: a caller that sizes against order counts must be able to see
        // that the venue did not give it one.
        self.ladder_mut(side)
            .set_level(price, quantity, order_count.unwrap_or(0));
        Ok(())
    }

    /// Apply a top-of-book quote.
    ///
    /// A quote is a statement that this *is* the touch, so any level more
    /// aggressive than it is stale and is dropped — otherwise a level that was
    /// never explicitly deleted sits in front of the real touch indefinitely.
    /// A side quoted as `None` is a statement that the side is empty, and it is
    /// cleared for the same reason.
    pub fn apply_quote(
        &mut self,
        bid: Option<(Decimal, Decimal)>,
        ask: Option<(Decimal, Decimal)>,
    ) -> Result<()> {
        for (side, quote) in [(BookSide::Bid, bid), (BookSide::Ask, ask)] {
            match quote {
                Some((price, size)) => {
                    if size.is_negative() {
                        return Err(Error::invalid(format!(
                            "the {} was quoted with a negative size {size}",
                            side.as_str()
                        )));
                    }
                    let ladder = self.ladder_mut(side);
                    ladder.prune_better_than(price);
                    ladder.set_level(price, size, 0);
                }
                None => self.ladder_mut(side).clear(),
            }
        }
        Ok(())
    }

    /// Discard everything.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
    }

    fn ladder(&self, side: BookSide) -> &Ladder {
        match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        }
    }

    fn ladder_mut(&mut self, side: BookSide) -> &mut Ladder {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }
}

impl BookView for L2Book {
    fn kind(&self) -> BookKind {
        BookKind::Aggregated
    }

    fn walk(&self, side: BookSide) -> crate::ladder::LevelWalk<'_> {
        self.ladder(side).walk()
    }

    fn size_at(&self, side: BookSide, price: Decimal) -> Decimal {
        self.ladder(side).size_at(price)
    }

    fn level_count(&self, side: BookSide) -> usize {
        self.ladder(side).level_count()
    }

    /// Always zero: an aggregated feed knows sizes, not orders.
    fn resting_orders(&self) -> usize {
        0
    }

    fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }
}
