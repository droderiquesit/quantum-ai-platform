//! The Hedge Brain's search: the cheapest executable hedge that keeps the
//! book above its survival floor in every world it is asked to protect
//! (blueprint §14.1; R12-RISK-4105..4109).
//!
//! [`crate::hedge`] sizes one declared instrument by one declared beta. That
//! is auditable and it is the right tool for a standing policy — "keep the
//! sector inside its cap" — and it is the wrong tool for survival, for the
//! three reasons §14.1 gives:
//!
//! * **One beta protects one future.** The ratio that offsets a fall in a
//!   calm world over- or under-hedges in a crash, where the hedge's own
//!   convexity and the book's correlation both move. Here a candidate is
//!   scored world by world: each [`ForecastWorld`] carries the book's loss in
//!   that world and each candidate states its own underlying and volatility
//!   move there, so the hedge that survives the crash and the hedge that
//!   survives the squeeze can be different instruments (R12-RISK-4108).
//! * **A declared instrument is rarely the cheapest protection.** The search
//!   runs over every candidate and every pair of candidates and returns the
//!   least-cost portfolio that meets the floor (R12-RISK-4106), across
//!   options, futures, FX, rates, commodities, volatility, correlated assets,
//!   event contracts and cash or collateral actions, each sized through its
//!   own contract multiplier and Greeks (R12-RISK-4107).
//! * **Correlation is not a hedge's only property.** Every plan records its
//!   cost, basis risk, convexity, liquidity decay, counterparty risk and
//!   whether it can be unwound; a candidate that cannot be unwound inside the
//!   horizon at a bounded share of its stressed volume is **refused**, not
//!   sized smaller (R12-RISK-4109).
//!
//! # What this module is not
//!
//! **It is not an enforcement path.** It returns a [`HedgeSearch`] — a plan or
//! the reasons there is none — and nothing here holds an order type, reaches a
//! broker, or vetoes anything. A hedge reaches a market as every order does,
//! through proposal, approval and the pre-trade Risk Gate, where the capital
//! it consumes is held to the survival ceiling the gate already reads
//! (R12-CAPITAL-7204, R12-RISK-7205). The search's budget is the hedge
//! capacity the survival waterfall withheld, so a plan the search returns is
//! one the waterfall has already funded.
//!
//! **It does not estimate.** Every candidate's moves, Greeks, costs and
//! liquidity arrive from the caller, which reads them from the platform's own
//! records; this module only does the arithmetic and refuses figures that are
//! not numbers.
//!
//! Statistics are `f64` (world P&L, Greeks, volatilities); money that leaves
//! this module — quantities, cost, capital used — is `Decimal`, and the
//! crossing is made once, in [`HedgeCandidate::capital_for`] and
//! [`Leg::measures`], where it is commented.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The classes §14.1 names. A fixed set, so a label or journal field built
/// from it is bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentClass {
    Option,
    Future,
    Fx,
    Rate,
    Commodity,
    Volatility,
    CorrelatedAsset,
    EventContract,
    /// Raising cash or posting collateral: a linear, unlevered action whose
    /// "move" is the move of the exposure it reduces.
    CashCollateral,
}

impl InstrumentClass {
    pub const ALL: [Self; 9] = [
        Self::Option,
        Self::Future,
        Self::Fx,
        Self::Rate,
        Self::Commodity,
        Self::Volatility,
        Self::CorrelatedAsset,
        Self::EventContract,
        Self::CashCollateral,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Option => "option",
            Self::Future => "future",
            Self::Fx => "fx",
            Self::Rate => "rate",
            Self::Commodity => "commodity",
            Self::Volatility => "volatility",
            Self::CorrelatedAsset => "correlated_asset",
            Self::EventContract => "event_contract",
            Self::CashCollateral => "cash_collateral",
        }
    }

    /// Whether buying it pays its whole value up front (premium), rather than
    /// posting a margin fraction against it.
    const fn pays_premium(self) -> bool {
        matches!(
            self,
            Self::Option
                | Self::Volatility
                | Self::EventContract
                | Self::CorrelatedAsset
                | Self::CashCollateral
        )
    }

    /// Whether the class carries convexity at all. A future has no gamma;
    /// stating one is a configuration error, not a refinement.
    const fn convex(self) -> bool {
        matches!(self, Self::Option | Self::Volatility | Self::EventContract)
    }
}

/// The initial margin a linear derivative consumes from hedge capacity, as a
/// fraction of notional. A standing assumption, named so it can be argued
/// with, in the register of `qip_capital::margin::MarginModel::default`.
pub const LINEAR_INITIAL_MARGIN: f64 = 0.10;

/// The share of normal daily volume assumed available in the stress a hedge
/// is bought for. Half: the hedge is unwound in the world it protects, which
/// is not a world with normal depth.
pub const STRESSED_VOLUME_SHARE: f64 = 0.5;

/// The one-sided 99% normal quantile basis risk is reported at.
pub const BASIS_QUANTILE: f64 = 2.326;

/// The most quantities tried for the first leg of a pair. The second leg is
/// solved exactly, so this bounds the search at `candidates² × STEPS`.
pub const PAIR_STEPS: u64 = 256;

/// Per-contract sensitivities.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Greeks {
    /// Change in contract value per unit change in the underlying price, per
    /// unit of multiplier. One for a linear instrument; in `[-1, 1]` for an
    /// option.
    pub delta: f64,
    /// Change in delta per unit change in the underlying price.
    pub gamma: f64,
    /// Change in contract value per volatility point, per unit of multiplier.
    pub vega: f64,
}

impl Greeks {
    pub const fn linear() -> Self {
        Self {
            delta: 1.0,
            gamma: 0.0,
            vega: 0.0,
        }
    }
}

/// What a candidate's underlying does in one world.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldMove {
    /// Fractional change in the underlying's price.
    pub underlying: f64,
    /// Change in implied volatility, in volatility points.
    pub volatility: f64,
}

/// One instrument the search may hedge with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeCandidate {
    pub object_id: ObjectId,
    pub symbol: String,
    pub venue: String,
    pub class: InstrumentClass,
    /// The underlying's price the Greeks are quoted against.
    pub price: Decimal,
    pub contract_multiplier: Decimal,
    pub lot_size: Decimal,
    pub greeks: Greeks,
    /// Carry — financing, theta, roll — per day, in basis points of notional.
    pub carry_bps_per_day: f64,
    /// The quoted spread, in basis points of notional, paid once to put the
    /// hedge on and once to take it off.
    pub spread_bps: f64,
    /// Normal daily volume, in units of the instrument.
    pub average_daily_volume: Decimal,
    /// Expected loss per unit of notional if the counterparty fails: the
    /// probability of failure times the loss given it. Zero for a cleared
    /// contract with a solvent clearing house is a statement, not a default.
    pub counterparty_loss_rate: f64,
    /// Daily volatility of the gap between this instrument and the exposure
    /// it stands in for. Zero only for the exposure itself.
    pub basis_volatility: f64,
    /// The underlying's move in each world. A world this map does not name is
    /// a world in which the candidate does nothing — never one it is assumed
    /// to protect.
    pub moves: BTreeMap<String, WorldMove>,
}

impl HedgeCandidate {
    /// Refuses a candidate whose figures are not numbers, whose class
    /// contradicts its Greeks, or whose mechanics are not positive.
    pub fn validate(&self) -> Result<()> {
        let who = &self.symbol;
        if who.trim().is_empty() || self.venue.trim().is_empty() {
            return Err(Error::invalid(
                "a hedge candidate needs a symbol and a venue; a plan in an unnamed instrument \
                 cannot be approved by anyone",
            ));
        }
        for (name, value) in [
            ("price", self.price),
            ("contract multiplier", self.contract_multiplier),
            ("lot size", self.lot_size),
            ("average daily volume", self.average_daily_volume),
        ] {
            if !value.is_positive() {
                return Err(Error::invalid(format!(
                    "hedge candidate {who} has a non-positive {name} ({value})"
                )));
            }
        }
        let g = self.greeks;
        for (name, value) in [
            ("delta", g.delta),
            ("gamma", g.gamma),
            ("vega", g.vega),
            ("carry", self.carry_bps_per_day),
            ("spread", self.spread_bps),
            ("counterparty loss rate", self.counterparty_loss_rate),
            ("basis volatility", self.basis_volatility),
        ] {
            if !value.is_finite() {
                return Err(Error::invalid(format!(
                    "hedge candidate {who} states a {name} that is not a number"
                )));
            }
        }
        if self.spread_bps < 0.0
            || self.basis_volatility < 0.0
            || !(0.0..=1.0).contains(&self.counterparty_loss_rate)
        {
            return Err(Error::invalid(format!(
                "hedge candidate {who} states a negative spread or basis volatility, or a \
                 counterparty loss rate outside [0, 1]"
            )));
        }
        if !self.class.convex() && (g.gamma != 0.0 || g.vega != 0.0) {
            return Err(Error::invalid(format!(
                "hedge candidate {who} is a {} and states gamma {} and vega {}; a linear \
                 instrument has neither, and a stated one would size a convex hedge out of a \
                 straight line",
                self.class.as_str(),
                g.gamma,
                g.vega
            )));
        }
        if self.class == InstrumentClass::Option && !(-1.0..=1.0).contains(&g.delta) {
            return Err(Error::invalid(format!(
                "option {who} states a delta of {}, outside [-1, 1]",
                g.delta
            )));
        }
        for (world, step) in &self.moves {
            if !step.underlying.is_finite() || !step.volatility.is_finite() {
                return Err(Error::invalid(format!(
                    "hedge candidate {who} states a move in {world} that is not a number"
                )));
            }
        }
        Ok(())
    }

    /// Profit of one unit (one contract) in `world`, through the Greeks: the
    /// second-order Taylor expansion in the underlying plus the vega term,
    /// times the multiplier. Zero in a world the candidate states no move for.
    pub fn unit_pnl(&self, world: &str) -> f64 {
        let Some(step) = self.moves.get(world) else {
            return 0.0;
        };
        let price = self.price.to_f64();
        let change = price * step.underlying;
        let g = self.greeks;
        self.contract_multiplier.to_f64()
            * (g.delta * change + 0.5 * g.gamma * change * change + g.vega * step.volatility)
    }

    /// Notional of `units` contracts: units × price × multiplier.
    fn notional(&self, units: f64) -> f64 {
        units.abs() * self.price.to_f64() * self.contract_multiplier.to_f64()
    }

    /// Hedge capacity consumed by holding `units`: the premium for a class
    /// that pays one, the initial margin for a linear derivative.
    ///
    /// The statistics-to-money crossing for capital: the notional is an `f64`
    /// product of the Greeks' scale and is carried into `Decimal` here.
    pub fn capital_for(&self, units: f64) -> Result<Decimal> {
        let notional = self.notional(units);
        let used = if self.class.pays_premium() {
            notional
        } else {
            notional * LINEAR_INITIAL_MARGIN
        };
        Decimal::from_f64(used).ok_or_else(|| {
            Error::numeric(format!(
                "the capital {} units of {} consume cannot be carried as money",
                units, self.symbol
            ))
        })
    }

    /// The most units that can be unwound within `days` at `share` of the
    /// stressed volume. A hedge that cannot leave inside its horizon is a
    /// second position, not a hedge.
    fn unwindable_units(&self, share: f64, days: f64) -> f64 {
        self.average_daily_volume.to_f64() * STRESSED_VOLUME_SHARE * share * days
    }
}

/// One world of the forecast distribution and what the book makes or loses
/// in it, unhedged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForecastWorld {
    pub name: String,
    /// Weight in the distribution. Non-negative; the weights need not sum to
    /// one and are normalised by their total.
    pub probability: f64,
    /// The book's profit in the world, in money: negative is a loss.
    pub book_pnl: f64,
}

/// What the search is asked to achieve, and within what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeObjective {
    /// The largest loss the book may take in any world, in money. Every world
    /// whose unhedged loss exceeds it is a world the plan must protect, and
    /// every other world must not be pushed past it by the hedge.
    pub survival_floor: f64,
    /// The hedge capacity the survival waterfall withheld: the most capital
    /// the plan may consume.
    pub budget: Decimal,
    /// The largest share of stressed daily volume the plan may need to exit.
    pub max_volume_share: f64,
    /// The longest an unwind may take, in days.
    pub max_unwind_days: f64,
    /// How long the hedge is expected to be held, for carry.
    pub holding_days: f64,
}

impl HedgeObjective {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("survival floor", self.survival_floor),
            ("volume share", self.max_volume_share),
            ("unwind horizon", self.max_unwind_days),
            ("holding period", self.holding_days),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(Error::invalid(format!(
                    "a hedge objective's {name} of {value} is not a positive number"
                )));
            }
        }
        if self.max_volume_share > 1.0 {
            return Err(Error::invalid(format!(
                "a volume share of {} is more than the market trades",
                self.max_volume_share
            )));
        }
        if self.budget.is_negative() {
            return Err(Error::invalid(format!(
                "a hedge budget of {} is less than nothing",
                self.budget
            )));
        }
        Ok(())
    }
}

/// Which way a leg trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegSide {
    Buy,
    Sell,
}

/// The six measures §14.1 requires of every hedge (R12-RISK-4109), for one
/// leg or summed over a plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HedgeMeasures {
    /// Carry over the holding period plus the spread in and out, in money.
    pub cost: Decimal,
    /// The expected P&L the hedge gives up across the distribution, in money:
    /// positive is edge surrendered. Reported beside cost, not inside it,
    /// because a hedge with positive expectation is not free and must not
    /// read as cheaper than nothing.
    pub edge_given_up: f64,
    /// Notional × basis volatility × √holding × the 99% quantile: what the
    /// gap between hedge and exposure can cost over the holding period.
    pub basis_risk: f64,
    /// Dollar gamma: signed units × multiplier × gamma × price².
    pub convexity: f64,
    /// Days to exit at the allowed share of *stressed* volume.
    pub liquidity_decay_days: f64,
    /// Notional × counterparty loss rate.
    pub counterparty_risk: f64,
    /// Whether every leg can be unwound inside the horizon.
    pub unwind_feasible: bool,
}

/// One instrument in a plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub object_id: ObjectId,
    pub symbol: String,
    pub venue: String,
    pub class: InstrumentClass,
    pub side: LegSide,
    /// Contracts, rounded to the lot, always positive; `side` carries the sign.
    pub quantity: Decimal,
    pub capital: Decimal,
    pub measures: HedgeMeasures,
}

impl Leg {
    /// The six measures for `units` (signed) of `candidate`.
    ///
    /// The cost crossing: carry and spread are `f64` products of basis points
    /// and notional and are carried into `Decimal` here, once.
    fn measures(
        candidate: &HedgeCandidate,
        units: f64,
        objective: &HedgeObjective,
        worlds: &[ForecastWorld],
        total_weight: f64,
    ) -> Result<HedgeMeasures> {
        let notional = candidate.notional(units);
        let carry = notional * candidate.carry_bps_per_day / 10_000.0 * objective.holding_days;
        let spread = notional * candidate.spread_bps / 10_000.0 * 2.0;
        let cost = Decimal::from_f64(carry + spread)
            .filter(|cost| !cost.is_negative())
            .ok_or_else(|| {
                Error::numeric(format!(
                    "the cost of holding {units} units of {} cannot be carried as money",
                    candidate.symbol
                ))
            })?;
        let expected = if total_weight > 0.0 {
            worlds
                .iter()
                .map(|world| world.probability * units * candidate.unit_pnl(&world.name))
                .sum::<f64>()
                / total_weight
        } else {
            0.0
        };
        let price = candidate.price.to_f64();
        let capacity = candidate.unwindable_units(objective.max_volume_share, 1.0);
        let liquidity_decay_days = if capacity > 0.0 {
            units.abs() / capacity
        } else {
            f64::INFINITY
        };
        Ok(HedgeMeasures {
            cost,
            edge_given_up: -expected,
            basis_risk: notional
                * candidate.basis_volatility
                * objective.holding_days.sqrt()
                * BASIS_QUANTILE,
            convexity: units
                * candidate.contract_multiplier.to_f64()
                * candidate.greeks.gamma
                * price
                * price,
            liquidity_decay_days,
            counterparty_risk: notional * candidate.counterparty_loss_rate,
            unwind_feasible: liquidity_decay_days <= objective.max_unwind_days,
        })
    }
}

/// The plan the search chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeSolution {
    pub legs: Vec<Leg>,
    /// The plan's measures summed over its legs; `liquidity_decay_days` is the
    /// slowest leg's and `unwind_feasible` holds only if every leg's does.
    pub measures: HedgeMeasures,
    /// Capital the plan consumes from hedge capacity.
    pub capital: Decimal,
    /// Worlds whose unhedged loss exceeded the floor.
    pub protected: Vec<String>,
    /// The book's P&L in every world with the plan on.
    pub hedged_pnl: BTreeMap<String, f64>,
}

/// Why a candidate, or the whole search, produced no plan. A fixed set of
/// causes, so a label built from it is bounded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum SearchRefusal {
    /// The candidate moves the wrong way, or not at all, in a world it must
    /// protect.
    DoesNotProtect { symbol: String, world: String },
    /// The size that protects pushes another world past the floor.
    OverHedges { symbol: String },
    /// The size that protects cannot be unwound inside the horizon.
    UnwindInfeasible {
        symbol: String,
        days: f64,
        horizon: f64,
    },
    /// The size that protects consumes more than the hedge capacity.
    OverBudget {
        symbol: String,
        capital: Decimal,
        budget: Decimal,
    },
    /// The candidate's figures were refused by [`HedgeCandidate::validate`].
    Invalid { symbol: String, message: String },
}

impl SearchRefusal {
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::DoesNotProtect { .. } => "does_not_protect",
            Self::OverHedges { .. } => "over_hedges",
            Self::UnwindInfeasible { .. } => "unwind_infeasible",
            Self::OverBudget { .. } => "over_budget",
            Self::Invalid { .. } => "invalid",
        }
    }
}

/// What the search found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HedgeSearch {
    /// `None` when no world needs protecting, or when nothing protects them
    /// all within the objective. The two are told apart by `protected`.
    pub plan: Option<HedgeSolution>,
    /// Worlds whose unhedged loss exceeded the floor. Empty means the book
    /// already survives every world, and no hedge is the right answer.
    pub protected: Vec<String>,
    /// Every single-instrument refusal, in candidate order.
    pub refusals: Vec<SearchRefusal>,
    /// How many single and pair portfolios were evaluated.
    pub evaluated: usize,
}

impl HedgeSearch {
    /// One line for the cycle report.
    pub fn summary(&self) -> String {
        match (&self.plan, self.protected.is_empty()) {
            (_, true) => {
                "the book survives every forecast world unhedged; no hedge is planned".to_string()
            }
            (Some(plan), false) => format!(
                "{} world(s) need protection ({}); the cheapest plan is {} for {} cost and {} \
                 of hedge capacity, out of {} portfolio(s) evaluated",
                self.protected.len(),
                self.protected.join(", "),
                plan.legs
                    .iter()
                    .map(|leg| format!(
                        "{} {} {}",
                        match leg.side {
                            LegSide::Buy => "buy",
                            LegSide::Sell => "sell",
                        },
                        leg.quantity,
                        leg.symbol
                    ))
                    .collect::<Vec<_>>()
                    .join(" + "),
                plan.measures.cost,
                plan.capital,
                self.evaluated
            ),
            (None, false) => format!(
                "{} world(s) need protection ({}) and no candidate or pair protects them all \
                 within the hedge capacity and unwind horizon; {} refusal(s)",
                self.protected.len(),
                self.protected.join(", "),
                self.refusals.len()
            ),
        }
    }
}

/// The units of `candidate`, in whole lots and signed, that keep every world
/// at or above the floor given `base` (the book's P&L per world before this
/// leg), or the reason there are none. Exact: each world is one linear
/// inequality in the lot count, so the feasible set is an interval and its
/// smallest member is the cheapest.
fn solve_single(
    candidate: &HedgeCandidate,
    base: &BTreeMap<String, f64>,
    objective: &HedgeObjective,
) -> std::result::Result<f64, SearchRefusal> {
    let lot = candidate.lot_size.to_f64();
    let floor = -objective.survival_floor;
    // The world that must be protected most decides the side.
    let worst = base
        .iter()
        .filter(|(_, pnl)| **pnl < floor)
        .min_by(|a, b| a.1.total_cmp(b.1));
    let Some((worst_world, _)) = worst else {
        return Ok(0.0);
    };
    let unit = candidate.unit_pnl(worst_world);
    if unit == 0.0 {
        return Err(SearchRefusal::DoesNotProtect {
            symbol: candidate.symbol.clone(),
            world: worst_world.clone(),
        });
    }
    let sign = unit.signum();
    let mut lower = 0.0_f64;
    let mut upper = f64::INFINITY;
    for (world, pnl) in base {
        // per-lot P&L in this world on the chosen side
        let per_lot = sign * lot * candidate.unit_pnl(world);
        let needed = floor - pnl;
        if needed > 0.0 {
            if per_lot <= 0.0 {
                return Err(SearchRefusal::DoesNotProtect {
                    symbol: candidate.symbol.clone(),
                    world: world.clone(),
                });
            }
            lower = lower.max((needed / per_lot).ceil());
        } else if per_lot < 0.0 {
            upper = upper.min((needed / per_lot).floor());
        }
    }
    if lower > upper {
        return Err(SearchRefusal::OverHedges {
            symbol: candidate.symbol.clone(),
        });
    }
    Ok(sign * lower * lot)
}

fn apply(
    base: &BTreeMap<String, f64>,
    candidate: &HedgeCandidate,
    units: f64,
) -> BTreeMap<String, f64> {
    base.iter()
        .map(|(world, pnl)| (world.clone(), pnl + units * candidate.unit_pnl(world)))
        .collect()
}

/// Search `candidates` for the cheapest plan of one or two legs that keeps the
/// book above `objective.survival_floor` in every world of `worlds`.
///
/// Refuses a malformed objective and a distribution with a negative or
/// non-finite weight or P&L; a candidate with malformed figures is a
/// per-candidate refusal, so one bad record does not stop the others being
/// searched. Ties on cost break on capital and then on the legs' object ids,
/// so the same inputs choose the same plan on every replay.
pub fn search(
    worlds: &[ForecastWorld],
    candidates: &[HedgeCandidate],
    objective: &HedgeObjective,
) -> Result<HedgeSearch> {
    objective.validate()?;
    let mut names = BTreeSet::new();
    for world in worlds {
        if !world.probability.is_finite() || world.probability < 0.0 || !world.book_pnl.is_finite()
        {
            return Err(Error::invalid(format!(
                "forecast world {} carries a weight of {} and a book P&L of {}; a distribution \
                 the search cannot read is not one it can protect",
                world.name, world.probability, world.book_pnl
            )));
        }
        if !names.insert(world.name.clone()) {
            return Err(Error::invalid(format!(
                "forecast world {} appears twice; two P&Ls for one future is two claims about \
                 one fact",
                world.name
            )));
        }
    }
    let total_weight: f64 = worlds.iter().map(|world| world.probability).sum();
    let base: BTreeMap<String, f64> = worlds
        .iter()
        .map(|world| (world.name.clone(), world.book_pnl))
        .collect();
    let protected: Vec<String> = worlds
        .iter()
        .filter(|world| world.book_pnl < -objective.survival_floor)
        .map(|world| world.name.clone())
        .collect();
    let mut out = HedgeSearch {
        plan: None,
        protected: protected.clone(),
        refusals: Vec::new(),
        evaluated: 0,
    };
    if protected.is_empty() {
        return Ok(out);
    }

    let mut valid: Vec<&HedgeCandidate> = Vec::new();
    for candidate in candidates {
        match candidate.validate() {
            Ok(()) => valid.push(candidate),
            Err(error) => out.refusals.push(SearchRefusal::Invalid {
                symbol: candidate.symbol.clone(),
                message: error.message().to_string(),
            }),
        }
    }

    // A portfolio is a list of (candidate, signed units); `consider` scores
    // it and keeps it if it is the cheapest admissible so far.
    let mut best: Option<HedgeSolution> = None;
    let mut consider = |legs: &[(&HedgeCandidate, f64)],
                        refusals: &mut Vec<SearchRefusal>,
                        record: bool|
     -> Result<()> {
        let mut built = Vec::new();
        let mut capital = Decimal::ZERO;
        let mut hedged = base.clone();
        for (candidate, units) in legs {
            if *units == 0.0 {
                continue;
            }
            let measures = Leg::measures(candidate, *units, objective, worlds, total_weight)?;
            if !measures.unwind_feasible {
                if record {
                    refusals.push(SearchRefusal::UnwindInfeasible {
                        symbol: candidate.symbol.clone(),
                        days: measures.liquidity_decay_days,
                        horizon: objective.max_unwind_days,
                    });
                }
                return Ok(());
            }
            let used = candidate.capital_for(*units)?;
            capital = capital
                .checked_add(used)
                .ok_or_else(|| Error::numeric("hedge capital overflows".to_string()))?;
            hedged = apply(&hedged, candidate, *units);
            // The quantity crossing: whole lots, computed in `f64` by the
            // exact solve, carried into `Decimal` here.
            let quantity = Decimal::from_f64(units.abs()).ok_or_else(|| {
                Error::numeric(format!("{units} units of {} overflow", candidate.symbol))
            })?;
            built.push(Leg {
                object_id: candidate.object_id.clone(),
                symbol: candidate.symbol.clone(),
                venue: candidate.venue.clone(),
                class: candidate.class,
                side: if *units > 0.0 {
                    LegSide::Buy
                } else {
                    LegSide::Sell
                },
                quantity,
                capital: used,
                measures,
            });
        }
        if built.is_empty() {
            return Ok(());
        }
        if capital > objective.budget {
            if record && let Some((candidate, _)) = legs.first() {
                refusals.push(SearchRefusal::OverBudget {
                    symbol: candidate.symbol.clone(),
                    capital,
                    budget: objective.budget,
                });
            }
            return Ok(());
        }
        // Every world at or above the floor with the plan on — the property,
        // checked on the result rather than trusted to the solve.
        if hedged.values().any(|pnl| *pnl < -objective.survival_floor) {
            return Ok(());
        }
        let mut measures = HedgeMeasures {
            unwind_feasible: true,
            ..HedgeMeasures::default()
        };
        for leg in &built {
            measures.cost = measures
                .cost
                .checked_add(leg.measures.cost)
                .ok_or_else(|| Error::numeric("hedge cost overflows".to_string()))?;
            measures.edge_given_up += leg.measures.edge_given_up;
            measures.basis_risk += leg.measures.basis_risk;
            measures.convexity += leg.measures.convexity;
            measures.liquidity_decay_days = measures
                .liquidity_decay_days
                .max(leg.measures.liquidity_decay_days);
            measures.counterparty_risk += leg.measures.counterparty_risk;
            measures.unwind_feasible &= leg.measures.unwind_feasible;
        }
        let solution = HedgeSolution {
            legs: built,
            measures,
            capital,
            protected: protected.clone(),
            hedged_pnl: hedged,
        };
        let key = |plan: &HedgeSolution| {
            (
                plan.measures.cost,
                plan.capital,
                plan.legs
                    .iter()
                    .map(|leg| leg.object_id.as_str().to_string())
                    .collect::<Vec<_>>(),
            )
        };
        let replace = best.as_ref().is_none_or(|held| key(&solution) < key(held));
        if replace {
            best = Some(solution);
        }
        Ok(())
    };

    // Singles, solved exactly.
    for candidate in &valid {
        out.evaluated += 1;
        match solve_single(candidate, &base, objective) {
            Ok(units) => consider(&[(candidate, units)], &mut out.refusals, true)?,
            Err(refusal) => out.refusals.push(refusal),
        }
    }
    // Pairs: the first leg stepped over what it can unwind, the second solved
    // exactly against what the first leaves.
    for (i, first) in valid.iter().enumerate() {
        for second in valid.iter().skip(i + 1) {
            for (lead, follow) in [(first, second), (second, first)] {
                let lot = lead.lot_size.to_f64();
                let most = (lead
                    .unwindable_units(objective.max_volume_share, objective.max_unwind_days)
                    / lot)
                    .floor();
                if !(most.is_finite() && most >= 1.0) {
                    continue;
                }
                let stride = (most / PAIR_STEPS as f64).ceil().max(1.0);
                // The lead's side is the one that helps its own worst world.
                let side = protected
                    .iter()
                    .map(|world| lead.unit_pnl(world))
                    .find(|pnl| *pnl != 0.0)
                    .map_or(0.0, f64::signum);
                if side == 0.0 {
                    continue;
                }
                let mut lots = stride;
                while lots <= most {
                    out.evaluated += 1;
                    let units = side * lots * lot;
                    let residual = apply(&base, lead, units);
                    if let Ok(rest) = solve_single(follow, &residual, objective) {
                        consider(&[(lead, units), (follow, rest)], &mut Vec::new(), false)?;
                    }
                    lots += stride;
                }
            }
        }
    }
    out.plan = best;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn worlds() -> Vec<ForecastWorld> {
        vec![
            ForecastWorld {
                name: "calm".into(),
                probability: 0.8,
                book_pnl: 20_000.0,
            },
            ForecastWorld {
                name: "crash".into(),
                probability: 0.15,
                book_pnl: -300_000.0,
            },
            ForecastWorld {
                name: "squeeze".into(),
                probability: 0.05,
                book_pnl: -50_000.0,
            },
        ]
    }

    fn objective(budget: Decimal) -> HedgeObjective {
        HedgeObjective {
            survival_floor: 100_000.0,
            budget,
            max_volume_share: 0.10,
            max_unwind_days: 5.0,
            holding_days: 20.0,
        }
    }

    fn future(symbol: &str, crash: f64, carry: f64) -> HedgeCandidate {
        HedgeCandidate {
            object_id: ObjectId::from_string(format!("obj-{symbol}")),
            symbol: symbol.into(),
            venue: "SIM".into(),
            class: InstrumentClass::Future,
            price: dec!("100"),
            contract_multiplier: dec!("50"),
            lot_size: dec!("1"),
            greeks: Greeks::linear(),
            carry_bps_per_day: carry,
            spread_bps: 2.0,
            average_daily_volume: dec!("100000"),
            counterparty_loss_rate: 0.0,
            basis_volatility: 0.005,
            moves: BTreeMap::from([
                (
                    "calm".to_string(),
                    WorldMove {
                        underlying: 0.01,
                        volatility: 0.0,
                    },
                ),
                (
                    "crash".to_string(),
                    WorldMove {
                        underlying: crash,
                        volatility: 0.0,
                    },
                ),
            ]),
        }
    }

    /// Mutation: pick the first feasible candidate instead of the cheapest
    /// (replace `key(&solution) < key(held)` with `false`) and this fails.
    #[test]
    fn of_several_instruments_that_protect_the_crash_the_search_returns_the_cheapest() {
        let dear = future("DEAR", -0.20, 5.0);
        let cheap = future("CHEAP", -0.20, 0.5);
        let result = search(
            &worlds(),
            &[dear.clone(), cheap.clone()],
            &objective(dec!("10000000")),
        )
        .unwrap();
        assert_eq!(result.protected, vec!["crash".to_string()], "premise");
        let plan = result.plan.expect("a plan exists");
        assert_eq!(plan.legs.len(), 1);
        assert_eq!(plan.legs[0].symbol, "CHEAP");
        assert_eq!(plan.legs[0].side, LegSide::Sell);
        // 200,000 of protection at 100 x 50 x 0.20 = 1,000 a contract.
        assert_eq!(plan.legs[0].quantity, dec!("200"));
        assert!(plan.hedged_pnl["crash"] >= -100_000.0);
        // Each of the six measures is recorded.
        let m = &plan.measures;
        assert!(m.cost.is_positive());
        assert!(m.basis_risk > 0.0);
        assert!(m.edge_given_up.is_finite());
        assert!(m.convexity.abs() < 1e-12, "a future has no gamma");
        assert!(m.liquidity_decay_days > 0.0);
        assert!(m.unwind_feasible);
        assert!(m.counterparty_risk.abs() < 1e-12);
    }

    /// Mutation: drop the `unwind_feasible` refusal in `consider` and the
    /// thin instrument is sized anyway.
    #[test]
    fn a_hedge_that_cannot_be_unwound_inside_the_horizon_is_refused_not_shrunk() {
        let mut thin = future("THIN", -0.20, 0.1);
        thin.average_daily_volume = dec!("100");
        let result = search(&worlds(), &[thin], &objective(dec!("10000000"))).unwrap();
        assert!(!result.protected.is_empty(), "premise");
        assert!(result.plan.is_none());
        assert!(
            result
                .refusals
                .iter()
                .any(|r| matches!(r, SearchRefusal::UnwindInfeasible { .. })),
            "{:?}",
            result.refusals
        );
    }

    /// Mutation: skip the budget check and the dear premium is admitted.
    #[test]
    fn a_plan_never_consumes_more_than_the_hedge_capacity_withheld() {
        let mut put = future("PUT", -0.20, 0.1);
        put.class = InstrumentClass::Option;
        put.greeks = Greeks {
            delta: -0.5,
            gamma: 0.0002,
            vega: 0.1,
        };
        let unconstrained = search(&worlds(), &[put.clone()], &objective(dec!("100000000")))
            .unwrap()
            .plan
            .expect("premise: the put protects with room");
        assert!(unconstrained.capital.is_positive());
        let tight = search(
            &worlds(),
            &[put],
            &objective(unconstrained.capital - dec!("1")),
        )
        .unwrap();
        assert!(tight.plan.is_none());
        assert!(
            tight
                .refusals
                .iter()
                .any(|r| matches!(r, SearchRefusal::OverBudget { .. }))
        );
    }

    /// The forecast distribution decides which instrument protects which
    /// future: a hedge that only moves in the crash cannot protect a world it
    /// does nothing in, and the pair search finds the two-instrument plan.
    /// Mutation: skip the pair search (`.take(0)` on the second leg) and no
    /// plan is found. Sizing a candidate on its worst world alone is not
    /// enough on its own — `consider` re-checks every world before keeping a
    /// plan, and the round-2 mutation audit found that mutation survives
    /// unless that re-check is also removed, when the squeeze world is left
    /// below the floor and this fails.
    #[test]
    fn different_worlds_are_protected_by_different_instruments() {
        let mut worlds = worlds();
        worlds[2].book_pnl = -250_000.0;
        let crash = future("CRASH", -0.20, 0.5);
        let mut squeeze = future("SQZ", 0.0, 0.5);
        squeeze.moves.insert(
            "squeeze".to_string(),
            WorldMove {
                underlying: 0.25,
                volatility: 0.0,
            },
        );
        let result = search(&worlds, &[crash, squeeze], &objective(dec!("10000000"))).unwrap();
        assert_eq!(result.protected.len(), 2, "premise: two worlds fail");
        let plan = result.plan.expect("a pair protects both");
        let symbols: BTreeSet<&str> = plan.legs.iter().map(|leg| leg.symbol.as_str()).collect();
        assert_eq!(symbols, BTreeSet::from(["CRASH", "SQZ"]));
        for pnl in plan.hedged_pnl.values() {
            assert!(*pnl >= -100_000.0, "{:?}", plan.hedged_pnl);
        }
    }

    #[test]
    fn every_class_can_be_named_and_a_linear_class_with_convexity_is_refused() {
        for class in InstrumentClass::ALL {
            let mut candidate = future("X", -0.2, 0.1);
            candidate.class = class;
            if class.convex() {
                candidate.greeks = Greeks {
                    delta: -0.4,
                    gamma: 0.001,
                    vega: 0.2,
                };
            }
            assert!(candidate.validate().is_ok(), "{}", class.as_str());
        }
        let mut wrong = future("FUT", -0.2, 0.1);
        wrong.greeks.gamma = 0.01;
        assert!(wrong.validate().is_err());
    }

    /// Multiplier and delta both size the leg: halving delta doubles the
    /// contracts. Mutation: drop `g.delta` from `unit_pnl` and this fails.
    #[test]
    fn an_option_is_sized_through_its_multiplier_and_delta() {
        let mut put = future("PUT", -0.20, 0.0);
        put.class = InstrumentClass::Option;
        put.greeks = Greeks {
            delta: 0.5,
            gamma: 0.0,
            vega: 0.0,
        };
        let plan = search(&worlds(), &[put], &objective(dec!("100000000")))
            .unwrap()
            .plan
            .expect("a plan");
        // 200,000 / (100 x 50 x 0.5 x 0.20) = 400 contracts.
        assert_eq!(plan.legs[0].quantity, dec!("400"));
    }

    #[test]
    fn a_book_that_survives_every_world_plans_no_hedge() {
        let calm = vec![ForecastWorld {
            name: "calm".into(),
            probability: 1.0,
            book_pnl: -10.0,
        }];
        let result = search(&calm, &[future("F", -0.2, 0.1)], &objective(dec!("1"))).unwrap();
        assert!(result.protected.is_empty());
        assert!(result.plan.is_none());
    }
}
