//! What the realised corpus says about family structure — blueprint §23.1
//! LEVEL 1, measured on this platform's own returns.
//!
//! `qip_optimization_engine::families` clusters strategies on the correlation
//! measured **inside a stated stress window**, and refuses to cluster on the
//! full sample: two strategies that look independent in calm markets and move
//! together in a drawdown are one bet, and an allocator told it holds two
//! would find out in the drawdown. The stage was complete and tested and had
//! no caller, because nothing here could hand it one series per strategy on
//! one calendar. [`super::realised`] now can. This is the seam between them.
//!
//! # What this measures, and what it does not
//!
//! It measures. It does not allocate: no seam in this platform takes a
//! [`FamilyAssignment`] — `qip_portfolio_engine`'s construction is per
//! instrument, `qip_capital` sizes per strategy, and `qip_lifecycle`'s
//! `StrategyFamily` is a provenance key fixed at enrolment which a family
//! recomputed every cycle must not be confused with. Wiring a decision to a
//! family nothing consumes would be a gate with no subject.
//!
//! What it produces is the number the optimisation crate's own documentation
//! says has never been computed over a real population:
//! [`Diagnostics::pairs_calm_would_have_misfiled`], the cost of keying on
//! stress rather than on the full sample, for this desk's strategies on this
//! desk's days. That, and [`Diagnostics::mean_stress_excess`] beside it, is
//! evidence a person can check. It reaches the cycle journal, so it is
//! reproducible from the log.
//!
//! # The stress axis, and why it is the desk's own book
//!
//! [`StressWindow`] wants a benchmark whose tail is stress. The platform
//! records no regime classification, no volatility index and no funding
//! spread, so there is exactly one series derivable from facts it holds: the
//! desk's own attributed daily return over its granted book — every
//! strategy's P&L on the day over every grant behind it, which
//! [`RealisedCalendar::desk`] accumulates as the per-strategy days are
//! absorbed.
//!
//! That is not a fallback. The family boundary exists to survive the moment
//! diversification is being asked for, and the moment this desk asks for it is
//! the day this desk is losing. A volatility index would be somebody else's
//! definition of a bad day, correct for a book this one does not hold.
//!
//! It has a bias and the bias is recorded rather than argued away. Selecting
//! the worst days of an aggregate and then measuring correlation *among its
//! own components* is exceedance selection, and it pushes the estimated stress
//! correlation up: conditioning on the sum being extreme makes the parts look
//! more alike than they are. The direction is the conservative one — an
//! inflated stress correlation merges strategies into one family and reports
//! *less* diversification than the desk may hold — so it errs toward the
//! allocator being told it has fewer bets. But `mean_stress_excess` cut this
//! way is not an unbiased test of the blueprint's claim that calm correlation
//! understates stress correlation, and must not be quoted as one. The window
//! carries a provenance string saying exactly how it was cut, and that string
//! travels into [`StressCorrelation::provenance`] so the caveat arrives with
//! the number.
//!
//! # The window, and the arithmetic that fixes it
//!
//! [`StressWindow`] refuses fewer than
//! [`qip_optimization_engine::families::MIN_WINDOW_OBSERVATIONS`] — twelve —
//! on either side. With a tail fraction `q` over `w` observations that means
//! `floor(w·q) >= 12` and `w - floor(w·q) >= 12`. Two consequences worth
//! stating plainly:
//!
//! * At [`REALISED_SESSIONS`] = 252 retained sessions the tightest tail
//!   expressible at all is `12/252 ≈ 4.77%`. A 4% tail is refused outright; a
//!   1% tail would need about 1,200 retained sessions, which is a retention
//!   decision and not a caller's to take.
//! * [`CLUSTERING_WINDOW`] = 120 at [`STRESS_QUANTILE`] = 0.10 gives exactly
//!   twelve stress sessions and 108 calm ones. A decile of a daily series is
//!   the desk's worst fortnight in six months, which is a defensible reading
//!   of "stress" and is the tightest tail six months of sessions admits.
//!
//! Waiting for 240 sessions to cut a 5% tail was the alternative, and it was
//! rejected: the stage would then say nothing for a year after the first
//! grant, and what it says first is a number nobody here has ever measured.
//!
//! The window is a fixed count rather than "everything retained" because every
//! strategy in a run must be present on **every** day of it — that is what
//! aligned means — so a longer window silently drops the younger strategies.
//! Fixing it makes that trade explicit and keeps the quantile arithmetic from
//! moving as the corpus fills.
//!
//! # Determinism
//!
//! Everything here is a function of the retained sessions and `now`: the
//! window is the last [`CLUSTERING_WINDOW`] days of a `BTreeMap`'s key order,
//! the strategies are those complete over it in `BTreeMap` order, the target
//! family count is a function of the population size, and the clustering
//! itself sorts its input and has no random initialisation. A replay of the
//! same reports produces the same families.

use super::plane::CentralPlane;
use super::realised::{GrantedDay, REALISED_SESSIONS, RealisedCalendar};
use qip_core::error::{Error, Result};
use qip_core::{Timestamp, ids::StrategyId as NumericStrategyId};
use qip_numerics::stats;
use qip_optimization_engine::families::{
    Diagnostics, FamilyAssignment, FamilyClustering, MAX_STRATEGIES, StrategyReturns, StressAxis,
    StressCorrelation, StressWindow,
};
use serde::{Deserialize, Serialize};

/// Closed sessions one clustering is measured over. See the module note for
/// why this is fixed and why it is this number.
pub const CLUSTERING_WINDOW: usize = 120;

/// The fraction of the window whose worst readings count as stress.
pub const STRESS_QUANTILE: f64 = 0.10;

/// What the LEARN stage's family measurement left in the journal.
///
/// Counts and the four correlation figures, not the families themselves. The
/// membership is a function of the retained sessions and reproducible from
/// them; what the entry has to record is that the measurement *ran*, over how
/// much, and what it found — in particular how many pairs the calm view would
/// have filed differently, which is the cost of the design decision the
/// clustering rests on and had never been computed over a real population.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilyStructureJournal {
    /// Closed sessions the window covered.
    pub sessions: usize,
    /// Strategies clustered.
    pub strategies: usize,
    /// Strategies with a granted day in the window but not on every day of
    /// it, left out because a series that is not aligned cannot be
    /// correlated. Counted rather than dropped silently: a run over three of
    /// thirty strategies is a different claim from a run over thirty.
    pub excluded_unaligned: usize,
    /// Strategies that did not move at all inside the stress window, left out
    /// because a family boundary drawn around a flat series asserts a
    /// relationship nobody estimated.
    pub excluded_flat: usize,
    pub families: usize,
    pub stress_sessions: usize,
    pub calm_sessions: usize,
    /// Mean stress-less-calm correlation over every pair. Positive means the
    /// calm view understated co-movement in this population — biased upward
    /// by the window's own construction; see the module note.
    pub mean_stress_excess: f64,
    pub mean_intra_family_correlation: f64,
    pub mean_inter_family_correlation: f64,
    /// Pairs the calm-keyed clustering would have filed differently: the cost
    /// of keying on stress, in pairs, for this population.
    pub pairs_calm_would_have_misfiled: usize,
    pub pairs_total: usize,
}

impl FamilyStructureJournal {
    fn of(assignment: &FamilyAssignment, excluded_unaligned: usize, excluded_flat: usize) -> Self {
        let Diagnostics {
            strategies,
            stress_observations,
            calm_observations,
            mean_stress_excess,
            mean_intra_family_correlation,
            mean_inter_family_correlation,
            pairs_calm_would_have_misfiled,
            pairs_total,
            ..
        } = *assignment.diagnostics();
        Self {
            sessions: stress_observations + calm_observations,
            strategies,
            excluded_unaligned,
            excluded_flat,
            families: assignment.family_count(),
            stress_sessions: stress_observations,
            calm_sessions: calm_observations,
            mean_stress_excess,
            mean_intra_family_correlation,
            mean_inter_family_correlation,
            pairs_calm_would_have_misfiled,
            pairs_total,
        }
    }

    /// The line an operator reads in the stage's account of itself.
    pub fn describe(&self) -> String {
        format!(
            "{} strategy(ies) clustered into {} family(ies) on {} closed session(s) ({} stress, \
             {} calm); stress correlation exceeds calm by {:.4} on average and the calm view \
             would have misfiled {} of {} pair(s) ({} strategy(ies) unaligned, {} flat under \
             stress)",
            self.strategies,
            self.families,
            self.sessions,
            self.stress_sessions,
            self.calm_sessions,
            self.mean_stress_excess,
            self.pairs_calm_would_have_misfiled,
            self.pairs_total,
            self.excluded_unaligned,
            self.excluded_flat
        )
    }
}

impl CentralPlane {
    /// Measure the family structure of what this plane's cells have realised.
    ///
    /// `Ok(None)` where the corpus does not yet carry an aligned window — no
    /// day-keyed observations at all, fewer than [`CLUSTERING_WINDOW`] closed
    /// sessions, or fewer than two strategies granted on every one of them.
    /// That is most cycles on a platform whose centre has issued no grant, and
    /// it is quiet on purpose: a stage that reported a shortfall every cycle
    /// for six months would bury the cycle in a fact that has not changed.
    ///
    /// `Err` where a window *was* assembled and the clustering refused it.
    /// That is a finding — a matrix that is not positive semi-definite, a
    /// population over [`MAX_STRATEGIES`] — and the caller reports it as a
    /// problem on the cycle rather than swallowing it.
    pub fn family_structure(&self, now: Timestamp) -> Result<Option<FamilyStructureJournal>> {
        measure(&self.realised_calendar(now))
    }
}

/// Cluster the aligned tail of a calendar, or say why there is not one.
///
/// Separate from the plane so the arithmetic can be exercised against a
/// calendar built by hand, without a signing key, an approval chain and a
/// cell report standing between a test and the thing under test.
pub fn measure(calendar: &RealisedCalendar) -> Result<Option<FamilyStructureJournal>> {
    let days: Vec<Timestamp> = calendar.days().collect();
    // The most recent window, because a clustering is a statement about the
    // population as it stands, and the oldest of 252 retained sessions is a
    // year of promotions and retirements away from it. The shortfall is one
    // subtraction rather than a length check beside a slice: two expressions
    // of the same bound would eventually disagree, and the one that lost
    // would take a panic out of a `Result`-returning function with it.
    let Some(first) = days.len().checked_sub(CLUSTERING_WINDOW) else {
        return Ok(None);
    };
    let window = &days[first..];

    let mut aligned: Vec<(NumericStrategyId, Vec<f64>)> = Vec::new();
    let mut unaligned = 0usize;
    for (strategy, series) in calendar.by_strategy() {
        // `None` the moment one day of the window is missing: a strategy
        // granted on 119 of 120 days is not aligned to the calendar, and the
        // one absent day must not be filled with a zero — that would invent
        // the very observation this corpus was changed to stop inventing.
        let returns: Option<Vec<f64>> = window
            .iter()
            .map(|day| series.get(day).and_then(GrantedDay::fraction))
            .collect();
        match returns {
            // The two `StrategyId` types are two newtypes over the same
            // string and both order by it, so the canonical sort the
            // clustering imposes is the order this `BTreeMap` was already in.
            Some(returns) => {
                aligned.push((NumericStrategyId::from_string(strategy.as_str()), returns))
            }
            None => unaligned += 1,
        }
    }
    if aligned.len() < 2 {
        return Ok(None);
    }
    // Refused here, before a `Vec<StrategyReturns>` of that size is built and
    // long before the cubic merge behind it. The bound belongs to the
    // clustering stage and is not raised to admit a population: what an
    // over-large desk needs is a pre-partition by horizon or venue ahead of
    // this seam, which nothing in this platform builds.
    if aligned.len() > MAX_STRATEGIES {
        return Err(Error::invalid(format!(
            "the realised corpus carries {} aligned strategies, above the {MAX_STRATEGIES} the \
             family stage clusters in one pass; pre-partition the population before this seam \
             rather than raising the bound",
            aligned.len()
        )));
    }

    let benchmark: Vec<f64> = window
        .iter()
        .map(|day| {
            calendar
                .desk(*day)
                .and_then(|desk| desk.fraction())
                .ok_or_else(|| {
                    Error::guard(format!(
                        "the desk calendar holds no return for {}, a day it listed itself; the \
                         per-strategy days and the desk total have come apart",
                        day.as_secs()
                    ))
                })
        })
        .collect::<Result<Vec<f64>>>()?;

    // The axis is stated rather than defaulted. This benchmark is a return
    // series: it is worst at its most negative, so the low tail is the stress.
    // A volatility index or a funding spread would be the other way round, and
    // a caller that inherited a default would key its families on the twelve
    // calmest sessions the moment the desk changed what it measures stress by.
    let stress = StressWindow::worst_quantile_on(
        &benchmark,
        STRESS_QUANTILE,
        StressAxis::LowReadingsAreStress,
        format!(
            "the desk's own attributed daily return over its granted book; worst {:.0}% of the \
             last {CLUSTERING_WINDOW} closed sessions. Cut from the aggregate of the same \
             strategies it selects days for, so the stress correlation it keys is biased upward \
             by exceedance selection",
            STRESS_QUANTILE * 100.0
        ),
    )?;

    let mut series: Vec<StrategyReturns> = Vec::new();
    let mut flat = 0usize;
    for (strategy, returns) in aligned {
        // The exclusion `StressCorrelation::from_returns`'s own refusal names
        // as the remedy — "exclude it or widen the window" — applied here so
        // that one strategy which held a grant and traded nothing through the
        // desk's worst fortnight does not cost the whole desk its clustering.
        // `from_returns` remains the authority: if this filter and its check
        // ever disagree, it refuses and the stage reports the refusal.
        //
        // Collected through `get` rather than indexed, and a miss refused
        // rather than defaulted: a zero substituted for an observation the
        // window named would move a standard deviation, and this is the one
        // arithmetic in the stage that decides whether a strategy is
        // clustered at all.
        let inside: Option<Vec<f64>> = stress
            .stress_indices()
            .iter()
            .map(|index| returns.get(*index).copied())
            .collect();
        let Some(inside) = inside else {
            return Err(Error::guard(format!(
                "the stress window names an observation outside the {CLUSTERING_WINDOW} the \
                 series carries; the window and the calendar were built from different lengths"
            )));
        };
        if stats::stddev(&inside) <= 0.0 {
            flat += 1;
            continue;
        }
        series.push(StrategyReturns::new(strategy, returns)?);
    }
    if series.len() < 2 {
        return Ok(None);
    }

    let correlation = StressCorrelation::from_returns(&series, &stress)?;
    let assignment = FamilyClustering::new(target_families(series.len()))?.cluster(&correlation)?;
    Ok(Some(FamilyStructureJournal::of(
        &assignment,
        unaligned,
        flat,
    )))
}

/// How many families to ask for, given the population.
///
/// The square root of the population, rounded up: a stated default, not a
/// measurement. Nothing in this platform consumes a family, so no consumer
/// states a target, and the alternative to a rule is a constant that would be
/// wrong at both ends — thirty-two families over forty strategies asserts a
/// diversification nobody has, and two over four hundred asserts none. The
/// rule is a function of the population size alone, so a replay reproduces it,
/// and the figure asked for is recorded beside what it produced.
///
/// The bound is on a derived figure rather than on an input: a target above
/// the population is refused by [`FamilyClustering::cluster`], and one of zero
/// by [`FamilyClustering::new`], so neither may be handed on.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn target_families(strategies: usize) -> usize {
    ((strategies as f64).sqrt().ceil() as usize).clamp(1, strategies)
}

// The window has to fit inside what the corpus retains, or the stage would
// wait for a calendar the eviction bound will never let it have. Checked at
// compile time because the two constants are set in two crates: a session
// bound lowered in `central::realised` would otherwise make this stage
// silently permanent-`None` rather than failing anywhere a reader looks.
const _: () = {
    assert!(CLUSTERING_WINDOW <= REALISED_SESSIONS);
};

#[cfg(test)]
mod tests {
    use super::*;
    use qip_contracts::signal::StrategyId;
    use qip_core::{Duration, dec};
    use qip_optimization_engine::families::MIN_WINDOW_OBSERVATIONS;

    fn day(n: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
            .start_of_day()
            .saturating_add(Duration::from_days(n))
    }

    /// A calendar of `days` sessions in which strategy `n` returns
    /// `shape(n, d)` of a grant of 10,000 on day `d`.
    fn calendar(
        strategies: usize,
        days: i64,
        shape: impl Fn(usize, i64) -> f64,
    ) -> RealisedCalendar {
        let mut calendar = RealisedCalendar::default();
        let grant = dec!("10000");
        for index in 0..strategies {
            let id = StrategyId::new(format!("strategy-{index:03}"));
            for d in 0..days {
                let pnl = qip_core::Decimal::from_f64(shape(index, d) * grant.to_f64())
                    .unwrap_or(qip_core::Decimal::ZERO);
                calendar.absorb(&id, day(d), GrantedDay { pnl, grant });
            }
        }
        calendar
    }

    /// The window and the quantile have to admit the correlation stage's own
    /// floor on both sides, or every measurement this module makes is refused
    /// at the last step. Asserted rather than commented because the two
    /// constants are set here and the floor is set in another crate.
    #[test]
    fn the_window_and_the_quantile_clear_the_correlation_stages_floor_on_both_sides() {
        #[allow(clippy::cast_precision_loss)]
        let stress = (CLUSTERING_WINDOW as f64 * STRESS_QUANTILE).floor() as usize;
        assert!(
            stress >= MIN_WINDOW_OBSERVATIONS,
            "a {CLUSTERING_WINDOW}-session window at {STRESS_QUANTILE} cuts {stress} stress \
             sessions, below the {MIN_WINDOW_OBSERVATIONS} a correlation is estimated from"
        );
        assert!(
            CLUSTERING_WINDOW - stress >= MIN_WINDOW_OBSERVATIONS,
            "the calm complement holds {} sessions, below {MIN_WINDOW_OBSERVATIONS}",
            CLUSTERING_WINDOW - stress
        );
    }

    /// The quiet cases are quiet: a corpus shorter than the window, and one
    /// with a long calendar but only one strategy on it, are both absences
    /// rather than problems.
    #[test]
    fn a_corpus_without_an_aligned_window_is_measured_as_an_absence_not_a_problem() {
        let short = calendar(4, (CLUSTERING_WINDOW - 1) as i64, |n, d| {
            0.001 * ((n + 1) as f64) * ((d % 7) as f64 - 3.0)
        });
        assert_eq!(
            short.day_count(),
            CLUSTERING_WINDOW - 1,
            "premise: the calendar is one session short of the window"
        );
        assert_eq!(
            measure(&short).expect("a short corpus is not an error"),
            None
        );

        let lonely = calendar(1, CLUSTERING_WINDOW as i64, |_, d| 0.001 * (d as f64));
        assert_eq!(
            lonely.day_count(),
            CLUSTERING_WINDOW,
            "premise: the calendar is long enough"
        );
        assert_eq!(
            measure(&lonely).expect("one strategy is not an error"),
            None,
            "one strategy has nothing to be correlated with"
        );
    }

    /// A strategy granted on all but one day of the window is left out and
    /// counted, and the missing day is not filled. This is the whole point of
    /// the retention change: an absence stays an absence.
    #[test]
    fn a_strategy_missing_one_session_is_excluded_rather_than_filled_with_a_zero() {
        let mut corpus = calendar(3, CLUSTERING_WINDOW as i64, |n, d| {
            0.002 * ((n + 1) as f64) * (((d + n as i64) % 5) as f64 - 2.0)
        });
        let missing = StrategyId::new("strategy-002");
        let held = corpus
            .by_strategy()
            .get(&missing)
            .map(std::collections::BTreeMap::len)
            .unwrap_or_default();
        assert_eq!(
            held, CLUSTERING_WINDOW,
            "premise: the strategy starts aligned on every session"
        );

        let journal = measure(&corpus)
            .expect("an aligned corpus clusters")
            .expect("three strategies over the window are enough");
        assert_eq!(journal.strategies, 3);
        assert_eq!(journal.excluded_unaligned, 0);

        // Now take one day away from one strategy. The desk calendar still
        // holds the day, because the other two were granted on it.
        corpus = calendar(2, CLUSTERING_WINDOW as i64, |n, d| {
            0.002 * ((n + 1) as f64) * (((d + n as i64) % 5) as f64 - 2.0)
        });
        for d in 0..(CLUSTERING_WINDOW as i64) {
            if d == 4 {
                continue;
            }
            let pnl =
                qip_core::Decimal::from_f64(0.002 * 3.0 * ((((d + 2) % 5) as f64) - 2.0) * 10000.0)
                    .unwrap_or(qip_core::Decimal::ZERO);
            corpus.absorb(
                &missing,
                day(d),
                GrantedDay {
                    pnl,
                    grant: dec!("10000"),
                },
            );
        }
        assert_eq!(
            corpus.day_count(),
            CLUSTERING_WINDOW,
            "premise: the desk calendar is unchanged — the other two held the day"
        );
        let journal = measure(&corpus)
            .expect("an aligned corpus clusters")
            .expect("two aligned strategies are enough");
        assert_eq!(
            journal.strategies, 2,
            "the strategy short one session is not clustered"
        );
        assert_eq!(
            journal.excluded_unaligned, 1,
            "and it is counted rather than dropped silently"
        );
    }

    /// The measurement itself, over a population with a planted structure:
    /// two pairs that move together, and the window cut from the desk's own
    /// worst decile.
    #[test]
    fn two_pairs_that_move_together_are_measured_and_the_window_carries_its_provenance() {
        // Strategies 0 and 1 track one factor, 2 and 3 another, each with a
        // per-strategy wobble so no two series are identical.
        let corpus = calendar(4, CLUSTERING_WINDOW as i64, |n, d| {
            let factor = if n < 2 {
                ((d % 11) as f64 - 5.0) * 0.002
            } else {
                ((d % 7) as f64 - 3.0) * 0.002
            };
            factor + ((d % 3) as f64 - 1.0) * 0.0002 * ((n + 1) as f64)
        });
        let journal = measure(&corpus)
            .expect("the corpus clusters")
            .expect("four aligned strategies over the window");
        assert_eq!(journal.strategies, 4);
        assert_eq!(journal.sessions, CLUSTERING_WINDOW);
        assert_eq!(
            journal.stress_sessions, MIN_WINDOW_OBSERVATIONS,
            "a decile of 120 sessions is twelve"
        );
        assert_eq!(
            journal.calm_sessions,
            CLUSTERING_WINDOW - MIN_WINDOW_OBSERVATIONS
        );
        assert_eq!(journal.families, 2, "four strategies are asked for two");
        assert_eq!(journal.pairs_total, 6);
        assert!(
            journal.mean_intra_family_correlation > journal.mean_inter_family_correlation,
            "a clustering that did any work puts the co-movers together: intra {} inter {}",
            journal.mean_intra_family_correlation,
            journal.mean_inter_family_correlation
        );
        assert!(
            journal.describe().contains("clustered into 2 family(ies)"),
            "the stage says what it measured: {}",
            journal.describe()
        );
    }

    /// A strategy that held a grant and settled nothing through the desk's
    /// worst decile is excluded and counted, rather than filed in a family on
    /// a flat series or costing the whole desk its clustering.
    #[test]
    fn a_strategy_flat_through_the_stress_window_is_excluded_and_counted() {
        let mut corpus = calendar(3, CLUSTERING_WINDOW as i64, |n, d| {
            ((d % 11) as f64 - 5.0) * 0.002 + ((d % 3) as f64 - 1.0) * 0.0003 * ((n + 1) as f64)
        });
        let flat = StrategyId::new("strategy-flat");
        for d in 0..(CLUSTERING_WINDOW as i64) {
            corpus.absorb(
                &flat,
                day(d),
                GrantedDay {
                    pnl: qip_core::Decimal::ZERO,
                    grant: dec!("10000"),
                },
            );
        }
        let journal = measure(&corpus)
            .expect("the corpus clusters")
            .expect("three moving strategies remain");
        assert_eq!(
            journal.strategies, 3,
            "the flat strategy is not one of the clustered"
        );
        assert_eq!(journal.excluded_flat, 1);
        assert_eq!(
            journal.excluded_unaligned, 0,
            "it was granted on every session — its exclusion is about movement, not alignment"
        );
    }

    /// The population bound is refused at this seam, before anything cubic
    /// runs. The refusal names pre-partitioning, which is what an over-large
    /// desk actually needs.
    #[test]
    fn a_population_over_the_clustering_bound_is_refused_before_the_merge() {
        let corpus = calendar(MAX_STRATEGIES + 1, CLUSTERING_WINDOW as i64, |n, d| {
            0.001 * (((d + n as i64) % 5) as f64 - 2.0)
        });
        assert_eq!(
            corpus.strategy_count(),
            MAX_STRATEGIES + 1,
            "premise: the population is one over the bound"
        );
        let error = measure(&corpus).expect_err("a population over the bound is refused");
        assert!(
            error
                .message()
                .contains("pre-partition the population before this seam"),
            "the refusal is this seam's own, not the clustering stage's: {}",
            error.message()
        );
    }
}
