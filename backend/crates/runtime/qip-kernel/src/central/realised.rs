//! What each strategy realised at each cell, session by session, as the
//! centre's own attribution booked it.
//!
//! The demotion monitor judges a strategy on a series of live returns, and
//! until this existed nothing in a deployed process produced one: the review
//! seam took a [`CellOutcome`] a caller had to assemble, and the only callers
//! were tests. A cell could decay for a year and the trigger written to
//! catch it never ran, because the number it reads was never computed.
//!
//! The series is built from one source, deliberately. A cell's report carries
//! its own claim about its realised loss in [`qip_contracts::Utilisation`],
//! and the centre could have read that. It reads the attribution instead —
//! the exact decomposition of the fills it settled, the same figure it bills
//! and charges into the risk aggregate — because two claims about the same
//! P&L will disagree, and a demotion argued from the cell's figure would be
//! contested by the centre's own books.
//!
//! A session is a UTC day of the cell's report instants. The monitor's
//! thresholds are written in sessions — twenty live observations before decay
//! is judged, a run of losing *days* as a kill condition — and the baseline
//! it compares against is a daily series. A return per report, at whatever
//! cadence a cell ships its deltas, would have a per-period volatility far
//! below the baseline's, and the regime-drift trigger would read that as the
//! world having changed rather than as the clock ticking faster.

use super::learning::CellOutcome;
use qip_contracts::signal::StrategyId;
use qip_core::{Decimal, Timestamp};
use std::collections::BTreeMap;

/// Sessions retained per strategy at each cell — a trading year. The bound
/// is on the working set, not the record: every fill the sessions were summed
/// from is in the event log, and the review reads only what is retained.
pub const REALISED_SESSIONS: usize = 252;

/// One session's attributed P&L for one strategy at one cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealisedSession {
    /// The start of the UTC day the session is.
    pub day: Timestamp,
    /// Attributed P&L, signed, summed over every report the day carried.
    pub pnl: Decimal,
    /// The gross limit of the envelope the strategy held at the cell when the
    /// day's last fill settled, which is what the day's return is a fraction
    /// of. `None` where the centre held no envelope for the pair: the P&L is
    /// still counted toward the realised-loss kill condition, and no return
    /// is stated for the day, because a return over a denominator nobody
    /// granted would be a number invented to fill a series.
    pub capital: Option<Decimal>,
}

/// The retained sessions for one strategy at one cell, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RealisedSeries {
    sessions: BTreeMap<Timestamp, RealisedSession>,
}

impl RealisedSeries {
    /// Add one settlement's attributed P&L to the session of `at`.
    ///
    /// Keyed on the day rather than appended, so a report that arrives after
    /// a later one lands in its own session instead of a new one; the oldest
    /// session is evicted once the bound is reached, whichever order the
    /// reports came in.
    pub fn absorb(&mut self, at: Timestamp, pnl: Decimal, capital: Option<Decimal>) {
        let day = at.start_of_day();
        let session = self.sessions.entry(day).or_insert(RealisedSession {
            day,
            pnl: Decimal::ZERO,
            capital: None,
        });
        session.pnl += pnl;
        if capital.is_some() {
            session.capital = capital;
        }
        while self.sessions.len() > REALISED_SESSIONS {
            let Some(oldest) = self.sessions.keys().next().copied() else {
                break;
            };
            self.sessions.remove(&oldest);
        }
    }

    /// The sessions retained, oldest first.
    pub fn sessions(&self) -> impl Iterator<Item = &RealisedSession> {
        self.sessions.values()
    }

    /// The observation the demotion monitor reads, or `None` where there is
    /// nothing closed to observe.
    ///
    /// Only sessions on or after `since` — the instant the baseline was
    /// established — count, because the monitor asks what the strategy has
    /// done since it was promoted on that baseline; a strategy re-promoted
    /// on fresh evidence is not re-demoted next cycle on the series that
    /// pushed it down. Only sessions before the day of `now` count, because
    /// a day still being traded is not a return yet, and a partial day read
    /// as a whole one would make the series lurch on every cycle.
    ///
    /// The realised cost is reported as zero and that is a stated limit, not
    /// a measurement: the wire carries no cost for a cell's fill and the
    /// centre invents none (`CentralPlane::settle` says the same), so the
    /// realised-cost kill condition cannot fire from this series. The other
    /// three can — loss, drawdown and losing days are all read off what was
    /// attributed — and the decay and drift triggers read the returns.
    pub fn outcome(
        &self,
        strategy: &StrategyId,
        cell: &str,
        since: Timestamp,
        now: Timestamp,
    ) -> Option<CellOutcome> {
        let today = now.start_of_day();
        let floor = since.start_of_day();
        let closed: Vec<&RealisedSession> = self
            .sessions
            .values()
            .filter(|session| session.day >= floor && session.day < today)
            .collect();
        if closed.is_empty() {
            return None;
        }

        // Money is `Decimal` up to this line. The return, the drawdown and
        // the losing-day count are statistics the monitor compares against a
        // baseline of `f64` returns, and this is where the attributed figures
        // cross into that arithmetic.
        let realised_returns: Vec<f64> = closed
            .iter()
            .filter_map(|session| {
                session
                    .capital
                    .filter(|capital| capital.is_positive())
                    .and_then(|capital| session.pnl.checked_div(capital))
                    .map(Decimal::to_f64)
            })
            .collect();

        let mut cumulative = Decimal::ZERO;
        let mut consecutive_losing_days = 0u32;
        for session in &closed {
            cumulative += session.pnl;
            if session.pnl.is_negative() {
                consecutive_losing_days += 1;
            } else {
                consecutive_losing_days = 0;
            }
        }
        let realised_loss = (-cumulative).max(Decimal::ZERO);

        // Drawdown is measured on the equity the grant put behind the
        // strategy plus what it has since made, against that equity's high
        // water mark, over the closed sessions. The base is the latest grant
        // the closed sessions were made under; where none was, there is no
        // equity to draw down from and the figure is zero rather than a
        // fraction of a P&L peak that may itself be zero or negative.
        let base = closed.iter().rev().find_map(|session| session.capital);
        let peak_to_trough_drawdown = base
            .filter(|base| base.is_positive())
            .map(|base| {
                let mut equity = base;
                let mut high_water = base;
                let mut worst = 0.0_f64;
                for session in &closed {
                    equity += session.pnl;
                    high_water = high_water.max(equity);
                    if high_water.is_positive() {
                        let drawdown = (high_water - equity).to_f64() / high_water.to_f64();
                        worst = worst.max(drawdown);
                    }
                }
                worst
            })
            .unwrap_or(0.0);

        Some(CellOutcome {
            strategy: strategy.clone(),
            cell: cell.to_string(),
            at: now,
            realised_returns,
            realised_loss,
            peak_to_trough_drawdown,
            consecutive_losing_days,
            realised_cost_bps: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::{Duration, dec};

    fn day(n: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000).saturating_add(Duration::from_days(n))
    }

    fn id() -> StrategyId {
        StrategyId::new("realised-tests")
    }

    /// Two reports on one day are one session, and the return is the day's
    /// P&L over the grant, not each report's.
    #[test]
    fn reports_on_the_same_day_sum_into_one_session() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("100"), Some(dec!("10000")));
        series.absorb(
            day(0).saturating_add(Duration::from_hours(3)),
            dec!("-40"),
            Some(dec!("10000")),
        );
        let outcome = series
            .outcome(&id(), "cell", day(0), day(1))
            .expect("one closed session");
        assert_eq!(outcome.realised_returns, vec![0.006]);
        assert_eq!(outcome.realised_loss, Decimal::ZERO);
    }

    /// The day still being traded is not a return yet.
    #[test]
    fn the_current_day_is_not_observed_until_it_has_closed() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("-100"), Some(dec!("10000")));
        assert!(series.outcome(&id(), "cell", day(0), day(0)).is_none());
        let closed = series
            .outcome(&id(), "cell", day(0), day(1))
            .expect("closed once the day has passed");
        assert_eq!(closed.consecutive_losing_days, 1);
        assert_eq!(closed.realised_loss, dec!("100"));
    }

    /// Sessions before the baseline was established are not the strategy's
    /// record on that baseline.
    #[test]
    fn sessions_before_the_baseline_are_excluded() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("-500"), Some(dec!("10000")));
        series.absorb(day(1), dec!("50"), Some(dec!("10000")));
        let outcome = series
            .outcome(&id(), "cell", day(1), day(2))
            .expect("one session since the baseline");
        assert_eq!(outcome.realised_returns, vec![0.005]);
        assert_eq!(outcome.realised_loss, Decimal::ZERO);
    }

    /// A session under no grant counts toward the loss and states no return.
    #[test]
    fn a_session_under_no_envelope_counts_its_loss_and_states_no_return() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("-300"), None);
        let outcome = series
            .outcome(&id(), "cell", day(0), day(1))
            .expect("one closed session");
        assert!(outcome.realised_returns.is_empty());
        assert_eq!(outcome.realised_loss, dec!("300"));
        assert!(outcome.peak_to_trough_drawdown.abs() < f64::EPSILON);
    }

    /// Drawdown is the fall from the equity's high-water mark, as a fraction
    /// of that mark.
    #[test]
    fn drawdown_is_measured_from_the_equity_high_water_mark() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("1000"), Some(dec!("10000")));
        series.absorb(day(1), dec!("-2200"), Some(dec!("10000")));
        series.absorb(day(2), dec!("500"), Some(dec!("10000")));
        let outcome = series
            .outcome(&id(), "cell", day(0), day(3))
            .expect("three closed sessions");
        // Peak 11000, trough 8800: a fifth of the peak. The net is a loss of
        // 700, and the last session was a gain, so no losing run is open.
        assert!((outcome.peak_to_trough_drawdown - 0.2).abs() < 1e-12);
        assert_eq!(outcome.consecutive_losing_days, 0);
        assert_eq!(outcome.realised_loss, dec!("700"));
    }

    /// The working set is bounded and it is the oldest session that goes.
    #[test]
    fn the_series_keeps_a_trading_year_and_evicts_the_oldest() {
        let mut series = RealisedSeries::default();
        for n in 0..(REALISED_SESSIONS as i64 + 3) {
            series.absorb(day(n), dec!("1"), Some(dec!("10000")));
        }
        assert_eq!(series.sessions().count(), REALISED_SESSIONS);
        assert_eq!(
            series.sessions().next().map(|session| session.day),
            Some(day(3).start_of_day())
        );
    }
}
