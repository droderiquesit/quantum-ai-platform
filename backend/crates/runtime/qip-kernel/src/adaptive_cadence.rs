//! Blueprint §23.6, adaptive cadence and sequencing: work runs when there is
//! something for it to do, and the cycles where there is not are the saving.
//!
//! The section is a table of signals and the work each triggers, closing with
//! "Nothing changed. Do not run. **This is the saving.**" Two things had to be
//! decided before it could be built here, and both are load-bearing.
//!
//! # A control is never skipped for a saving
//!
//! The section's third row reads "regime belief changed → trigger
//! regime-conditional weighting". Taken literally against this platform it
//! would be a defect: [`crate::regime_allocation::narrow`] is a *control* —
//! it takes risk off when the platform cannot name the regime — and a control
//! that runs only when a signal moved is a control that stops holding when
//! nothing moves. So the regime narrowing is recomputed on every construction
//! and appears nowhere in this module's plan.
//!
//! **The saving applies only to work that produces a record.** Both work
//! items below do: an evaluation-tier census (§19.2) and a reinvestment plan
//! (§18.4). Skipping either loses nothing but a line in a cycle entry, and
//! running either on a cycle with no subject writes a line that says nothing
//! — which is the cost the section is complaining about.
//!
//! # Two of the four rows can be fed here and two cannot, stated plainly
//!
//! | §23.6 row | Here |
//! |---|---|
//! | Whitelist hit rate falling → cycle selection | **Not built.** Nothing in this platform measures a whitelist hit rate; a trigger keyed on a number nobody computes is a branch no input can reach. |
//! | Family correlations shifting → clustering and family allocation | Built as the population signal: the tier census runs when the foundry has classified strategies to tier. The correlation clustering itself is `CentralPlane::family_structure`, which is not this lane's to gate. |
//! | Regime belief changed → regime-conditional weighting | **Deliberately not gated.** See above: it is a control. |
//! | Inventory deviation rising → placement | **Not built.** Placement is the cells', and the centre holds no deviation figure this module could read without inventing a second one. |
//!
//! Two rows built, two named as unbuilt with the reason. A table with four
//! arms of which two can never fire would read as a scheduler and be half a
//! scheduler.
//!
//! # The cadence is counted in cycles
//!
//! `Platform::cycle_count` is a fact the platform already holds; a wall-clock
//! cadence would need a "when did this last run" that something would have to
//! store, and the event log is already the record of what ran. One cycle in
//! [`TIERING_CADENCE_CYCLES`] and one in [`REINVESTMENT_CADENCE_CYCLES`]
//! respectively; the rest are the saving.

use crate::central::factory::StrategyFactory;
use crate::platform::Platform;
use qip_capital::compounding::CompoundingPolicy;
use qip_core::Decimal;
use qip_core::error::Result;
use qip_optimization_engine::tiers::{HOT_TIER_CAP, TierPlan};
use qip_optimization_engine::universe::AlphaFamily;
use std::collections::{BTreeMap, BTreeSet};

/// One cycle in twelve takes the evaluation-tier census.
///
/// The census is itself batch work in §19.2's own terms — tier 4, "daily,
/// rebalancing and long-horizon allocation" — and twelve cycles is the
/// closest this platform has to a day at the cadence a research loop runs.
/// The number is here, once, rather than inside a condition, so that
/// changing it is one edit a reviewer can see.
pub const TIERING_CADENCE_CYCLES: u64 = 12;

/// One cycle in twelve considers reinvestment.
///
/// Separate from [`TIERING_CADENCE_CYCLES`] despite being equal today: they
/// answer different questions and a future change to one must not silently
/// move the other.
pub const REINVESTMENT_CADENCE_CYCLES: u64 = 12;

/// The most of a reinvestment lot that may be spent moving it: five percent.
///
/// Past that the redeployment is mostly a payment to a venue, and §18.4's
/// whole question is "realised profit redeployed **against the transaction
/// cost of redeploying it**".
pub const REINVESTMENT_COST_CEILING: f64 = 0.05;

/// Work this module may run or hold.
///
/// A closed set, because the plan reaches a record and a label nobody can
/// enumerate is a record nobody can chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Work {
    /// §19.2: assign the classified population to evaluation tiers and prove
    /// the hot tier is under its cap.
    EvaluationTiering,
    /// §18.4: decide whether realised profit is worth redeploying.
    CompoundingPlan,
}

impl Work {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EvaluationTiering => "evaluation_tiering",
            Self::CompoundingPlan => "compounding_plan",
        }
    }
}

/// The facts the plan is decided on, all readable from the platform's own
/// public surface at the instant the plan is made.
#[derive(Clone, Debug, PartialEq)]
pub struct CadenceSignals {
    pub cycle: u64,
    /// Strategies the foundry has registered under a family name, summed
    /// over families. Zero means there is nothing to tier.
    pub classified_strategies: usize,
    /// Realised profit net of the costs paid to realise it, floored at zero.
    /// Zero means there is nothing to redeploy.
    pub undeployed_profit: Decimal,
}

/// What runs this cycle, and why the rest did not.
#[derive(Clone, Debug, PartialEq)]
pub struct CadencePlan {
    work: BTreeSet<Work>,
    held: Vec<String>,
}

impl CadencePlan {
    pub fn wants(&self, work: Work) -> bool {
        self.work.contains(&work)
    }

    pub fn work(&self) -> &BTreeSet<Work> {
        &self.work
    }

    /// Nothing runs this cycle. §23.6's last row, and the reason this module
    /// exists.
    pub fn is_saving(&self) -> bool {
        self.work.is_empty()
    }

    /// Why each held item was held, for a caller that wants to say so.
    pub fn held(&self) -> &[String] {
        &self.held
    }
}

/// Decide what runs.
///
/// Each item needs two things at once: a subject, and its cadence. A subject
/// with no cadence would run the same census every cycle and bury the log; a
/// cadence with no subject would write "nothing to tier" on a schedule, which
/// is the line §23.6 calls the waste.
pub fn plan(signals: &CadenceSignals, policy: &CompoundingPolicy) -> CadencePlan {
    let mut work = BTreeSet::new();
    let mut held = Vec::new();

    if signals.classified_strategies == 0 {
        held.push(
            "no strategy is registered under a family, so there is nothing to tier".to_string(),
        );
    } else if !signals.cycle.is_multiple_of(TIERING_CADENCE_CYCLES) {
        held.push(format!(
            "the evaluation-tier census runs on one cycle in {TIERING_CADENCE_CYCLES} and this is \
             cycle {}",
            signals.cycle
        ));
    } else {
        work.insert(Work::EvaluationTiering);
    }

    if signals.undeployed_profit <= Decimal::ZERO {
        held.push("there is no realised profit to redeploy".to_string());
    } else if !policy.due(signals.cycle) {
        held.push(format!(
            "reinvestment is considered again at cycle {}",
            policy.next_due(signals.cycle)
        ));
    } else {
        work.insert(Work::CompoundingPlan);
    }

    CadencePlan { work, held }
}

/// The signals, read off the platform.
///
/// `undeployed_profit` is realised P&L less the costs paid to realise it,
/// which is the whole of what a paper book has ever had available to
/// compound: the platform redeploys nothing, so nothing has been taken out of
/// this figure. Floored at zero — a loss is not a negative amount of profit
/// to reinvest, and a negative lot would read as a withdrawal nobody made.
pub fn signals_of(platform: &Platform) -> CadenceSignals {
    let realised = platform.realised_pnl();
    let undeployed = realised
        .checked_sub(platform.trading_costs())
        .unwrap_or(Decimal::ZERO)
        .max(Decimal::ZERO);
    CadenceSignals {
        cycle: platform.cycle_count(),
        classified_strategies: population_of(platform.central().factory())
            .values()
            .map(|(_, members)| *members)
            .sum(),
        undeployed_profit: undeployed,
    }
}

/// The registered population, as §19.2 needs it: sweep name → (the alpha
/// family its candidates declared, members registered under it).
///
/// Read from the candidates' own declarations
/// (`StrategyCandidate::alpha_family`, §26.1's `family` field) and from
/// nothing else. Until 2026-09-19 this parsed the sweep's *name* whole
/// against the ten alpha families, which was honest about not guessing —
/// `momentum-v3` never tiered anything — but left the hot tier reachable
/// only through a lineage string the deployed deep brain hard-codes as
/// `evo-{subject}`, so no operator could reach it at all. A declaration on
/// the record is what a person chose and the register can read back.
///
/// A sweep whose candidates disagree about their family is counted under
/// `None`: the hot tier's cap is a latency budget and a sweep that cannot
/// say what it harvests may not spend it. The foundry cannot produce such a
/// sweep — it declares once and copies — so this arm is reachable only
/// through the factory's own `register`.
pub fn population_of(factory: &StrategyFactory) -> BTreeMap<String, (Option<AlphaFamily>, usize)> {
    let mut population: BTreeMap<String, (Option<AlphaFamily>, usize)> = BTreeMap::new();
    let mut mixed: BTreeSet<String> = BTreeSet::new();
    for candidate in factory.candidates() {
        let sweep = candidate.family().as_str().to_string();
        let declared = candidate.alpha_family();
        let entry = population.entry(sweep.clone()).or_insert((declared, 0));
        if entry.1 > 0 && entry.0 != declared {
            mixed.insert(sweep);
        }
        entry.1 += 1;
    }
    for sweep in mixed {
        if let Some(entry) = population.get_mut(&sweep) {
            entry.0 = None;
        }
    }
    population
}

/// The compounding policy this book is under.
///
/// Every number it needs that the desk has already stated is read from the
/// desk's own mandate rather than restated here: the redeployment cost is
/// the mandate's `turnover_cost_bps`, and the minimum lot is the smallest
/// position the mandate will hold (`minimum_position` of current equity) —
/// because redeploying less than that cannot buy a position, so a smaller
/// lot would plan a trade the constructor would then drop. Two independent
/// claims about one number will disagree, and the louder one will be wrong.
///
/// `f64 → Decimal` crosses here, once: `minimum_position` is a fraction of
/// equity and equity is money, so the fraction becomes basis points and the
/// multiplication happens in `Decimal`.
pub fn policy_for(platform: &Platform) -> Result<CompoundingPolicy> {
    let mandate = platform.config().mandate;
    let minimum_lot = platform
        .equity()
        .checked_apply_bps(mandate.minimum_position * 10_000.0)
        .unwrap_or(Decimal::ZERO);
    CompoundingPolicy::new(
        REINVESTMENT_CADENCE_CYCLES,
        minimum_lot,
        mandate.turnover_cost_bps,
        REINVESTMENT_COST_CEILING,
    )
}

/// The LEARN stage's cadence review: run what has a subject and is due, hold
/// the rest, and say nothing at all on a cycle where nothing runs.
///
/// Returns the `(summary, problems)` shape every review in this kernel
/// returns, so the stage folds it exactly as it folds `review_sizing` and
/// `review_family_allocation`. `None` is the saving: the cycle entry gains no
/// detail, which is the visible form of work that did not happen.
///
/// Takes `&Platform` and not `&mut`: nothing here writes to the platform,
/// moves capital, or changes a bound. The tier census can *refuse* — a hot
/// tier over its cap — and a refusal is a problem on the cycle rather than a
/// demotion chosen by a scheduler.
pub fn review(platform: &Platform) -> (Option<String>, Vec<String>) {
    let signals = signals_of(platform);
    let policy = match policy_for(platform) {
        Ok(policy) => policy,
        Err(error) => {
            // Reachable through the desk's own mandate and the book's
            // equity: a book at zero equity has no smallest position, so
            // there is no lot a reinvestment could be measured against. The
            // cycle records that rather than the process asserting a number.
            return (
                None,
                vec![format!(
                    "the compounding policy could not be built from the desk's mandate: {}",
                    error.message()
                )],
            );
        }
    };
    let plan = plan(&signals, &policy);
    if plan.is_saving() {
        // A saving cycle says so, naming what it held and why, rather than
        // returning nothing.
        //
        // This returned `(None, Vec::new())` until 2026-09-14 and that was
        // inconsistent with this lane's own argument one module over: §19.2's
        // tier gauge is written *every* cycle including when every arm is
        // zero, precisely so that "this has never had a subject" is a fact
        // somebody can read rather than a silence. A cadence that skips
        // silently fails the same way — an operator reading a LEARN detail
        // cannot tell a review that ran and found nothing due from a review
        // that was never wired in, and on today's platform every cycle is a
        // saving cycle, because no strategy is registered under an alpha
        // family and no profit has been realised. So the wiring would have
        // been unobservable in every state a deployment reaches, which is the
        // condition under which a call site rots unnoticed.
        //
        // `held` is bounded: one entry per `Work` arm, both of them literals
        // built here, so this cannot grow with the book.
        let summary = format!("cadence saving: {}", plan.held().join("; "));
        return (Some(summary), Vec::new());
    }

    let mut parts = Vec::new();
    let mut problems = Vec::new();

    if plan.wants(Work::EvaluationTiering) {
        match TierPlan::assign_counted(&population_of(platform.central().factory()), HOT_TIER_CAP) {
            Ok(census) => parts.push(census.describe(HOT_TIER_CAP)),
            Err(error) => problems.push(format!(
                "the evaluation-tier census refused the registered population: {}",
                error.message()
            )),
        }
    }

    if plan.wants(Work::CompoundingPlan) {
        match policy.plan(signals.undeployed_profit, signals.cycle) {
            // `describe` says what the plan is and, where there is one, that
            // nothing was redeployed. The decision is not held anywhere and
            // nothing downstream reads it: `ReinvestmentPlan` has no method
            // that applies it and no function in this workspace takes one.
            Ok(decision) => parts.push(decision.describe()),
            Err(error) => problems.push(format!(
                "the reinvestment plan could not be computed: {}",
                error.message()
            )),
        }
    }

    let summary = (!parts.is_empty()).then(|| parts.join("; "));
    (summary, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn policy() -> CompoundingPolicy {
        CompoundingPolicy::new(REINVESTMENT_CADENCE_CYCLES, dec!("1000"), 15.0, 0.05)
            .expect("a checked policy")
    }

    fn signals(cycle: u64, strategies: usize, profit: Decimal) -> CadenceSignals {
        CadenceSignals {
            cycle,
            classified_strategies: strategies,
            undeployed_profit: profit,
        }
    }

    #[test]
    fn a_cycle_where_nothing_has_a_subject_runs_no_work_at_all() {
        // §23.6's last row, and the premise that it is a decision rather
        // than an accident: the *same* cycle with subjects runs both items,
        // so the empty plan is the absence of a subject and not a cadence
        // that never comes round.
        let due = TIERING_CADENCE_CYCLES;
        let nothing = plan(&signals(due, 0, Decimal::ZERO), &policy());
        assert!(
            nothing.is_saving(),
            "work ran on a cycle with no strategies and no profit: {:?}",
            nothing.work()
        );
        assert_eq!(
            nothing.held().len(),
            2,
            "the plan does not say why each item was held: {:?}",
            nothing.held()
        );

        let something = plan(&signals(due, 3, dec!("10000")), &policy());
        assert!(!something.is_saving());
        assert!(something.wants(Work::EvaluationTiering));
        assert!(something.wants(Work::CompoundingPlan));
    }

    #[test]
    fn work_with_a_subject_still_waits_for_its_cadence() {
        // A subject on a cycle that is not the item's own: held, and the
        // plan says when it will be considered.
        let between = TIERING_CADENCE_CYCLES + 1;
        assert_ne!(between % TIERING_CADENCE_CYCLES, 0, "the premise is wrong");
        let held = plan(&signals(between, 3, dec!("10000")), &policy());
        assert!(
            held.is_saving(),
            "an item ran between its cadences: {:?}",
            held.work()
        );
        assert!(
            held.held().iter().any(|note| note.contains("one cycle in")),
            "the tier census was held without saying why: {:?}",
            held.held()
        );
        assert!(
            held.held()
                .iter()
                .any(|note| note.contains("considered again at cycle")),
            "reinvestment was held without saying when it comes round: {:?}",
            held.held()
        );
    }

    #[test]
    fn a_book_with_profit_and_no_strategies_runs_only_the_item_that_has_a_subject() {
        // The two items are independently gated. A plan that ran both
        // whenever either had a subject would write a census of nothing on
        // every profitable cycle.
        let due = TIERING_CADENCE_CYCLES;
        let profit_only = plan(&signals(due, 0, dec!("10000")), &policy());
        assert!(profit_only.wants(Work::CompoundingPlan));
        assert!(
            !profit_only.wants(Work::EvaluationTiering),
            "a census ran over an empty population"
        );

        let strategies_only = plan(&signals(due, 5, Decimal::ZERO), &policy());
        assert!(strategies_only.wants(Work::EvaluationTiering));
        assert!(
            !strategies_only.wants(Work::CompoundingPlan),
            "a reinvestment was considered against no profit"
        );
    }
}
