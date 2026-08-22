//! The read surface every book exposes.
//!
//! A consumer of market state should not have to know whether the venue
//! publishes order-by-order or aggregated depth. Both answer the same
//! questions here; the L3 book simply also answers [`crate::L3Book::queue_position`],
//! which no aggregated feed can.
//!
//! Two decisions in this module are worth stating outright, because both trade
//! convenience for safety:
//!
//! * [`BookView::sweep_cost`] returns what the book *has*, never what the
//!   caller asked for. Its result carries the filled quantity next to the
//!   price, so an undersized book shows up as a shortfall rather than as an
//!   attractive average.
//! * [`BookView::mid`] and [`BookView::microprice`] return `None` on a crossed
//!   book. The arithmetic would succeed — that is the problem. A mid computed
//!   from an inverted touch is a plausible-looking number that no strategy
//!   should size against, so the condition is surfaced through
//!   [`BookView::condition`] and [`BookView::crossed_by`] and the derived price
//!   is withheld.

use crate::ladder::LevelWalk;
use crate::snapshot::{BookKind, BookSnapshot};
use qip_contracts::BookSide;
use qip_core::Decimal;
use serde::{Deserialize, Serialize};

/// One aggregated price level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Level {
    pub price: Decimal,
    pub size: Decimal,
    /// Resting orders at this level. Zero where the venue does not publish a
    /// count — an empty level does not exist, so zero is never a real count.
    pub order_count: u32,
}

impl Level {
    pub fn new(price: Decimal, size: Decimal, order_count: u32) -> Self {
        Self {
            price,
            size,
            order_count,
        }
    }

    /// Cash value resting at this level.
    pub fn notional(&self) -> Decimal {
        self.price * self.size
    }
}

/// The relationship between the two sides of the touch.
///
/// Reported rather than resolved. A crossed book at a lit venue is usually a
/// sequencing error; across venues it is the arbitrage the platform exists to
/// find. This type cannot tell those apart, and neither should it try.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BookCondition {
    /// Both sides present, bid strictly below ask.
    Normal,
    /// Bid equals ask. Legal on several venues and common at the open; no
    /// spread to earn, but nothing is inconsistent.
    Locked,
    /// Bid above ask. Either a data error or free money, and the difference
    /// matters enough that the book refuses to decide.
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

    /// Whether the touch is internally consistent.
    ///
    /// False only for [`Self::Crossed`]: a locked book is unusual but not
    /// wrong, and an absent side is missing data rather than bad data.
    pub const fn is_consistent(&self) -> bool {
        !matches!(self, Self::Crossed)
    }
}

/// What taking liquidity would actually get you.
///
/// The filled quantity is reported next to the price because the two are only
/// meaningful together: an average price for a fill the book cannot supply is
/// the single most dangerous number an order book can hand out. `filled` is
/// never larger than the size the book is showing, and a walk that would
/// overflow the fixed-point range stops early rather than inventing depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sweep {
    /// The side of the book that would be consumed: `Ask` for a buy, `Bid` for
    /// a sell.
    pub side: BookSide,
    /// What the caller asked for.
    pub requested: Decimal,
    /// What the book can actually supply.
    pub filled: Decimal,
    /// Cash paid or received for `filled`.
    pub notional: Decimal,
    /// How many levels the fill would reach into.
    pub levels_consumed: usize,
    /// The last and worst price that would print.
    pub worst_price: Option<Decimal>,
}

impl Sweep {
    /// Volume-weighted price of the fill, or `None` when nothing could fill.
    ///
    /// `None` rather than zero: a zero price reads as free, and somewhere
    /// downstream it would be multiplied by a quantity.
    pub fn average_price(&self) -> Option<Decimal> {
        self.notional.checked_div(self.filled)
    }

    /// Whether the book could supply the whole requested quantity.
    pub fn is_complete(&self) -> bool {
        self.filled >= self.requested
    }

    /// Quantity the book could not supply.
    pub fn shortfall(&self) -> Decimal {
        (self.requested - self.filled).max(Decimal::ZERO)
    }

    /// Cost of the sweep against a reference price, in basis points, signed so
    /// that positive is always worse for the taker.
    ///
    /// `None` when nothing filled or the reference is not positive, so a caller
    /// deducting this from an edge estimate cannot silently deduct zero.
    pub fn slippage_bps(&self, reference: Decimal) -> Option<f64> {
        if !reference.is_positive() {
            return None;
        }
        let average = self.average_price()?;
        let signed = match self.side {
            BookSide::Ask => average - reference,
            BookSide::Bid => reference - average,
        };
        Some(signed.to_f64() / reference.to_f64() * 10_000.0)
    }
}

/// Everything a consumer can ask of a book, whatever its depth resolution.
///
/// Implementors supply the four primitives; every derived read is written once
/// here so an L2 book and an L3 book cannot drift apart on what "depth to a
/// price" or "the cost of a sweep" means.
pub trait BookView {
    /// Which resolution this book was built from.
    fn kind(&self) -> BookKind;

    /// Levels on one side, from the touch outward.
    fn walk(&self, side: BookSide) -> LevelWalk<'_>;

    /// Size resting at exactly `price`. Zero when no such level exists.
    fn size_at(&self, side: BookSide, price: Decimal) -> Decimal;

    /// Number of distinct price levels on a side.
    fn level_count(&self, side: BookSide) -> usize;

    /// Resting orders tracked individually. Always zero for aggregated books,
    /// which know sizes but not orders.
    fn resting_orders(&self) -> usize;

    /// The best bid, if there is one.
    fn best_bid(&self) -> Option<Level> {
        self.walk(BookSide::Bid).next()
    }

    /// The best ask, if there is one.
    fn best_ask(&self) -> Option<Level> {
        self.walk(BookSide::Ask).next()
    }

    /// The `n` levels nearest the touch on one side.
    fn levels(&self, side: BookSide, n: usize) -> Vec<Level> {
        self.walk(side).take(n).collect()
    }

    /// Total size resting at prices at least as good as `price`, inclusive.
    fn depth_to(&self, side: BookSide, price: Decimal) -> Decimal {
        self.walk(side)
            .take_while(|level| level.price == price || side.is_better(level.price, price))
            .map(|level| level.size)
            .sum()
    }

    /// Total size resting on a side.
    fn total_size(&self, side: BookSide) -> Decimal {
        self.walk(side).map(|level| level.size).sum()
    }

    /// Whether either side holds anything.
    fn is_empty(&self) -> bool {
        self.level_count(BookSide::Bid) == 0 && self.level_count(BookSide::Ask) == 0
    }

    /// The state of the touch.
    fn condition(&self) -> BookCondition {
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

    fn is_crossed(&self) -> bool {
        self.condition() == BookCondition::Crossed
    }

    fn is_locked(&self) -> bool {
        self.condition() == BookCondition::Locked
    }

    /// How far the bid is through the ask, when the book is crossed.
    ///
    /// The one place a negative spread is available, and it is positive here
    /// and named for what it is, so no caller reaches it by accident.
    fn crossed_by(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        (bid.price > ask.price).then(|| bid.price - ask.price)
    }

    /// Ask less bid, `None` unless the touch is consistent.
    ///
    /// A crossed book has no spread worth reporting: the number would be
    /// negative, and a negative spread flows straight into a width filter that
    /// will read it as the tightest market of the day.
    fn spread(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        (ask.price >= bid.price).then(|| ask.price - bid.price)
    }

    /// Spread as a fraction of the mid, in basis points.
    fn spread_bps(&self) -> Option<f64> {
        let mid = self.mid()?;
        if !mid.is_positive() {
            return None;
        }
        Some(self.spread()?.to_f64() / mid.to_f64() * 10_000.0)
    }

    /// The arithmetic mid, or `None` when the touch is incomplete or crossed.
    fn mid(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        if bid.price > ask.price {
            return None;
        }
        (bid.price + ask.price).checked_div(Decimal::from_int(2))
    }

    /// Size-weighted mid: each side's price weighted by the *other* side's
    /// size.
    ///
    /// The better short-horizon predictor of where the next trade prints, and
    /// no more expensive than the mid. It sits between the two touch prices by
    /// construction and coincides with the mid when the sizes match. Falls back
    /// to the mid when the touch shows no size, which happens on venues that
    /// publish a price with an undisclosed quantity.
    fn microprice(&self) -> Option<Decimal> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        if bid.price > ask.price {
            return None;
        }
        let total = bid.size + ask.size;
        if !total.is_positive() {
            return self.mid();
        }
        (bid.price * ask.size + ask.price * bid.size).checked_div(total)
    }

    /// Walk the book to fill `quantity`.
    ///
    /// `taking` is the side being consumed — `Ask` to buy, `Bid` to sell. The
    /// result reports the quantity actually available alongside the price, and
    /// it stops at the last published level: this feeds the arbitrage engine's
    /// slippage deduction, and an optimistic answer there turns a losing trade
    /// into a signal.
    fn sweep_cost(&self, taking: BookSide, quantity: Decimal) -> Sweep {
        let requested = quantity.max(Decimal::ZERO);
        let mut remaining = requested;
        let mut filled = Decimal::ZERO;
        let mut notional = Decimal::ZERO;
        let mut levels_consumed = 0usize;
        let mut worst_price = None;

        for level in self.walk(taking) {
            if !remaining.is_positive() {
                break;
            }
            let take = level.size.min(remaining);
            if !take.is_positive() {
                continue;
            }
            // Overflow ends the walk rather than saturating: a saturated
            // notional is indistinguishable from a real one, and stopping can
            // only ever understate available liquidity.
            let Some(cost) = take.checked_mul(level.price) else {
                break;
            };
            let (Some(next_notional), Some(next_filled)) =
                (notional.checked_add(cost), filled.checked_add(take))
            else {
                break;
            };
            notional = next_notional;
            filled = next_filled;
            remaining -= take;
            levels_consumed += 1;
            worst_price = Some(level.price);
        }

        Sweep {
            side: taking,
            requested,
            filled,
            notional,
            levels_consumed,
            worst_price,
        }
    }

    /// A comparable picture of the book, to `depth` levels a side.
    fn snapshot_to(&self, depth: usize) -> BookSnapshot {
        BookSnapshot {
            kind: self.kind(),
            bids: self.levels(BookSide::Bid, depth),
            asks: self.levels(BookSide::Ask, depth),
            resting_orders: self.resting_orders(),
        }
    }

    /// A comparable picture of the whole book.
    fn snapshot(&self) -> BookSnapshot {
        self.snapshot_to(usize::MAX)
    }
}
