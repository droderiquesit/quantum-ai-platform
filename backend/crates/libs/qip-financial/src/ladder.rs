//! The liquidity ladder: what can become cash, how fast, and at what cost
//! (blueprint §25.4).
//!
//! The ladder orders everything the platform holds into seven rungs, from cash
//! at a venue down to private-equity commitments. Its defining property is
//! that cost to liquidate rises monotonically as you descend, and a request
//! for cash is served from the top downward — so a routine reduction is met
//! from the cheapest rungs and never by forcing a position closed at a bad
//! price.
//!
//! # This describes depth. It never reaches for it.
//!
//! Nothing in this module creates, enables or eases an order path. A
//! [`LiquidationPlan`] carries object identifiers, amounts and costs; it
//! carries no venue, no side, no order type, no time in force and no
//! identifier any execution surface could act on, and there is no function
//! here that hands one to anything. It is an answer to "where would the money
//! come from, and what would it cost", computed so that a risk read or a
//! divestment ranking can be argued about before anything is decided. Turning
//! a plan into orders would be a separate, reviewed act in the execution
//! domain, under the paper-trading boundary that domain already holds.
//!
//! The blueprint's own caller for this — serving a withdrawal from the top of
//! the ladder downward — does not exist and cannot. ADR 0021 and ADR 0023
//! refuse the path by which capital leaves the platform, and
//! `qip_capital::ledger::WithdrawalEntitlement` has exactly one variant,
//! `Refused`. The ladder is therefore a valuation and risk instrument here:
//! it says how much of the book is reachable within a horizon, which is a
//! question worth answering whether or not anything is ever withdrawn.
//!
//! Every value and every cost is a [`Decimal`], because both are money. The
//! horizons are categorical and the rungs are an enum, so no money here is
//! ever a statistic. The one number that is not money is
//! [`LiquidationHorizon::least_days`], which exists because the risk domain
//! states its liquidity floors in days and something has to translate; it is
//! a floor rather than an estimate, for the reason given on it.

use crate::asset_class::AssetClass;
use crate::costs::LiquidityProfile;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How long a rung takes to turn into cash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiquidationHorizon {
    Immediate,
    Seconds,
    SameDay,
    Days,
    Months,
    Years,
}

impl LiquidationHorizon {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Seconds => "seconds",
            Self::SameDay => "same_day",
            Self::Days => "days",
            Self::Months => "months",
            Self::Years => "years",
        }
    }

    /// The fewest days an exit on this horizon can honestly be claimed to
    /// take.
    ///
    /// A **floor**, not an estimate, and the distinction is the whole point.
    /// The horizons are categorical because that is what the ladder can
    /// defend; a risk limit stated in days — `qip_risk`'s `MinLiquidity` and
    /// `MaxDaysToLiquidate` both are — needs a number, and the only number
    /// this type can supply without inventing one is the boundary of its own
    /// bucket. Reading a floor into a liquidity control fails in the safe
    /// direction: it can call a position slower to exit than it is, never
    /// faster, so a floor that is wrong makes a book look less liquid and
    /// tightens the floor rather than relaxing it.
    ///
    /// The values are the boundaries the horizon names, not risk parameters:
    /// `Immediate` and `Seconds` are both inside one trading day and are
    /// therefore zero days, `SameDay` is one, `Days` is two because
    /// [`Rung::classify`] already reserves it for anything stated as taking
    /// more than a single day, `Months` is thirty and `Years` is
    /// three-hundred-and-sixty-five. Anyone tempted to tune one of these is
    /// tuning a calendar, which is the signal that the limit wanted a
    /// different horizon rather than a different number.
    pub fn least_days(&self) -> f64 {
        match self {
            Self::Immediate | Self::Seconds => 0.0,
            Self::SameDay => 1.0,
            Self::Days => 2.0,
            Self::Months => 30.0,
            Self::Years => 365.0,
        }
    }

    /// The deepest horizon that is still inside `days`, or `None` when not
    /// even an immediate exit is.
    ///
    /// The inverse of [`Self::least_days`], and the function a caller needs to
    /// turn a limit stated in days into the ladder question
    /// [`LiquidityLadder::reachable_within`] answers. Deriving it here rather
    /// than at the call site keeps one rule: a caller that re-implemented the
    /// mapping would sooner or later place the boundary on the other side of
    /// a comparison from this one, and the two would disagree about a book
    /// sitting exactly on a horizon.
    ///
    /// `None` for a negative or non-finite `days`, which is a caller stating
    /// a horizon that cannot exist. Refused rather than floored at
    /// `Immediate`, because a limit configured with a nonsense horizon should
    /// read as unevaluated, not as one that passed.
    ///
    /// [`Self::Immediate`] is never returned and is deliberately absent from
    /// the search: it and [`Self::Seconds`] are both zero days, so `Seconds`
    /// is always the deeper of the two answers to the same question, and
    /// `reachable_within(Seconds)` already includes every `Immediate` rung.
    pub fn deepest_within(days: f64) -> Option<Self> {
        if !days.is_finite() || days < 0.0 {
            return None;
        }
        [
            Self::Years,
            Self::Months,
            Self::Days,
            Self::SameDay,
            Self::Seconds,
        ]
        .into_iter()
        .find(|horizon| horizon.least_days() <= days)
    }
}

/// A rung of the liquidity ladder, blueprint §25.4.
///
/// The declaration order **is** the ladder order, top first, and the derived
/// [`Ord`] is what a [`BTreeMap`] keyed on a rung iterates in. That is
/// deliberate rather than incidental: the ordering reaches output, so it must
/// be a property of the type rather than of a sort call somebody could forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    /// Cash and stablecoin at a venue. Immediate, zero cost.
    CashAtVenue,
    /// Liquid spot and perpetual. Seconds; spread and fee.
    LiquidSpotAndPerpetual,
    /// Listed equities and futures. Same day; spread, fee and impact.
    ListedEquityAndFutures,
    /// Bonds and less liquid listed. Days; wider spread, dealer inventory.
    BondsAndLessLiquidListed,
    /// Resting and anchored positions. Days, by abandoning the cycle;
    /// opportunity cost plus unwind.
    RestingAndAnchored,
    /// Private credit and real assets. Months, if at all; substantial discount.
    PrivateCreditAndRealAssets,
    /// Private-equity commitments. Years, or a secondary at a discount.
    PrivateEquityCommitments,
}

impl Rung {
    /// Every rung, top of the ladder first.
    pub const ALL: [Self; 7] = [
        Self::CashAtVenue,
        Self::LiquidSpotAndPerpetual,
        Self::ListedEquityAndFutures,
        Self::BondsAndLessLiquidListed,
        Self::RestingAndAnchored,
        Self::PrivateCreditAndRealAssets,
        Self::PrivateEquityCommitments,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CashAtVenue => "cash_at_venue",
            Self::LiquidSpotAndPerpetual => "liquid_spot_and_perpetual",
            Self::ListedEquityAndFutures => "listed_equity_and_futures",
            Self::BondsAndLessLiquidListed => "bonds_and_less_liquid_listed",
            Self::RestingAndAnchored => "resting_and_anchored",
            Self::PrivateCreditAndRealAssets => "private_credit_and_real_assets",
            Self::PrivateEquityCommitments => "private_equity_commitments",
        }
    }

    /// Depth from the top of the ladder, `0` being cash.
    pub fn depth(&self) -> usize {
        match self {
            Self::CashAtVenue => 0,
            Self::LiquidSpotAndPerpetual => 1,
            Self::ListedEquityAndFutures => 2,
            Self::BondsAndLessLiquidListed => 3,
            Self::RestingAndAnchored => 4,
            Self::PrivateCreditAndRealAssets => 5,
            Self::PrivateEquityCommitments => 6,
        }
    }

    pub fn horizon(&self) -> LiquidationHorizon {
        match self {
            Self::CashAtVenue => LiquidationHorizon::Immediate,
            Self::LiquidSpotAndPerpetual => LiquidationHorizon::Seconds,
            Self::ListedEquityAndFutures => LiquidationHorizon::SameDay,
            Self::BondsAndLessLiquidListed | Self::RestingAndAnchored => LiquidationHorizon::Days,
            Self::PrivateCreditAndRealAssets => LiquidationHorizon::Months,
            Self::PrivateEquityCommitments => LiquidationHorizon::Years,
        }
    }

    /// The rung an instrument sits on, from its class and its liquidity.
    ///
    /// Two facts decide it, and the second can only push an object *down*: an
    /// instrument that trades by negotiation cannot settle in seconds however
    /// its asset class is labelled, and one that takes more than a day to exit
    /// is not a same-day rung. That is classification from evidence, not a
    /// correction of a bad input — a negotiated equity in a private placement
    /// is genuinely on a lower rung than a listed one.
    ///
    /// [`Rung::RestingAndAnchored`] is never returned. A position is on that
    /// rung because a strategy is holding it deliberately, and no property of
    /// the instrument reveals that; the caller who knows the strategy sets it.
    pub fn classify(class: AssetClass, liquidity: &LiquidityProfile) -> Self {
        let by_class = match class {
            AssetClass::Cash => Self::CashAtVenue,
            AssetClass::DigitalAsset | AssetClass::ForeignExchange => Self::LiquidSpotAndPerpetual,
            AssetClass::Equity
            | AssetClass::Derivative
            | AssetClass::Commodity
            | AssetClass::Rates => Self::ListedEquityAndFutures,
            AssetClass::FixedIncome
            | AssetClass::Credit
            | AssetClass::Fund
            | AssetClass::StructuredProduct => Self::BondsAndLessLiquidListed,
            AssetClass::RealAsset => Self::PrivateCreditAndRealAssets,
            AssetClass::PrivateMarket => Self::PrivateEquityCommitments,
        };
        let by_liquidity = if liquidity.is_negotiated {
            Self::PrivateCreditAndRealAssets
        } else if liquidity.days_to_liquidate > 1.0 {
            Self::BondsAndLessLiquidListed
        } else {
            Self::CashAtVenue
        };
        by_class.max(by_liquidity)
    }
}

/// One holding placed on the ladder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderEntry {
    /// The object this value sits in.
    pub object_id: String,
    pub rung: Rung,
    /// What the holding is marked at. Money.
    pub value: Decimal,
    /// What it would cost to turn all of `value` into cash — spread, fee,
    /// impact and discount together. Money, in the same currency as `value`.
    pub cost_to_liquidate: Decimal,
}

impl LadderEntry {
    pub fn new(
        object_id: impl Into<String>,
        rung: Rung,
        value: Decimal,
        cost_to_liquidate: Decimal,
    ) -> Self {
        Self {
            object_id: object_id.into(),
            rung,
            value,
            cost_to_liquidate,
        }
    }
}

/// One rung's contribution to a plan.
///
/// Deliberately not an order: there is no venue, no side and no time in force
/// here, and nothing in this crate turns one into any of those.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLeg {
    pub object_id: String,
    pub rung: Rung,
    /// Value drawn from this holding. Money.
    pub amount: Decimal,
    /// Cost of drawing exactly that much. Money.
    pub cost: Decimal,
}

/// Where a given amount of cash would come from, and what it would cost.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationPlan {
    pub legs: Vec<PlanLeg>,
    /// Total value drawn. Equals the requested amount.
    pub raised: Decimal,
    /// Total cost of drawing it. Money.
    pub cost: Decimal,
    /// The lowest rung the plan had to reach. The number an operator reads
    /// first: a routine request that reaches private credit is a warning
    /// about the shape of the book, not about the request.
    pub deepest_rung: Rung,
}

/// The book, ordered by how readily it becomes cash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidityLadder {
    /// Keyed on `(rung, object_id)` so iteration order is ladder order and a
    /// replay reproduces the same plan leg for leg.
    entries: BTreeMap<(Rung, String), LadderEntry>,
}

impl LiquidityLadder {
    /// Build a ladder, refusing anything that would make it lie.
    ///
    /// Refusals, each naming what to do instead:
    ///
    /// * a non-positive `value` — a holding worth nothing is not a rung of the
    ///   ladder, and dividing a cost across it has no meaning;
    /// * a negative `cost_to_liquidate`;
    /// * a cost exceeding the value it liquidates — a position that costs more
    ///   to exit than it is marked at is a marking error, and serving a
    ///   request from it would report raising cash while destroying more;
    /// * the same `object_id` twice, which would double-count the book;
    /// * a **non-monotonic ladder**: a lower rung cheaper to liquidate than a
    ///   higher one. The ladder's entire purpose is that serving from the top
    ///   downward is serving from the cheapest downward. If that does not
    ///   hold, the rungs are assigned wrongly and every plan built on them
    ///   picks the expensive source first.
    pub fn new(entries: Vec<LadderEntry>) -> Result<Self> {
        let mut map: BTreeMap<(Rung, String), LadderEntry> = BTreeMap::new();
        let mut seen: BTreeMap<String, Rung> = BTreeMap::new();

        for entry in entries {
            if !entry.value.is_positive() {
                return Err(Error::invalid(format!(
                    "holding {id} has value {value}, which is not positive; remove a written-\
                     down holding from the ladder rather than placing it on a rung",
                    id = entry.object_id,
                    value = entry.value
                )));
            }
            if entry.cost_to_liquidate.is_negative() {
                return Err(Error::invalid(format!(
                    "holding {id} has a negative cost to liquidate ({cost}); a cost is what \
                     exiting consumes — supply zero if exiting is free",
                    id = entry.object_id,
                    cost = entry.cost_to_liquidate
                )));
            }
            if entry.cost_to_liquidate > entry.value {
                return Err(Error::invalid(format!(
                    "holding {id} costs {cost} to liquidate but is marked at {value}; correct \
                     the mark or the cost model — a holding that consumes more than it \
                     realises raises no cash and this ladder will not pretend otherwise",
                    id = entry.object_id,
                    cost = entry.cost_to_liquidate,
                    value = entry.value
                )));
            }
            if let Some(existing) = seen.insert(entry.object_id.clone(), entry.rung) {
                return Err(Error::invalid(format!(
                    "holding {id} appears twice, on rungs {first} and {second}; place each \
                     holding on exactly one rung — a repeat double-counts the book",
                    id = entry.object_id,
                    first = existing.as_str(),
                    second = entry.rung.as_str()
                )));
            }
            map.insert((entry.rung, entry.object_id.clone()), entry);
        }

        let ladder = Self { entries: map };
        ladder.prove_monotonic()?;
        Ok(ladder)
    }

    /// Refuse a ladder whose cost does not rise as it descends.
    ///
    /// Rates are compared by cross-multiplication rather than by dividing:
    /// `cost_a / value_a > cost_b / value_b` becomes
    /// `cost_a * value_b > cost_b * value_a`, which keeps money in [`Decimal`]
    /// and never rounds a comparison into or out of a refusal.
    fn prove_monotonic(&self) -> Result<()> {
        let totals = self.value_and_cost_by_rung();
        let mut previous: Option<(Rung, Decimal, Decimal)> = None;
        for (rung, (value, cost)) in totals {
            if let Some((above, above_value, above_cost)) = previous {
                let left = above_cost.checked_mul(value).ok_or_else(|| {
                    Error::numeric(format!(
                        "comparing the cost of rung {} against rung {} overflowed; the values \
                         on this ladder are implausibly large",
                        above.as_str(),
                        rung.as_str()
                    ))
                })?;
                let right = cost.checked_mul(above_value).ok_or_else(|| {
                    Error::numeric(format!(
                        "comparing the cost of rung {} against rung {} overflowed; the values \
                         on this ladder are implausibly large",
                        above.as_str(),
                        rung.as_str()
                    ))
                })?;
                if left > right {
                    return Err(Error::invalid(format!(
                        "rung {above} costs more to liquidate than the lower rung {rung} \
                         ({above_cost} on {above_value} against {cost} on {value}); the \
                         ladder must get more expensive as it descends, so reassign these \
                         holdings to the rungs their exit cost actually places them on",
                        above = above.as_str(),
                        rung = rung.as_str()
                    )));
                }
            }
            previous = Some((rung, value, cost));
        }
        Ok(())
    }

    fn value_and_cost_by_rung(&self) -> BTreeMap<Rung, (Decimal, Decimal)> {
        let mut totals: BTreeMap<Rung, (Decimal, Decimal)> = BTreeMap::new();
        for entry in self.entries.values() {
            let slot = totals
                .entry(entry.rung)
                .or_insert((Decimal::ZERO, Decimal::ZERO));
            slot.0 = slot.0.checked_add(entry.value).unwrap_or(Decimal::MAX);
            slot.1 = slot
                .1
                .checked_add(entry.cost_to_liquidate)
                .unwrap_or(Decimal::MAX);
        }
        totals
    }

    /// Every entry, ladder order, top rung first.
    pub fn entries(&self) -> impl Iterator<Item = &LadderEntry> {
        self.entries.values()
    }

    /// Value on each occupied rung, ladder order.
    pub fn value_by_rung(&self) -> BTreeMap<Rung, Decimal> {
        self.value_and_cost_by_rung()
            .into_iter()
            .map(|(rung, (value, _))| (rung, value))
            .collect()
    }

    /// Everything the ladder holds.
    pub fn total_value(&self) -> Decimal {
        self.entries.values().fold(Decimal::ZERO, |acc, e| {
            acc.checked_add(e.value).unwrap_or(acc)
        })
    }

    /// How much of the book could become cash within `horizon`.
    ///
    /// The risk read the ladder exists to give: a book where nine tenths of
    /// the value is reachable only in months is a different book from one
    /// where nine tenths is reachable the same day, whatever their marks say
    /// they are worth.
    pub fn reachable_within(&self, horizon: LiquidationHorizon) -> Decimal {
        self.entries
            .values()
            .filter(|e| e.rung.horizon() <= horizon)
            .fold(Decimal::ZERO, |acc, e| {
                acc.checked_add(e.value).unwrap_or(acc)
            })
    }

    /// Where `amount` of cash would come from, served from the top downward.
    ///
    /// Refuses a non-positive amount, and refuses **a depth that exceeds the
    /// book**: a request larger than the ladder holds is not served as far as
    /// it goes, because a plan that raises less than it was asked for reads
    /// downstream as a plan that worked. The shortfall is named.
    ///
    /// A partial draw on a holding is charged pro rata: the cost of taking
    /// half a position is half its cost to liquidate. That is an assumption,
    /// and it is the conservative one for the rungs where it is wrong — impact
    /// on a listed name is sublinear in size, so a pro-rata charge on a
    /// partial exit overstates rather than understates the cost.
    pub fn plan(&self, amount: Decimal) -> Result<LiquidationPlan> {
        if !amount.is_positive() {
            return Err(Error::invalid(format!(
                "cannot plan for {amount}; ask for a positive amount of cash"
            )));
        }
        let available = self.total_value();
        if amount > available {
            let short = amount.checked_sub(available).unwrap_or(Decimal::ZERO);
            return Err(Error::invalid(format!(
                "the ladder holds {available} but {amount} was asked for, {short} short; ask \
                 for no more than the book holds — a plan that raises less than it was asked \
                 for reads as one that succeeded"
            )));
        }

        let mut remaining = amount;
        let mut legs = Vec::new();
        let mut cost = Decimal::ZERO;
        let mut deepest = Rung::CashAtVenue;

        for entry in self.entries.values() {
            if !remaining.is_positive() {
                break;
            }
            let take = remaining.min(entry.value);
            // Pro rata: cost * take / value. `value` is positive by `new`'s
            // refusal, so the division has a defined result.
            let leg_cost = entry
                .cost_to_liquidate
                .checked_mul(take)
                .and_then(|n| n.checked_div(entry.value))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "the pro-rata exit cost of holding {id} could not be computed; check \
                         its mark and cost for implausible magnitudes",
                        id = entry.object_id
                    ))
                })?;
            cost = cost.checked_add(leg_cost).ok_or_else(|| {
                Error::numeric("the plan's total exit cost overflowed; the ladder's values are implausibly large")
            })?;
            remaining = remaining.checked_sub(take).ok_or_else(|| {
                Error::numeric("the plan's remaining amount underflowed while drawing down")
            })?;
            deepest = deepest.max(entry.rung);
            legs.push(PlanLeg {
                object_id: entry.object_id.clone(),
                rung: entry.rung,
                amount: take,
                cost: leg_cost,
            });
        }

        if remaining.is_positive() {
            return Err(Error::numeric(format!(
                "the ladder reported enough value but the plan came up {remaining} short; the \
                 ladder's totals and its entries disagree"
            )));
        }

        Ok(LiquidationPlan {
            legs,
            raised: amount,
            cost,
            deepest_rung: deepest,
        })
    }
}
