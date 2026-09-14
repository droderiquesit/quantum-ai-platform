//! Blueprint §19.2, evaluation tiers: which strategies are evaluated how
//! often, and the count at which the hottest tier stops accepting more.
//!
//! Five tiers, each with a cadence, a share of the population and a place it
//! runs. The two decisions in the section that are actually load-bearing are
//! the cadence — cheap work happens often and expensive work does not — and
//! the **hot-tier cap**, which the blueprint states as an operational fact:
//! "a node carries roughly 3,600 strategies of which about 1,000 sit in the
//! hot tier against a cap of 1,200. That cap is the count at which measured
//! p99 evaluation reaches seventy percent of the 90 µs budget."
//!
//! # What this module refuses
//!
//! [`TierPlan::assign`] refuses a population whose hot tier exceeds
//! [`HOT_TIER_CAP`], naming the two things a caller can do instead — reclassify
//! into a colder tier, or run a second node. It does not admit them and
//! record a warning, and it does not silently demote the overflow: a demotion
//! chosen by the scheduler would change which strategies see an event without
//! anyone deciding that, and the first evidence of it would be a latency
//! number nobody could attribute.
//!
//! **Be precise about what that gate can do today.** Its input is the size of
//! the classified population, which is a real production input and is
//! nowhere near the cap: this platform registers strategies in tens. So the
//! refusal is a bound on a working set that has not yet grown, not a control
//! with a live subject. That is a different thing from the
//! `MaxExpectedShortfall` failure — that limit read a figure that was empty
//! *by construction*, so no growth in any input could ever have fired it —
//! but it is not the same as a control that fires today, and this module does
//! not claim it is.
//!
//! # Unattributed strategies are cold, not hot
//!
//! [`tier_of`] answers [`EvaluationTier::Batch`] for a strategy whose alpha
//! family nobody recorded. The hot tier's cap is a latency budget, and
//! spending it on a strategy nobody classified is spending it on a guess.
//! The cold answer costs a strategy its cadence, which is recoverable by
//! classifying it; the hot answer costs the node's p99, which is recoverable
//! only by finding out afterwards.

use crate::universe::AlphaFamily;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How often a tier's strategies are evaluated.
///
/// Two arms rather than a `Duration` for every tier, because the hot tier's
/// cadence in the blueprint is "every relevant event, microseconds" — an
/// answer to a different question than "at most every N". Encoding it as a
/// very small `Duration` would let a scheduler skip a hot-tier evaluation
/// because the clock had not moved far enough, which is precisely what the
/// hot tier exists not to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TierCadence {
    /// Every relevant event. Never skipped, whatever the clock says.
    EveryEvent,
    /// At most once per interval.
    AtMost(Duration),
}

impl TierCadence {
    /// Whether work on this cadence is due at `now`, given when it last ran.
    ///
    /// `None` for `last` means it has never run, which is always due: a
    /// cadence that withheld the first run would be indistinguishable from
    /// one that never ran at all.
    pub fn due(self, last: Option<Timestamp>, now: Timestamp) -> bool {
        match self {
            Self::EveryEvent => true,
            Self::AtMost(interval) => match last {
                None => true,
                // `since` saturates at zero, so a clock that stepped
                // backwards reads as "no time has passed" and the work is
                // held rather than run again — the direction that cannot
                // turn a wound-back clock into unbounded work.
                Some(last) => now.since(last).as_nanos() >= interval.as_nanos(),
            },
        }
    }
}

/// One of §19.2's five evaluation tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvaluationTier {
    /// Every relevant event, microseconds. Node, pinned cores.
    Hot,
    /// Sub-second, event-driven. Node, lower-priority core.
    Fast,
    /// Seconds to minutes. Node background thread.
    Warm,
    /// Minutes to hours. Cloud Run.
    Slow,
    /// Daily. Cloud Run Jobs.
    Batch,
}

impl EvaluationTier {
    /// The tiers, hottest first — the blueprint's own table order, which is
    /// also the `Ord` these reach a `BTreeMap` under.
    pub const ALL: [Self; 5] = [Self::Hot, Self::Fast, Self::Warm, Self::Slow, Self::Batch];

    /// The tier number the blueprint's family table gives (0 through 4).
    pub const fn index(self) -> u8 {
        match self {
            Self::Hot => 0,
            Self::Fast => 1,
            Self::Warm => 2,
            Self::Slow => 3,
            Self::Batch => 4,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Fast => "fast",
            Self::Warm => "warm",
            Self::Slow => "slow",
            Self::Batch => "batch",
        }
    }

    /// The cadence §19.2 states for this tier.
    ///
    /// The intervals are the *slow* end of each row's range — "seconds to
    /// minutes" is scheduled at a minute — because a cadence bound that
    /// promised the fast end would be a promise about a machine nobody has
    /// measured, and the cost of being slower than the table is a stale
    /// evaluation while the cost of being faster is an over-subscribed node.
    pub const fn cadence(self) -> TierCadence {
        match self {
            Self::Hot => TierCadence::EveryEvent,
            Self::Fast => TierCadence::AtMost(Duration::from_millis(1_000)),
            Self::Warm => TierCadence::AtMost(Duration::from_mins(1)),
            Self::Slow => TierCadence::AtMost(Duration::from_hours(1)),
            Self::Batch => TierCadence::AtMost(Duration::from_days(1)),
        }
    }

    /// The share of a node's population §19.2 expects in this tier, in whole
    /// percent. Descriptive: nothing here enforces it, and the one number
    /// that *is* enforced is [`HOT_TIER_CAP`], which is a count and not a
    /// share — a share of a population that grew is not a latency budget.
    pub const fn target_share_percent(self) -> u32 {
        match self {
            Self::Hot => 10,
            Self::Fast => 20,
            Self::Warm => 30,
            Self::Slow => 35,
            Self::Batch => 5,
        }
    }
}

/// The largest number of strategies that may sit in the hot tier on one
/// node.
///
/// §19.2's own figure: the count at which measured p99 evaluation reaches
/// seventy percent of the 90 µs budget. Not a round number chosen for
/// comfort — it is the point at which the measurement was taken, and the
/// thirty percent that remains is the headroom a tail needs.
pub const HOT_TIER_CAP: usize = 1_200;

/// The tier a strategy of this alpha family is evaluated in.
///
/// `None` — nobody recorded the family — is [`EvaluationTier::Batch`], the
/// coldest. See the module note: the hot tier's cap is a latency budget and
/// an unclassified strategy may not spend it.
///
/// Where §19's table gives a range (`1–2`, `2–3`) the **hotter** end is
/// taken. That is the conservative direction for the cap: it counts a
/// range-tier strategy against the tighter budget rather than letting it
/// disappear into a colder tier the cap does not watch.
pub const fn tier_of(family: Option<AlphaFamily>) -> EvaluationTier {
    match family {
        None => EvaluationTier::Batch,
        Some(family) => match family {
            AlphaFamily::MarketMaking
            | AlphaFamily::Arbitrage
            | AlphaFamily::Microstructure
            | AlphaFamily::ExecutionAlpha => EvaluationTier::Hot,
            AlphaFamily::ShortHorizonReversion | AlphaFamily::EventDriven => EvaluationTier::Fast,
            AlphaFamily::MomentumAndTrend
            | AlphaFamily::StatisticalArbitrage
            | AlphaFamily::Volatility => EvaluationTier::Warm,
            AlphaFamily::Carry => EvaluationTier::Slow,
        },
    }
}

/// A population assigned to tiers, with the hot tier proven to be under its
/// cap.
///
/// Holding the proof in the type is the point: a `TierPlan` cannot be
/// constructed over an oversubscribed hot tier, so a scheduler handed one
/// does not have to re-check and cannot forget to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierPlan {
    assignments: BTreeMap<String, EvaluationTier>,
    counts: BTreeMap<EvaluationTier, usize>,
}

impl TierPlan {
    /// Assign every strategy to its tier, refusing an oversubscribed hot
    /// tier.
    ///
    /// `population` maps a strategy's identifier to the alpha family
    /// somebody recorded for it, or `None` where nobody did. `cap` is the
    /// node's own hot-tier ceiling: [`HOT_TIER_CAP`] is the blueprint's
    /// measured figure, and a smaller machine passes a smaller one rather
    /// than discovering the ceiling in a latency tail.
    ///
    /// Refuses rather than demoting the overflow. A demotion the scheduler
    /// chose would silently change which strategies see an event.
    pub fn assign(population: &BTreeMap<String, Option<AlphaFamily>>, cap: usize) -> Result<Self> {
        let counted: BTreeMap<String, (Option<AlphaFamily>, usize)> = population
            .iter()
            .map(|(strategy, family)| (strategy.clone(), (*family, 1)))
            .collect();
        Self::assign_counted(&counted, cap)
    }

    /// Assign groups of strategies that share a classification.
    ///
    /// The same rule as [`Self::assign`] over a population presented as
    /// `group → (family, members)`, for the caller that knows how many
    /// strategies stand under a name without holding each one's identifier.
    /// The cap counts **members**, not groups: the hot tier's ceiling is a
    /// latency budget spent per evaluation, and one group of four hundred
    /// market-making strategies spends four hundred strategies' worth of it.
    pub fn assign_counted(
        population: &BTreeMap<String, (Option<AlphaFamily>, usize)>,
        cap: usize,
    ) -> Result<Self> {
        if cap == 0 {
            return Err(Error::invalid(
                "a hot-tier cap of zero admits no strategy to the tier the whole latency budget \
                 exists for; supply the count at which p99 evaluation reaches seventy percent of \
                 the budget, or run no hot-tier strategies at all",
            ));
        }
        let mut assignments = BTreeMap::new();
        let mut counts: BTreeMap<EvaluationTier, usize> = BTreeMap::new();
        for (group, (family, members)) in population {
            let tier = tier_of(*family);
            assignments.insert(group.clone(), tier);
            *counts.entry(tier).or_insert(0) += members;
        }
        let hot = counts.get(&EvaluationTier::Hot).copied().unwrap_or(0);
        if hot > cap {
            return Err(Error::invalid(format!(
                "{hot} strategies are classified into the hot tier against a cap of {cap}; the \
                 cap is the count at which p99 evaluation reaches seventy percent of the \
                 evaluation budget, so admitting them would spend a budget nobody measured. \
                 Reclassify the surplus into a colder tier or run them on a second node — they \
                 are not demoted here, because a demotion nobody chose changes which strategies \
                 see an event"
            )));
        }
        Ok(Self {
            assignments,
            counts,
        })
    }

    /// The tier a strategy was assigned to, if it was in the population.
    pub fn tier(&self, strategy: &str) -> Option<EvaluationTier> {
        self.assignments.get(strategy).copied()
    }

    /// How many strategies landed in each tier. Every tier that took at
    /// least one strategy appears; a tier that took none does not, and the
    /// caller reads zero for it through [`Self::count`].
    pub fn counts(&self) -> &BTreeMap<EvaluationTier, usize> {
        &self.counts
    }

    pub fn count(&self, tier: EvaluationTier) -> usize {
        self.counts.get(&tier).copied().unwrap_or(0)
    }

    /// How many strategies were tiered, summed over every group.
    pub fn population(&self) -> usize {
        self.counts.values().sum()
    }

    /// How many distinct groups were classified. One per strategy where the
    /// caller used [`Self::assign`].
    pub fn groups(&self) -> usize {
        self.assignments.len()
    }

    /// How much of the hot tier's cap is unspent, as the plan stands.
    pub fn hot_headroom(&self, cap: usize) -> usize {
        cap.saturating_sub(self.count(EvaluationTier::Hot))
    }

    /// One line for a cycle's record, tiers hottest first.
    pub fn describe(&self, cap: usize) -> String {
        let parts: Vec<String> = EvaluationTier::ALL
            .into_iter()
            .map(|tier| format!("{}={}", tier.as_str(), self.count(tier)))
            .collect();
        format!(
            "{} strategies tiered {} with {} of the hot-tier cap of {cap} unspent",
            self.population(),
            parts.join(" "),
            self.hot_headroom(cap)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn population(hot: usize, cold: usize) -> BTreeMap<String, Option<AlphaFamily>> {
        let mut population = BTreeMap::new();
        for index in 0..hot {
            population.insert(
                format!("strat-hot-{index:05}"),
                Some(AlphaFamily::MarketMaking),
            );
        }
        for index in 0..cold {
            population.insert(format!("strat-cold-{index:05}"), Some(AlphaFamily::Carry));
        }
        population
    }

    #[test]
    fn a_population_one_over_the_hot_tier_cap_is_refused_and_one_at_the_cap_is_admitted() {
        // The admitting half is the half that makes this a gate rather than
        // a function that refuses everything: a cap nothing can satisfy and
        // a cap nothing can cross are the same defect wearing different
        // clothes.
        let at_cap = population(HOT_TIER_CAP, 3);
        assert_eq!(
            at_cap.len(),
            HOT_TIER_CAP + 3,
            "the premise is a population of exactly the cap plus three cold names"
        );
        let plan = TierPlan::assign(&at_cap, HOT_TIER_CAP).expect("the cap itself is admitted");
        assert_eq!(plan.count(EvaluationTier::Hot), HOT_TIER_CAP);
        assert_eq!(plan.count(EvaluationTier::Slow), 3);
        assert_eq!(plan.hot_headroom(HOT_TIER_CAP), 0);

        let over = population(HOT_TIER_CAP + 1, 0);
        let refusal = TierPlan::assign(&over, HOT_TIER_CAP).expect_err("one over is refused");
        assert!(
            refusal.message().contains("against a cap of 1200"),
            "the refusal does not name the cap it enforced: {}",
            refusal.message()
        );
        assert!(
            refusal.message().contains("Reclassify"),
            "the refusal does not say what to do instead: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_strategy_whose_alpha_family_nobody_recorded_is_evaluated_in_the_coldest_tier() {
        // The hot tier's cap is a latency budget; an unclassified strategy
        // may not spend it. The premise: the same population classified
        // *does* reach the hot tier, so this is the effect of the missing
        // attribution and not of the assignment refusing everything.
        let mut unattributed = BTreeMap::new();
        unattributed.insert("strat-unknown".to_string(), None);
        let plan = TierPlan::assign(&unattributed, HOT_TIER_CAP).expect("a lone strategy fits");
        assert_eq!(plan.tier("strat-unknown"), Some(EvaluationTier::Batch));
        assert_eq!(plan.count(EvaluationTier::Hot), 0);

        let mut attributed = BTreeMap::new();
        attributed.insert(
            "strat-unknown".to_string(),
            Some(AlphaFamily::Microstructure),
        );
        let plan = TierPlan::assign(&attributed, HOT_TIER_CAP).expect("a lone strategy fits");
        assert_eq!(plan.tier("strat-unknown"), Some(EvaluationTier::Hot));
        assert_eq!(plan.hot_headroom(HOT_TIER_CAP), HOT_TIER_CAP - 1);
        assert_eq!(plan.tier("strat-absent"), None);
    }

    #[test]
    fn a_cadence_holds_work_until_its_interval_has_passed_and_the_hot_tier_never_holds_any() {
        let start = Timestamp::from_secs(1_760_000_000);
        // The premise: a tier whose cadence is an interval, and the interval.
        let TierCadence::AtMost(interval) = EvaluationTier::Warm.cadence() else {
            panic!("the warm tier's cadence is an interval in §19.2");
        };
        assert_eq!(interval, Duration::from_mins(1));
        assert!(
            EvaluationTier::Warm.cadence().due(None, start),
            "work that has never run was withheld"
        );
        let one_short = start
            .saturating_add(interval)
            .saturating_sub(Duration::from_nanos(1));
        assert!(
            !EvaluationTier::Warm.cadence().due(Some(start), one_short),
            "the warm tier ran a nanosecond inside its own interval"
        );
        assert!(
            EvaluationTier::Warm
                .cadence()
                .due(Some(start), start.saturating_add(interval)),
            "the warm tier did not run when its interval had passed"
        );
        // A clock that stepped backwards holds the work rather than running
        // it again: `since` saturates at zero.
        assert!(
            !EvaluationTier::Warm
                .cadence()
                .due(Some(start), start.saturating_sub(Duration::from_hours(1))),
            "a wound-back clock made the work due again"
        );
        // The hot tier is never held, whatever the clock says.
        assert_eq!(EvaluationTier::Hot.cadence(), TierCadence::EveryEvent);
        assert!(EvaluationTier::Hot.cadence().due(Some(start), start));
    }

    #[test]
    fn every_alpha_family_has_a_tier_and_the_shares_are_the_blueprints() {
        // A premise worth asserting because the table is transcribed from a
        // document: the five shares are the five in §19.2 and they total a
        // hundred. A transcription that dropped a row would still assign
        // every family and would quietly stop summing.
        let total: u32 = EvaluationTier::ALL
            .into_iter()
            .map(EvaluationTier::target_share_percent)
            .sum();
        assert_eq!(
            total, 100,
            "the tier shares no longer describe a population"
        );
        let indices: Vec<u8> = EvaluationTier::ALL
            .into_iter()
            .map(EvaluationTier::index)
            .collect();
        assert_eq!(indices, vec![0, 1, 2, 3, 4]);
        let hot: Vec<&str> = AlphaFamily::ALL
            .into_iter()
            .filter(|family| tier_of(Some(*family)) == EvaluationTier::Hot)
            .map(AlphaFamily::as_str)
            .collect();
        assert_eq!(
            hot,
            vec![
                "market_making",
                "arbitrage",
                "microstructure",
                "execution_alpha"
            ],
            "§19's tier-0 families are not the ones in the hot tier"
        );
        assert_eq!(tier_of(Some(AlphaFamily::Carry)), EvaluationTier::Slow);
        // The ranged rows take the hotter end.
        assert_eq!(
            tier_of(Some(AlphaFamily::EventDriven)),
            EvaluationTier::Fast
        );
        assert_eq!(
            tier_of(Some(AlphaFamily::MomentumAndTrend)),
            EvaluationTier::Warm
        );
    }

    #[test]
    fn a_group_spends_the_hot_tier_cap_once_per_member_and_not_once_per_group() {
        // The failure this guards: counting groups rather than members would
        // admit any number of strategies as long as they were named in few
        // enough families, and the cap is a latency budget spent per
        // evaluation. The premise: one group, and it is over the cap on
        // members while being one on groups.
        let mut population = BTreeMap::new();
        population.insert(
            "family-mm".to_string(),
            (Some(AlphaFamily::MarketMaking), HOT_TIER_CAP + 1),
        );
        assert_eq!(population.len(), 1, "the premise is a single group");
        let refusal =
            TierPlan::assign_counted(&population, HOT_TIER_CAP).expect_err("over on members");
        assert!(
            refusal.message().contains("1201 strategies"),
            "the refusal counted groups rather than members: {}",
            refusal.message()
        );
        // And at the cap it is admitted, with the members counted through.
        population.insert(
            "family-mm".to_string(),
            (Some(AlphaFamily::MarketMaking), HOT_TIER_CAP),
        );
        let plan = TierPlan::assign_counted(&population, HOT_TIER_CAP).expect("at the cap");
        assert_eq!(plan.count(EvaluationTier::Hot), HOT_TIER_CAP);
        assert_eq!(plan.population(), HOT_TIER_CAP);
        assert_eq!(plan.groups(), 1);
        assert_eq!(plan.tier("family-mm"), Some(EvaluationTier::Hot));
    }

    #[test]
    fn a_hot_tier_cap_of_zero_is_refused_rather_than_admitting_nothing_for_ever() {
        let refusal = TierPlan::assign(&population(0, 1), 0).expect_err("zero is refused");
        assert!(
            refusal.message().contains("admits no strategy"),
            "the refusal does not name the failure: {}",
            refusal.message()
        );
        // And the premise: the same population is fine under a real cap.
        assert!(TierPlan::assign(&population(0, 1), 1).is_ok());
    }
}
