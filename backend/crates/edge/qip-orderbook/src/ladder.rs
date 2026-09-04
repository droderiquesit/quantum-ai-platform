//! One side of a book, held in price order.
//!
//! Both book flavours rest on this. An aggregated book is an order-by-order
//! book whose queues happen to be empty, so sharing the structure means the
//! touch, the walk, `depth_to` and `sweep_cost` are written once instead of
//! twice with two chances to disagree about — say — whether the level at the
//! requested price is included.
//!
//! A `BTreeMap` keyed by price, rather than a sorted vector: the touch is
//! `O(1)` at either end, an insert away from the touch does not memmove the
//! book, and iteration order is a function of the prices alone. That last point
//! is what a hash map would cost us; replay has to produce the same walk every
//! time, and `RandomState` draws from the operating system.

use crate::view::Level;
use qip_contracts::BookSide;
use qip_core::Decimal;
use std::collections::BTreeMap;
use std::collections::btree_map::Iter as PriceIter;

/// A resting price level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LadderLevel {
    /// Total resting size. Cached rather than summed from `queue` so the touch
    /// is readable without walking every order at it.
    pub(crate) size: Decimal,
    /// Resting orders, where they are known. Zero means the venue does not
    /// publish a count, which is not the same as an empty level — an empty
    /// level is removed.
    pub(crate) order_count: u32,
    /// Order references in time priority, front of the queue first. Always
    /// empty on an aggregated book.
    pub(crate) queue: Vec<u64>,
}

impl LadderLevel {
    fn to_level(&self, price: Decimal) -> Level {
        Level {
            price,
            size: self.size,
            order_count: self.order_count,
        }
    }
}

/// The price levels resting on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ladder {
    side: BookSide,
    levels: BTreeMap<Decimal, LadderLevel>,
}

impl Ladder {
    pub(crate) fn new(side: BookSide) -> Self {
        Self {
            side,
            levels: BTreeMap::new(),
        }
    }

    /// Levels from the touch outward.
    pub(crate) fn walk(&self) -> LevelWalk<'_> {
        let inner = match self.side {
            BookSide::Bid => WalkInner::Descending(self.levels.iter().rev()),
            BookSide::Ask => WalkInner::Ascending(self.levels.iter()),
        };
        LevelWalk { inner }
    }

    pub(crate) fn size_at(&self, price: Decimal) -> Decimal {
        self.levels.get(&price).map_or(Decimal::ZERO, |l| l.size)
    }

    pub(crate) fn level_at(&self, price: Decimal) -> Option<&LadderLevel> {
        self.levels.get(&price)
    }

    pub(crate) fn level_count(&self) -> usize {
        self.levels.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.levels.clear();
    }

    /// Set an aggregated level. A size of zero removes it.
    pub(crate) fn set_level(&mut self, price: Decimal, size: Decimal, order_count: u32) {
        if !size.is_positive() {
            self.levels.remove(&price);
            return;
        }
        let level = self.levels.entry(price).or_default();
        level.size = size;
        level.order_count = order_count;
    }

    /// Drop every level more aggressive than `price`.
    ///
    /// Used when a venue publishes only a quote: the quote *is* the touch, so
    /// anything better than it is stale by construction and would otherwise sit
    /// in front of the real touch forever.
    pub(crate) fn prune_better_than(&mut self, price: Decimal) {
        let side = self.side;
        self.levels
            .retain(|resting, _| !side.is_better(*resting, price));
    }

    /// Append an order to the back of its level's queue.
    pub(crate) fn insert_order(&mut self, price: Decimal, order_ref: u64, quantity: Decimal) {
        let level = self.levels.entry(price).or_default();
        level.queue.push(order_ref);
        level.size += quantity;
        level.order_count = level.queue.len() as u32;
    }

    /// Take an order out of its level, preserving the order of everything else.
    ///
    /// Returns false when the ladder does not hold it, which means the order
    /// index and the ladder have diverged and the caller must say so rather
    /// than continue against a book it can no longer explain.
    pub(crate) fn detach_order(
        &mut self,
        price: Decimal,
        order_ref: u64,
        quantity: Decimal,
    ) -> bool {
        let Some(level) = self.levels.get_mut(&price) else {
            return false;
        };
        let Some(index) = level.queue.iter().position(|r| *r == order_ref) else {
            return false;
        };
        level.queue.remove(index);
        level.size -= quantity;
        level.order_count = level.queue.len() as u32;
        if level.queue.is_empty() {
            self.levels.remove(&price);
        }
        true
    }

    /// Change a level's size without touching its queue, for the one case that
    /// keeps time priority: a resting order shrinking in place.
    ///
    /// Returns false when no level rests at `price`, which means the order
    /// index and the ladder have already diverged. Silently doing nothing
    /// here would leave the caller's order record showing a new quantity
    /// while the level total it is supposed to be part of never moved — the
    /// aggregate size would then understate what the index believes is
    /// resting, and nothing downstream could tell the book was wrong. The
    /// caller must say so, the same way [`Self::detach_order`] already does.
    pub(crate) fn resize_level(&mut self, price: Decimal, delta: Decimal) -> bool {
        let Some(level) = self.levels.get_mut(&price) else {
            return false;
        };
        level.size += delta;
        true
    }
}

/// Price levels from the touch outward.
///
/// Borrows the book, so a consumer can size against depth without copying it.
#[derive(Debug)]
pub struct LevelWalk<'a> {
    inner: WalkInner<'a>,
}

#[derive(Debug)]
enum WalkInner<'a> {
    /// Asks, cheapest first.
    Ascending(PriceIter<'a, Decimal, LadderLevel>),
    /// Bids, dearest first.
    Descending(std::iter::Rev<PriceIter<'a, Decimal, LadderLevel>>),
}

impl Iterator for LevelWalk<'_> {
    type Item = Level;

    fn next(&mut self) -> Option<Level> {
        let (price, level) = match &mut self.inner {
            WalkInner::Ascending(iter) => iter.next()?,
            WalkInner::Descending(iter) => iter.next()?,
        };
        Some(level.to_level(*price))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    /// `resize_level` must report a level it cannot find rather than quietly
    /// doing nothing to it.
    ///
    /// `L3Book::reduce` and `L3Book::replace` trust this return value to
    /// decide whether the order index and the ladder still agree; if it
    /// silently succeeded on an absent level, a future divergence between the
    /// two would leave a caller's order marked with a new quantity while no
    /// level total ever moved to match it — the exact "looks healthy, is
    /// missing depth" failure this crate's module documentation warns against
    /// for `LevelSet` on the wrong book kind.
    #[test]
    fn resizing_a_level_that_does_not_rest_is_reported_rather_than_ignored() {
        let mut ladder = Ladder::new(BookSide::Bid);
        // The premise: no level rests at this price yet.
        assert!(ladder.level_at(dec!("100.00")).is_none());

        let resized = ladder.resize_level(dec!("100.00"), dec!("10"));

        assert!(!resized, "resizing an absent level must report failure");
        assert!(
            ladder.level_at(dec!("100.00")).is_none(),
            "an absent level must not spring into existence by being resized"
        );
    }

    /// The ordinary case: resizing a level that does rest succeeds and the
    /// change lands on its size.
    #[test]
    fn resizing_a_level_that_rests_changes_its_size_and_reports_success() {
        let mut ladder = Ladder::new(BookSide::Bid);
        ladder.insert_order(dec!("100.00"), 1, dec!("50"));

        let resized = ladder.resize_level(dec!("100.00"), dec!("-20"));

        assert!(resized, "resizing a level that rests must report success");
        assert_eq!(ladder.size_at(dec!("100.00")), dec!("30"));
    }
}
