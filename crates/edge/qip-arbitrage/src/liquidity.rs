//! Where an executable price comes from.
//!
//! The order book lives in another cell and this crate must not reach for it:
//! `qip-orderbook` is being written concurrently and nothing here may depend on
//! it. [`LiquiditySource`] is the seam. It asks the only question an arbitrage
//! decision needs answered — what would this size actually cost — and leaves
//! the answer to whoever owns the depth. The edge cell substitutes the real
//! book at assembly; [`StaticLiquidity`] stands in until then.
//!
//! Every method returns the quantity that was available alongside the price. A
//! source that reported only a price would let a path be priced at a size the
//! book cannot fill, which is the exact failure this crate exists to prevent.

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_market::book::{BookLevel, OrderBook, Side};
use std::collections::BTreeMap;
use std::fmt;

/// Map a book side to the aggressing side that consumes it.
///
/// Resting bids are consumed by a seller and resting offers by a buyer. The
/// mapping is written once, here, because getting it backwards prices every
/// path at the wrong sign and still produces plausible-looking numbers.
const fn aggressor_for(side: BookSide) -> Side {
    match side {
        BookSide::Bid => Side::Sell,
        BookSide::Ask => Side::Buy,
    }
}

/// Depth an arbitrage path can be priced against.
///
/// Throughout, `side` names the side of the book being *consumed*:
/// [`BookSide::Bid`] means selling into resting bids, [`BookSide::Ask`] means
/// buying from resting offers.
pub trait LiquiditySource: fmt::Debug {
    /// Volume-weighted price, and the quantity actually available, for a sweep
    /// of `quantity`.
    ///
    /// `None` means this source has no depth for the pair at all. That is a
    /// different answer from `Some((price, less_than_asked))`, which means the
    /// book exists and is too thin — the first is ignorance, the second is a
    /// fact about the market, and a caller must be able to tell them apart.
    fn sweep_cost(
        &self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        quantity: Decimal,
    ) -> Option<(Decimal, Decimal)>;

    /// Best price on a side, and the size resting at it.
    ///
    /// Kept separate from [`LiquiditySource::sweep_cost`] because crossing to
    /// the touch and walking past it are different costs charged to different
    /// people: everyone pays the spread, only size pays the slippage. Reporting
    /// them as one number hides which of the two a path is actually losing to.
    fn touch(&self, venue: &VenueId, object: &ObjectId, side: BookSide)
    -> Option<(Decimal, Decimal)>;

    /// Mid price — the reference the spread deduction is measured from.
    fn mid(&self, venue: &VenueId, object: &ObjectId) -> Option<Decimal> {
        let (bid, _) = self.touch(venue, object, BookSide::Bid)?;
        let (ask, _) = self.touch(venue, object, BookSide::Ask)?;
        (bid + ask).checked_div(Decimal::from_int(2))
    }

    /// When this depth was observed.
    ///
    /// Not the time the caller is reasoning at. The gap between the two is what
    /// the uncertainty haircut is priced from, so a source that cannot say when
    /// it last looked cannot be traded on.
    fn as_of(&self, venue: &VenueId, object: &ObjectId) -> Option<Timestamp>;

    /// How many independent observations back the depth on offer.
    ///
    /// One snapshot of a book is one observation, however many levels it has.
    /// A single observation is not evidence of a repeatable price, and the
    /// uncertainty haircut prices it accordingly.
    fn observations(&self, venue: &VenueId, object: &ObjectId) -> u32;
}

/// One venue's depth in one instrument, with its provenance.
#[derive(Clone, Debug, PartialEq)]
struct DepthRecord {
    book: OrderBook,
    observations: u32,
}

/// Depth held in memory, for tests and for the replay harness.
///
/// Backed by [`qip_market::book::OrderBook`] rather than a private level
/// representation, so the sweep arithmetic under test is the same arithmetic
/// the rest of the platform uses. A second implementation of "walk the levels"
/// would be a second place for the two to disagree.
#[derive(Clone, Debug, Default)]
pub struct StaticLiquidity {
    depth: BTreeMap<VenueId, BTreeMap<ObjectId, DepthRecord>>,
}

impl StaticLiquidity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a book for a venue, replacing whatever was there.
    pub fn insert_book(&mut self, venue: VenueId, book: OrderBook, observations: u32) {
        self.depth
            .entry(venue)
            .or_default()
            .insert(book.object_id.clone(), DepthRecord { book, observations });
    }

    /// Builder form of [`StaticLiquidity::insert_book`].
    pub fn with_book(mut self, venue: VenueId, book: OrderBook, observations: u32) -> Self {
        self.insert_book(venue, book, observations);
        self
    }

    /// Record a single-level book: the shape most fixtures need.
    #[allow(clippy::too_many_arguments)]
    pub fn with_quote(
        self,
        venue: VenueId,
        object: ObjectId,
        at: Timestamp,
        bid: Decimal,
        bid_size: Decimal,
        ask: Decimal,
        ask_size: Decimal,
        observations: u32,
    ) -> Self {
        let book = OrderBook::from_levels(
            object,
            venue.as_str(),
            at,
            vec![BookLevel::new(bid, bid_size)],
            vec![BookLevel::new(ask, ask_size)],
        );
        self.with_book(venue, book, observations)
    }

    fn record(&self, venue: &VenueId, object: &ObjectId) -> Option<&DepthRecord> {
        self.depth.get(venue)?.get(object)
    }
}

impl LiquiditySource for StaticLiquidity {
    fn sweep_cost(
        &self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        quantity: Decimal,
    ) -> Option<(Decimal, Decimal)> {
        self.record(venue, object)?
            .book
            .sweep(aggressor_for(side), quantity)
    }

    fn touch(
        &self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
    ) -> Option<(Decimal, Decimal)> {
        let book = &self.record(venue, object)?.book;
        let level = match side {
            BookSide::Bid => book.best_bid()?,
            BookSide::Ask => book.best_ask()?,
        };
        Some((level.price, level.size))
    }

    fn as_of(&self, venue: &VenueId, object: &ObjectId) -> Option<Timestamp> {
        Some(self.record(venue, object)?.book.at)
    }

    fn observations(&self, venue: &VenueId, object: &ObjectId) -> u32 {
        self.record(venue, object)
            .map_or(0, |record| record.observations)
    }
}
