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
//!
//! # Two facts, two instants they become knowable
//!
//! A session carries two things the centre learns at different moments. The
//! attributed P&L is knowable when the fills settle. The grant behind it is
//! knowable when the grant is made, which is earlier and has nothing to do
//! with whether anything traded.
//!
//! Until [`RealisedSeries::retain_grant`] existed only the first was kept, and
//! a day on which a strategy held a grant and settled nothing produced no
//! session at all. "Held a grant and made nothing" — a return of zero, which
//! is a fact about the day — was then indistinguishable afterwards from "held
//! no grant", which is not a return at all. Anything that needed one
//! observation per strategy per day had to invent the difference: fill the
//! gap with a zero and it fabricates a return; intersect to the days every
//! strategy settled on and nothing is left. Recording the grant on its own
//! keeps the two apart.
//!
//! What is still an absence stays one. A day the centre saw no report from a
//! cell has no session, because a silent cell has said nothing about the day;
//! and a day whose grant had lapsed carries no [`RealisedSession::grant`],
//! because a return over an authority the cell may no longer commit against
//! is a fraction of something nobody held.

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
    /// Zero on a session that only retained a grant, which is what `settled`
    /// distinguishes.
    pub pnl: Decimal,
    /// The gross limit of the envelope the strategy held at the cell when the
    /// day's last fill settled, which is what the day's return is a fraction
    /// of. `None` where the centre held no envelope for the pair: the P&L is
    /// still counted toward the realised-loss kill condition, and no return
    /// is stated for the day, because a return over a denominator nobody
    /// granted would be a number invented to fill a series.
    pub capital: Option<Decimal>,
    /// Whether a settlement contributed to this session.
    ///
    /// False on a day that carries only a retained grant. [`RealisedSeries::outcome`]
    /// reads settled sessions and nothing else, so a day the strategy held a
    /// grant and traded nothing neither breaks a run of losing days nor adds
    /// a zero to the returns the demotion monitor judges. That restraint is
    /// deliberate: what the monitor reads is the input to a kill condition,
    /// and widening it is a change to a risk control rather than to a record.
    pub settled: bool,
    /// The gross limit of a grant the centre held **live** for the pair at
    /// the instant this session was written, or `None` where it held none.
    ///
    /// Distinct from `capital` above, and the distinction is the point.
    /// `capital` is stamped from whatever envelope the centre has for the
    /// pair, expired or not; this is stamped only from one live at that
    /// instant. A zero-P&L day may be read as a return of zero only against
    /// this one — over a lapsed grant it would be a fraction of an authority
    /// the cell could no longer commit against.
    pub grant: Option<Decimal>,
}

/// The retained sessions for one strategy at one cell, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RealisedSeries {
    sessions: BTreeMap<Timestamp, RealisedSession>,
}

impl RealisedSeries {
    /// Add one settlement's attributed P&L to the session of `at`.
    ///
    /// Keyed on the day by [`RealisedSeries::session`], so a report that
    /// arrives after a later one lands in its own session instead of a new
    /// one; the oldest session is evicted once the bound is reached, whichever
    /// order the reports came in. Marks the session settled, which is what
    /// separates a day that traded from a day that only held a grant.
    pub fn absorb(&mut self, at: Timestamp, pnl: Decimal, capital: Option<Decimal>) {
        let session = self.session(at);
        session.pnl += pnl;
        session.settled = true;
        if capital.is_some() {
            session.capital = capital;
        }
        self.evict();
    }

    /// Record that the centre held a live grant of `grant` for the pair on the
    /// day of `at`, whether or not anything settled.
    ///
    /// The other half of a session, and the half nothing kept before: without
    /// it a day under a grant that traded nothing leaves no trace, and the
    /// difference between that and a day under no grant cannot be recovered
    /// afterwards from anything the centre retains. Touches neither `pnl` nor
    /// `capital`, so a grant retained after a settlement cannot move a figure
    /// the demotion monitor reads.
    pub fn retain_grant(&mut self, at: Timestamp, grant: Decimal) {
        let session = self.session(at);
        session.grant = Some(grant);
        self.evict();
    }

    /// The session of `at`'s day, created empty if the day has none.
    ///
    /// Keyed on the day rather than appended, so a report that arrives after a
    /// later one lands in its own session instead of a new one.
    fn session(&mut self, at: Timestamp) -> &mut RealisedSession {
        let day = at.start_of_day();
        self.sessions.entry(day).or_insert(RealisedSession {
            day,
            pnl: Decimal::ZERO,
            capital: None,
            settled: false,
            grant: None,
        })
    }

    /// Hold the working set at [`REALISED_SESSIONS`], oldest out first.
    ///
    /// Called by every writer rather than by one of them: a retained grant
    /// creates a session exactly as a settlement does, and a bound only the
    /// settlement path enforced would be no bound at all on a strategy that
    /// holds a grant and trades rarely.
    fn evict(&mut self) {
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

    /// The closed days this series can be aligned to a calendar on: each day
    /// carrying a grant the centre held live, with the P&L attributed on it.
    ///
    /// Closed days only, and for the reason [`RealisedSeries::outcome`] gives
    /// twice over: the day being traded is not an observation yet, and a
    /// feature readable before its knowable instant invalidates every backtest
    /// that touches it. Both facts a day yields here were knowable on the day
    /// itself — the grant when it was made, the P&L when the fills settled —
    /// and neither is legible until the day has closed.
    ///
    /// No baseline floor, unlike `outcome`. Co-movement between two strategies
    /// is not a verdict on either one's record since it was promoted, and
    /// cutting each series at its own baseline would leave two series aligned
    /// to two different calendars, which is exactly what a correlation cannot
    /// be estimated from.
    pub fn granted_days(&self, now: Timestamp) -> impl Iterator<Item = (Timestamp, GrantedDay)> {
        let today = now.start_of_day();
        self.sessions.values().filter_map(move |session| {
            if session.day >= today {
                return None;
            }
            let grant = session.grant.filter(|grant| grant.is_positive())?;
            Some((
                session.day,
                GrantedDay {
                    pnl: session.pnl,
                    grant,
                },
            ))
        })
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
    /// Only *settled* sessions count. A session carrying nothing but a
    /// retained grant says the strategy held capital and traded, or the
    /// venue reported, nothing; it is a day in the day-keyed corpus and it
    /// is not an observation the monitor judges. Admitting it would change
    /// two inputs to a kill condition at once — a zero P&L would break a run
    /// of losing days, and a zero return would join the series the decay and
    /// drift triggers read — and neither change belongs in the record that
    /// merely stopped throwing the day away.
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
            .filter(|session| session.settled && session.day >= floor && session.day < today)
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

/// One closed day of one strategy's realised record, under a grant the centre
/// held live on the day.
///
/// The unit a calendar is built from. Both figures are the centre's own: the
/// P&L is what its attribution booked, the grant is what its issuer signed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GrantedDay {
    /// Attributed P&L on the day, signed. Zero where the strategy held the
    /// grant and nothing settled — a return of zero, not an absence.
    pub pnl: Decimal,
    /// The gross limit behind it, summed where the day spans more than one
    /// cell or more than one grant. Positive by construction.
    pub grant: Decimal,
}

impl GrantedDay {
    /// The day's return: attributed P&L over the grant it was made under.
    ///
    /// Money is `Decimal` up to this line. Everything downstream of it —
    /// correlation, the stress window, the clustering — is `f64` statistics,
    /// and this is where the two meet. `None` only if the grant is zero,
    /// which [`RealisedSeries::granted_days`] has already excluded.
    pub fn fraction(&self) -> Option<f64> {
        self.pnl.checked_div(self.grant).map(Decimal::to_f64)
    }

    fn absorb(&mut self, other: Self) {
        self.pnl += other.pnl;
        self.grant += other.grant;
    }
}

/// The retained corpus re-keyed by strategy and day, summed across cells.
///
/// What [`RealisedSeries`] holds is one series per `(cell, strategy)`; what
/// anything estimating co-movement between strategies needs is one series per
/// strategy on one calendar. Summing across cells rather than clustering per
/// cell is the honest reading of the desk's exposure to a strategy: a strategy
/// funded at three cells is one bet with three grants behind it, and its
/// return on a day is what all three attributed over what all three were
/// granted — a capital-weighted mean of the per-cell returns, never an
/// unweighted one, which would let a cell holding a hundredth of the capital
/// move the number as far as the cell holding the rest.
///
/// Holds no day on which the centre held no live grant, so a caller cannot
/// mistake an absence for a flat day: the days present are exactly the days a
/// return exists for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RealisedCalendar {
    by_strategy: BTreeMap<StrategyId, BTreeMap<Timestamp, GrantedDay>>,
    /// The same days summed across every strategy: the desk's own book, which
    /// is the only series the platform holds that a stress window could be cut
    /// from. Accumulated here rather than re-derived, so the desk figure and
    /// the per-strategy figures cannot disagree about a day.
    desk: BTreeMap<Timestamp, GrantedDay>,
}

impl RealisedCalendar {
    /// Add one `(cell, strategy)` series' day to the calendar.
    ///
    /// Keyed on the start of `at`'s day rather than on `at`, so a caller
    /// holding an instant cannot split one session into two days that then
    /// align to nothing. [`RealisedSeries::granted_days`] already yields day
    /// starts; this makes that a property of the calendar rather than of its
    /// one caller.
    pub(super) fn absorb(&mut self, strategy: &StrategyId, at: Timestamp, granted: GrantedDay) {
        let day = at.start_of_day();
        self.by_strategy
            .entry(strategy.clone())
            .or_default()
            .entry(day)
            .or_default()
            .absorb(granted);
        self.desk.entry(day).or_default().absorb(granted);
    }

    /// Every strategy's days, in strategy then day order.
    pub fn by_strategy(&self) -> &BTreeMap<StrategyId, BTreeMap<Timestamp, GrantedDay>> {
        &self.by_strategy
    }

    /// The desk calendar: every day on which any strategy held a live grant,
    /// oldest first.
    pub fn days(&self) -> impl Iterator<Item = Timestamp> {
        self.desk.keys().copied()
    }

    /// What the whole book attributed on a day, over what it was granted.
    pub fn desk(&self, day: Timestamp) -> Option<GrantedDay> {
        self.desk.get(&day).copied()
    }

    pub fn strategy_count(&self) -> usize {
        self.by_strategy.len()
    }

    pub fn day_count(&self) -> usize {
        self.desk.len()
    }

    pub fn is_empty(&self) -> bool {
        self.desk.is_empty()
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

    /// The distinction the retention change exists to make: a day under a
    /// grant that settled nothing is a return of zero, and a day under no
    /// grant is not an observation at all. Before `retain_grant` the first
    /// left no session, so the two were the same absence afterwards.
    #[test]
    fn a_day_under_a_grant_that_settled_nothing_is_a_return_of_zero_and_a_day_under_none_is_not() {
        let mut series = RealisedSeries::default();
        series.retain_grant(day(0), dec!("10000"));
        // Day 1: nothing at all. No report, no grant, no session.
        let observed: Vec<(Timestamp, GrantedDay)> = series.granted_days(day(2)).collect();
        assert_eq!(
            observed.len(),
            1,
            "one day held a grant and one held nothing: {observed:?}"
        );
        assert_eq!(observed[0].0, day(0).start_of_day());
        assert_eq!(observed[0].1.fraction(), Some(0.0));
        assert_eq!(observed[0].1.pnl, Decimal::ZERO);
    }

    /// `capital` is not `grant`. A settled day whose only denominator came
    /// from the envelope map — expired or not, the map keeps it — states no
    /// aligned observation, because nothing established that a grant was live.
    #[test]
    fn a_settled_day_with_no_live_grant_states_no_aligned_observation() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("-300"), Some(dec!("10000")));
        assert_eq!(
            series
                .outcome(&id(), "cell", day(0), day(1))
                .map(|outcome| outcome.realised_returns),
            Some(vec![-0.03]),
            "premise: the demotion monitor still reads the day against `capital`"
        );
        assert_eq!(
            series.granted_days(day(1)).count(),
            0,
            "and the calendar states nothing, because no live grant was retained"
        );
    }

    /// A retained grant is not an observation the demotion monitor judges.
    /// Admitting one would break a run of losing days with a day nothing
    /// traded on, which is an input to a kill condition.
    #[test]
    fn a_retained_grant_changes_nothing_the_demotion_monitor_reads() {
        let mut series = RealisedSeries::default();
        series.absorb(day(0), dec!("-100"), Some(dec!("10000")));
        series.retain_grant(day(1), dec!("10000"));
        series.absorb(day(2), dec!("-200"), Some(dec!("10000")));
        let outcome = series
            .outcome(&id(), "cell", day(0), day(3))
            .expect("two settled sessions");
        assert_eq!(
            outcome.realised_returns,
            vec![-0.01, -0.02],
            "the grant-only day adds no zero to the series the decay trigger reads"
        );
        assert_eq!(
            outcome.consecutive_losing_days, 2,
            "and does not break the losing run"
        );
        assert_eq!(outcome.realised_loss, dec!("300"));
        assert_eq!(
            series.granted_days(day(3)).count(),
            1,
            "premise: the grant-only day is retained and is exactly the day the \
             calendar gains"
        );
    }

    /// The day still being traded is not an aligned observation either.
    #[test]
    fn a_grant_retained_today_is_not_read_until_the_day_has_closed() {
        let mut series = RealisedSeries::default();
        series.retain_grant(day(0), dec!("10000"));
        assert_eq!(series.granted_days(day(0)).count(), 0);
        assert_eq!(series.granted_days(day(1)).count(), 1);
    }

    /// The bound holds on the writer that creates sessions without a
    /// settlement too. A strategy that holds a grant and trades rarely would
    /// otherwise grow the working set without limit.
    #[test]
    fn retaining_a_grant_evicts_the_oldest_session_like_a_settlement_does() {
        let mut series = RealisedSeries::default();
        for n in 0..(REALISED_SESSIONS as i64 + 3) {
            series.retain_grant(day(n), dec!("10000"));
        }
        assert_eq!(series.sessions().count(), REALISED_SESSIONS);
        assert_eq!(
            series.sessions().next().map(|session| session.day),
            Some(day(3).start_of_day())
        );
    }

    /// A cell reports many times a day and its reports arrive out of order.
    /// Neither doubles a day's grant nor opens a second session for a day
    /// already retained: the grant is a fact about the day, stated again,
    /// not a quantity accumulated per report.
    #[test]
    fn a_grant_retained_twice_on_one_day_is_one_days_grant_and_a_late_report_keeps_its_own_day() {
        let mut series = RealisedSeries::default();
        series.retain_grant(day(5), dec!("10000"));
        series.retain_grant(day(3), dec!("10000"));
        series.retain_grant(
            day(3).saturating_add(Duration::from_hours(4)),
            dec!("10000"),
        );
        let observed: Vec<(Timestamp, GrantedDay)> = series.granted_days(day(6)).collect();
        assert_eq!(
            observed.iter().map(|(day, _)| *day).collect::<Vec<_>>(),
            vec![day(3).start_of_day(), day(5).start_of_day()],
            "two days, oldest first, whatever order the reports came in"
        );
        assert_eq!(
            observed[0].1.grant,
            dec!("10000"),
            "three reports, one grant: the second statement of a fact is not a second grant"
        );
    }

    /// Two cells' grants for one strategy are one day of the calendar, and
    /// the day's return is capital-weighted rather than an average of two
    /// per-cell returns.
    #[test]
    fn a_strategys_day_sums_the_cells_that_granted_it() {
        let mut calendar = RealisedCalendar::default();
        calendar.absorb(
            &id(),
            day(0),
            GrantedDay {
                pnl: dec!("100"),
                grant: dec!("10000"),
            },
        );
        calendar.absorb(
            &id(),
            day(0),
            GrantedDay {
                pnl: dec!("-100"),
                grant: dec!("30000"),
            },
        );
        assert_eq!(calendar.day_count(), 1);
        assert_eq!(calendar.strategy_count(), 1);
        assert_eq!(
            calendar.desk(day(0).start_of_day()).map(|d| d.grant),
            Some(dec!("40000")),
            "the day carries both grants, because both were held"
        );
        assert_eq!(
            calendar
                .desk(day(0).start_of_day())
                .and_then(|d| d.fraction()),
            Some(0.0),
            "100 made on 10,000 and 100 lost on 30,000 is nothing on 40,000, not the \
             mean of +1% and -0.33%"
        );
    }
}
