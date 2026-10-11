//! The internal Forecast Market's synthetic-capital ledger (blueprint §9.8,
//! §22.1; R12-MODEL-3093, R12-MODEL-6022).
//!
//! # This is not capital
//!
//! Every balance here is a [`SyntheticCredits`]: a reputation score with the
//! arithmetic of a stake. It has no conversion to `Decimal`, to `Money`, to an
//! account or to anything a ledger, a broker or a sizing function reads, and
//! that absence is the guarantee — a credit that could be summed with a cash
//! balance would one day be. The word "capital" in §22.1 is a metaphor for
//! track record, and this type exists so the metaphor cannot be mistaken for
//! a book. Nothing in this module reaches the order path.
//!
//! # Why a market rather than a hit rate
//!
//! A hit rate rewards a forecaster for agreeing with everyone else when
//! everyone else is right. The settlement here is the logarithmic market
//! scoring rule taken against the *aggregate*: a forecaster stakes a fixed
//! fraction of its balance on each question and is paid `stake × ln(p_i /
//! p_agg)` on the outcome that happened. Saying what the crowd already said
//! earns nothing; being more right than the crowd earns; being more wrong
//! loses — so the balance measures **incremental** skill, which is what the
//! aggregate should be weighted by, and size (a model's parameter count, or
//! the number of copies of it) buys nothing.
//!
//! A forecaster can never lose more than it staked on one question. That is
//! the market's rule, stated here, not a correction of an input: a log ratio
//! is unbounded below, and an unbounded loss would let one confident miss
//! erase a record built over hundreds of questions.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Micro-credits per credit. Integers so a replay settles to the same unit.
pub const MICRO_PER_CREDIT: i64 = 1_000_000;

/// What every forecaster starts with: one thousand credits.
pub const INITIAL_CREDITS: SyntheticCredits = SyntheticCredits(1_000 * MICRO_PER_CREDIT);

/// The share of its balance a forecaster stakes on each question, in basis
/// points. Five percent: small enough that twenty consecutive maximal losses
/// leave a third of the balance, large enough that a forecaster's share moves
/// within a few dozen questions.
pub const STAKE_BP: i64 = 500;

/// A synthetic, non-monetary reputation balance. See the module doc: there is
/// deliberately no conversion to any money type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyntheticCredits(i64);

impl SyntheticCredits {
    /// The balance in micro-credits.
    pub const fn micro(self) -> i64 {
        self.0
    }

    /// The balance as a statistic, for computing a share. Not money.
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / MICRO_PER_CREDIT as f64
    }
}

/// One forecaster's settlement on one question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settlement {
    pub forecaster: String,
    pub staked: SyntheticCredits,
    /// Signed change in micro-credits.
    pub payoff_micro: i64,
    pub balance_after: SyntheticCredits,
}

/// Every forecaster's synthetic balance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ForecastMarket {
    balances: BTreeMap<String, SyntheticCredits>,
}

impl ForecastMarket {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open an account at [`INITIAL_CREDITS`]. Idempotent: re-admitting a
    /// forecaster must not refill a balance it has lost.
    pub fn open(&mut self, forecaster: &str) {
        self.balances
            .entry(forecaster.to_string())
            .or_insert(INITIAL_CREDITS);
    }

    /// Close an account, for a retired forecaster. Its record lives in the
    /// event log; holding the balance would grow the market with uptime.
    pub fn close(&mut self, forecaster: &str) {
        self.balances.remove(forecaster);
    }

    pub fn balance(&self, forecaster: &str) -> Option<SyntheticCredits> {
        self.balances.get(forecaster).copied()
    }

    pub fn len(&self) -> usize {
        self.balances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.balances.is_empty()
    }

    /// Each forecaster's share of the total balance held by `among`, in
    /// `[0, 1]`, summing to one where any of them holds anything. A name
    /// with no account is absent rather than zero, so a caller cannot read
    /// "never traded" as "lost everything".
    pub fn shares<'a>(&self, among: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, f64> {
        let held: Vec<(&str, i64)> = among
            .into_iter()
            .filter_map(|name| self.balances.get(name).map(|b| (name, b.0)))
            .collect();
        let total: i64 = held.iter().map(|(_, b)| *b).sum();
        held.into_iter()
            .map(|(name, balance)| {
                let share = if total > 0 {
                    balance as f64 / total as f64
                } else {
                    0.0
                };
                (name.to_string(), share)
            })
            .collect()
    }

    /// Settle one question.
    ///
    /// `participants` is each forecaster and the probability it gave the
    /// outcome that happened; `aggregate` is the market's probability for the
    /// same outcome. Probabilities must lie in `(0, 1]` — a forecaster that
    /// put zero on what happened is passed in at the floor the caller states,
    /// because the log of zero is not a payoff. Refuses anything else rather
    /// than clamping it.
    pub fn settle(
        &mut self,
        participants: &[(String, f64)],
        aggregate: f64,
    ) -> Result<Vec<Settlement>> {
        if !(aggregate.is_finite() && aggregate > 0.0 && aggregate <= 1.0) {
            return Err(Error::invalid(format!(
                "the aggregate probability {aggregate} is not in (0, 1]; floor it before settling"
            )));
        }
        let mut settlements = Vec::with_capacity(participants.len());
        for (forecaster, probability) in participants {
            if !(probability.is_finite() && *probability > 0.0 && *probability <= 1.0) {
                return Err(Error::invalid(format!(
                    "{forecaster} gave the outcome probability {probability}, not in (0, 1]; \
                     floor it before settling"
                )));
            }
            let Some(balance) = self.balances.get(forecaster).copied() else {
                return Err(Error::not_found(format!(
                    "{forecaster} holds no account in the forecast market; open one at admission"
                )));
            };
            let stake = balance.0 / 10_000 * STAKE_BP;
            // The crossing from a statistic to the integer ledger: the log
            // ratio is f64 and the payoff is rounded to the micro-credit once.
            let raw = stake as f64 * (probability / aggregate).ln();
            let payoff = if raw.is_finite() {
                (raw.round() as i64).max(-stake)
            } else {
                -stake
            };
            let after = SyntheticCredits(balance.0.saturating_add(payoff).max(0));
            self.balances.insert(forecaster.clone(), after);
            settlements.push(Settlement {
                forecaster: forecaster.clone(),
                staked: SyntheticCredits(stake),
                payoff_micro: payoff,
                balance_after: after,
            });
        }
        Ok(settlements)
    }
}

/// Split `budget` units of compute across `weights` by largest remainder, so
/// the allocation sums to the budget exactly and a forecaster with zero
/// weight receives nothing. Ties break on name so two runs agree.
pub fn allocate(weights: &BTreeMap<String, f64>, budget: u64) -> Result<BTreeMap<String, u64>> {
    if weights.values().any(|w| !w.is_finite() || *w < 0.0) {
        return Err(Error::invalid(
            "a compute weight is negative or not finite; an allocation cannot be computed from it",
        ));
    }
    let total: f64 = weights.values().sum();
    if total <= 0.0 {
        return Ok(weights.keys().map(|name| (name.clone(), 0)).collect());
    }
    let mut floors: BTreeMap<String, u64> = BTreeMap::new();
    let mut remainders: Vec<(f64, &String)> = Vec::new();
    for (name, weight) in weights {
        let exact = budget as f64 * weight / total;
        let floor = exact.floor();
        floors.insert(name.clone(), floor as u64);
        remainders.push((exact - floor, name));
    }
    let allocated: u64 = floors.values().sum();
    remainders.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    for (_, name) in remainders
        .into_iter()
        .take(budget.saturating_sub(allocated) as usize)
    {
        if let Some(units) = floors.get_mut(name) {
            *units += 1;
        }
    }
    Ok(floors)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;

    /// Mutation: paying `stake × ln(p_i)` instead of `ln(p_i / p_agg)`
    /// (scoring against certainty rather than the crowd) pays the echo a
    /// loss on every question and fails the first assertion.
    #[test]
    fn echoing_the_aggregate_earns_nothing_and_beating_it_earns_credits() -> Result<()> {
        let mut market = ForecastMarket::new();
        market.open("echo");
        market.open("sharp");
        market.open("wrong");
        let settled = market.settle(
            &[
                ("echo".to_string(), 0.6),
                ("sharp".to_string(), 0.9),
                ("wrong".to_string(), 0.2),
            ],
            0.6,
        )?;
        assert_eq!(settled.len(), 3);
        assert_eq!(
            settled[0].payoff_micro, 0,
            "saying what the crowd said paid"
        );
        assert!(
            settled[1].payoff_micro > 0,
            "beating the crowd earned nothing"
        );
        assert!(
            settled[2].payoff_micro < 0,
            "being worse than the crowd cost nothing"
        );
        assert!(
            market.balance("sharp") > market.balance("wrong"),
            "the record does not order the forecasters by skill"
        );
        Ok(())
    }

    /// Mutation: removing `.max(-stake)` lets one confident miss take more
    /// than the stake.
    #[test]
    fn a_confident_miss_costs_at_most_the_stake() -> Result<()> {
        let mut market = ForecastMarket::new();
        market.open("reckless");
        let before = market
            .balance("reckless")
            .map_or(0, SyntheticCredits::micro);
        let settled = market.settle(&[("reckless".to_string(), 1e-9)], 0.9)?;
        let stake = before / 10_000 * STAKE_BP;
        assert_eq!(settled[0].payoff_micro, -stake);
        assert_eq!(
            market.balance("reckless").map(SyntheticCredits::micro),
            Some(before - stake)
        );
        Ok(())
    }

    #[test]
    fn a_probability_outside_the_unit_interval_is_refused_rather_than_clamped() {
        let mut market = ForecastMarket::new();
        market.open("a");
        assert!(market.settle(&[("a".to_string(), 0.0)], 0.5).is_err());
        assert!(market.settle(&[("a".to_string(), 0.5)], 1.5).is_err());
    }

    /// Mutation: rounding each share independently instead of handing the
    /// leftover units out by remainder makes the allocation miss the budget.
    #[test]
    fn a_compute_allocation_sums_to_the_budget_and_follows_the_weights() -> Result<()> {
        let weights = BTreeMap::from([
            ("a".to_string(), 1.0),
            ("b".to_string(), 1.0),
            ("c".to_string(), 1.0),
            ("idle".to_string(), 0.0),
        ]);
        let allocation = allocate(&weights, 100)?;
        assert_eq!(allocation.values().sum::<u64>(), 100);
        assert_eq!(allocation["idle"], 0);
        assert_eq!(allocation["a"], 34, "the leftover unit breaks ties by name");
        Ok(())
    }
}
