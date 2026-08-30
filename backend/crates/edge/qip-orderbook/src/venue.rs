//! Per-instrument venue state.
//!
//! The book is only part of what a consumer needs to know. The same levels mean
//! different things depending on whether the venue is open, whether an auction
//! is running, and — the one that matters most — whether this cell still
//! believes the book is right.
//!
//! That last distinction is the reason this type exists. A
//! [`MessageBody::Reset`] arrives when the sequencer has detected a gap: the
//! book is discarded and the state is left *awaiting a snapshot*, and until it
//! is resynchronised it serves no derived price at all. An empty book and a
//! book known to be wrong look identical from the levels alone, and trading off
//! the second is far worse than trading off neither. Everything a strategy
//! would size against — [`VenueState::mid`], [`VenueState::microprice`],
//! [`VenueState::sweep_cost`] — is gated on that flag. The raw book stays
//! reachable through [`VenueState::book`] for diagnostics and for the recovery
//! path that rebuilds it.
//!
//! Nothing here reads a clock. Every timestamp comes off the message being
//! applied, or is passed in by the caller, which is what lets a replay of a
//! captured session reproduce this state exactly.

use crate::auction::AuctionState;
use crate::book::Book;
use crate::snapshot::{BookKind, VenueSnapshot};
use crate::view::{BookCondition, BookView, Level, Sweep};
use qip_contracts::{BookSide, MarketMessage, MessageBody, TradeCondition, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};

/// The most recent print that counts as the last sale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastTrade {
    pub price: Decimal,
    pub quantity: Decimal,
    pub condition: TradeCondition,
    /// The aggressing side, where the venue discloses it.
    pub aggressor: Option<BookSide>,
    /// Venue time of the print.
    pub at: Timestamp,
}

/// One instrument at one venue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueState {
    object_id: ObjectId,
    venue: VenueId,
    status: VenueStatus,
    book: Book,
    auction: Option<AuctionState>,
    last_trade: Option<LastTrade>,
    session_volume: Decimal,
    session_notional: Decimal,
    trade_count: u64,
    awaiting_snapshot: bool,
    reset_reason: Option<String>,
    last_update: Option<Timestamp>,
    last_sequence: Option<u64>,
    applied: u64,
}

impl VenueState {
    /// A fresh state holding an empty book.
    ///
    /// Not awaiting a snapshot: an empty book we have never been told is wrong
    /// is simply empty, and it will serve no prices because it has no levels.
    /// The flag is reserved for a book we know to be wrong.
    pub fn new(object_id: ObjectId, venue: VenueId, book: Book, status: VenueStatus) -> Self {
        Self {
            object_id,
            venue,
            status,
            book,
            auction: None,
            last_trade: None,
            session_volume: Decimal::ZERO,
            session_notional: Decimal::ZERO,
            trade_count: 0,
            awaiting_snapshot: false,
            reset_reason: None,
            last_update: None,
            last_sequence: None,
            applied: 0,
        }
    }

    /// A fresh state for a venue publishing order-by-order depth.
    pub fn order_by_order(object_id: ObjectId, venue: VenueId, status: VenueStatus) -> Self {
        Self::new(object_id, venue, Book::order_by_order(), status)
    }

    /// A fresh state for a venue publishing aggregated depth.
    pub fn aggregated(object_id: ObjectId, venue: VenueId, status: VenueStatus) -> Self {
        Self::new(object_id, venue, Book::aggregated(), status)
    }

    /// Apply one decoded message.
    ///
    /// Book updates go to the book; trades, status changes and auction updates
    /// are this type's own. A message from another venue is refused rather than
    /// applied, because a mis-wired subscription that silently merges two
    /// venues' books produces a plausible book that is nobody's.
    ///
    /// Bookkeeping is only advanced once the update has been accepted, so a
    /// refused message leaves the state exactly as it was.
    pub fn apply(&mut self, message: &MarketMessage) -> Result<()> {
        if message.origin.venue != self.venue {
            return Err(Error::invalid(format!(
                "{} state was given a message from {}",
                self.venue, message.origin.venue
            )));
        }

        match &message.body {
            MessageBody::Trade {
                price,
                quantity,
                condition,
                aggressor,
            } => self.apply_trade(
                *price,
                *quantity,
                *condition,
                *aggressor,
                message.venue_time,
            )?,
            MessageBody::StatusChange { status } => self.apply_status(*status),
            MessageBody::AuctionUpdate {
                indicative_price,
                paired,
                imbalance,
                imbalance_side,
            } => {
                if imbalance.is_negative() || paired.is_negative() {
                    return Err(Error::invalid(format!(
                        "auction update for {} carries a negative quantity",
                        self.object_id
                    )));
                }
                self.auction = Some(AuctionState::new(
                    *indicative_price,
                    *paired,
                    *imbalance,
                    *imbalance_side,
                    message.venue_time,
                ));
            }
            MessageBody::Reset { reason } => self.reset(reason.clone()),
            body => self.book.apply(body)?,
        }

        self.last_update = Some(message.venue_time);
        self.last_sequence = Some(message.origin.sequence);
        self.applied += 1;
        Ok(())
    }

    /// Discard the book and mark it as awaiting a snapshot.
    ///
    /// Session volume and the last trade survive: a sequence gap says the
    /// book is unreliable, not that the trades that printed before it did not
    /// happen.
    pub fn reset(&mut self, reason: impl Into<String>) {
        self.book.clear();
        self.auction = None;
        self.awaiting_snapshot = true;
        self.reset_reason = Some(reason.into());
    }

    /// Declare the book rebuilt and safe to read again.
    ///
    /// Called by the feed handler once it has replayed a snapshot into this
    /// state. Deliberately explicit: nothing about an ordinary incremental
    /// update proves the book is whole again, so no incremental update clears
    /// the flag on its own.
    pub fn resynchronised(&mut self, at: Timestamp) {
        self.awaiting_snapshot = false;
        self.reset_reason = None;
        self.last_update = Some(at);
    }

    /// Start a new session: volume, notional and trade count return to zero.
    pub fn roll_session(&mut self, at: Timestamp) {
        self.session_volume = Decimal::ZERO;
        self.session_notional = Decimal::ZERO;
        self.trade_count = 0;
        self.last_update = Some(at);
    }

    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    pub fn venue(&self) -> &VenueId {
        &self.venue
    }

    pub fn status(&self) -> VenueStatus {
        self.status
    }

    /// The book, whatever its state. Reachable so a recovery path can rebuild
    /// it and an operator can look at what went wrong.
    pub fn book(&self) -> &Book {
        &self.book
    }

    /// The book, mutably, for the snapshot-replay path.
    pub fn book_mut(&mut self) -> &mut Book {
        &mut self.book
    }

    pub fn auction(&self) -> Option<&AuctionState> {
        self.auction.as_ref()
    }

    pub fn last_trade(&self) -> Option<&LastTrade> {
        self.last_trade.as_ref()
    }

    /// Traded volume this session, excluding prints that do not count toward it.
    pub fn session_volume(&self) -> Decimal {
        self.session_volume
    }

    /// Traded cash this session, on the same prints as [`Self::session_volume`].
    pub fn session_notional(&self) -> Decimal {
        self.session_notional
    }

    pub fn trade_count(&self) -> u64 {
        self.trade_count
    }

    /// Volume-weighted average price of the session's counted prints.
    pub fn session_vwap(&self) -> Option<Decimal> {
        self.session_notional.checked_div(self.session_volume)
    }

    /// Venue time of the last message applied.
    pub fn last_update(&self) -> Option<Timestamp> {
        self.last_update
    }

    /// The venue sequence of the last message applied.
    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    /// Messages applied since this state was created.
    pub fn applied(&self) -> u64 {
        self.applied
    }

    /// Whether the book is known to be wrong and has not been rebuilt.
    pub fn is_stale(&self) -> bool {
        self.awaiting_snapshot
    }

    /// Why the book was discarded, if it was.
    pub fn reset_reason(&self) -> Option<&str> {
        self.reset_reason.as_deref()
    }

    /// Whether the continuous book can be traded against right now.
    ///
    /// False during an auction even though the venue is accepting orders: what
    /// is resting is not what will match.
    pub fn continuous_trading(&self) -> bool {
        self.status == VenueStatus::Open && !self.awaiting_snapshot
    }

    /// Whether prices derived from this state may be used.
    pub fn prices_are_usable(&self) -> bool {
        !self.awaiting_snapshot && self.status.prices_are_usable()
    }

    /// The touch, when prices are usable.
    pub fn best_bid(&self) -> Option<Level> {
        if !self.prices_are_usable() {
            return None;
        }
        self.book.best_bid()
    }

    /// The touch, when prices are usable.
    pub fn best_ask(&self) -> Option<Level> {
        if !self.prices_are_usable() {
            return None;
        }
        self.book.best_ask()
    }

    /// The mid, or `None` when this state must not be priced off.
    pub fn mid(&self) -> Option<Decimal> {
        if !self.prices_are_usable() {
            return None;
        }
        self.book.mid()
    }

    /// The microprice, or `None` when this state must not be priced off.
    pub fn microprice(&self) -> Option<Decimal> {
        if !self.prices_are_usable() {
            return None;
        }
        self.book.microprice()
    }

    /// The spread, or `None` when this state must not be priced off.
    pub fn spread(&self) -> Option<Decimal> {
        if !self.prices_are_usable() {
            return None;
        }
        self.book.spread()
    }

    /// What taking `quantity` from `taking` would cost.
    ///
    /// `None` while the state is unusable, rather than a sweep of a book we do
    /// not trust: a router handed an empty sweep would read it as a liquidity
    /// shortage and try a different venue, which is nearly right by accident
    /// and completely wrong in reasoning.
    pub fn sweep_cost(&self, taking: BookSide, quantity: Decimal) -> Option<Sweep> {
        self.prices_are_usable()
            .then(|| self.book.sweep_cost(taking, quantity))
    }

    /// The state of the touch. Always reported, whatever the venue status —
    /// a crossed book is worth knowing about precisely when it is not tradeable.
    pub fn condition(&self) -> BookCondition {
        self.book.condition()
    }

    /// Which resolution the book is built from.
    pub fn kind(&self) -> BookKind {
        self.book.kind()
    }

    /// A comparable picture of the whole state.
    pub fn snapshot(&self) -> VenueSnapshot {
        VenueSnapshot {
            status: self.status,
            awaiting_snapshot: self.awaiting_snapshot,
            book: self.book.snapshot(),
            session_volume: self.session_volume,
            session_notional: self.session_notional,
            trade_count: self.trade_count,
            last_trade: self.last_trade,
            auction: self.auction,
            applied: self.applied,
        }
    }

    fn apply_trade(
        &mut self,
        price: Decimal,
        quantity: Decimal,
        condition: TradeCondition,
        aggressor: Option<BookSide>,
        at: Timestamp,
    ) -> Result<()> {
        if quantity.is_negative() {
            return Err(Error::invalid(format!(
                "a trade printed on {} with a negative quantity {quantity}",
                self.object_id
            )));
        }
        if condition.counts_toward_volume() {
            self.session_volume += quantity;
            self.session_notional += price * quantity;
            self.trade_count += 1;
        }
        if condition.updates_last() {
            self.last_trade = Some(LastTrade {
                price,
                quantity,
                condition,
                aggressor,
                at,
            });
        }
        Ok(())
    }

    fn apply_status(&mut self, status: VenueStatus) {
        self.status = status;
        // An indicative price outlives its auction only as a trap: once the
        // venue has moved on, the last one published is a number that looks
        // current and describes an uncross that already happened.
        if status != VenueStatus::Auction {
            self.auction = None;
        }
    }
}
