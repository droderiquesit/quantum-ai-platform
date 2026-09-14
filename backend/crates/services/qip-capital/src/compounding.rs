//! Blueprint §18.4, the compounding policy: when realised profit is worth
//! redeploying, what capital levels unlock what, and what trailing volume a
//! venue's fee ladder is claimed on.
//!
//! "At small capital the reinvestment schedule is not an accounting detail;
//! it is a strategy in its own right, and there was no layer that reasoned
//! about it." This is that layer, and it reasons rather than acts.
//!
//! # Nothing here moves capital, and that is the design
//!
//! Reinvestment is the one direction in this platform that *adds* to what may
//! be deployed. A policy that redeployed profit on its own schedule would be
//! an escalation path whose authority is an arithmetic threshold — the shape
//! ADR 0061 and ADR 0063 refuse for a risk bound and a position size, and it
//! is no more acceptable for the size of the book. So [`ReinvestmentPlan`] is
//! a record: no function in this module takes `&mut` anything that holds
//! money, and the plan is something a person reads and acts on.
//!
//! What *is* automatic is the refusal: [`CompoundingPolicy::plan`] answers
//! [`ReinvestmentDecision::BelowMinimumLot`] or
//! [`ReinvestmentDecision::CostExceedsBenefit`] rather than a plan whenever
//! the redeployment would not pay for itself. Those two are the section's own
//! question — "how often realised profit is redeployed **against the
//! transaction cost of redeploying it**" — and they are the arms that fire
//! most often on a small book, which is the book the section is about.
//!
//! # Money is `Decimal`, a rate is `f64`, and the crossing point is
//! [`CompoundingPolicy::redeployment_cost`]
//!
//! Every amount here — profit, lot, threshold, trailing volume, saving — is
//! [`Decimal`]. The only `f64` is a rate in basis points, which is a
//! statistic about a venue's schedule rather than an amount of money, and it
//! crosses into `Decimal` exactly once per entry point, through
//! [`Decimal::checked_apply_bps`], which refuses rather than saturating. A
//! cost computed in `f64` and subtracted from a `Decimal` balance is how a
//! book acquires a few units of error per trade, which is a position nobody
//! approved.
//!
//! # Cadence is counted in cycles
//!
//! [`CompoundingPolicy::due`] is a function of the platform's cycle count
//! rather than of a wall clock, because the cycle count is a fact the caller
//! already holds and a wall-clock cadence needs a "when did this last run"
//! that something would have to store — a second source of truth for a fact
//! the event log already holds. A cadence of `n` means the policy is
//! considered on one cycle in `n`, and the other `n - 1` are the saving.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most venues one ledger will carry trailing volume for.
///
/// A bounded working set, as every accumulator in this platform has: the
/// ledger is keyed by venue and a caller that invented a venue key per order
/// would otherwise grow it without limit. Sixty-four is far above the venue
/// list any deployment of this platform configures, so crossing it is
/// evidence of a key that is not a venue rather than of a large desk.
pub const MAX_TRACKED_VENUES: usize = 64;

/// The most days of volume history one venue keeps.
///
/// Fee schedules are quoted on trailing thirty-day volume almost everywhere,
/// so thirty is the window the number is claimed on; a longer memory would
/// overstate what a venue will actually price.
pub const TRAILING_VOLUME_DAYS: usize = 30;

/// What the platform does with realised profit, and how often it asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CompoundingPolicyWire")]
pub struct CompoundingPolicy {
    cadence_cycles: u64,
    minimum_lot: Decimal,
    redeployment_cost_bps: f64,
    cost_ceiling_fraction: f64,
}

/// The on-disk shape of [`CompoundingPolicy`]. Deserialisation is routed
/// through [`CompoundingPolicy::new`] by `serde(try_from)`, because a plain
/// derive over private fields is a second constructor that writes past every
/// check — the mistake `AssetValuation`, `Judgement` and `ValuationInput`
/// each made in this workspace. Here the value walked around would be the
/// zero cadence, which turns [`CompoundingPolicy::due`] into a division by
/// zero.
#[derive(Deserialize)]
struct CompoundingPolicyWire {
    cadence_cycles: u64,
    minimum_lot: Decimal,
    redeployment_cost_bps: f64,
    cost_ceiling_fraction: f64,
}

impl TryFrom<CompoundingPolicyWire> for CompoundingPolicy {
    type Error = Error;

    fn try_from(wire: CompoundingPolicyWire) -> Result<Self> {
        Self::new(
            wire.cadence_cycles,
            wire.minimum_lot,
            wire.redeployment_cost_bps,
            wire.cost_ceiling_fraction,
        )
    }
}

impl CompoundingPolicy {
    /// A policy, checked.
    ///
    /// Refuses rather than clamps, every time: a cadence of zero, a
    /// non-positive minimum lot, a negative or non-finite cost, and a cost
    /// ceiling outside `(0, 1]`. Each refusal names what to supply instead,
    /// because each is a caller that computed something wrong rather than a
    /// value that needs rounding into range.
    pub fn new(
        cadence_cycles: u64,
        minimum_lot: Decimal,
        redeployment_cost_bps: f64,
        cost_ceiling_fraction: f64,
    ) -> Result<Self> {
        if cadence_cycles == 0 {
            return Err(Error::invalid(
                "a reinvestment cadence of zero cycles asks the question on no cycle at all; \
                 supply the number of cycles between one consideration and the next",
            ));
        }
        if minimum_lot <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "a minimum reinvestment lot of {minimum_lot} would redeploy any profit at all, \
                 including one smaller than the cost of redeploying it; supply a positive lot"
            )));
        }
        if !redeployment_cost_bps.is_finite() || redeployment_cost_bps < 0.0 {
            return Err(Error::numeric(format!(
                "a redeployment cost of {redeployment_cost_bps} bps is not a cost; supply a \
                 finite, non-negative rate in basis points of the amount redeployed"
            )));
        }
        if !cost_ceiling_fraction.is_finite()
            || cost_ceiling_fraction <= 0.0
            || cost_ceiling_fraction > 1.0
        {
            return Err(Error::invalid(format!(
                "a cost ceiling of {cost_ceiling_fraction} is outside (0, 1]; it is the fraction \
                 of a lot that may be spent redeploying it, and a ceiling above one would permit \
                 a redeployment that costs more than it moves"
            )));
        }
        Ok(Self {
            cadence_cycles,
            minimum_lot,
            redeployment_cost_bps,
            cost_ceiling_fraction,
        })
    }

    pub fn cadence_cycles(&self) -> u64 {
        self.cadence_cycles
    }

    pub fn minimum_lot(&self) -> Decimal {
        self.minimum_lot
    }

    pub fn redeployment_cost_bps(&self) -> f64 {
        self.redeployment_cost_bps
    }

    /// Whether this cycle is one the policy considers reinvestment on.
    ///
    /// Cycle zero is due: a process that had to run a full cadence before
    /// considering anything would be silent for exactly the period an
    /// operator is watching it most closely.
    pub fn due(&self, cycle: u64) -> bool {
        cycle.is_multiple_of(self.cadence_cycles)
    }

    /// What redeploying `amount` costs, in money.
    ///
    /// The crossing point from a basis-point rate (`f64`, a statistic about
    /// a schedule) into money (`Decimal`). Refuses rather than saturating:
    /// an amount whose cost cannot be represented is one nobody should be
    /// handed a plan for.
    pub fn redeployment_cost(&self, amount: Decimal) -> Result<Decimal> {
        amount
            .checked_apply_bps(self.redeployment_cost_bps)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the cost of redeploying {amount} at {} bps cannot be represented",
                    self.redeployment_cost_bps
                ))
            })
    }

    /// What the platform would do with `undeployed_profit` this cycle.
    ///
    /// Four answers, and each is a different fact about the book, which is
    /// why this is an enum rather than an `Option`: "not this cycle", "too
    /// small to bother", "the cost eats it" and "here is the plan" are the
    /// four things an operator asks of a compounding schedule, and
    /// collapsing the first three into `None` would make a book that never
    /// compounds indistinguishable from one merely between cadences.
    pub fn plan(&self, undeployed_profit: Decimal, cycle: u64) -> Result<ReinvestmentDecision> {
        if !self.due(cycle) {
            return Ok(ReinvestmentDecision::NotDue {
                next_cycle: self.next_due(cycle),
            });
        }
        if undeployed_profit < self.minimum_lot {
            return Ok(ReinvestmentDecision::BelowMinimumLot {
                available: undeployed_profit,
                minimum_lot: self.minimum_lot,
            });
        }
        let cost = self.redeployment_cost(undeployed_profit)?;
        let ceiling = undeployed_profit
            .checked_apply_bps(self.cost_ceiling_fraction * 10_000.0)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the cost ceiling on {undeployed_profit} cannot be represented"
                ))
            })?;
        if cost > ceiling {
            return Ok(ReinvestmentDecision::CostExceedsBenefit {
                available: undeployed_profit,
                cost,
                ceiling,
            });
        }
        Ok(ReinvestmentDecision::Plan(ReinvestmentPlan {
            amount: undeployed_profit,
            cost,
            cycle,
        }))
    }

    /// The next cycle on which the policy asks again.
    pub fn next_due(&self, cycle: u64) -> u64 {
        let past = cycle % self.cadence_cycles;
        cycle.saturating_add(self.cadence_cycles - past)
    }
}

/// What [`CompoundingPolicy::plan`] concluded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReinvestmentDecision {
    /// This cycle is between cadences. This arm is the saving.
    NotDue { next_cycle: u64 },
    /// There is profit, and not enough of it to be worth a trade.
    BelowMinimumLot {
        available: Decimal,
        minimum_lot: Decimal,
    },
    /// Redeploying this lot would spend more than the ceiling allows.
    CostExceedsBenefit {
        available: Decimal,
        cost: Decimal,
        ceiling: Decimal,
    },
    /// A lot worth redeploying, and what it would cost. **A record. Nothing
    /// in this platform acts on it without a person.**
    Plan(ReinvestmentPlan),
}

impl ReinvestmentDecision {
    /// One line for a cycle's record.
    pub fn describe(&self) -> String {
        match self {
            Self::NotDue { next_cycle } => {
                format!("reinvestment is not considered until cycle {next_cycle}")
            }
            Self::BelowMinimumLot {
                available,
                minimum_lot,
            } => format!(
                "{available} of undeployed profit is below the minimum reinvestment lot of \
                 {minimum_lot}, so nothing is redeployed"
            ),
            Self::CostExceedsBenefit {
                available,
                cost,
                ceiling,
            } => format!(
                "redeploying {available} would cost {cost} against a ceiling of {ceiling}, so \
                 nothing is redeployed"
            ),
            Self::Plan(plan) => plan.describe(),
        }
    }

    pub fn plan(&self) -> Option<&ReinvestmentPlan> {
        match self {
            Self::Plan(plan) => Some(plan),
            _ => None,
        }
    }
}

/// A lot of realised profit worth redeploying, and what redeploying it
/// costs.
///
/// Carries no authority. The type holds two amounts and a cycle; there is no
/// method on it that applies anything, and no function in this crate takes
/// one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReinvestmentPlan {
    pub amount: Decimal,
    pub cost: Decimal,
    pub cycle: u64,
}

impl ReinvestmentPlan {
    /// What is left of the lot after the cost of moving it.
    pub fn net(&self) -> Decimal {
        self.amount.checked_sub(self.cost).unwrap_or(Decimal::ZERO)
    }

    pub fn describe(&self) -> String {
        format!(
            "{} of realised profit is worth redeploying at cycle {} for a cost of {} ({} net); \
             the platform has redeployed nothing and this is a plan for a person",
            self.amount,
            self.cycle,
            self.cost,
            self.net()
        )
    }
}

/// One capital level and what reaching it makes feasible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapitalThreshold {
    /// The equity at which this becomes feasible.
    pub at: Decimal,
    /// What it unlocks — a venue, a strategy, a size. Free text, because the
    /// thing unlocked is the operator's own vocabulary; it reaches a record
    /// and nothing branches on it.
    pub unlocks: String,
}

/// §18.4's threshold-crossing row: "capital levels at which new venues,
/// strategies or sizes become feasible. Approaching one is worth planning
/// for."
///
/// Sorted and checked once, so every read is a scan of a canonical ladder
/// rather than a sort at the call site. A ladder whose rungs a caller could
/// supply out of order would answer [`Self::next_above`] with whichever rung
/// happened to be first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ThresholdLadderWire")]
pub struct ThresholdLadder {
    rungs: Vec<CapitalThreshold>,
}

#[derive(Deserialize)]
struct ThresholdLadderWire {
    rungs: Vec<CapitalThreshold>,
}

impl TryFrom<ThresholdLadderWire> for ThresholdLadder {
    type Error = Error;

    fn try_from(wire: ThresholdLadderWire) -> Result<Self> {
        Self::new(wire.rungs)
    }
}

impl ThresholdLadder {
    /// A ladder, sorted and checked.
    ///
    /// Refuses an empty ladder, a non-positive level, a blank description
    /// and two rungs at the same level. The duplicate is refused rather than
    /// deduplicated because two different things unlocking at the same
    /// capital is a fact an operator wrote down twice or wrote down wrongly,
    /// and silently keeping one of them loses whichever the reader needed.
    pub fn new(mut rungs: Vec<CapitalThreshold>) -> Result<Self> {
        if rungs.is_empty() {
            return Err(Error::invalid(
                "a threshold ladder with no rungs answers every question with 'nothing unlocks'; \
                 supply the capital levels at which a venue, a strategy or a size becomes \
                 feasible",
            ));
        }
        for rung in &rungs {
            if rung.at <= Decimal::ZERO {
                return Err(Error::invalid(format!(
                    "a threshold at {} is not a capital level; supply a positive one",
                    rung.at
                )));
            }
            if rung.unlocks.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "the threshold at {} names nothing it unlocks, so crossing it would be a \
                     record nobody can act on",
                    rung.at
                )));
            }
        }
        rungs.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.unlocks.cmp(&b.unlocks)));
        if let Some(pair) = rungs.windows(2).find(|pair| pair[0].at == pair[1].at) {
            return Err(Error::invalid(format!(
                "two thresholds sit at {}: {:?} and {:?}; one ladder cannot have two rungs at one \
                 level, so state them as one rung or move one",
                pair[0].at, pair[0].unlocks, pair[1].unlocks
            )));
        }
        Ok(Self { rungs })
    }

    pub fn rungs(&self) -> &[CapitalThreshold] {
        &self.rungs
    }

    /// The rungs `equity` has already reached, lowest first.
    pub fn reached(&self, equity: Decimal) -> Vec<&CapitalThreshold> {
        self.rungs.iter().filter(|rung| equity >= rung.at).collect()
    }

    /// The next rung above `equity`, if there is one.
    pub fn next_above(&self, equity: Decimal) -> Option<&CapitalThreshold> {
        self.rungs.iter().find(|rung| rung.at > equity)
    }

    /// What is still needed to reach the next rung.
    pub fn distance_to_next(&self, equity: Decimal) -> Option<Decimal> {
        self.next_above(equity)
            .and_then(|rung| rung.at.checked_sub(equity))
    }

    /// The next rung, if the book is within `margin` of it — "approaching
    /// one is worth planning for".
    ///
    /// Refuses a negative margin rather than reading it as zero: a caller
    /// that computed a negative proximity has a sign error, and answering
    /// `None` would hide it behind a plausible "nothing is close".
    pub fn approaching(
        &self,
        equity: Decimal,
        margin: Decimal,
    ) -> Result<Option<&CapitalThreshold>> {
        if margin < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "a proximity margin of {margin} is negative; supply the distance below a rung at \
                 which the platform should start planning for it"
            )));
        }
        let within = self
            .distance_to_next(equity)
            .is_some_and(|gap| gap <= margin);
        Ok(self.next_above(equity).filter(|_| within))
    }
}

/// Trailing traded volume per venue, which is what a fee ladder is claimed
/// on.
///
/// §18.4's fee-tier row — "volume thresholds that reduce fees. Reaching one
/// can be worth more than the trades that reach it" — needs two things: the
/// volume, and the ladder. **This type holds the volume and deliberately
/// does not restate the ladder.** `qip_routing::FeeSchedule` already owns
/// the rungs, their thresholds and their maker and taker rates, and a second
/// ladder here would be a second answer to what a venue charges — the
/// failure this lane's regime reader is careful to avoid for the regime
/// classifier. A caller reads a rung's `from_volume` off the schedule and
/// asks [`Self::distance_to`]; it reads the rate difference off the schedule
/// and asks [`Self::saving_from`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeVolumeLedger {
    /// Venue → day → notional traded that day. `BTreeMap` twice over: the
    /// trailing total reaches a record, and a replay that reorders is not a
    /// replay.
    volume: BTreeMap<String, BTreeMap<i64, Decimal>>,
}

impl FeeVolumeLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a fill's notional to a venue's trailing volume.
    ///
    /// Refuses a negative notional — a fill that reduced a position is not a
    /// thing a venue's ladder knows about, and netting one off would
    /// understate the tier the desk is entitled to — a blank venue, and a
    /// venue past [`MAX_TRACKED_VENUES`]. Days older than
    /// [`TRAILING_VOLUME_DAYS`] are dropped on every write, so the working
    /// set is bounded by construction rather than by a caller remembering to
    /// prune.
    pub fn record(&mut self, venue: &str, notional: Decimal, at: Timestamp) -> Result<()> {
        if venue.trim().is_empty() {
            return Err(Error::invalid(
                "trailing volume was recorded against a blank venue; supply the venue the fill \
                 happened at, because a fee ladder is a venue's and not the desk's",
            ));
        }
        if notional < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "a notional of {notional} at {venue} is negative; a venue's trailing volume \
                 counts what was traded, so supply the absolute notional of the fill"
            )));
        }
        if !self.volume.contains_key(venue) && self.volume.len() >= MAX_TRACKED_VENUES {
            return Err(Error::invalid(format!(
                "{venue} would be venue number {} with trailing volume, past the bound of \
                 {MAX_TRACKED_VENUES}; a key that is not a venue is the usual cause, so check \
                 the key rather than raising the bound",
                self.volume.len() + 1
            )));
        }
        let day = at.start_of_day().as_secs();
        let days = self.volume.entry(venue.to_string()).or_default();
        let entry = days.entry(day).or_insert(Decimal::ZERO);
        *entry = entry.checked_add(notional).ok_or_else(|| {
            Error::numeric(format!(
                "adding {notional} to {venue}'s volume for the day overflows"
            ))
        })?;
        while days.len() > TRAILING_VOLUME_DAYS {
            let Some(oldest) = days.keys().next().copied() else {
                break;
            };
            days.remove(&oldest);
        }
        Ok(())
    }

    /// A venue's trailing volume over the days still retained.
    pub fn trailing(&self, venue: &str) -> Decimal {
        self.volume.get(venue).map_or(Decimal::ZERO, |days| {
            days.values().fold(Decimal::ZERO, |total, amount| {
                total.checked_add(*amount).unwrap_or(total)
            })
        })
    }

    /// Every venue carrying volume, in a stable order.
    pub fn venues(&self) -> Vec<&str> {
        self.volume.keys().map(String::as_str).collect()
    }

    /// How much more volume `venue` needs to reach `threshold` — a rung's
    /// `from_volume`, read off the venue's own fee schedule by the caller.
    ///
    /// `None` where the rung is already reached, which is a different answer
    /// from zero: zero would mean "one unit short".
    pub fn distance_to(&self, venue: &str, threshold: Decimal) -> Option<Decimal> {
        let trailing = self.trailing(venue);
        if trailing >= threshold {
            return None;
        }
        threshold.checked_sub(trailing)
    }

    /// What reaching a rung would have been worth on the volume already
    /// traded: the trailing volume at the rate difference between the rung
    /// in force and the rung above it.
    ///
    /// `rate_improvement_bps` is the caller's subtraction between two rates
    /// on the venue's schedule, and it is a rate rather than money, which is
    /// why it is `f64`; the multiplication into money happens here and
    /// refuses rather than saturating. Refuses a negative improvement: a
    /// higher rung that charges more is not a saving, and a negative
    /// "saving" would read as a cost in a record that says saving.
    pub fn saving_from(&self, venue: &str, rate_improvement_bps: f64) -> Result<Decimal> {
        if !rate_improvement_bps.is_finite() || rate_improvement_bps < 0.0 {
            return Err(Error::numeric(format!(
                "a rate improvement of {rate_improvement_bps} bps at {venue} is not an \
                 improvement; supply the current rate less the rung's rate, in basis points"
            )));
        }
        let trailing = self.trailing(venue);
        trailing
            .checked_apply_bps(rate_improvement_bps)
            .ok_or_else(|| {
                Error::numeric(format!(
                    "{trailing} of volume at {rate_improvement_bps} bps cannot be represented"
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;
    use qip_core::time::Duration;

    fn policy() -> CompoundingPolicy {
        CompoundingPolicy::new(4, dec!("1000"), 15.0, 0.05).expect("a checked policy")
    }

    #[test]
    fn profit_smaller_than_the_lot_is_not_redeployed_and_profit_at_it_is_planned() {
        let policy = policy();
        // The premise: the cycle is one the policy asks on, so the refusal
        // below is about the amount and not about the cadence.
        assert!(policy.due(8), "cycle 8 is not a cadence cycle under n=4");
        let small = policy
            .plan(dec!("999.999999999"), 8)
            .expect("a representable plan");
        assert!(
            matches!(small, ReinvestmentDecision::BelowMinimumLot { .. }),
            "a lot one unit under the minimum was planned: {small:?}"
        );
        assert!(small.plan().is_none());

        let exact = policy.plan(dec!("1000"), 8).expect("a representable plan");
        let plan = exact
            .plan()
            .expect("the minimum lot itself is redeployable");
        assert_eq!(plan.amount, dec!("1000"));
        // 15 bps of 1000 is 1.5, against a ceiling of 5% of 1000 = 50.
        assert_eq!(plan.cost, dec!("1.5"));
        assert_eq!(plan.net(), dec!("998.5"));
    }

    #[test]
    fn a_redeployment_whose_cost_eats_the_lot_is_refused_rather_than_planned() {
        // The cost row of §18.4 with a rate that makes it bind: 600 bps is
        // six percent against a ceiling of five. The premise first — the
        // same lot at the ordinary rate *is* planned, so this arm is about
        // the cost and not about the amount.
        let ordinary = policy();
        assert!(
            ordinary
                .plan(dec!("2000"), 0)
                .expect("representable")
                .plan()
                .is_some(),
            "the premise fails: this lot is not planned even at the ordinary rate"
        );
        let expensive =
            CompoundingPolicy::new(4, dec!("1000"), 600.0, 0.05).expect("a checked policy");
        let decision = expensive.plan(dec!("2000"), 0).expect("representable");
        match decision {
            ReinvestmentDecision::CostExceedsBenefit {
                cost,
                ceiling,
                available,
            } => {
                assert_eq!(available, dec!("2000"));
                assert_eq!(cost, dec!("120"));
                assert_eq!(ceiling, dec!("100"));
            }
            other => {
                panic!("a redeployment costing 6% against a 5% ceiling was planned: {other:?}")
            }
        }
    }

    #[test]
    fn the_cadence_asks_on_one_cycle_in_n_and_says_when_it_will_ask_again() {
        let policy = policy();
        let due: Vec<u64> = (0..9).filter(|cycle| policy.due(*cycle)).collect();
        assert_eq!(
            due,
            vec![0, 4, 8],
            "the cadence is not one cycle in four, so the saving is not the saving"
        );
        match policy.plan(dec!("100000"), 5).expect("representable") {
            ReinvestmentDecision::NotDue { next_cycle } => assert_eq!(next_cycle, 8),
            other => panic!("a cycle between cadences planned a redeployment: {other:?}"),
        }
        // A cadence of zero is refused rather than read as "every cycle":
        // `due` would divide by zero.
        let refusal = CompoundingPolicy::new(0, dec!("1"), 1.0, 0.5).expect_err("zero is refused");
        assert!(
            refusal.message().contains("cadence of zero"),
            "the refusal does not name the failure: {}",
            refusal.message()
        );
    }

    #[test]
    fn a_policy_that_arrives_by_deserialisation_goes_through_the_same_checks() {
        // The second constructor this repository has been burned by three
        // times. The premise: a good document round-trips.
        let good = serde_json::json!({
            "cadence_cycles": 4,
            "minimum_lot": "1000",
            "redeployment_cost_bps": 15.0,
            "cost_ceiling_fraction": 0.05,
        });
        let parsed: CompoundingPolicy =
            serde_json::from_value(good).expect("a valid policy deserialises");
        assert_eq!(parsed, policy());

        let bad = serde_json::json!({
            "cadence_cycles": 0,
            "minimum_lot": "1000",
            "redeployment_cost_bps": 15.0,
            "cost_ceiling_fraction": 0.05,
        });
        let refusal = serde_json::from_value::<CompoundingPolicy>(bad)
            .expect_err("a zero cadence is refused at the door");
        assert!(
            refusal.to_string().contains("cadence of zero"),
            "deserialisation walked around the constructor: {refusal}"
        );
    }

    #[test]
    fn a_ladder_names_the_next_rung_and_says_how_far_away_it_is() {
        let ladder = ThresholdLadder::new(vec![
            CapitalThreshold {
                at: dec!("500000"),
                unlocks: "a second venue".to_string(),
            },
            CapitalThreshold {
                at: dec!("100000"),
                unlocks: "overnight carry".to_string(),
            },
        ])
        .expect("a checked ladder");
        // Sorted on the way in, so the supplied order cannot decide the
        // answer: the premise is that these went in high-then-low.
        assert_eq!(ladder.rungs()[0].at, dec!("100000"));
        assert_eq!(ladder.reached(dec!("120000")).len(), 1);
        assert_eq!(
            ladder.next_above(dec!("120000")).map(|rung| rung.at),
            Some(dec!("500000"))
        );
        assert_eq!(
            ladder.distance_to_next(dec!("120000")),
            Some(dec!("380000"))
        );
        assert_eq!(ladder.next_above(dec!("900000")), None);
        assert_eq!(ladder.distance_to_next(dec!("900000")), None);

        // "Approaching one is worth planning for": inside the margin it is
        // named, outside it is not.
        assert_eq!(
            ladder
                .approaching(dec!("450000"), dec!("50000"))
                .expect("a non-negative margin")
                .map(|rung| rung.unlocks.as_str()),
            Some("a second venue")
        );
        assert_eq!(
            ladder
                .approaching(dec!("449999"), dec!("50000"))
                .expect("a non-negative margin")
                .map(|rung| rung.at),
            None
        );
        assert!(
            ladder.approaching(dec!("1"), dec!("-1")).is_err(),
            "a negative proximity margin was read as zero instead of refused"
        );
    }

    #[test]
    fn two_rungs_at_one_capital_level_are_refused_rather_than_silently_merged() {
        let refusal = ThresholdLadder::new(vec![
            CapitalThreshold {
                at: dec!("100000"),
                unlocks: "overnight carry".to_string(),
            },
            CapitalThreshold {
                at: dec!("100000"),
                unlocks: "a second venue".to_string(),
            },
        ])
        .expect_err("a duplicate level is refused");
        assert!(
            refusal.message().contains("two thresholds sit at"),
            "the refusal does not name the failure: {}",
            refusal.message()
        );
        assert!(ThresholdLadder::new(Vec::new()).is_err());
        assert!(
            ThresholdLadder::new(vec![CapitalThreshold {
                at: dec!("100000"),
                unlocks: "   ".to_string(),
            }])
            .is_err(),
            "a rung that unlocks nothing nameable was admitted"
        );
    }

    #[test]
    fn trailing_volume_is_bounded_at_thirty_days_and_a_reached_rung_is_no_distance_at_all() {
        let mut ledger = FeeVolumeLedger::new();
        let start = Timestamp::from_secs(1_760_000_000);
        // Forty days of a thousand each: the premise is forty writes, and
        // the assertion is that only thirty are still counted.
        for index in 0..40i64 {
            ledger
                .record(
                    "venue-alpha",
                    dec!("1000"),
                    start.saturating_add(Duration::from_days(index)),
                )
                .expect("a positive notional at a named venue");
        }
        assert_eq!(
            ledger.trailing("venue-alpha"),
            dec!("30000"),
            "the trailing window is not bounded at thirty days"
        );
        assert_eq!(ledger.venues(), vec!["venue-alpha"]);
        assert_eq!(ledger.trailing("venue-beta"), Decimal::ZERO);

        // A rung above the volume is a distance; one already reached is
        // `None`, which is not zero.
        assert_eq!(
            ledger.distance_to("venue-alpha", dec!("50000")),
            Some(dec!("20000"))
        );
        assert_eq!(ledger.distance_to("venue-alpha", dec!("30000")), None);
        // Two basis points off the whole trailing volume.
        assert_eq!(
            ledger
                .saving_from("venue-alpha", 2.0)
                .expect("a positive improvement"),
            dec!("6")
        );
        assert!(
            ledger.saving_from("venue-alpha", -2.0).is_err(),
            "a rung that charges more was reported as a saving"
        );
        assert!(
            ledger.record("", dec!("1"), start).is_err(),
            "volume was recorded against a blank venue"
        );
        assert!(
            ledger.record("venue-alpha", dec!("-1"), start).is_err(),
            "a negative notional was netted off a venue's trailing volume"
        );
    }

    #[test]
    fn a_ledger_refuses_a_venue_past_its_bound_rather_than_growing_without_limit() {
        let start = Timestamp::from_secs(1_760_000_000);
        let mut ledger = FeeVolumeLedger::new();
        for index in 0..MAX_TRACKED_VENUES {
            ledger
                .record(&format!("venue-{index:03}"), dec!("1"), start)
                .expect("within the bound");
        }
        assert_eq!(ledger.venues().len(), MAX_TRACKED_VENUES);
        let refusal = ledger
            .record("venue-one-too-many", dec!("1"), start)
            .expect_err("past the bound");
        assert!(
            refusal.message().contains("past the bound"),
            "the refusal does not name the bound: {}",
            refusal.message()
        );
        // And a venue already tracked still records, so the bound is on new
        // keys rather than on writes.
        assert!(ledger.record("venue-000", dec!("1"), start).is_ok());
    }
}
