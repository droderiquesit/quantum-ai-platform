//! Which alternatives would systematically have done better, and by how much.
//!
//! One decision where standing aside would have helped is an anecdote. The
//! question this module answers is whether a *kind* of alternative beats the
//! platform often enough to be worth changing something for, and the honest
//! answer to that depends as much on how many observations there are as on how
//! large the average gap is.
//!
//! So nothing here reports a raw mean. The win rate goes through
//! [`Conviction::shrunk`], the same shrinkage a strategy's own conviction goes
//! through, and the mean gap is pulled toward zero by the same weight. Three
//! observations of a large win come out at a fraction of their face value,
//! which is what stops a regret analysis from becoming a machine for
//! discovering patterns in noise.
//!
//! The shrinkage constant is thirty because that is the constant
//! [`Conviction::shrunk`] uses, and two different half-lives for the same idea
//! in one platform is how two teams end up disagreeing about whether something
//! is significant.

use crate::counterfactual::{Counterfactual, CounterfactualSet};
use crate::value::Simulated;
use qip_contracts::signal::Conviction;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The observation count at which a mean is trusted halfway.
///
/// Matches `Conviction::shrunk`, deliberately.
const SHRINKAGE_PRIOR: i64 = 30;

/// How one kind of alternative did against the actions actually taken.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlternativeRegret {
    /// The alternative's kind, e.g. `do_not_trade`.
    pub alternative: String,
    /// How many decisions this rests on. Reported next to every figure below,
    /// because a regret without a sample size invites the reader to treat three
    /// observations like three thousand.
    pub observations: u32,
    /// How often the alternative beat the action taken.
    pub better: u32,
    /// The sum of the gaps. Simulated: it is a total of things that did not
    /// happen.
    pub total: Simulated<Decimal>,
    /// The raw mean gap, before shrinkage. Present so the shrinkage is visible
    /// rather than baked in.
    pub mean: Simulated<Decimal>,
    /// The win rate with its sample size, for shrinking.
    pub conviction: Conviction,
}

impl AlternativeRegret {
    /// The mean gap, pulled toward zero by how little evidence supports it.
    ///
    /// The figure to quote. With three observations it is roughly a tenth of
    /// the raw mean; with three thousand it is the raw mean.
    pub fn shrunk_mean(&self) -> Simulated<Decimal> {
        let n = Decimal::from_int(i64::from(self.observations));
        let weight = n
            .checked_div(n + Decimal::from_int(SHRINKAGE_PRIOR))
            // The divisor is at least thirty, so this cannot be `None`. A zero
            // here would understate the regret, which is the safe direction for
            // a fallback that should never fire.
            .unwrap_or(Decimal::ZERO);
        self.mean.scaled_by(weight)
    }

    /// The win rate shrunk toward a coin flip by the sample size.
    pub fn shrunk_win_rate(&self) -> f64 {
        self.conviction.shrunk()
    }

    /// Whether the alternative beats the platform often enough to act on.
    ///
    /// `bar` is a shrunk win rate, so a bar of 0.6 is genuinely unreachable
    /// from a handful of observations however one-sided they were.
    pub fn is_systematic(&self, bar: f64) -> bool {
        self.conviction.clears(bar)
    }

    pub fn summarise(&self) -> String {
        format!(
            "{}: better on {} of {} decisions ({:.0}% shrunk), mean gap {} shrunk to {}",
            self.alternative,
            self.better,
            self.observations,
            self.shrunk_win_rate() * 100.0,
            self.mean,
            self.shrunk_mean()
        )
    }
}

/// Regret across many decisions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegretAnalysis {
    /// How many decisions the analysis covers.
    pub decisions: usize,
    by_alternative: Vec<AlternativeRegret>,
}

impl RegretAnalysis {
    /// Aggregate the counterfactual sets of many decisions.
    ///
    /// Alternatives that could not be priced — refused by the participation
    /// limit, or unfilled — are counted as observations with a zero gap rather
    /// than dropped. Dropping them would mean the sample silently consisted of
    /// the occasions the alternative happened to be feasible, which is the
    /// selection that makes "we should have traded larger" true everywhere.
    pub fn over(sets: &[CounterfactualSet]) -> Result<Self> {
        if sets.is_empty() {
            return Err(Error::invalid(
                "a regret analysis over no decisions has nothing to be regretful about",
            ));
        }
        let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
        for set in sets {
            for entry in set.entries() {
                buckets
                    .entry(entry.counterfactual_action.kind().to_string())
                    .or_default()
                    .add(entry);
            }
        }
        let by_alternative = buckets
            .into_iter()
            .map(|(alternative, bucket)| bucket.finish(alternative))
            .collect();
        Ok(Self {
            decisions: sets.len(),
            by_alternative,
        })
    }

    pub fn by_alternative(&self) -> &[AlternativeRegret] {
        &self.by_alternative
    }

    pub fn get(&self, alternative: &str) -> Option<&AlternativeRegret> {
        self.by_alternative
            .iter()
            .find(|regret| regret.alternative == alternative)
    }

    /// The alternatives that clear a shrunk-win-rate bar.
    pub fn systematic(&self, bar: f64) -> Vec<&AlternativeRegret> {
        self.by_alternative
            .iter()
            .filter(|regret| regret.is_systematic(bar))
            .collect()
    }

    /// The alternative with the largest shrunk mean gap, if any is positive.
    ///
    /// Ranked on the shrunk figure rather than the raw one, so an alternative
    /// tried twice cannot outrank one tried two thousand times.
    pub fn worst_forgone(&self) -> Option<&AlternativeRegret> {
        self.by_alternative
            .iter()
            .filter(|regret| regret.shrunk_mean().is_positive())
            .max_by(|a, b| {
                a.shrunk_mean()
                    .as_f64_for_statistics()
                    .total_cmp(&b.shrunk_mean().as_f64_for_statistics())
            })
    }

    pub fn summarise(&self) -> String {
        let lines: Vec<String> = self
            .by_alternative
            .iter()
            .map(AlternativeRegret::summarise)
            .collect();
        format!("{} decisions:\n  {}", self.decisions, lines.join("\n  "))
    }
}

/// Running totals for one kind of alternative.
#[derive(Debug, Default)]
struct Bucket {
    observations: u32,
    better: u32,
    total: Simulated<Decimal>,
}

impl Bucket {
    fn add(&mut self, entry: &Counterfactual) {
        self.observations = self.observations.saturating_add(1);
        if entry.favours_the_alternative() {
            self.better = self.better.saturating_add(1);
        }
        self.total = self.total + entry.difference;
    }

    fn finish(self, alternative: String) -> AlternativeRegret {
        let count = Decimal::from_int(i64::from(self.observations));
        let mean = if self.observations == 0 {
            Simulated::ZERO
        } else {
            self.total.divided_by(count)
        };
        let rate = if self.observations == 0 {
            0.0
        } else {
            f64::from(self.better) / f64::from(self.observations)
        };
        AlternativeRegret {
            alternative,
            observations: self.observations,
            better: self.better,
            total: self.total,
            mean,
            conviction: Conviction::new(rate, self.observations),
        }
    }
}
