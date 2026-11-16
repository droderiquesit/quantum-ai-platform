//! Counterfactuals for hedged and unwound paths (R12-MODEL-6004, ADR 0680).
//!
//! Declined and partly filled orders are priced in LEARN already. A hedge
//! and an unwind are not orders the cycle declined: they are legs it chose,
//! and a choice whose alternative is never priced cannot be judged
//! protective or merely expensive.
//!
//! # What the alternative is
//!
//! - An **unwound** position's alternative is holding it to the horizon: what
//!   the quantity closed at the exit price would have been worth at the mark
//!   when the horizon arrived.
//! - A **hedged** position's alternative is the unhedged hold: the same
//!   exposure with no hedge leg, so the hedge leg's own result is exactly what
//!   the hedge cost or earned.
//!
//! Both are reported as `regret`, the alternative's result minus the chosen
//! path's, so a positive regret means the alternative was better and a
//! negative one means the choice was protective. One sign convention, so a
//! reader does not have to know which kind it is looking at.
//!
//! # Bounded
//!
//! The queue holds at most [`MAX_PENDING`] paths. A path offered beyond that is
//! refused with an error naming the bound, not dropped: a counterfactual that
//! silently vanished is a veto nobody can judge. The figure is provisional
//! (ADR 0680) and was chosen low, in the restrictive direction.
//!
//! This module holds the arithmetic and the queue. It does not call the
//! platform and nothing journals its output yet; see ADR 0680.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use std::collections::BTreeMap;

/// The most paths held awaiting their horizon. Provisional (ADR 0680).
pub const MAX_PENDING: usize = 256;

/// Which kind of chosen path a counterfactual prices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathKind {
    /// A hedge leg opened against an exposure.
    Hedged,
    /// A position, or part of one, closed before its horizon.
    Unwound,
}

/// One chosen path awaiting its horizon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPath {
    pub id: String,
    pub kind: PathKind,
    pub instrument: String,
    /// Signed units of the leg that was executed: the hedge leg's own side
    /// for a hedge, the side of the position closed for an unwind.
    pub quantity: Decimal,
    /// The price that leg executed at.
    pub price: Decimal,
    pub at: Timestamp,
    /// How long after `at` the alternative is read.
    pub horizon: Duration,
}

/// What the alternative would have done, priced at the horizon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCounterfactual {
    pub id: String,
    pub kind: PathKind,
    pub instrument: String,
    pub mark: Decimal,
    /// Alternative minus chosen. Positive: the alternative was better.
    pub regret: Decimal,
}

/// Price one path at `mark`.
pub fn price(path: &PendingPath, mark: Decimal) -> Result<PathCounterfactual> {
    let overflow = || {
        Error::numeric(format!(
            "path {} ({}) overflowed when priced at {mark}; refusing to report a regret",
            path.id, path.instrument
        ))
    };
    let move_ = mark.checked_sub(path.price).ok_or_else(overflow)?;
    let leg = path.quantity.checked_mul(move_).ok_or_else(overflow)?;
    let regret = match path.kind {
        // Held instead of closed: the closed quantity's move since the exit.
        PathKind::Unwound => leg,
        // Unhedged instead of hedged: nothing, less what the hedge leg made.
        PathKind::Hedged => Decimal::ZERO.checked_sub(leg).ok_or_else(overflow)?,
    };
    Ok(PathCounterfactual {
        id: path.id.clone(),
        kind: path.kind,
        instrument: path.instrument.clone(),
        mark,
        regret,
    })
}

/// The bounded queue of chosen paths awaiting their horizon.
#[derive(Clone, Debug, Default)]
pub struct PathQueue {
    pending: BTreeMap<String, PendingPath>,
}

impl PathQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Hold `path` until its horizon. Refused past [`MAX_PENDING`], and
    /// refused if its id is already held, because two paths under one id
    /// would price one of them twice.
    pub fn push(&mut self, path: PendingPath) -> Result<()> {
        if self.pending.contains_key(&path.id) {
            return Err(Error::invalid(format!(
                "path {} is already awaiting its horizon; offer each path once",
                path.id
            )));
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(Error::invalid(format!(
                "{MAX_PENDING} paths already await their horizon; settle them before \
                 offering path {}",
                path.id
            )));
        }
        self.pending.insert(path.id.clone(), path);
        Ok(())
    }

    /// Price every path whose horizon has arrived at `now`, using `mark` for
    /// its instrument. A due path with no mark stays queued: pricing it at a
    /// guess would report a regret nobody measured.
    pub fn settle(
        &mut self,
        now: Timestamp,
        mark: impl Fn(&str) -> Option<Decimal>,
    ) -> Result<Vec<PathCounterfactual>> {
        let mut priced = Vec::new();
        let mut settled = Vec::new();
        for (id, path) in &self.pending {
            if path.at.saturating_add(path.horizon) > now {
                continue;
            }
            let Some(at_mark) = mark(&path.instrument) else {
                continue;
            };
            priced.push(price(path, at_mark)?);
            settled.push(id.clone());
        }
        for id in settled {
            self.pending.remove(&id);
        }
        Ok(priced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn path(id: &str, kind: PathKind, quantity: Decimal) -> PendingPath {
        PendingPath {
            id: id.to_string(),
            kind,
            instrument: "AAPL".to_string(),
            quantity,
            price: dec!("100"),
            at: Timestamp::from_secs(1_760_000_000),
            horizon: Duration::from_days(1),
        }
    }

    /// A long position unwound at 100 and marked 110 at the horizon: holding
    /// would have gained 10 a unit, so the regret is positive. Marked 90, the
    /// unwind was protective and the regret is negative. Premise first: the
    /// two marks differ.
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn an_unwind_is_regretted_when_the_hold_would_have_gained_and_protective_when_it_would_not() {
        let unwind = path("u1", PathKind::Unwound, dec!("10"));
        let up = price(&unwind, dec!("110")).expect("prices");
        let down = price(&unwind, dec!("90")).expect("prices");
        assert_eq!(up.regret, dec!("100"));
        assert_eq!(down.regret, dec!("-100"));
    }

    /// A short hedge of 10 opened at 100: the underlying falling to 90 makes
    /// the hedge 100, so the unhedged alternative was worse by 100 (negative
    /// regret, protective); rising to 110 cost the hedge 100 (positive regret,
    /// merely expensive).
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn a_hedge_is_protective_when_it_paid_and_expensive_when_it_cost() {
        let hedge = path("h1", PathKind::Hedged, dec!("-10"));
        let fell = price(&hedge, dec!("90")).expect("prices");
        let rose = price(&hedge, dec!("110")).expect("prices");
        assert_eq!(fell.regret, dec!("-100"));
        assert_eq!(rose.regret, dec!("100"));
    }

    /// A path is priced once, only after its horizon, only with a mark; a
    /// duplicate id and a full queue are refused.
    ///
    /// Mutation: not run (nothing was compiled, owner instruction 2026-11-15).
    #[test]
    fn the_queue_prices_a_path_once_after_its_horizon_and_refuses_duplicates_and_overflow() {
        let mut queue = PathQueue::new();
        queue
            .push(path("u1", PathKind::Unwound, dec!("10")))
            .expect("queues");
        assert!(
            queue
                .push(path("u1", PathKind::Unwound, dec!("10")))
                .is_err(),
            "a duplicate id was accepted"
        );
        let start = Timestamp::from_secs(1_760_000_000);
        let early = queue.settle(start, |_| Some(dec!("110"))).expect("settles");
        assert!(early.is_empty(), "a path was priced before its horizon");
        let unmarked = queue
            .settle(start.saturating_add(Duration::from_days(2)), |_| None)
            .expect("settles");
        assert!(unmarked.is_empty(), "a path was priced without a mark");
        assert_eq!(queue.len(), 1, "an unmarked path left the queue");
        let due = queue
            .settle(start.saturating_add(Duration::from_days(2)), |_| {
                Some(dec!("110"))
            })
            .expect("settles");
        assert_eq!(due.len(), 1);
        assert!(queue.is_empty(), "a priced path stayed queued");
        for n in 0..MAX_PENDING {
            queue
                .push(path(&format!("p{n}"), PathKind::Hedged, dec!("-1")))
                .expect("room");
        }
        assert!(
            queue
                .push(path("over", PathKind::Hedged, dec!("-1")))
                .is_err(),
            "the queue grew past its bound"
        );
    }
}
