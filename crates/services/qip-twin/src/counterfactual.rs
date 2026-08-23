//! The alternatives a decision did not take, priced honestly.
//!
//! For every action the platform took there are nine things it could have done
//! instead, and the only interesting question about any of them is what it
//! would have earned. Three rules decide whether that answer is worth having,
//! and all three are enforced by structure rather than by care:
//!
//! * **An alternative is planned from what was knowable and settled against
//!   what happened.** [`CounterfactualEngine::plan`] sees only a
//!   [`DecisionView`], which cannot read past the decision instant.
//!   [`CounterfactualEngine::settle`] sees the realised path but receives the
//!   plan already fixed, so nothing it reads can change what the alternative
//!   was. Choosing the size after seeing the move is the leak the whole
//!   bitemporal apparatus exists to prevent, and here it is two functions apart
//!   with a value in between.
//! * **The fill model is the simulator's.** Prices come from
//!   [`qip_simulation_engine::clock::SimulationClock::fill_price`] and costs
//!   from [`CostModel::cost_of`]. There is no second model in this crate, which
//!   matters because a more generous one would make every alternative beat the
//!   action taken — including the ones that were correctly refused.
//! * **What comes out is simulated and stays simulated.**
//!   [`SimulatedOutcome`] has no accessor returning a bare [`Decimal`] for
//!   money. See [`crate::value`].
//!
//! Where an alternative involves uncertainty the platform genuinely faced — a
//! venue whose quotes are not firm, an entry deferred by five minutes — the
//! engine draws from a seeded [`Xoshiro256`] stream derived from the decision
//! id, so the same seed reproduces the same set whatever order the decisions
//! are evaluated in. The draw can only remove a fill, never improve one.

use crate::asof::{DecisionView, Liquidity, TwinMarket};
use crate::capture::{Decision, RealisedOutcome};
use crate::value::Simulated;
use qip_contracts::message::BookSide;
use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::ids::{DecisionId, ObjectId};
use qip_core::lineage::TraceId;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_simulation_engine::costs::CostModel;
use serde::{Deserialize, Serialize};

/// One of the nine things the platform could have done instead.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "alternative", rename_all = "snake_case")]
pub enum Alternative {
    /// Do exactly what was done. The control: its difference from the action
    /// taken should be small, and a set where it is not has a bug in it.
    Trade,
    /// Stand aside. On a losing trade this is the alternative with positive
    /// regret, and it is the one a platform most needs to be able to price.
    DoNotTrade,
    /// The same trade, scaled up.
    LargerSize { factor: Decimal },
    /// The same trade, scaled down.
    SmallerSize { factor: Decimal },
    /// The same trade at a venue with a different fee schedule and a different
    /// certainty of getting filled.
    DifferentVenue { venue: VenueId },
    /// The same view expressed in another region, through the instrument that
    /// carries it there.
    DifferentRegion { region: String, proxy: ObjectId },
    /// The same trade, entered later.
    DelayedEntry { delay: Duration },
    /// The same trade with a different hedge against it.
    DifferentHedge { hedge: ObjectId, ratio: Decimal },
    /// The same trade with the hedge taken off.
    NoHedge,
}

impl Alternative {
    /// A short label, stable enough to group regret by.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Trade => "trade",
            Self::DoNotTrade => "do_not_trade",
            Self::LargerSize { .. } => "larger_size",
            Self::SmallerSize { .. } => "smaller_size",
            Self::DifferentVenue { .. } => "different_venue",
            Self::DifferentRegion { .. } => "different_region",
            Self::DelayedEntry { .. } => "delayed_entry",
            Self::DifferentHedge { .. } => "different_hedge",
            Self::NoHedge => "no_hedge",
        }
    }

    /// Whether acting on this puts any capital at risk at all.
    pub const fn trades(&self) -> bool {
        !matches!(self, Self::DoNotTrade)
    }
}

/// A venue the twin can move a trade to.
///
/// Carries a [`CostModel`] rather than a fee number because a venue *is* a fee
/// schedule and a fill probability. Reusing the simulator's model with
/// different parameters keeps the alternative priced by the same law as the
/// action taken; inventing a venue-specific formula here would be the second
/// fill model this crate must not have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueOption {
    pub venue: VenueId,
    pub costs: CostModel,
    /// Whether a quote at this venue can be relied on to still be there.
    /// False makes the alternative subject to the availability draw.
    pub firm_quotes: bool,
}

/// Which alternatives the twin will consider.
///
/// [`Alternative::DifferentVenue`], [`Alternative::DifferentRegion`] and
/// [`Alternative::DifferentHedge`] need somewhere to go, so they appear only
/// when the menu names one. The rest are always generated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlternativeMenu {
    /// Multiple for the larger-size alternative. Must exceed one.
    pub larger: Decimal,
    /// Multiple for the smaller-size alternative. Must be between zero and one.
    pub smaller: Decimal,
    /// How long the delayed-entry alternative waits.
    pub delay: Duration,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub venue: Option<VenueOption>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub region: Option<(String, ObjectId)>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hedge: Option<(ObjectId, Decimal)>,
}

impl AlternativeMenu {
    /// Twice the size, half the size, and an entry deferred by `delay`.
    pub fn standard(delay: Duration) -> Self {
        Self {
            larger: Decimal::from_int(2),
            // Half. `Decimal` scales by 10^9, so half a unit is 5·10^8 raw;
            // written as a raw literal because the alternative is a fallible
            // parse in a constructor that has nothing to report a failure to.
            smaller: Decimal::from_raw(500_000_000),
            delay,
            venue: None,
            region: None,
            hedge: None,
        }
    }

    pub fn with_venue(mut self, venue: VenueOption) -> Self {
        self.venue = Some(venue);
        self
    }

    pub fn with_region(mut self, region: impl Into<String>, proxy: ObjectId) -> Self {
        self.region = Some((region.into(), proxy));
        self
    }

    pub fn with_hedge(mut self, hedge: ObjectId, ratio: Decimal) -> Self {
        self.hedge = Some((hedge, ratio));
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.larger <= Decimal::ONE {
            return Err(Error::invalid(
                "the larger-size alternative must be larger than the size actually traded",
            ));
        }
        if self.smaller <= Decimal::ZERO || self.smaller >= Decimal::ONE {
            return Err(Error::invalid(
                "the smaller-size alternative must be a fraction of the size actually traded",
            ));
        }
        if self.delay <= Duration::ZERO {
            return Err(Error::invalid(
                "a delayed entry with no delay is the entry that happened",
            ));
        }
        Ok(())
    }

    /// The alternatives to one action, in a stable order.
    pub fn alternatives_to(&self, actual: &ActualTrade) -> Vec<Alternative> {
        let mut alternatives = vec![
            Alternative::Trade,
            Alternative::DoNotTrade,
            Alternative::LargerSize {
                factor: self.larger,
            },
            Alternative::SmallerSize {
                factor: self.smaller,
            },
        ];
        if let Some(option) = &self.venue {
            if option.venue != actual.venue {
                alternatives.push(Alternative::DifferentVenue {
                    venue: option.venue.clone(),
                });
            }
        }
        if let Some((region, proxy)) = &self.region {
            alternatives.push(Alternative::DifferentRegion {
                region: region.clone(),
                proxy: proxy.clone(),
            });
        }
        alternatives.push(Alternative::DelayedEntry { delay: self.delay });
        if let Some((hedge, ratio)) = &self.hedge {
            alternatives.push(Alternative::DifferentHedge {
                hedge: hedge.clone(),
                ratio: *ratio,
            });
        }
        alternatives.push(Alternative::NoHedge);
        alternatives
    }
}

/// What the platform actually did, in the terms the fill model needs.
///
/// Separate from [`crate::capture::Action`], which is the audit record. This is
/// the projection of that record into the four things a counterfactual has to
/// vary: instrument, direction, size and where it was done.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActualTrade {
    pub object_id: ObjectId,
    pub side: BookSide,
    /// Always positive; direction lives in `side`.
    pub quantity: Decimal,
    pub venue: VenueId,
    pub region: String,
    /// The hedge that was actually put on, if any.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hedge: Option<(ObjectId, Decimal)>,
    /// When the decision was made. Nothing later may be read to evaluate it.
    pub decided_at: Timestamp,
}

impl ActualTrade {
    pub fn new(
        object_id: ObjectId,
        side: BookSide,
        quantity: Decimal,
        venue: VenueId,
        region: impl Into<String>,
        decided_at: Timestamp,
    ) -> Result<Self> {
        if quantity <= Decimal::ZERO {
            return Err(Error::invalid(
                "a trade to counterfact needs a positive quantity; direction belongs in the side",
            ));
        }
        Ok(Self {
            object_id,
            side,
            quantity,
            venue,
            region: region.into(),
            hedge: None,
            decided_at,
        })
    }

    pub fn hedged_with(mut self, hedge: ObjectId, ratio: Decimal) -> Self {
        self.hedge = Some((hedge, ratio));
        self
    }

    /// +1 long, -1 short.
    const fn direction(&self) -> i64 {
        match self.side {
            BookSide::Bid => 1,
            BookSide::Ask => -1,
        }
    }
}

/// An alternative, fixed against what was knowable and not yet settled.
///
/// Every field was decided from a [`DecisionView`]. Settlement can read the
/// realised path but receives this by value, so there is no way for a later
/// price to change what the alternative was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualPlan {
    alternative: Alternative,
    object_id: ObjectId,
    side: BookSide,
    quantity: Decimal,
    hedge: Option<(ObjectId, Decimal)>,
    entry_delay: Duration,
    costs: CostModel,
    firm_quotes: bool,
    liquidity: Liquidity,
    hedge_liquidity: Option<Liquidity>,
    /// The price that had printed when the plan was made. Not used to settle —
    /// carried so a reader can see what the plan was fixed against.
    reference_price: Decimal,
    decided_at: Timestamp,
    /// The instant the view served. At or before `decided_at`, never after.
    fixed_as_of: Timestamp,
}

impl CounterfactualPlan {
    pub const fn alternative(&self) -> &Alternative {
        &self.alternative
    }

    pub const fn quantity(&self) -> Decimal {
        self.quantity
    }

    pub const fn reference_price(&self) -> Decimal {
        self.reference_price
    }

    /// The instant the plan could see up to. Never later than the decision.
    pub const fn fixed_as_of(&self) -> Timestamp {
        self.fixed_as_of
    }

    pub const fn costs(&self) -> CostModel {
        self.costs
    }
}

/// What became of an alternative.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fill", rename_all = "snake_case")]
pub enum SimulatedFill {
    /// It would have traded.
    Filled {
        quantity: Simulated<Decimal>,
        /// Prices that genuinely printed, so exact and untainted.
        entry_price: Decimal,
        exit_price: Decimal,
    },
    /// The alternative was to stand aside.
    NotTraded,
    /// The size was more of the day's volume than the impact law is calibrated
    /// for, so the twin refuses to price it rather than returning a number.
    ///
    /// The reason "we should have traded ten times the size" does not
    /// automatically win.
    Unfillable { reason: String },
    /// The liquidity was not there when the alternative would have reached it.
    Unfilled { reason: String },
}

impl SimulatedFill {
    pub const fn traded(&self) -> bool {
        matches!(self, Self::Filled { .. })
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Filled { .. } => "filled",
            Self::NotTraded => "not_traded",
            Self::Unfillable { .. } => "unfillable",
            Self::Unfilled { .. } => "unfilled",
        }
    }
}

/// What an alternative would have produced.
///
/// Every money figure is a [`Simulated<Decimal>`] and there is no accessor that
/// returns a bare one. The type is the guarantee; see [`crate::value`] for why
/// it is a type rather than a flag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulatedOutcome {
    alternative: Alternative,
    fill: SimulatedFill,
    pnl: Simulated<Decimal>,
    costs: Simulated<Decimal>,
    entry_at: Timestamp,
    exit_at: Timestamp,
    fixed_as_of: Timestamp,
}

impl SimulatedOutcome {
    pub const fn alternative(&self) -> &Alternative {
        &self.alternative
    }

    pub const fn fill(&self) -> &SimulatedFill {
        &self.fill
    }

    /// What it would have earned. Simulated, and unable to become anything else.
    pub const fn simulated_pnl(&self) -> Simulated<Decimal> {
        self.pnl
    }

    /// What it would have cost.
    pub const fn simulated_costs(&self) -> Simulated<Decimal> {
        self.costs
    }

    pub const fn entry_at(&self) -> Timestamp {
        self.entry_at
    }

    pub const fn exit_at(&self) -> Timestamp {
        self.exit_at
    }

    /// The instant the alternative was chosen against.
    pub const fn fixed_as_of(&self) -> Timestamp {
        self.fixed_as_of
    }
}

/// One comparison: what was done, what could have been, and the gap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counterfactual {
    pub actual_action: ActualTrade,
    pub actual_outcome: RealisedOutcome,
    pub counterfactual_action: Alternative,
    pub counterfactual_outcome: SimulatedOutcome,
    /// Counterfactual less actual. Simulated, because one of its two terms is:
    /// a difference containing a number that never happened is a number that
    /// never happened.
    pub difference: Simulated<Decimal>,
}

impl Counterfactual {
    /// Whether the alternative would have done better.
    pub fn favours_the_alternative(&self) -> bool {
        self.difference.is_positive()
    }
}

/// Every alternative to one decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualSet {
    pub decision_id: DecisionId,
    pub trace: TraceId,
    /// The decision instant. Every entry was planned as of here or earlier.
    pub decided_at: Timestamp,
    entries: Vec<Counterfactual>,
}

impl CounterfactualSet {
    pub fn entries(&self) -> &[Counterfactual] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for one kind of alternative, e.g. `do_not_trade`.
    pub fn by_kind(&self, kind: &str) -> Option<&Counterfactual> {
        self.entries
            .iter()
            .find(|entry| entry.counterfactual_action.kind() == kind)
    }

    /// The alternatives that would have beaten the action taken.
    pub fn regrets(&self) -> Vec<&Counterfactual> {
        self.entries
            .iter()
            .filter(|entry| entry.favours_the_alternative())
            .collect()
    }
}

/// Generates and prices the alternatives to a decision.
#[derive(Clone, Debug)]
pub struct CounterfactualEngine {
    seed: u64,
    menu: AlternativeMenu,
    horizon: Duration,
}

impl CounterfactualEngine {
    /// `horizon` is how long each alternative is held before it is marked.
    ///
    /// The seed is explicit because there is no ambient randomness anywhere in
    /// this platform: the same seed and the same market reproduce the same set,
    /// which is what makes a regret analysis something two people can argue
    /// about rather than something one of them has to re-run.
    pub fn new(seed: u64, menu: AlternativeMenu, horizon: Duration) -> Result<Self> {
        menu.validate()?;
        if horizon <= Duration::ZERO {
            return Err(Error::invalid(
                "a counterfactual held for no time earns nothing by construction",
            ));
        }
        if horizon <= menu.delay {
            return Err(Error::invalid(
                "the delayed-entry alternative would enter after the horizon closed",
            ));
        }
        Ok(Self {
            seed,
            menu,
            horizon,
        })
    }

    pub const fn seed(&self) -> u64 {
        self.seed
    }

    pub const fn menu(&self) -> &AlternativeMenu {
        &self.menu
    }

    pub const fn horizon(&self) -> Duration {
        self.horizon
    }

    /// Fix an alternative against what was knowable at the decision.
    ///
    /// Refuses a view that has seen past the decision. That refusal is the
    /// runtime half of the leakage guard — the compile-time half is that a
    /// [`DecisionView`] has no accessor taking a timestamp, so a caller cannot
    /// reach a later price even with a view in hand.
    pub fn plan(
        &self,
        actual: &ActualTrade,
        alternative: Alternative,
        view: &DecisionView<'_>,
    ) -> Result<CounterfactualPlan> {
        if view.as_of() > actual.decided_at {
            return Err(Error::guard(format!(
                "leak: evaluating an alternative to a decision made at {} against a market view as of {} would use prices that had not printed when the decision was taken",
                actual.decided_at,
                view.as_of()
            )));
        }

        let mut object_id = actual.object_id.clone();
        let mut quantity = actual.quantity;
        let mut hedge = actual.hedge.clone();
        let mut entry_delay = Duration::ZERO;
        let mut costs = view.costs();
        let mut firm_quotes = true;

        match &alternative {
            Alternative::Trade => {}
            Alternative::DoNotTrade => quantity = Decimal::ZERO,
            Alternative::LargerSize { factor } | Alternative::SmallerSize { factor } => {
                quantity = quantity.checked_mul(*factor).ok_or_else(|| {
                    Error::numeric("resizing the counterfactual overflowed the quantity")
                })?;
            }
            Alternative::DifferentVenue { venue } => {
                let option = self.menu.venue.as_ref().ok_or_else(|| {
                    Error::invalid(format!(
                        "the menu names no venue option, so {venue} cannot be priced"
                    ))
                })?;
                costs = option.costs;
                firm_quotes = option.firm_quotes;
            }
            Alternative::DifferentRegion { proxy, .. } => object_id = proxy.clone(),
            Alternative::DelayedEntry { delay } => entry_delay = *delay,
            Alternative::DifferentHedge {
                hedge: instrument,
                ratio,
            } => hedge = Some((instrument.clone(), *ratio)),
            Alternative::NoHedge => hedge = None,
        }

        let reference_price = view.last_close(&object_id).ok_or_else(|| {
            Error::not_found(format!(
                "no price for {object_id} had printed by {}, so this alternative cannot be planned from what was knowable",
                view.as_of()
            ))
        })?;
        let liquidity = view.liquidity(&object_id).ok_or_else(|| {
            Error::not_found(format!(
                "no traded volume for {object_id} by {}",
                view.as_of()
            ))
        })?;
        let hedge_liquidity = match &hedge {
            Some((instrument, _)) => Some(view.liquidity(instrument).ok_or_else(|| {
                Error::not_found(format!(
                    "no traded volume for the hedge {instrument} by {}",
                    view.as_of()
                ))
            })?),
            None => None,
        };

        Ok(CounterfactualPlan {
            alternative,
            object_id,
            side: actual.side,
            quantity,
            hedge,
            entry_delay,
            costs,
            firm_quotes,
            liquidity,
            hedge_liquidity,
            reference_price,
            decided_at: actual.decided_at,
            fixed_as_of: view.as_of(),
        })
    }

    /// Price a fixed plan against what the market then did.
    ///
    /// `stream` labels the random stream, so two runs over the same decisions
    /// draw the same numbers regardless of the order they are settled in.
    pub fn settle(
        &self,
        market: &TwinMarket,
        actual: &ActualTrade,
        plan: CounterfactualPlan,
        stream: &str,
    ) -> Result<SimulatedOutcome> {
        let entry_at = plan.decided_at.saturating_add(plan.entry_delay);
        let exit_at = plan.decided_at.saturating_add(self.horizon);
        if exit_at <= entry_at {
            return Err(Error::invalid(format!(
                "the {} alternative would exit at {exit_at} having entered at {entry_at}",
                plan.alternative.kind()
            )));
        }

        if !plan.alternative.trades() || plan.quantity <= Decimal::ZERO {
            return Ok(SimulatedOutcome {
                alternative: plan.alternative,
                fill: SimulatedFill::NotTraded,
                pnl: Simulated::ZERO,
                costs: Simulated::ZERO,
                entry_at,
                exit_at,
                fixed_as_of: plan.fixed_as_of,
            });
        }

        let entry_price = market.price_at(&plan.object_id, entry_at).ok_or_else(|| {
            Error::not_found(format!(
                "no fill price for {} at {entry_at}",
                plan.object_id
            ))
        })?;
        let exit_price = market.price_at(&plan.object_id, exit_at).ok_or_else(|| {
            Error::not_found(format!(
                "the horizon runs past the data: no price for {} at {exit_at}",
                plan.object_id
            ))
        })?;

        // The availability haircut. A quote that is not firm, or an entry
        // deferred while everyone else is looking at the same thing, may simply
        // not be there. Drawn from a stream derived from the decision id so the
        // result is reproducible, and applied only in the direction that
        // removes a fill: an alternative can never be improved by luck here.
        if !plan.firm_quotes || plan.entry_delay > Duration::ZERO {
            let mut root = Xoshiro256::seeded(self.seed);
            let mut rng = root.fork(stream);
            let participation = if plan.liquidity.daily_volume_f64 > 0.0 {
                plan.quantity.to_f64().abs() / plan.liquidity.daily_volume_f64
            } else {
                1.0
            };
            let availability = (1.0 - participation).clamp(0.0, 1.0);
            if !rng.bernoulli(availability) {
                return Ok(SimulatedOutcome {
                    alternative: plan.alternative,
                    fill: SimulatedFill::Unfilled {
                        reason: format!(
                            "the size was {:.1}% of the day's volume and the liquidity was not there when the alternative reached it",
                            participation * 100.0
                        ),
                    },
                    pnl: Simulated::ZERO,
                    costs: Simulated::ZERO,
                    entry_at,
                    exit_at,
                    fixed_as_of: plan.fixed_as_of,
                });
            }
        }

        let entry_cost = match plan.costs.cost_of(
            plan.quantity,
            entry_price,
            plan.liquidity.daily_volume_f64,
            plan.liquidity.daily_volatility_f64,
        ) {
            Ok(cost) => cost,
            Err(unfillable) => {
                return Ok(SimulatedOutcome {
                    alternative: plan.alternative,
                    fill: SimulatedFill::Unfillable {
                        reason: unfillable.describe(),
                    },
                    pnl: Simulated::ZERO,
                    costs: Simulated::ZERO,
                    entry_at,
                    exit_at,
                    fixed_as_of: plan.fixed_as_of,
                });
            }
        };
        let exit_cost = match plan.costs.cost_of(
            plan.quantity,
            exit_price,
            plan.liquidity.daily_volume_f64,
            plan.liquidity.daily_volatility_f64,
        ) {
            Ok(cost) => cost,
            Err(unfillable) => {
                return Ok(SimulatedOutcome {
                    alternative: plan.alternative,
                    fill: SimulatedFill::Unfillable {
                        reason: unfillable.describe(),
                    },
                    pnl: Simulated::ZERO,
                    costs: Simulated::ZERO,
                    entry_at,
                    exit_at,
                    fixed_as_of: plan.fixed_as_of,
                });
            }
        };

        let direction = Decimal::from_int(actual.direction());
        let gross = (exit_price - entry_price)
            .checked_mul(plan.quantity)
            .and_then(|value| value.checked_mul(direction))
            .ok_or_else(|| Error::numeric("the counterfactual's gross profit overflowed"))?;

        let mut costs = to_decimal(entry_cost.total() + exit_cost.total())?;
        let mut hedge_gross = Decimal::ZERO;
        if let (Some((instrument, ratio)), Some(liquidity)) = (&plan.hedge, plan.hedge_liquidity) {
            let hedge_quantity = plan
                .quantity
                .checked_mul(*ratio)
                .ok_or_else(|| Error::numeric("the counterfactual's hedge size overflowed"))?;
            let hedge_entry = market.price_at(instrument, entry_at).ok_or_else(|| {
                Error::not_found(format!(
                    "no fill price for the hedge {instrument} at {entry_at}"
                ))
            })?;
            let hedge_exit = market.price_at(instrument, exit_at).ok_or_else(|| {
                Error::not_found(format!(
                    "no fill price for the hedge {instrument} at {exit_at}"
                ))
            })?;
            // A hedge is the opposite direction by definition; a "hedge" on the
            // same side is just more of the position and would flatter the
            // alternative that carries it.
            hedge_gross = (hedge_exit - hedge_entry)
                .checked_mul(hedge_quantity)
                .and_then(|value| value.checked_mul(-direction))
                .ok_or_else(|| Error::numeric("the counterfactual's hedge profit overflowed"))?;
            for price in [hedge_entry, hedge_exit] {
                if let Ok(cost) = plan.costs.cost_of(
                    hedge_quantity,
                    price,
                    liquidity.daily_volume_f64,
                    liquidity.daily_volatility_f64,
                ) {
                    costs += to_decimal(cost.total())?;
                }
            }
        }

        Ok(SimulatedOutcome {
            alternative: plan.alternative,
            fill: SimulatedFill::Filled {
                quantity: Simulated::of(plan.quantity),
                entry_price,
                exit_price,
            },
            pnl: Simulated::of(gross + hedge_gross - costs),
            costs: Simulated::of(costs),
            entry_at,
            exit_at,
            fixed_as_of: plan.fixed_as_of,
        })
    }

    /// Generate and price every alternative to one decision.
    ///
    /// The two phases are visible in the body: the view lives inside a block
    /// and every plan is fixed before it is dropped, and only then does
    /// settlement get to see the realised path. Nothing settlement reads can
    /// reach back into a plan.
    pub fn evaluate(
        &self,
        market: &mut TwinMarket,
        decision: &Decision,
        actual: &ActualTrade,
        actual_outcome: &RealisedOutcome,
    ) -> Result<CounterfactualSet> {
        if actual.decided_at != decision.at {
            return Err(Error::invalid(format!(
                "the trade to counterfact is dated {} and the decision {}; a counterfactual evaluated as of the wrong instant is not a counterfactual",
                actual.decided_at, decision.at
            )));
        }

        let plans = {
            let view = market.view_at(decision.at)?;
            self.menu
                .alternatives_to(actual)
                .into_iter()
                .map(|alternative| self.plan(actual, alternative, &view))
                .collect::<Result<Vec<_>>>()?
        };

        let mut entries = Vec::with_capacity(plans.len());
        for plan in plans {
            let stream = format!("{}|{}", decision.decision_id, plan.alternative.kind());
            let alternative = plan.alternative.clone();
            let outcome = self.settle(market, actual, plan, &stream)?;
            let difference = outcome.simulated_pnl() - Simulated::of(actual_outcome.realised_pnl());
            entries.push(Counterfactual {
                actual_action: actual.clone(),
                actual_outcome: *actual_outcome,
                counterfactual_action: alternative,
                counterfactual_outcome: outcome,
                difference,
            });
        }

        Ok(CounterfactualSet {
            decision_id: decision.decision_id.clone(),
            trace: decision.trace.clone(),
            decided_at: decision.at,
            entries,
        })
    }
}

/// Cross an `f64` cost into the exact lane, refusing rather than rounding away
/// a value the fixed-point representation cannot hold.
///
/// The crossing exists because the fill model this crate composes computes in
/// `f64`. Re-deriving its arithmetic in [`Decimal`] would be the second fill
/// model, so the conversion happens here, once, where it can be seen.
fn to_decimal(value: f64) -> Result<Decimal> {
    Decimal::from_f64(value)
        .ok_or_else(|| Error::numeric(format!("a simulated cost of {value} is not representable")))
}
