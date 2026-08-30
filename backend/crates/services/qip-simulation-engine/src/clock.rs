//! The simulation clock and the leakage guard built into it.
//!
//! Every look-ahead bug in a backtest is the same bug: some piece of code read
//! a value that would not have existed yet. The usual defences — careful
//! indexing, review, a naming convention — all rely on nobody making a
//! mistake.
//!
//! [`SimulationClock`] takes a different approach. A backtest reads market
//! data only through a [`PointInTimeView`], which the clock hands out and which
//! borrows from it. The view carries the current simulation time and filters
//! every read against it, and the clock refuses to advance while a view is
//! outstanding. Reading tomorrow's price is not something a strategy has to
//! remember not to do; it is something there is no method for.
//!
//! The clock also enforces the decision lag: a signal computed at time `t` can
//! only be acted on at `t + decision_lag`. Backtests that assume instantaneous
//! execution overstate returns by exactly the amount that matters.

use qip_core::error::{Error, Result};
use qip_core::ids::ObjectId;
use qip_core::time::{Duration, Timestamp};
use qip_market::bar::Bar;
use qip_market::snapshot::MarketSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How the simulation treats the gap between deciding and trading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAssumptions {
    /// Delay between a signal being computable and an order reaching the
    /// market. Zero is available and is almost always wrong.
    pub decision_lag: Duration,
    /// Whether a fill may occur at the same bar's close that produced the
    /// signal. Permitting it is the single most common backtest error.
    pub allow_same_bar_fill: bool,
}

impl ExecutionAssumptions {
    /// The defensible default for a daily strategy: decide on today's close,
    /// trade at tomorrow's open.
    ///
    /// A backtest that fills at the close of the bar whose close produced the
    /// signal is trading on information it did not have when the order would
    /// have been sent.
    pub fn next_bar() -> Self {
        Self {
            decision_lag: Duration::from_days(1),
            allow_same_bar_fill: false,
        }
    }

    /// An intraday strategy reacting within the session.
    pub fn intraday(lag: Duration) -> Self {
        Self {
            decision_lag: lag,
            allow_same_bar_fill: false,
        }
    }

    /// No lag at all. Only defensible for a strategy whose signal is genuinely
    /// available before the price it trades at — and the backtest report says
    /// so, loudly, because it usually is not.
    pub fn instantaneous() -> Self {
        Self {
            decision_lag: Duration::ZERO,
            allow_same_bar_fill: true,
        }
    }

    /// Whether these assumptions are the optimistic ones.
    pub fn is_optimistic(&self) -> bool {
        self.allow_same_bar_fill || self.decision_lag.is_zero()
    }

    pub fn describe(&self) -> String {
        if self.is_optimistic() {
            format!(
                "decision lag {:?}, same-bar fills {}: an optimistic assumption that will overstate returns unless the signal genuinely precedes the price",
                self.decision_lag,
                if self.allow_same_bar_fill {
                    "allowed"
                } else {
                    "disallowed"
                }
            )
        } else {
            format!("decision lag {:?}, no same-bar fills", self.decision_lag)
        }
    }
}

/// A read-only view of the market as of one instant.
///
/// Obtained from [`SimulationClock::view`] and borrowing from it, so it cannot
/// outlive the instant it describes. Every accessor filters on the view's own
/// timestamp, and there is no accessor that does not.
#[derive(Debug)]
pub struct PointInTimeView<'a> {
    as_of: Timestamp,
    history: &'a BTreeMap<String, Vec<Bar>>,
    reads: std::cell::Cell<usize>,
}

impl<'a> PointInTimeView<'a> {
    pub fn as_of(&self) -> Timestamp {
        self.as_of
    }

    /// Bars for one instrument that had *closed* by the view's instant.
    ///
    /// Keyed on close time, not open time. A daily bar stamped with today's
    /// date does not exist until the session ends, and treating it as
    /// available at the open is a full day of look-ahead.
    pub fn bars(&self, object_id: &ObjectId) -> &'a [Bar] {
        self.reads.set(self.reads.get() + 1);
        let Some(all) = self.history.get(object_id.as_str()) else {
            return &[];
        };
        let cut = all.partition_point(|bar| bar.close_time() <= self.as_of);
        &all[..cut]
    }

    /// Closing prices available at the view's instant.
    pub fn closes(&self, object_id: &ObjectId) -> Vec<f64> {
        self.bars(object_id)
            .iter()
            .map(|bar| bar.close.to_f64())
            .collect()
    }

    /// The most recent close available, if any.
    pub fn last_close(&self, object_id: &ObjectId) -> Option<qip_core::Decimal> {
        self.bars(object_id).last().map(|bar| bar.close)
    }

    /// The instruments with at least one closed bar by this instant.
    pub fn available(&self) -> Vec<ObjectId> {
        self.history
            .iter()
            .filter(|(_, bars)| {
                bars.first()
                    .is_some_and(|bar| bar.close_time() <= self.as_of)
            })
            .map(|(id, _)| ObjectId::from_string(id.clone()))
            .collect()
    }

    /// A market snapshot as of this instant, for code that wants one.
    pub fn snapshot(&self) -> MarketSnapshot {
        let mut snapshot = MarketSnapshot::new(self.as_of);
        for object in self.available() {
            for bar in self.bars(&object) {
                snapshot.apply_bar(bar.clone());
            }
        }
        snapshot
    }

    /// How many reads this view served, for the leakage report.
    pub fn read_count(&self) -> usize {
        self.reads.get()
    }
}

/// Drives a simulation forward and hands out point-in-time views.
#[derive(Debug)]
pub struct SimulationClock {
    history: BTreeMap<String, Vec<Bar>>,
    /// Every instant at which some bar closed, ascending and deduplicated.
    steps: Vec<Timestamp>,
    cursor: usize,
    assumptions: ExecutionAssumptions,
}

impl SimulationClock {
    /// Build from history, sorting each instrument's bars by close time.
    ///
    /// Sorting here rather than trusting the caller is what makes the
    /// `partition_point` in [`PointInTimeView::bars`] correct; an unsorted
    /// series would silently truncate at the first out-of-order bar.
    pub fn new(bars: Vec<Bar>, assumptions: ExecutionAssumptions) -> Result<Self> {
        if bars.is_empty() {
            return Err(Error::invalid("a simulation needs at least one bar"));
        }
        let mut history: BTreeMap<String, Vec<Bar>> = BTreeMap::new();
        for bar in bars {
            if !bar.is_coherent() {
                return Err(Error::invalid(format!(
                    "incoherent bar for {} at {}: high {} low {} open {} close {}",
                    bar.object_id.as_str(),
                    bar.open_time,
                    bar.high,
                    bar.low,
                    bar.open,
                    bar.close
                )));
            }
            history
                .entry(bar.object_id.as_str().to_string())
                .or_default()
                .push(bar);
        }
        let mut steps: Vec<Timestamp> = Vec::new();
        for series in history.values_mut() {
            series.sort_by_key(Bar::close_time);
            steps.extend(series.iter().map(Bar::close_time));
        }
        steps.sort_unstable();
        steps.dedup();

        Ok(Self {
            history,
            steps,
            cursor: 0,
            assumptions,
        })
    }

    pub fn assumptions(&self) -> ExecutionAssumptions {
        self.assumptions
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn is_finished(&self) -> bool {
        self.cursor >= self.steps.len()
    }

    /// The instant the simulation is currently at.
    pub fn now(&self) -> Option<Timestamp> {
        self.steps.get(self.cursor).copied()
    }

    /// The first and last instants covered.
    pub fn span(&self) -> Option<(Timestamp, Timestamp)> {
        Some((*self.steps.first()?, *self.steps.last()?))
    }

    /// A view of the market as it stood at the current instant.
    ///
    /// The borrow is what prevents time travel: the clock cannot advance while
    /// a view is alive, so a view cannot be carried into a later step and used
    /// to read data that was future at the moment it was created.
    pub fn view(&self) -> Option<PointInTimeView<'_>> {
        Some(PointInTimeView {
            as_of: self.now()?,
            history: &self.history,
            reads: std::cell::Cell::new(0),
        })
    }

    /// The instant at which a decision made now could actually be executed.
    ///
    /// `None` when the lag runs past the end of the data — a decision at the
    /// last bar of a backtest cannot be executed inside it, and pretending
    /// otherwise adds a free trade at the end of every run.
    pub fn executable_at(&self) -> Option<Timestamp> {
        let now = self.now()?;
        let earliest = now.saturating_add(self.assumptions.decision_lag);
        self.steps
            .iter()
            .find(|step| {
                if self.assumptions.allow_same_bar_fill {
                    **step >= earliest
                } else {
                    **step > now && **step >= earliest
                }
            })
            .copied()
    }

    /// Advance to the next instant. Returns false at the end of the data.
    pub fn advance(&mut self) -> bool {
        if self.cursor >= self.steps.len() {
            return false;
        }
        self.cursor += 1;
        self.cursor < self.steps.len()
    }

    /// Rewind to the start, for a second pass over the same history.
    pub fn reset(&mut self) {
        self.cursor = 0;
    }

    /// The price a fill at `at` would occur at, if the data supports one.
    ///
    /// Returns the *open* of the bar covering `at`, not its close: an order
    /// sent after the previous close is worked from the open, and using the
    /// close would be another day of hindsight.
    pub fn fill_price(&self, object_id: &ObjectId, at: Timestamp) -> Option<qip_core::Decimal> {
        let series = self.history.get(object_id.as_str())?;
        series
            .iter()
            .find(|bar| bar.close_time() >= at && bar.open_time <= at)
            .or_else(|| series.iter().find(|bar| bar.close_time() >= at))
            .map(|bar| bar.open)
    }
}
