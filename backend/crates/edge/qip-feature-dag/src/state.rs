//! The observed market state the feature definitions read.
//!
//! Features cannot be computed from a message in isolation: a realised
//! volatility needs a price history, an order-flow imbalance needs the previous
//! touch. This module holds exactly the history the shipped definitions need
//! and nothing else, in fixed-capacity buffers, so the memory a cell uses is
//! decided at construction rather than by how long the session runs.
//!
//! Price observations are sampled onto a fixed grid rather than stored per
//! update. Two instruments quoted at unrelated moments have no common index to
//! correlate on otherwise, and pairing the *n*th update of one with the *n*th
//! of the other is a correlation between two different stretches of the day.

use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook};
use std::collections::BTreeMap;

/// Observations kept per instrument, per series.
pub const DEFAULT_HISTORY: usize = 512;
/// Grid that price observations are sampled onto.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
/// How long an instrument's state stays usable without an update.
pub const DEFAULT_MAX_STALENESS: Duration = Duration::from_secs(30);

/// Which part of an instrument's state a feature reads, and which parts a
/// message changes.
///
/// The intersection of the two decides whether a message dirties a node. It is
/// the whole reason a print on a busy instrument does not recompute its
/// volatility surface: a trade changes the trade series and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketReads(u8);

impl MarketReads {
    /// Reads nothing from the market — a feature computed purely from its
    /// dependencies.
    pub const NONE: Self = Self(0);
    /// The top of book: best bid and ask, their sizes, and anything derived
    /// from them.
    pub const TOUCH: Self = Self(1);
    /// Aggregated depth away from the touch.
    pub const DEPTH: Self = Self(2);
    /// The trade print series.
    pub const TRADES: Self = Self(4);
    /// The venue's trading state for the instrument.
    pub const STATUS: Self = Self(8);
    /// Every aspect — what a book reset invalidates.
    pub const ALL: Self = Self(0b1111);

    /// Union of two read sets.
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the two sets overlap at all.
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether this set names any aspect.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// What applying this message can change.
    ///
    /// The touch bit comes from [`MessageBody::may_move_touch`] rather than
    /// from a second opinion held here. One predicate, stated once, in the
    /// contract both sides read.
    pub fn of_message(body: &MessageBody) -> Self {
        if matches!(body, MessageBody::Reset { .. }) {
            return Self::ALL;
        }
        let touch = if body.may_move_touch() {
            Self::TOUCH
        } else {
            Self::NONE
        };
        let specific = match body {
            MessageBody::OrderAdded { .. }
            | MessageBody::OrderReduced { .. }
            | MessageBody::OrderRemoved { .. }
            | MessageBody::OrderReplaced { .. }
            | MessageBody::LevelSet { .. }
            | MessageBody::Quote { .. } => Self::DEPTH,
            MessageBody::Trade { .. } => Self::TRADES,
            MessageBody::StatusChange { .. } | MessageBody::AuctionUpdate { .. } => Self::STATUS,
            MessageBody::Reset { .. } => Self::ALL,
        };
        touch.with(specific)
    }
}

/// One trade print, with the side that initiated it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradePrint {
    pub price: Decimal,
    pub quantity: Decimal,
    /// +1 buyer-initiated, -1 seller-initiated, 0 where neither could be
    /// established. Zero is kept rather than guessed: a print at the mid with
    /// no prior tick genuinely has no sign.
    pub sign: i8,
    pub at: Timestamp,
}

/// A resting order, for venues that publish order by order.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RestingOrder {
    side: BookSide,
    price: Decimal,
    quantity: Decimal,
}

/// The touch, remembered so the next one can be compared against it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Touch {
    bid_price: Decimal,
    bid_size: Decimal,
    ask_price: Decimal,
    ask_size: Decimal,
}

/// Everything observed about one instrument.
#[derive(Clone, Debug)]
pub struct InstrumentState {
    book: OrderBook,
    orders: BTreeMap<u64, RestingOrder>,
    status: VenueStatus,
    /// `(grid bucket, mid)` — at most one entry per bucket, latest wins.
    samples: Vec<(i64, Decimal)>,
    spreads: Vec<Decimal>,
    /// Order-flow imbalance increments, one per touch change.
    flow: Vec<f64>,
    trades: Vec<TradePrint>,
    last_trade_at: Option<Timestamp>,
    last_touch: Option<Touch>,
    /// Sign of the last signed print, for the tick rule.
    last_sign: i8,
    updated_at: Timestamp,
    history: usize,
    sample_interval: Duration,
}

impl InstrumentState {
    fn new(
        object_id: ObjectId,
        venue: &str,
        at: Timestamp,
        history: usize,
        sample_interval: Duration,
    ) -> Self {
        Self {
            book: OrderBook::new(object_id, venue, at),
            orders: BTreeMap::new(),
            status: VenueStatus::Open,
            samples: Vec::new(),
            spreads: Vec::new(),
            flow: Vec::new(),
            trades: Vec::new(),
            last_trade_at: None,
            last_touch: None,
            last_sign: 0,
            updated_at: at,
            history: history.max(2),
            sample_interval,
        }
    }

    /// The current aggregated book.
    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    /// The venue's trading state for this instrument.
    pub const fn status(&self) -> VenueStatus {
        self.status
    }

    /// Sampled mid history as `(bucket, mid)`, oldest first.
    pub fn samples(&self) -> &[(i64, Decimal)] {
        &self.samples
    }

    /// Observed spreads, one per touch change, oldest first.
    pub fn spreads(&self) -> &[Decimal] {
        &self.spreads
    }

    /// Order-flow imbalance increments, oldest first.
    pub fn flow(&self) -> &[f64] {
        &self.flow
    }

    /// Trade prints that counted toward volume, oldest first.
    pub fn trades(&self) -> &[TradePrint] {
        &self.trades
    }

    /// When the last print that updates the last sale arrived.
    pub const fn last_trade_at(&self) -> Option<Timestamp> {
        self.last_trade_at
    }

    /// When any message last touched this instrument.
    pub const fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    /// The grid the mid samples are taken on.
    pub const fn sample_interval(&self) -> Duration {
        self.sample_interval
    }

    /// The last `count` sampled mids as statistics, oldest first.
    ///
    /// Empty when there are fewer than `count` samples: a caller asking for a
    /// window is asking for a full window, and a short one silently returned
    /// would be a volatility measured over the wrong horizon.
    pub fn recent_mids(&self, count: usize) -> Vec<f64> {
        if count == 0 || self.samples.len() < count {
            return Vec::new();
        }
        self.samples[self.samples.len() - count..]
            .iter()
            .map(|(_, mid)| mid.to_f64())
            .collect()
    }

    /// Whether the state has gone unrefreshed for longer than `limit`.
    pub fn is_stale(&self, as_of: Timestamp, limit: Duration) -> bool {
        as_of.since(self.updated_at) > limit
    }

    fn apply(&mut self, message: &MarketMessage) -> Result<()> {
        let at = message.venue_time;
        self.updated_at = at;
        self.book.at = at;
        self.book.sequence = message.origin.sequence;

        match &message.body {
            MessageBody::OrderAdded {
                order_ref,
                side,
                price,
                quantity,
            } => {
                self.remove_order(*order_ref)?;
                self.orders.insert(
                    *order_ref,
                    RestingOrder {
                        side: *side,
                        price: *price,
                        quantity: *quantity,
                    },
                );
                self.adjust_level(*side, *price, *quantity)?;
            }
            MessageBody::OrderReduced {
                order_ref,
                remaining,
            } => {
                if let Some(order) = self.orders.get(order_ref).copied() {
                    let delta = remaining
                        .checked_sub(order.quantity)
                        .ok_or_else(|| Error::numeric("order reduction overflowed"))?;
                    self.adjust_level(order.side, order.price, delta)?;
                    if remaining.is_positive() {
                        if let Some(slot) = self.orders.get_mut(order_ref) {
                            slot.quantity = *remaining;
                        }
                    } else {
                        self.orders.remove(order_ref);
                    }
                }
            }
            MessageBody::OrderRemoved { order_ref } => self.remove_order(*order_ref)?,
            MessageBody::OrderReplaced {
                order_ref,
                price,
                quantity,
            } => {
                let side = self.orders.get(order_ref).map(|o| o.side);
                self.remove_order(*order_ref)?;
                // A replace keeps the order's identity, so its side is the one
                // it already had; without a prior add there is nothing to
                // replace and the message is dropped rather than guessed at.
                if let Some(side) = side {
                    self.orders.insert(
                        *order_ref,
                        RestingOrder {
                            side,
                            price: *price,
                            quantity: *quantity,
                        },
                    );
                    self.adjust_level(side, *price, *quantity)?;
                }
            }
            MessageBody::LevelSet {
                side,
                price,
                quantity,
                order_count,
            } => self.set_level(*side, *price, *quantity, order_count.unwrap_or(1)),
            MessageBody::Quote { bid, ask } => {
                // A quote-only feed publishes the touch and nothing behind it,
                // so the touch is the whole book rather than its first level.
                self.orders.clear();
                self.book.bids.clear();
                self.book.asks.clear();
                if let Some((price, size)) = bid {
                    self.set_level(BookSide::Bid, *price, *size, 1);
                }
                if let Some((price, size)) = ask {
                    self.set_level(BookSide::Ask, *price, *size, 1);
                }
            }
            MessageBody::Trade {
                price,
                quantity,
                condition,
                aggressor,
            } => {
                self.record_trade(*price, *quantity, *condition, *aggressor, at);
                return Ok(());
            }
            MessageBody::StatusChange { status } => {
                self.status = *status;
                return Ok(());
            }
            MessageBody::AuctionUpdate { .. } => return Ok(()),
            MessageBody::Reset { .. } => {
                // The book is known to be wrong; every series derived from it
                // is too. Keeping the history would carry the error forward
                // into features that report themselves as healthy.
                self.orders.clear();
                self.book.bids.clear();
                self.book.asks.clear();
                self.samples.clear();
                self.spreads.clear();
                self.flow.clear();
                self.trades.clear();
                self.last_touch = None;
                self.last_trade_at = None;
                self.last_sign = 0;
                return Ok(());
            }
        }

        self.refresh_touch(at);
        Ok(())
    }

    fn remove_order(&mut self, order_ref: u64) -> Result<()> {
        if let Some(order) = self.orders.remove(&order_ref) {
            let delta = Decimal::ZERO
                .checked_sub(order.quantity)
                .ok_or_else(|| Error::numeric("order removal overflowed"))?;
            self.adjust_level(order.side, order.price, delta)?;
        }
        Ok(())
    }

    fn adjust_level(&mut self, side: BookSide, price: Decimal, delta: Decimal) -> Result<()> {
        let levels = self.levels_mut(side);
        let existing = match Self::locate(levels, side, price) {
            Ok(pos) => levels[pos].size,
            Err(_) => Decimal::ZERO,
        };
        let updated = existing
            .checked_add(delta)
            .ok_or_else(|| Error::numeric("aggregated level size overflowed"))?;
        self.set_level(side, price, updated.max(Decimal::ZERO), 1);
        Ok(())
    }

    fn set_level(&mut self, side: BookSide, price: Decimal, size: Decimal, order_count: u32) {
        let levels = self.levels_mut(side);
        match Self::locate(levels, side, price) {
            Ok(pos) => {
                if size.is_positive() {
                    levels[pos].size = size;
                    levels[pos].order_count = order_count;
                } else {
                    levels.remove(pos);
                }
            }
            Err(pos) => {
                if size.is_positive() {
                    levels.insert(
                        pos,
                        BookLevel {
                            price,
                            size,
                            order_count,
                        },
                    );
                }
            }
        }
    }

    fn levels_mut(&mut self, side: BookSide) -> &mut Vec<BookLevel> {
        match side {
            BookSide::Bid => &mut self.book.bids,
            BookSide::Ask => &mut self.book.asks,
        }
    }

    /// Where `price` sits in a side kept in book order — bids descending,
    /// asks ascending.
    fn locate(
        levels: &[BookLevel],
        side: BookSide,
        price: Decimal,
    ) -> std::result::Result<usize, usize> {
        match side {
            BookSide::Bid => levels.binary_search_by(|level| price.cmp(&level.price)),
            BookSide::Ask => levels.binary_search_by(|level| level.price.cmp(&price)),
        }
    }

    /// Record what changed at the touch, and sample the mid onto the grid.
    fn refresh_touch(&mut self, at: Timestamp) {
        let (Some(bid), Some(ask)) = (self.book.best_bid(), self.book.best_ask()) else {
            self.last_touch = None;
            return;
        };
        let touch = Touch {
            bid_price: bid.price,
            bid_size: bid.size,
            ask_price: ask.price,
            ask_size: ask.size,
        };
        if let Some(previous) = self.last_touch
            && previous != touch
        {
            let increment = Self::flow_increment(previous, touch);
            push_capped(&mut self.flow, increment, self.history);
        }
        self.last_touch = Some(touch);

        if let Some(spread) = self.book.spread() {
            push_capped(&mut self.spreads, spread, self.history);
        }
        if let Some(mid) = self.book.mid() {
            // The sample series stays strictly ascending in bucket. A message
            // that arrives out of venue order refreshes the book but does not
            // rewrite history, so two instruments' series stay alignable.
            let bucket = bucket_of(at, self.sample_interval);
            match self.samples.last_mut() {
                Some(slot) if slot.0 == bucket => slot.1 = mid,
                Some(slot) if slot.0 > bucket => {}
                _ => push_capped(&mut self.samples, (bucket, mid), self.history),
            }
        }
    }

    /// The Cont-Kukanov-Stoikov order-flow increment between two touches.
    ///
    /// Size added at an unchanged price counts as flow; size that left with the
    /// price counts against it. Measuring flow as the change in touch size
    /// alone would read a price improvement of one lot as a collapse in depth.
    fn flow_increment(previous: Touch, current: Touch) -> f64 {
        let bid = match current.bid_price.cmp(&previous.bid_price) {
            std::cmp::Ordering::Greater => current.bid_size.to_f64(),
            std::cmp::Ordering::Equal => (current.bid_size - previous.bid_size).to_f64(),
            std::cmp::Ordering::Less => -previous.bid_size.to_f64(),
        };
        let ask = match current.ask_price.cmp(&previous.ask_price) {
            std::cmp::Ordering::Less => current.ask_size.to_f64(),
            std::cmp::Ordering::Equal => (current.ask_size - previous.ask_size).to_f64(),
            std::cmp::Ordering::Greater => -previous.ask_size.to_f64(),
        };
        bid - ask
    }

    fn record_trade(
        &mut self,
        price: Decimal,
        quantity: Decimal,
        condition: TradeCondition,
        aggressor: Option<BookSide>,
        at: Timestamp,
    ) {
        if condition.updates_last() {
            self.last_trade_at = Some(at);
        }
        if !condition.counts_toward_volume() {
            return;
        }
        let sign = match aggressor {
            Some(BookSide::Bid) => 1,
            Some(BookSide::Ask) => -1,
            // Lee-Ready: classify against the mid, and fall back to the last
            // established sign for a print that lands exactly on it.
            None => match self.book.mid() {
                Some(mid) if price > mid => 1,
                Some(mid) if price < mid => -1,
                _ => self.last_sign,
            },
        };
        if sign != 0 {
            self.last_sign = sign;
        }
        push_capped(
            &mut self.trades,
            TradePrint {
                price,
                quantity,
                sign,
                at,
            },
            self.history,
        );
    }
}

/// Every instrument this cell observes.
#[derive(Clone, Debug)]
pub struct MarketState {
    instruments: BTreeMap<String, InstrumentState>,
    history: usize,
    sample_interval: Duration,
}

impl Default for MarketState {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY, DEFAULT_SAMPLE_INTERVAL)
    }
}

impl MarketState {
    /// Keep `history` observations per series, sampling mids onto a grid of
    /// `sample_interval`.
    pub fn new(history: usize, sample_interval: Duration) -> Self {
        Self {
            instruments: BTreeMap::new(),
            history: history.max(2),
            sample_interval: if sample_interval.as_nanos() > 0 {
                sample_interval
            } else {
                DEFAULT_SAMPLE_INTERVAL
            },
        }
    }

    /// Fold one message into the state.
    pub fn apply(&mut self, message: &MarketMessage) -> Result<()> {
        let history = self.history;
        let interval = self.sample_interval;
        let entry = self
            .instruments
            .entry(message.object_id.as_str().to_string())
            .or_insert_with(|| {
                InstrumentState::new(
                    message.object_id.clone(),
                    message.origin.venue.as_str(),
                    message.venue_time,
                    history,
                    interval,
                )
            });
        entry.apply(message)
    }

    /// What is known about one instrument.
    pub fn instrument(&self, object_id: &ObjectId) -> Option<&InstrumentState> {
        self.instruments.get(object_id.as_str())
    }

    /// The same, by the identifier's string form.
    ///
    /// The dirty-marking path holds subjects as strings so it can compare them
    /// against every arriving message without allocating.
    pub fn instrument_named(&self, object_id: &str) -> Option<&InstrumentState> {
        self.instruments.get(object_id)
    }

    /// The grid mids are sampled onto.
    pub const fn sample_interval(&self) -> Duration {
        self.sample_interval
    }

    /// How many instruments have been observed.
    pub fn len(&self) -> usize {
        self.instruments.len()
    }

    /// Whether anything has been observed at all.
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }
}

/// Which grid bucket a timestamp falls in.
fn bucket_of(at: Timestamp, interval: Duration) -> i64 {
    let nanos = interval.as_nanos().max(1);
    at.as_nanos().div_euclid(nanos)
}

/// Append, dropping the oldest observation once the buffer is full.
fn push_capped<T>(buffer: &mut Vec<T>, value: T, capacity: usize) {
    buffer.push(value);
    if buffer.len() > capacity {
        buffer.remove(0);
    }
}
