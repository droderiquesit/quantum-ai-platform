//! Order conduct: a place-and-cancel pattern refused before it leaves the
//! platform — at the cell's send seam, and since R12-GOV-6126 at the desk's
//! order manager, the one seam a desk order reaches a broker through. The
//! module moved here from `qip-edge` so the two seams run one piece of
//! arithmetic; `qip_edge::conduct` re-exports it.
//!
//! Spoofing and layering are an order pattern, not an order: each resting
//! order a spoofer places is individually unremarkable, and what marks the
//! pattern is that the orders are withdrawn before anything trades against
//! them. Every per-order gate in this crate — risk, pricing, self-trade,
//! budget — admits each of those orders, because each is within limits. A
//! cell that decides alone (ADR 0008) has no surveillance desk behind it, so
//! the pattern is refused here, on the cell's own record of what it placed
//! and what it withdrew unfilled, before the next order of that shape
//! exists.
//!
//! **What counts as a withdrawal is the cell's own choice and nothing else.**
//! An order whose time to live elapsed with nothing filled is the cell
//! cancelling before execution. An order pulled by a halt's mass cancel or
//! from a failed venue was pulled by an event the cell did not choose, and
//! counting those would let one venue outage convict the desk of a pattern
//! it never ran. A requote whose order had nothing filled is counted, as a
//! withdrawal and a placement on the same side of the same book
//! (R12-GOV-6126): a fast enough repricer was otherwise the place-and-cancel
//! pattern on the one path this window never saw.
//!
//! **Requote layering is a second arm, beside that ratio and never in its
//! place** (ADR 0285, amending ADR 0155 section 13 by a new record). A
//! requote counts toward it only when it moved the order **away from the
//! touch**. The arm needs three things together: at least `min_cancellations`
//! of one side's requotes moved away, those are more than `max_cancel_ratio`
//! of that side's requotes, and the same instrument **filled on the opposite
//! side**, at any venue. That is the layering shape. Orders rest on one side
//! to lean on the price and are pulled before they trade, while the other
//! side takes the benefit. A market maker's requotes at or inside the touch
//! are trying to trade and never count. The bound is this policy's own; no
//! second number is chosen. Counting every requote as a withdrawal and a
//! placement keeps the cancel ratio of exactly this pattern under three in
//! four for ever, which is why the arm reads different facts rather than a
//! larger count.
//!
//! Keyed by venue, instrument and side, because a pattern is one side of
//! one book: a desk resting bids that expire while its asks fill is still
//! layering the bid. The working set is bounded twice — by the window,
//! pruned on every read and write, and by [`MAX_EVENTS_PER_KEY`] — so a cell
//! that runs for months holds what one window needs and no more.

use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use std::collections::{BTreeMap, VecDeque};

/// The most events one key keeps. A window holding more placements than this
/// on one side of one book is already far past any bound the policy can
/// state, and the oldest are dropped first so the ratio stays a statement
/// about the recent pattern.
pub const MAX_EVENTS_PER_KEY: usize = 512;

/// The committed bound on the cell's own cancel-before-fill pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConductPolicy {
    window: Duration,
    min_cancellations: usize,
    max_cancel_ratio: Decimal,
}

impl ConductPolicy {
    /// A policy refusing a further order on one side of one book once, within
    /// `window`, at least `min_cancellations` of the cell's orders there were
    /// withdrawn unfilled and they are more than `max_cancel_ratio` of the
    /// orders it placed there.
    ///
    /// Refused rather than clamped: a non-positive window or a ratio outside
    /// `[0, 1)` is a policy that can never fire or always fires, and either
    /// reads as a control while being none.
    pub fn new(
        window: Duration,
        min_cancellations: usize,
        max_cancel_ratio: Decimal,
    ) -> Result<Self> {
        if window <= Duration::ZERO {
            return Err(Error::invalid(
                "a conduct window must be positive; a zero window holds no pattern to judge",
            ));
        }
        if min_cancellations == 0 {
            return Err(Error::invalid(
                "a conduct policy needs at least one withdrawal before it judges; zero would \
                 refuse the first order the cell ever placed",
            ));
        }
        if max_cancel_ratio.is_negative() || max_cancel_ratio >= Decimal::ONE {
            return Err(Error::invalid(format!(
                "a cancel ratio bound of {max_cancel_ratio} can never be exceeded; state one \
                 at least zero and below one"
            )));
        }
        Ok(Self {
            window,
            min_cancellations,
            max_cancel_ratio,
        })
    }

    pub fn window(&self) -> Duration {
        self.window
    }
}

impl Default for ConductPolicy {
    /// Four unfilled withdrawals in a minute, more than three in four of what
    /// was placed. Four because one or two expiries are what a resting
    /// strategy in a quiet market produces; three quarters because a desk
    /// whose orders trade at all is under it by a wide margin.
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60),
            min_cancellations: 4,
            max_cancel_ratio: dec!("0.75"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Placed,
    CancelledUnfilled,
}

/// One requote of an order with nothing filled: whether it moved the order
/// away from the touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Requote {
    away_from_touch: bool,
}

type Key = (VenueId, ObjectId, BookSide);

/// Fills are keyed by instrument and side alone: the benefit of layering one
/// venue's book can be taken at another (ADR 0285 decision 1).
type FillKey = (ObjectId, BookSide);

fn opposite(side: BookSide) -> BookSide {
    match side {
        BookSide::Ask => BookSide::Bid,
        BookSide::Bid => BookSide::Ask,
    }
}

/// The cell's record of what it placed and withdrew unfilled, per side of
/// each book, over the policy's window.
#[derive(Debug)]
pub struct ConductWatch {
    policy: ConductPolicy,
    events: BTreeMap<Key, VecDeque<(Timestamp, Event)>>,
    requotes: BTreeMap<Key, VecDeque<(Timestamp, Requote)>>,
    fills: BTreeMap<FillKey, VecDeque<(Timestamp, ())>>,
}

impl ConductWatch {
    pub fn new(policy: ConductPolicy) -> Self {
        Self {
            policy,
            events: BTreeMap::new(),
            requotes: BTreeMap::new(),
            fills: BTreeMap::new(),
        }
    }

    /// A requote of an order on this side of this book that had nothing
    /// filled, and whether it moved the order away from the touch (ADR 0285).
    /// The caller judges the touch against its own book; one it cannot judge
    /// is not away.
    pub fn requoted(
        &mut self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        away_from_touch: bool,
        at: Timestamp,
    ) {
        let window = self.policy.window;
        let requotes = self
            .requotes
            .entry((venue.clone(), object.clone(), side))
            .or_default();
        bounded_push(requotes, at, window, Requote { away_from_touch });
    }

    /// A fill of this instrument on this side, at any venue.
    pub fn filled(&mut self, object: &ObjectId, side: BookSide, at: Timestamp) {
        let window = self.policy.window;
        let fills = self.fills.entry((object.clone(), side)).or_default();
        bounded_push(fills, at, window, ());
    }

    /// An order the venue accepted from the cell.
    pub fn placed(&mut self, venue: &VenueId, object: &ObjectId, side: BookSide, at: Timestamp) {
        self.push((venue.clone(), object.clone(), side), at, Event::Placed);
    }

    /// An order the cell chose to withdraw with nothing of it filled.
    pub fn cancelled_unfilled(
        &mut self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        at: Timestamp,
    ) {
        self.push(
            (venue.clone(), object.clone(), side),
            at,
            Event::CancelledUnfilled,
        );
    }

    /// Why a further order on this side of this book would continue a
    /// place-and-cancel pattern, or `None` when it would not.
    pub fn refusal(
        &mut self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        now: Timestamp,
    ) -> Option<String> {
        self.cancel_ratio_refusal(venue, object, side, now)
            .or_else(|| self.layering_refusal(venue, object, side, now))
    }

    /// The layering arm (ADR 0285): one side's requotes away from the touch,
    /// past the policy's minimum and ratio, while the instrument filled on
    /// the opposite side at any venue.
    fn layering_refusal(
        &mut self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        now: Timestamp,
    ) -> Option<String> {
        let window = self.policy.window;
        let requotes = self
            .requotes
            .get_mut(&(venue.clone(), object.clone(), side))?;
        prune(requotes, now, window);
        let away = requotes.iter().filter(|(_, r)| r.away_from_touch).count();
        if away < self.policy.min_cancellations || requotes.is_empty() {
            return None;
        }
        // Counts, not money, as in the cancel ratio.
        let ratio = Decimal::from(away as i64).checked_div(Decimal::from(requotes.len() as i64))?;
        if ratio <= self.policy.max_cancel_ratio {
            return None;
        }
        let fills = self.fills.get_mut(&(object.clone(), opposite(side)))?;
        prune(fills, now, window);
        if fills.is_empty() {
            return None;
        }
        Some(format!(
            "the cell requoted {away} of its last {} {} order(s) for {} at {} away from the \
             touch within {} s while {} filled on the other side, past the {} bound; a further \
             order of that shape would continue a layering pattern, so none is placed until \
             the window holds less",
            requotes.len(),
            match side {
                BookSide::Ask => "buy",
                BookSide::Bid => "sell",
            },
            object.as_str(),
            venue.as_str(),
            window.as_millis() / 1_000,
            object.as_str(),
            self.policy.max_cancel_ratio,
        ))
    }

    fn cancel_ratio_refusal(
        &mut self,
        venue: &VenueId,
        object: &ObjectId,
        side: BookSide,
        now: Timestamp,
    ) -> Option<String> {
        let key = (venue.clone(), object.clone(), side);
        let window = self.policy.window;
        let events = self.events.get_mut(&key)?;
        prune(events, now, window);
        let placed = events.iter().filter(|(_, e)| *e == Event::Placed).count();
        let cancelled = events.len() - placed;
        if cancelled < self.policy.min_cancellations || placed == 0 {
            return None;
        }
        // Counts, not money: both sides are small integers bounded by
        // `MAX_EVENTS_PER_KEY`, so the comparison is exact in `Decimal`.
        let ratio = Decimal::from(cancelled as i64).checked_div(Decimal::from(placed as i64))?;
        if ratio <= self.policy.max_cancel_ratio {
            return None;
        }
        Some(format!(
            "the cell withdrew {cancelled} of its last {placed} {} order(s) for {} at {} \
             unfilled within {} s, past the {} cancel-before-fill bound; a further order of \
             that shape would continue a place-and-cancel pattern, so none is placed until \
             the window holds less",
            // The side is the book the order takes from, as in `Cell::send`'s
            // risk refusal: an order on the ask is a buy.
            match side {
                BookSide::Ask => "buy",
                BookSide::Bid => "sell",
            },
            object.as_str(),
            venue.as_str(),
            window.as_millis() / 1_000,
            self.policy.max_cancel_ratio,
        ))
    }

    fn push(&mut self, key: Key, at: Timestamp, event: Event) {
        let window = self.policy.window;
        let events = self.events.entry(key).or_default();
        bounded_push(events, at, window, event);
    }
}

/// Every queue here is bounded twice: pruned to the window, and capped at
/// [`MAX_EVENTS_PER_KEY`] with the oldest dropped first.
fn bounded_push<T>(
    events: &mut VecDeque<(Timestamp, T)>,
    at: Timestamp,
    window: Duration,
    event: T,
) {
    prune(events, at, window);
    if events.len() >= MAX_EVENTS_PER_KEY {
        events.pop_front();
    }
    events.push_back((at, event));
}

fn prune<T>(events: &mut VecDeque<(Timestamp, T)>, now: Timestamp, window: Duration) {
    while let Some((at, _)) = events.front() {
        if now.since(*at) < window {
            break;
        }
        events.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_that_could_never_fire_is_refused() {
        assert!(ConductPolicy::new(Duration::ZERO, 4, dec!("0.75")).is_err());
        assert!(ConductPolicy::new(Duration::from_secs(60), 0, dec!("0.75")).is_err());
        assert!(ConductPolicy::new(Duration::from_secs(60), 4, dec!("1")).is_err());
        assert!(ConductPolicy::new(Duration::from_secs(60), 4, dec!("-0.1")).is_err());
        assert!(ConductPolicy::new(Duration::from_secs(60), 4, dec!("0.75")).is_ok());
    }
}
