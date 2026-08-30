//! The order-by-order book.
//!
//! Every resting order is tracked by the venue's reference, which is the whole
//! point: an aggregated feed can tell a strategy there are 4,000 shares at the
//! bid, and only this can tell it that 3,700 of them are in front of its own
//! order. Queue position is the difference between a passive fill rate that is
//! modelled and one that is guessed.
//!
//! Time priority is maintained by keeping each level's queue in arrival order
//! and appending to the back. The rule that matters is what a replace does to
//! it, because venues differ in what they *call* one and not in what it costs:
//!
//! * a new price, or more size than was resting — the order goes to the back
//!   of its new level, because it is a new order as far as the matching engine
//!   is concerned;
//! * less size at the same price — it holds its place, because shrinking never
//!   costs priority anywhere.
//!
//! Getting that backwards does not break anything visibly. It quietly makes
//! every queue-position estimate, and therefore every passive fill model, wrong
//! in the direction of optimism.
//!
//! Each level's queue is a `Vec` of order references rather than an intrusive
//! linked list. A list would make a cancellation from the middle `O(1)` instead
//! of `O(k)`, but it would not help the query this book exists to answer —
//! summing the size ahead of an order is `O(k)` either way — and it would trade
//! a contiguous scan of `u64`s for a pointer chase. The measured cost is in
//! `tests/throughput.rs`, taken on queues several hundred orders deep, which is
//! deeper than the venues this runs against. If a venue ever makes that the
//! bottleneck, the level is the place to change and nothing outside this module
//! needs to know.

use crate::ladder::Ladder;
use crate::snapshot::BookKind;
use crate::view::BookView;
use qip_contracts::{BookSide, MessageBody};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// An order resting on the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestingOrder {
    pub order_ref: u64,
    pub side: BookSide,
    pub price: Decimal,
    /// Quantity still resting, after any reduction.
    pub quantity: Decimal,
}

/// Where an order sits in its level's queue.
///
/// `ahead` is the answer a fill model needs: the size that must trade or leave
/// before this order is at the front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePosition {
    pub order_ref: u64,
    pub side: BookSide,
    pub price: Decimal,
    /// Size resting in front of this order at the same price.
    pub ahead: Decimal,
    /// This order's own remaining size.
    pub quantity: Decimal,
    /// Size resting behind it.
    pub behind: Decimal,
    /// Orders in front of it.
    pub orders_ahead: usize,
    /// Total size at the level.
    pub level_size: Decimal,
}

impl QueuePosition {
    /// Whether this order is next to fill at its price.
    pub fn is_at_front(&self) -> bool {
        !self.ahead.is_positive()
    }

    /// Fraction of the level that must clear before this order trades, in
    /// `[0, 1]`.
    ///
    /// A statistic, and `f64` for that reason. Zero when the level is empty,
    /// which cannot happen for a resting order but must not divide by zero if
    /// it does.
    pub fn queue_fraction(&self) -> f64 {
        if !self.level_size.is_positive() {
            return 0.0;
        }
        self.ahead.to_f64() / self.level_size.to_f64()
    }
}

/// A book that tracks every resting order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct L3Book {
    bids: Ladder,
    asks: Ladder,
    /// The order index. A `BTreeMap` rather than a hash map so that iteration —
    /// and therefore any snapshot or diagnostic built from it — is a function
    /// of the references alone and survives replay unchanged.
    orders: BTreeMap<u64, RestingOrder>,
}

impl Default for L3Book {
    fn default() -> Self {
        Self::new()
    }
}

impl L3Book {
    pub fn new() -> Self {
        Self {
            bids: Ladder::new(BookSide::Bid),
            asks: Ladder::new(BookSide::Ask),
            orders: BTreeMap::new(),
        }
    }

    /// Apply one decoded message.
    ///
    /// Messages that describe the venue rather than the book — trades, status,
    /// auction updates — are accepted and ignored here; [`crate::VenueState`]
    /// owns them. An aggregated update is refused rather than ignored: a
    /// `LevelSet` arriving on an order-by-order book means the feed is
    /// misconfigured, and silently dropping it would leave a book that looks
    /// healthy and is missing depth.
    pub fn apply(&mut self, body: &MessageBody) -> Result<()> {
        match body {
            MessageBody::OrderAdded {
                order_ref,
                side,
                price,
                quantity,
            } => self.add(*order_ref, *side, *price, *quantity),
            MessageBody::OrderReduced {
                order_ref,
                remaining,
            } => self.reduce(*order_ref, *remaining),
            MessageBody::OrderRemoved { order_ref } => self.remove(*order_ref),
            MessageBody::OrderReplaced {
                order_ref,
                price,
                quantity,
            } => self.replace(*order_ref, *price, *quantity),
            MessageBody::Reset { .. } => {
                self.clear();
                Ok(())
            }
            MessageBody::LevelSet { .. } | MessageBody::Quote { .. } => {
                Err(Error::invalid(format!(
                    "an order-by-order book cannot apply {}: the feed is publishing aggregated depth",
                    body.kind()
                )))
            }
            MessageBody::Trade { .. }
            | MessageBody::StatusChange { .. }
            | MessageBody::AuctionUpdate { .. } => Ok(()),
        }
    }

    /// Rest a new order at the back of its level's queue.
    pub fn add(
        &mut self,
        order_ref: u64,
        side: BookSide,
        price: Decimal,
        quantity: Decimal,
    ) -> Result<()> {
        if !quantity.is_positive() {
            return Err(Error::invalid(format!(
                "order {order_ref} was added with quantity {quantity}; a resting order has size"
            )));
        }
        if self.orders.contains_key(&order_ref) {
            return Err(Error::invalid(format!(
                "order reference {order_ref} is already resting; the feed reused a reference"
            )));
        }
        self.ladder_mut(side)
            .insert_order(price, order_ref, quantity);
        self.orders.insert(
            order_ref,
            RestingOrder {
                order_ref,
                side,
                price,
                quantity,
            },
        );
        Ok(())
    }

    /// Lower a resting order's quantity, keeping its place in the queue.
    ///
    /// A reduction that raises the quantity is refused: it is not a reduction,
    /// and honouring it would hand an order priority it never earned.
    pub fn reduce(&mut self, order_ref: u64, remaining: Decimal) -> Result<()> {
        let order = self.order(order_ref).copied().ok_or_else(|| {
            Error::not_found(format!("order reference {order_ref} is not resting"))
        })?;
        if remaining.is_negative() {
            return Err(Error::invalid(format!(
                "order {order_ref} was reduced to a negative remaining quantity {remaining}"
            )));
        }
        if remaining > order.quantity {
            return Err(Error::invalid(format!(
                "order {order_ref} was reduced from {} to {remaining}, which is an increase",
                order.quantity
            )));
        }
        if remaining.is_zero() {
            return self.remove(order_ref);
        }
        self.ladder_mut(order.side)
            .resize_level(order.price, remaining - order.quantity);
        if let Some(resting) = self.orders.get_mut(&order_ref) {
            resting.quantity = remaining;
        }
        Ok(())
    }

    /// Take an order off the book.
    pub fn remove(&mut self, order_ref: u64) -> Result<()> {
        let order = self.orders.remove(&order_ref).ok_or_else(|| {
            Error::not_found(format!("order reference {order_ref} is not resting"))
        })?;
        if !self
            .ladder_mut(order.side)
            .detach_order(order.price, order_ref, order.quantity)
        {
            return Err(Error::invalid(format!(
                "order {order_ref} was indexed at {} but is not in that level's queue",
                order.price
            )));
        }
        Ok(())
    }

    /// Change a resting order's price or quantity.
    ///
    /// Loses time priority when the price moves or the size grows; keeps it
    /// when the size only shrinks. See the module documentation for why that
    /// asymmetry is the one rule worth being pedantic about.
    pub fn replace(&mut self, order_ref: u64, price: Decimal, quantity: Decimal) -> Result<()> {
        let existing = self.order(order_ref).copied().ok_or_else(|| {
            Error::not_found(format!("order reference {order_ref} is not resting"))
        })?;
        if !quantity.is_positive() {
            return Err(Error::invalid(format!(
                "order {order_ref} was replaced with quantity {quantity}; use a removal to cancel"
            )));
        }

        let keeps_priority = price == existing.price && quantity <= existing.quantity;
        if keeps_priority {
            self.ladder_mut(existing.side)
                .resize_level(existing.price, quantity - existing.quantity);
        } else {
            if !self.ladder_mut(existing.side).detach_order(
                existing.price,
                order_ref,
                existing.quantity,
            ) {
                return Err(Error::invalid(format!(
                    "order {order_ref} was indexed at {} but is not in that level's queue",
                    existing.price
                )));
            }
            self.ladder_mut(existing.side)
                .insert_order(price, order_ref, quantity);
        }

        if let Some(resting) = self.orders.get_mut(&order_ref) {
            resting.price = price;
            resting.quantity = quantity;
        }
        Ok(())
    }

    /// Discard everything.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.orders.clear();
    }

    /// A resting order by reference.
    pub fn order(&self, order_ref: u64) -> Option<&RestingOrder> {
        self.orders.get(&order_ref)
    }

    /// Where an order sits in its level's queue.
    ///
    /// The single most valuable thing an L3 book gives you, and the reason to
    /// carry the cost of maintaining one.
    pub fn queue_position(&self, order_ref: u64) -> Result<QueuePosition> {
        let order = self.orders.get(&order_ref).ok_or_else(|| {
            Error::not_found(format!("order reference {order_ref} is not resting"))
        })?;
        let level = self
            .ladder(order.side)
            .level_at(order.price)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "order {order_ref} is indexed at {} where no level rests",
                    order.price
                ))
            })?;

        let mut ahead = Decimal::ZERO;
        let mut orders_ahead = 0usize;
        let mut found = false;
        for resting in &level.queue {
            if *resting == order_ref {
                found = true;
                break;
            }
            if let Some(other) = self.orders.get(resting) {
                ahead += other.quantity;
            }
            orders_ahead += 1;
        }
        if !found {
            return Err(Error::invalid(format!(
                "order {order_ref} is indexed at {} but absent from that level's queue",
                order.price
            )));
        }

        Ok(QueuePosition {
            order_ref,
            side: order.side,
            price: order.price,
            ahead,
            quantity: order.quantity,
            behind: level.size - ahead - order.quantity,
            orders_ahead,
            level_size: level.size,
        })
    }

    /// How much size is ahead of an order at its level.
    pub fn size_ahead_of(&self, order_ref: u64) -> Result<Decimal> {
        Ok(self.queue_position(order_ref)?.ahead)
    }

    /// Order references at a price, front of the queue first.
    ///
    /// Empty when no level rests there. Exposed because time priority is only
    /// testable if it is observable.
    pub fn queue_at(&self, side: BookSide, price: Decimal) -> Vec<u64> {
        self.ladder(side)
            .level_at(price)
            .map(|level| level.queue.clone())
            .unwrap_or_default()
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

impl BookView for L3Book {
    fn kind(&self) -> BookKind {
        BookKind::OrderByOrder
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

    fn resting_orders(&self) -> usize {
        self.orders.len()
    }

    fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }
}
