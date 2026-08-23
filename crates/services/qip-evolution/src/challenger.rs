//! Champion against challenger, on the same data, with the search declared.
//!
//! Generating a thousand strategies and putting the best one forward
//! guarantees a good backtest and means nothing. The best of a thousand
//! coin-flipping strategies over two years of daily data shows an annualised
//! Sharpe above two, and it shows it for the same reason the tallest of a
//! thousand people is tall: because a thousand were measured.
//!
//! [`qip_simulation_engine::validation::deflated_sharpe`] exists precisely for
//! this. It computes the Sharpe ratio the best of `trials` random strategies
//! would have shown by chance and asks whether the observed one exceeds it.
//! This module makes the trial count a required input rather than a parameter
//! with a comfortable default, and then goes one step further: the count is
//! [`TrialCount`], which has a private field and no constructor at all. The
//! only way to obtain one is [`TrialLedger::declared`], and the only way a
//! ledger's count moves is by folding in a [`GenerationRun`] or a
//! [`MutationRun`] — values that exist only because a generator or a mutator
//! actually ran. A caller cannot pass `1`, because there is no expression in
//! any crate that produces a `TrialCount` of one without one candidate having
//! been evaluated.
//!
//! The same reasoning applies to costs. A Sharpe ratio computed on gross
//! returns is a claim about a strategy nobody can trade, and the usual way it
//! reaches a promotion is that somebody forgot rather than that somebody lied.
//! [`ChallengerEntry`] therefore takes [`NetReturns`], which cannot be built
//! from a return series alone — see [`crate::cost_model`] — and a series that
//! was charged nothing over the whole window is refused rather than scored.
//!
//! Two bars are computed and both are reported:
//!
//! * The **naive** bar — the challenger's Sharpe beats the champion's by a
//!   margin. This is what a spreadsheet comparison would say, and it is
//!   reported so the gap between the two answers is visible rather than
//!   hidden behind a single verdict.
//! * The **deflated** bar — the challenger's Sharpe clears what the search
//!   alone explains, and the deflated probability is credible. Only this one
//!   decides. Below the selection threshold the deflated probability rises
//!   with uncertainty instead of falling, so it is not a confidence and is not
//!   read: see [`DeflatedSharpe::clears_selection_threshold`].
//!
//! Both series must cover the same window. A challenger measured over a
//! different period than the champion is not being compared with it.

use qip_simulation_engine::validation::{DeflatedSharpe, deflated_sharpe};

use crate::cost_model::NetReturns;
use crate::generate::{Candidate, GenerationRun};
use crate::mutate::MutationRun;
use qip_contracts::StrategyId;
use qip_core::error::{Error, Result};
use qip_numerics::stats;
use std::fmt;

/// How many candidates a search actually scored.
///
/// A number that was **counted**, not asserted. The field is private and there
/// is no constructor: [`TrialLedger::declared`] is the only expression in the
/// platform that produces one, and it produces the ledger's own tally. That is
/// the whole defence against the cheapest way to launder a selection bias,
/// which is to write `trials: 1` and move on.
///
/// ```compile_fail
/// # use qip_evolution::challenger::TrialCount;
/// // No constructor, and the field is private: a trial count cannot be
/// // asserted, only counted.
/// let forged = TrialCount(1);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TrialCount(usize);

impl TrialCount {
    /// The count, for handing to the deflation.
    pub const fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for TrialCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Counts every candidate a search actually scored.
///
/// The number that deflates a Sharpe ratio is the number of strategies whose
/// results were looked at, because that is the size of the set the best was
/// picked from. Candidates the compiler refused were never scored and did not
/// contribute a draw to that maximum, so they are counted separately and
/// reported rather than folded in — understating trials flatters the result
/// and overstating them buries a real one.
///
/// A ledger only goes up. There is no method that removes a trial, because
/// every reason anyone would want one is a reason to distrust the answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrialLedger {
    evaluated: usize,
    refused: usize,
}

impl TrialLedger {
    pub const fn new() -> Self {
        Self {
            evaluated: 0,
            refused: 0,
        }
    }

    /// Fold in a generation run.
    pub fn record_generation(&mut self, run: &GenerationRun) {
        self.evaluated = self.evaluated.saturating_add(run.evaluable());
        self.refused = self.refused.saturating_add(run.discarded().len());
    }

    /// Fold in a mutation run.
    pub fn record_mutation(&mut self, run: &MutationRun) {
        self.evaluated = self.evaluated.saturating_add(run.evaluable());
        self.refused = self.refused.saturating_add(run.rejected().len());
    }

    /// Candidates that reached an evaluation. This is the trial count.
    pub const fn trials(&self) -> usize {
        self.evaluated
    }

    /// Candidates the compiler refused, which never reached an evaluation.
    pub const fn refused(&self) -> usize {
        self.refused
    }

    /// The trial count as the challenger test wants it, or `None` when nothing
    /// has been recorded — a search of size zero cannot have produced a
    /// challenger, so the absence is reported rather than smoothed to one.
    pub const fn declared(&self) -> Option<TrialCount> {
        if self.evaluated == 0 {
            None
        } else {
            Some(TrialCount(self.evaluated))
        }
    }
}

/// One challenger, its champion, and the search that produced it.
///
/// Every field is private, and the two constructors differ in exactly one way:
/// [`Self::from_ledger`] takes the ledger that counted the search, and
/// [`Self::undeclared`] takes nothing and produces an entry the test refuses.
/// The second exists so that the refusal can be reached and tested, and is
/// named so that nobody reaches for it by accident.
///
/// `trials` is [`Option`] and not [`TrialCount`] for the same reason. There has
/// to be a representation of "nobody wrote it down" for the test to be able to
/// refuse it, and a default of one would silently turn an undeclared search
/// into the most flattering claim available.
#[derive(Clone, Debug, PartialEq)]
pub struct ChallengerEntry {
    champion: StrategyId,
    challenger: StrategyId,
    /// The champion's returns on the out-of-sample window, in time order, net
    /// of what it cost to earn them.
    champion_returns: NetReturns,
    /// The challenger's returns on the *same* window, net on the same basis.
    challenger_returns: NetReturns,
    periods_per_year: f64,
    trials: Option<TrialCount>,
}

impl ChallengerEntry {
    /// An entry whose trial count comes from the ledger the search kept.
    ///
    /// The challenger is a [`Candidate`] rather than a [`StrategyId`], so the
    /// thing being compared is one the compiler accepted. A string naming a
    /// strategy that never compiled cannot be entered into a comparison.
    pub fn from_ledger(
        champion: StrategyId,
        challenger: &Candidate,
        champion_returns: NetReturns,
        challenger_returns: NetReturns,
        periods_per_year: f64,
        ledger: &TrialLedger,
    ) -> Self {
        Self {
            champion,
            challenger: challenger.id().clone(),
            champion_returns,
            challenger_returns,
            periods_per_year,
            trials: ledger.declared(),
        }
    }

    /// An entry whose search size nobody recorded.
    ///
    /// [`ChallengerTest::evaluate`] refuses it. It exists so that the refusal
    /// is reachable from a test rather than being a branch nobody can execute,
    /// and it is deliberately the only constructor that does not mention a
    /// ledger.
    pub fn undeclared(
        champion: StrategyId,
        challenger: &Candidate,
        champion_returns: NetReturns,
        challenger_returns: NetReturns,
        periods_per_year: f64,
    ) -> Self {
        Self {
            champion,
            challenger: challenger.id().clone(),
            champion_returns,
            challenger_returns,
            periods_per_year,
            trials: None,
        }
    }

    pub const fn champion(&self) -> &StrategyId {
        &self.champion
    }

    pub const fn challenger(&self) -> &StrategyId {
        &self.challenger
    }

    pub const fn champion_returns(&self) -> &NetReturns {
        &self.champion_returns
    }

    pub const fn challenger_returns(&self) -> &NetReturns {
        &self.challenger_returns
    }

    pub const fn periods_per_year(&self) -> f64 {
        self.periods_per_year
    }

    /// The declared search size, or `None` where none was.
    pub const fn trials(&self) -> Option<TrialCount> {
        self.trials
    }
}

/// What the challenger test concluded, with both bars visible.
#[derive(Clone, Debug, PartialEq)]
pub struct ChallengerVerdict {
    pub champion: StrategyId,
    pub challenger: StrategyId,
    /// Annualised Sharpe of the champion over the window, net of costs.
    pub champion_sharpe: f64,
    /// Annualised Sharpe of the challenger over the same window, net on the
    /// same basis.
    pub challenger_sharpe: f64,
    /// Challenger minus champion, annualised.
    pub margin: f64,
    /// What the challenger's window was charged, in return units. Reported next
    /// to the result it changed, because a cost model that deducts nothing is
    /// invisible in the Sharpe ratio alone.
    pub challenger_cost_charged: f64,
    /// What the champion's window was charged, on the same basis.
    pub champion_cost_charged: f64,
    /// The challenger's Sharpe with the search corrected for.
    pub deflated: DeflatedSharpe,
    /// Whether the margin alone would have been enough. Reported, not obeyed.
    pub clears_naive_bar: bool,
    /// Whether the challenger's Sharpe exceeds what the search alone explains.
    pub clears_selection_threshold: bool,
    /// Whether the deflated result is credible at the conventional bar. Only
    /// meaningful when the selection threshold is cleared.
    pub deflated_is_credible: bool,
    /// Every check that ran and what it concluded, in the same shape a
    /// lifecycle gate reports.
    pub findings: Vec<(String, bool, String)>,
}

impl ChallengerVerdict {
    /// Whether the challenger has actually earned the champion's place.
    ///
    /// One failing check is enough, exactly as at a lifecycle gate. There is
    /// no weighting: a large margin cannot buy its way past a trial count that
    /// explains it, because the trial count is the reason the margin is not
    /// evidence.
    pub fn challenger_wins(&self) -> bool {
        self.findings.iter().all(|(_, passed, _)| *passed)
    }

    /// The gap between the two answers, which is the whole point of the test.
    ///
    /// True where a spreadsheet would have promoted this challenger and the
    /// deflation says the search alone explains it.
    pub fn selection_explains_the_margin(&self) -> bool {
        self.clears_naive_bar && !self.clears_selection_threshold
    }

    /// The checks that failed, as lines an operator can read.
    pub fn failures(&self) -> Vec<String> {
        self.findings
            .iter()
            .filter(|(_, passed, _)| !passed)
            .map(|(name, _, detail)| format!("{name}: {detail}"))
            .collect()
    }

    pub fn summarise(&self) -> String {
        format!(
            "{} at Sharpe {:.2} against {} at {:.2} (margin {:.2}, after costs of {:.4} \
             and {:.4}); {} trial(s) alone would produce {:.2}, so the challenger {}",
            self.challenger,
            self.challenger_sharpe,
            self.champion,
            self.champion_sharpe,
            self.margin,
            self.challenger_cost_charged,
            self.champion_cost_charged,
            self.deflated.trials,
            self.deflated.expected_maximum,
            if self.challenger_wins() {
                "wins"
            } else {
                "does not win"
            }
        )
    }
}

/// The bars a challenger must clear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChallengerPolicy {
    /// Observations below which neither Sharpe is worth comparing.
    pub minimum_observations: usize,
    /// Annualised Sharpe the challenger must beat the champion by before the
    /// deflation is even considered.
    pub minimum_margin: f64,
}

impl Default for ChallengerPolicy {
    fn default() -> Self {
        Self {
            // The same bar the holdout gate applies: below a year of daily
            // observations a Sharpe ratio's standard error is wide enough that
            // the comparison is noise.
            minimum_observations: 250,
            // A quarter of a Sharpe point. Smaller than that and the switching
            // cost — re-approval, a fresh walk up the ladder, a period of
            // running two strategies — exceeds the improvement.
            minimum_margin: 0.25,
        }
    }
}

/// Champion against challenger on one out-of-sample window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChallengerTest {
    pub policy: ChallengerPolicy,
}

impl ChallengerTest {
    pub const fn new(policy: ChallengerPolicy) -> Self {
        Self { policy }
    }

    /// Compare the two, refusing anything that cannot be compared honestly.
    ///
    /// Four refusals are errors rather than a failed verdict, because each one
    /// means the question was not asked properly: an undeclared trial count,
    /// two series over different windows, a series too short to deflate, and a
    /// window that was charged nothing. Everything else is a finding on the
    /// verdict, where it can be read next to the checks that passed.
    pub fn evaluate(&self, entry: &ChallengerEntry) -> Result<ChallengerVerdict> {
        let Some(trials) = entry.trials else {
            return Err(Error::invalid(format!(
                "challenger {} was evaluated without declaring how many candidates were \
                 tried; the best of an undeclared search is not a result",
                entry.challenger
            )));
        };
        if entry.champion_returns.len() != entry.challenger_returns.len() {
            return Err(Error::invalid(format!(
                "champion has {} observations and challenger {}; a challenger measured \
                 over a different window is not being compared with the champion",
                entry.champion_returns.len(),
                entry.challenger_returns.len()
            )));
        }
        if entry.champion_returns.len() < self.policy.minimum_observations {
            return Err(Error::invalid(format!(
                "{} observations is below the {} needed to compare two Sharpe ratios",
                entry.champion_returns.len(),
                self.policy.minimum_observations
            )));
        }
        for (whose, series) in [
            ("challenger", &entry.challenger_returns),
            ("champion", &entry.champion_returns),
        ] {
            if series.is_costless() {
                return Err(Error::invalid(format!(
                    "the {whose}'s {} observation(s) were charged nothing; a strategy that \
                     traded for a year and paid nothing has a backtest, not a result",
                    series.len()
                )));
            }
        }

        let champion_sharpe =
            annualised_sharpe(entry.champion_returns.as_slice(), entry.periods_per_year);
        let challenger_sharpe =
            annualised_sharpe(entry.challenger_returns.as_slice(), entry.periods_per_year);
        let margin = challenger_sharpe - champion_sharpe;
        let deflated = deflated_sharpe(
            entry.challenger_returns.as_slice(),
            trials.get(),
            entry.periods_per_year,
        )?;

        let clears_naive_bar = margin >= self.policy.minimum_margin;
        let clears_selection_threshold = deflated.clears_selection_threshold();
        let deflated_is_credible = clears_selection_threshold && deflated.is_credible();

        let findings = vec![
            (
                "beats_the_champion".to_string(),
                clears_naive_bar,
                format!(
                    "Sharpe {challenger_sharpe:.2} against the champion's \
                     {champion_sharpe:.2}, a margin of {margin:.2} against a {:.2} bar",
                    self.policy.minimum_margin
                ),
            ),
            (
                "survives_the_search".to_string(),
                clears_selection_threshold,
                format!(
                    "{trials} trial(s) alone would produce a Sharpe of {:.2}; this one is \
                     {:.2}",
                    deflated.expected_maximum, deflated.observed
                ),
            ),
            (
                "deflated_sharpe_credible".to_string(),
                deflated_is_credible,
                if clears_selection_threshold {
                    deflated.summarise()
                } else {
                    // Quoting the probability here would be worse than saying
                    // nothing: below the threshold it rises with uncertainty, so
                    // a noisier challenger would read as a more confident one.
                    "below the selection threshold the deflated probability rises with \
                     uncertainty instead of falling, so it is not a confidence and is not \
                     read"
                        .to_string()
                },
            ),
        ];

        Ok(ChallengerVerdict {
            champion: entry.champion.clone(),
            challenger: entry.challenger.clone(),
            champion_sharpe,
            challenger_sharpe,
            margin,
            challenger_cost_charged: entry.challenger_returns.total_cost(),
            champion_cost_charged: entry.champion_returns.total_cost(),
            deflated,
            clears_naive_bar,
            clears_selection_threshold,
            deflated_is_credible,
            findings,
        })
    }
}

/// Annualised Sharpe, zero where the series does not vary.
///
/// Matches how [`deflated_sharpe`] computes its observed figure, so the two
/// numbers in a verdict are on the same scale.
fn annualised_sharpe(returns: &[f64], periods_per_year: f64) -> f64 {
    let sigma = stats::stddev(returns);
    if sigma < 1e-12 {
        return 0.0;
    }
    stats::mean(returns) / sigma * periods_per_year.max(1.0).sqrt()
}
