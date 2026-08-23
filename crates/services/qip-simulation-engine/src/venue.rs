//! The simulated venue: a book with price-time priority, and marks that admit
//! how old they are.
//!
//! The rule this module exists to enforce is that **the simulator is never
//! more generous than reality**. A fill model that fills at the touch price
//! whatever the size makes every strategy look better than it is, and the
//! error is invisible in the equity curve because there is nothing in it to
//! compare against. So:
//!
//! * [`SimBook::sweep`] walks the levels and reports the quantity the book can
//!   actually supply next to the price. It stops at the last published level
//!   and never extrapolates. Its shape and its semantics are those of
//!   [`qip_market::book::OrderBook::sweep`] — the book the rest of the
//!   platform prices against — and the agreement is asserted in the tests
//!   rather than asserted in a comment.
//! * [`SimBook::take`] consumes resting orders in **price-time priority**:
//!   best price first, and within a price the order that arrived first. An
//!   order joining a level does not jump the queue, which is what makes a
//!   partial fill land where it actually would.
//! * A crossed book is reported, never repaired. [`SimBook::mid`] withholds a
//!   number computed from an inverted touch, [`SimBook::crossed_by`] says how
//!   far through it is, and a taker crossing it is filled at the *worse* of
//!   the two touch prices. Where the simulator cannot tell a data error from
//!   an arbitrage, it takes the reading that costs the strategy money.
//!
//! [`Mark`] carries the same discipline for prices rather than fills. A mark
//! knows the instant it was observed on the feed and the instant it is being
//! read at; when those differ it is a *last known* value and
//! [`Mark::current_price`] returns nothing at all. A delayed feed that
//! presented its last price as the current one would be indistinguishable from
//! a working feed right up to the point where it mattered.

use crate::conditions::FeedFault;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_market::book::{BookLevel, OrderBook, Side};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The relationship between the two sides of the touch.
///
/// Deliberately the same vocabulary the edge's own book uses, because a
/// simulated condition a strategy learns to handle has to be the condition it
/// will meet in production, under the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookCondition {
    /// Both sides present, bid strictly below ask.
    Normal,
    /// Bid equals ask. Legal on several venues; no spread to earn, nothing
    /// inconsistent.
    Locked,
    /// Bid above ask. Either a data error or free money, and the simulator
    /// refuses to decide which.
    Crossed,
    /// Liquidity on one side only.
    OneSided,
    /// No liquidity at all.
    Empty,
}

impl BookCondition {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Locked => "locked",
            Self::Crossed => "crossed",
            Self::OneSided => "one_sided",
            Self::Empty => "empty",
        }
    }

    /// Whether a two-sided price can be derived at all.
    pub const fn has_two_sides(&self) -> bool {
        matches!(self, Self::Normal | Self::Locked | Self::Crossed)
    }

    /// Whether the touch is internally consistent. False only when crossed.
    pub const fn is_consistent(&self) -> bool {
        !matches!(self, Self::Crossed)
    }
}

/// A resting order, identified so a consumption can be attributed to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestingOrder {
    pub id: u64,
    pub quantity: Decimal,
    /// When it joined the queue. Ties are impossible because ids are assigned
    /// in arrival order, so priority is total.
    pub entered_at: Timestamp,
}

/// One aggregated price level of a simulated book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimLevel {
    pub price: Decimal,
    pub size: Decimal,
    pub order_count: u32,
}

impl SimLevel {
    pub fn notional(&self) -> Decimal {
        self.price * self.size
    }
}

/// A resting order that a sweep consumed, in the order it was consumed.
///
/// Present so a test can assert time priority directly rather than infer it
/// from an average price that would look the same either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedOrder {
    pub order_id: u64,
    pub price: Decimal,
    pub quantity: Decimal,
    pub entered_at: Timestamp,
}

/// What taking liquidity would actually get.
///
/// `filled` sits next to `requested` because the two are only meaningful
/// together: an average price for a fill the book cannot supply is the single
/// most dangerous number a simulator can hand out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepOutcome {
    /// The aggressor's side. A buy consumes asks; a sell consumes bids.
    pub side: Side,
    pub requested: Decimal,
    pub filled: Decimal,
    pub notional: Decimal,
    pub levels_consumed: usize,
    /// The last and worst price that would print.
    pub worst_price: Option<Decimal>,
    /// Resting orders consumed, front of the queue first.
    pub consumed: Vec<ConsumedOrder>,
}

impl SweepOutcome {
    /// An empty result, for a book that could supply nothing.
    pub fn nothing(side: Side, requested: Decimal) -> Self {
        Self {
            side,
            requested,
            filled: Decimal::ZERO,
            notional: Decimal::ZERO,
            levels_consumed: 0,
            worst_price: None,
            consumed: Vec::new(),
        }
    }

    /// Volume-weighted price, or `None` when nothing filled.
    ///
    /// `None` rather than zero: a zero price reads as free, and somewhere
    /// downstream it would be multiplied by a quantity.
    pub fn average_price(&self) -> Option<Decimal> {
        self.notional.checked_div(self.filled)
    }

    /// Exactly what is left unfilled. Decimal throughout, so a chain of
    /// partial fills leaves a residual that closes to the last unit.
    pub fn residual(&self) -> Decimal {
        (self.requested - self.filled).max(Decimal::ZERO)
    }

    pub fn is_complete(&self) -> bool {
        self.filled >= self.requested
    }

    /// Cost against a reference price in basis points, signed so positive is
    /// always worse for the taker.
    pub fn slippage_bps(&self, reference: Decimal) -> Option<f64> {
        if !reference.is_positive() {
            return None;
        }
        let average = self.average_price()?;
        let signed = match self.side {
            Side::Buy => average - reference,
            Side::Sell => reference - average,
        };
        Some(signed.to_f64() / reference.to_f64() * 10_000.0)
    }
}

/// A simulated limit order book with price-time priority.
///
/// Levels are held in a `BTreeMap` keyed by price and each level holds its
/// queue in arrival order. A `BTreeMap` rather than a hash map for the reason
/// replay always needs one: iteration order is a function of the prices alone,
/// where `RandomState` draws from the operating system and would make two runs
/// of the same seed disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimBook {
    object_id: ObjectId,
    venue: String,
    at: Timestamp,
    bids: BTreeMap<Decimal, Vec<RestingOrder>>,
    asks: BTreeMap<Decimal, Vec<RestingOrder>>,
    next_order_id: u64,
}

impl SimBook {
    /// An empty book.
    pub fn new(object_id: ObjectId, venue: impl Into<String>, at: Timestamp) -> Self {
        Self {
            object_id,
            venue: venue.into(),
            at,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            next_order_id: 1,
        }
    }

    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    pub fn venue(&self) -> &str {
        &self.venue
    }

    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// Rest an order at a price, joining the back of that level's queue.
    ///
    /// Returns the order's id. Joining the back rather than anywhere else is
    /// the whole of time priority: a simulator that let a new order fill first
    /// would report queue-sensitive strategies as working.
    pub fn rest(
        &mut self,
        side: Side,
        price: Decimal,
        quantity: Decimal,
        at: Timestamp,
    ) -> Result<u64> {
        if !price.is_positive() {
            return Err(Error::invalid(format!(
                "cannot rest an order for {} at a non-positive price {price}",
                self.object_id
            )));
        }
        if !quantity.is_positive() {
            return Err(Error::invalid(format!(
                "cannot rest an order for {} with a non-positive quantity {quantity}",
                self.object_id
            )));
        }
        let id = self.next_order_id;
        self.next_order_id += 1;
        let ladder = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        ladder.entry(price).or_default().push(RestingOrder {
            id,
            quantity,
            entered_at: at,
        });
        if at > self.at {
            self.at = at;
        }
        Ok(id)
    }

    /// Remove a resting order, returning the quantity that was withdrawn.
    pub fn cancel(&mut self, side: Side, price: Decimal, order_id: u64) -> Option<Decimal> {
        let ladder = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = ladder.get_mut(&price)?;
        let index = level.iter().position(|order| order.id == order_id)?;
        let removed = level.remove(index);
        if level.is_empty() {
            ladder.remove(&price);
        }
        Some(removed.quantity)
    }

    /// Total size resting ahead of an order at its own level.
    ///
    /// The question an aggregated feed cannot answer and a queue-sensitive
    /// strategy lives or dies on. `None` when the order is not there.
    pub fn queue_ahead(&self, side: Side, price: Decimal, order_id: u64) -> Option<Decimal> {
        let level = self.ladder(side).get(&price)?;
        let mut ahead = Decimal::ZERO;
        for order in level {
            if order.id == order_id {
                return Some(ahead);
            }
            ahead += order.quantity;
        }
        None
    }

    /// Levels on one side, touch first.
    pub fn levels(&self, side: Side) -> Vec<SimLevel> {
        let ladder = self.ladder(side);
        let mut levels: Vec<SimLevel> = ladder
            .iter()
            .map(|(price, queue)| SimLevel {
                price: *price,
                size: queue.iter().map(|order| order.quantity).sum(),
                order_count: queue.len() as u32,
            })
            .collect();
        // Bids are best-first descending, asks best-first ascending. The map
        // yields ascending, so only the bids reverse.
        if side == Side::Buy {
            levels.reverse();
        }
        levels
    }

    pub fn best_bid(&self) -> Option<SimLevel> {
        self.levels(Side::Buy).into_iter().next()
    }

    pub fn best_ask(&self) -> Option<SimLevel> {
        self.levels(Side::Sell).into_iter().next()
    }

    /// Total size resting on one side.
    pub fn depth(&self, side: Side) -> Decimal {
        self.ladder(side)
            .values()
            .flat_map(|queue| queue.iter())
            .map(|order| order.quantity)
            .sum()
    }

    /// Number of price levels on one side.
    pub fn level_count(&self, side: Side) -> usize {
        self.ladder(side).len()
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// The state of the touch.
    pub fn condition(&self) -> BookCondition {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => match bid.price.cmp(&ask.price) {
                std::cmp::Ordering::Less => BookCondition::Normal,
                std::cmp::Ordering::Equal => BookCondition::Locked,
                std::cmp::Ordering::Greater => BookCondition::Crossed,
            },
            (None, None) => BookCondition::Empty,
            _ => BookCondition::OneSided,
        }
    }

    pub fn is_crossed(&self) -> bool {
        self.condition() == BookCondition::Crossed
    }

    /// How far the bid is through the ask, when the book is crossed.
    ///
    /// The one place a negative spread is reachable, and it is positive here
    /// and named for what it is, so nothing arrives at it by accident.
    pub fn crossed_by(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        (bid.price > ask.price).then(|| bid.price - ask.price)
    }

    /// Ask less bid, withheld unless the touch is consistent.
    pub fn spread(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        (ask.price >= bid.price).then(|| ask.price - bid.price)
    }

    /// The arithmetic mid, or `None` when the touch is incomplete or crossed.
    ///
    /// The arithmetic on a crossed book would succeed — that is the problem. A
    /// mid computed from an inverted touch is a plausible-looking number no
    /// strategy should size against.
    pub fn mid(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        if bid.price > ask.price {
            return None;
        }
        (bid.price + ask.price).checked_div(Decimal::from_int(2))
    }

    /// The arithmetic midpoint of the two touch prices, whatever their order.
    ///
    /// A *measurement anchor*, never a tradeable price — [`Self::mid`] is the
    /// tradeable one and it withholds a number on a crossed book. This exists
    /// because execution quality has to be measured against something that
    /// does not itself move when a condition is injected: the simulated cross
    /// is built symmetrically about the true mid, so this stays put while the
    /// touch inverts around it, and the extra cost a crossed market imposes
    /// shows up as extra slippage rather than as a shifted yardstick.
    pub fn touch_midpoint(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        (bid.price + ask.price).checked_div(Decimal::from_int(2))
    }

    /// The price a taker crossing this book should be assumed to get at the
    /// touch, including when the touch is inverted.
    ///
    /// On a normal book this is the far touch. On a crossed book it is the
    /// *worse* of the two, because a crossed market is as likely to be a bad
    /// print as an arbitrage, and a simulator that took the good side would
    /// hand every strategy free money in exactly the conditions where a real
    /// one loses it.
    pub fn taker_touch(&self, side: Side) -> Option<Decimal> {
        let bid = self.best_bid();
        let ask = self.best_ask();
        match side {
            Side::Buy => {
                let ask_price = ask?.price;
                Some(match bid {
                    Some(bid) if bid.price > ask_price => bid.price,
                    _ => ask_price,
                })
            }
            Side::Sell => {
                let bid_price = bid?.price;
                Some(match ask {
                    Some(ask) if ask.price < bid_price => ask.price,
                    _ => bid_price,
                })
            }
        }
    }

    /// Walk the book to fill `quantity` without consuming it.
    ///
    /// Stops at the last published level. An overflowing multiplication ends
    /// the walk rather than saturating, because a saturated notional is
    /// indistinguishable from a real one and stopping can only ever understate
    /// the liquidity available.
    pub fn sweep(&self, side: Side, quantity: Decimal) -> SweepOutcome {
        let requested = quantity.max(Decimal::ZERO);
        let mut outcome = SweepOutcome::nothing(side, requested);
        let mut remaining = requested;
        // Buying lifts offers; selling hits bids.
        for level in self.levels(side.opposite()) {
            if !remaining.is_positive() {
                break;
            }
            let Some(queue) = self.ladder(side.opposite()).get(&level.price) else {
                continue;
            };
            let mut consumed_here = false;
            for order in queue {
                if !remaining.is_positive() {
                    break;
                }
                let take = order.quantity.min(remaining);
                if !take.is_positive() {
                    continue;
                }
                let Some(cost) = take.checked_mul(level.price) else {
                    return outcome;
                };
                let (Some(notional), Some(filled)) = (
                    outcome.notional.checked_add(cost),
                    outcome.filled.checked_add(take),
                ) else {
                    return outcome;
                };
                outcome.notional = notional;
                outcome.filled = filled;
                remaining -= take;
                outcome.worst_price = Some(level.price);
                outcome.consumed.push(ConsumedOrder {
                    order_id: order.id,
                    price: level.price,
                    quantity: take,
                    entered_at: order.entered_at,
                });
                consumed_here = true;
            }
            if consumed_here {
                outcome.levels_consumed += 1;
            }
        }
        outcome
    }

    /// Fill `quantity` against the book, removing what was consumed.
    ///
    /// The result is exactly what [`Self::sweep`] would have reported, so a
    /// caller can size against a sweep and know the fill it gets is the fill
    /// it was quoted.
    pub fn take(&mut self, side: Side, quantity: Decimal, at: Timestamp) -> SweepOutcome {
        let outcome = self.sweep(side, quantity);
        let ladder = match side.opposite() {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        for consumption in &outcome.consumed {
            let Some(queue) = ladder.get_mut(&consumption.price) else {
                continue;
            };
            if let Some(index) = queue.iter().position(|o| o.id == consumption.order_id) {
                queue[index].quantity -= consumption.quantity;
                if !queue[index].quantity.is_positive() {
                    queue.remove(index);
                }
            }
            if queue.is_empty() {
                ladder.remove(&consumption.price);
            }
        }
        if at > self.at {
            self.at = at;
        }
        outcome
    }

    /// The same levels as the depth snapshot the rest of the platform passes
    /// around.
    ///
    /// Exists so the simulator's sweep can be checked against
    /// [`qip_market::book::OrderBook::sweep`] on identical input rather than
    /// asserted to agree with it in a comment.
    pub fn to_order_book(&self) -> OrderBook {
        let mut book = OrderBook::new(self.object_id.clone(), self.venue.clone(), self.at);
        book.bids = self
            .levels(Side::Buy)
            .into_iter()
            .map(|level| BookLevel {
                price: level.price,
                size: level.size,
                order_count: level.order_count,
            })
            .collect();
        book.asks = self
            .levels(Side::Sell)
            .into_iter()
            .map(|level| BookLevel {
                price: level.price,
                size: level.size,
                order_count: level.order_count,
            })
            .collect();
        book
    }

    fn ladder(&self, side: Side) -> &BTreeMap<Decimal, Vec<RestingOrder>> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }
}

/// Where a mark's price came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkSource {
    /// The mid of a two-sided book.
    Book,
    /// One side of the book only, because the other was not quoted.
    OneSidedBook,
    /// Nothing usable was published.
    Unavailable,
}

/// A price observation, with the age it actually has.
///
/// The distinction between [`Self::current_price`] and
/// [`Self::last_known_price`] is the whole type. A delayed feed still has a
/// price and a strategy may reasonably want it — as a last known value, marked
/// as such. What it must not be able to do is read it as the current one,
/// because that is precisely the mistake a delayed feed causes in production
/// and the one a simulator has to be able to reproduce.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub object_id: String,
    pub venue: String,
    /// The price, whatever its age. `None` when nothing was published at all.
    pub price: Option<Decimal>,
    /// The instant the price was true on the venue.
    pub as_of: Timestamp,
    /// The simulation instant the mark is being read at.
    pub observed_at: Timestamp,
    pub source: MarkSource,
    pub condition: BookCondition,
    /// How far the bid is through the ask, when the book is crossed. Carried
    /// on the mark so a strategy trading on a crossed market can see that it
    /// was crossed.
    pub crossed_by: Option<Decimal>,
    /// Faults on the feed at this instant.
    pub faults: Vec<FeedFault>,
}

impl Mark {
    /// A mark for an instrument with no usable observation.
    pub fn unavailable(
        object_id: impl Into<String>,
        venue: impl Into<String>,
        observed_at: Timestamp,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            venue: venue.into(),
            price: None,
            as_of: observed_at,
            observed_at,
            source: MarkSource::Unavailable,
            condition: BookCondition::Empty,
            crossed_by: None,
            faults: Vec::new(),
        }
    }

    /// How far behind the simulation instant the observation is.
    pub fn staleness(&self) -> Duration {
        if self.observed_at <= self.as_of {
            Duration::ZERO
        } else {
            self.observed_at.since(self.as_of)
        }
    }

    /// Whether this mark describes a market that has moved on.
    pub fn is_stale(&self) -> bool {
        !self.staleness().is_zero() || !self.faults.is_empty()
    }

    /// Whether the touch was inverted when this mark was taken.
    pub fn is_crossed(&self) -> bool {
        self.condition == BookCondition::Crossed
    }

    /// The price, if it is genuinely current.
    ///
    /// `None` for anything stale or faulted. A caller that wants the number
    /// anyway must ask for [`Self::last_known_price`] and thereby say in its
    /// own code that it knows what it is holding.
    pub fn current_price(&self) -> Option<Decimal> {
        if self.is_stale() {
            return None;
        }
        self.price
    }

    /// The most recent price, whatever its age.
    pub fn last_known_price(&self) -> Option<Decimal> {
        self.price
    }

    pub fn describe(&self) -> String {
        let price = match self.price {
            Some(price) => price.to_string(),
            None => "no price".to_string(),
        };
        if !self.is_stale() {
            return format!("{} {price} at {}", self.object_id, self.venue);
        }
        let faults: Vec<&str> = self.faults.iter().map(FeedFault::as_str).collect();
        format!(
            "{} {price} at {} is a LAST KNOWN value {:?} old{}",
            self.object_id,
            self.venue,
            self.staleness(),
            if faults.is_empty() {
                String::new()
            } else {
                format!(" with feed faults: {}", faults.join(", "))
            }
        )
    }
}

/// Whether a venue is answering, and why not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VenueHealth {
    /// Answering normally.
    Responding,
    /// Not answering. An order sent into this is neither filled nor rejected.
    Unreachable { since: Timestamp },
}

impl VenueHealth {
    pub const fn is_responding(&self) -> bool {
        matches!(self, Self::Responding)
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Responding => "responding".to_string(),
            Self::Unreachable { since } => format!(
                "unreachable since {since:?}: an order in flight is neither filled nor rejected, and the residual is still the caller's"
            ),
        }
    }
}
