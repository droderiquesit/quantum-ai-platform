//! The decision: move capital before the demand exists, or decline and say why.
//!
//! # The rule
//!
//! Move only when
//!
//! ```text
//! expected_value(demand, forecast_interval) > transfer_cost + funding_cost
//! ```
//!
//! and evaluate that inequality **with the interval's lower bound on the left
//! and the upper bound on the right**. Both halves matter and the second is the
//! one that gets dropped. A benefit taken at the point estimate and a cost taken
//! at the point estimate is a plan that clears its hurdle by construction about
//! half the time; the layer then loses money quietly, in small amounts, on
//! transfers that each looked marginally positive.
//!
//! Concretely:
//!
//! * The **benefit** is the shortfall this transfer avoids, computed on
//!   [`crate::forecast::Interval::lower`] — the demand the forecaster is
//!   confident about — never on the point estimate and never on the upper bound.
//! * The **cost** is [`crate::transfer::TransferCost::upper`], the widened
//!   figure, plus the carry of holding the balance at the destination until it
//!   is used.
//! * A refusal names both numbers, because "not worth it" without the two
//!   figures is not something anyone can act on.
//!
//! # What the benefit actually is
//!
//! Not "the value of the opportunity". The value of pre-positioning is the cost
//! of the alternative, and the alternative is not *never having the capital* —
//! it is **sourcing it reactively once the demand appears**. So the benefit is
//! the shortfall penalty over the settlement lag the fabric would be short for
//! if it waited: [`crate::settlement::SettlementCalendar`] is asked what a
//! transfer instructed at the moment of need would deliver, and the gap between
//! then and delivery is the exposure pre-positioning removes.
//!
//! This is why the same transfer is worth much more into a Monday morning than
//! into a Wednesday afternoon. Reactive funding of a Monday need is instructed
//! on Monday and settles on Tuesday; reactive funding of a Friday-evening need
//! settles the following Tuesday. The calendar, not the forecast, is what makes
//! those different.
//!
//! # Why a wider interval buys less, not more
//!
//! Textbook inventory theory says the opposite: under an asymmetric penalty the
//! cost-minimising order quantity is the critical fractile of the demand
//! distribution, so more dispersion means *order more*. That result assumes the
//! distribution is known. Here it is estimated, from a short history, by
//! [`crate::forecast::DemandForecaster`] — and a widening interval is evidence
//! that the forecaster does not know, not evidence that a larger buffer is
//! warranted. Reaching up into the wide part of a distribution the model does
//! not trust is precisely how capital ends up in the wrong place with
//! confidence.
//!
//! So the size is anchored to [`crate::forecast::Interval::lower`] and the
//! asymmetry buys a bounded buffer *on top of that anchor*
//! ([`PrePositioningPlanner::DEFAULT_SHORTFALL_BUFFER_CAP`]), scaled by
//! [`crate::transfer::ShortfallAsymmetry::critical_fractile`]. A wider interval
//! at the same point estimate therefore has a lower anchor, and buys strictly
//! less. Uncertainty reduces conviction, which is the whole point of carrying it
//! around.
//!
//! # The axes the allocation spans
//!
//! One decision, six axes, and they are the axes of a single problem rather than
//! six problems that happen to share a budget. **Cash location** and **broker
//! allocation** are the region and venue halves of [`CapitalLocation`];
//! **currency exposure** is its third component, and the reason a transfer may
//! have to buy the currency before it can place the balance. **Collateral
//! placement**, **inventory** and **margin reserves** are
//! [`DemandKind::Collateral`], [`DemandKind::Inventory`] and
//! [`DemandKind::Margin`] — competing for the same budget as
//! [`DemandKind::Cash`] and [`DemandKind::FxFunding`], and ranked against them
//! by value per unit of capital committed rather than by category.
//!
//! # Composing rather than duplicating the allocator
//!
//! The fabric does not have a budget. It spends the headroom left in
//! [`qip_capital::AllocationPlan`] — the same drawdown-adjusted budget the
//! [`qip_capital::CapitalAllocator`] distributed to strategies — and it checks
//! every destination against that allocator's own per-venue limit. Two systems
//! each holding a budget is two systems each believing they own the same dollar.
//!
//! A move that would breach a limit is **refused, not clipped**. That is a
//! deliberate difference from the allocator, which reduces to the binding limit
//! and says so. A strategy allocation reduced by a third is a smaller version of
//! the same trade; a pre-position reduced by a third pays the full fixed
//! transfer cost for partial coverage and may no longer clear its own hurdle. A
//! clipped transfer is a different decision, and it deserves to be taken as one.

use crate::forecast::{DemandForecast, DemandKind};
use crate::location::CapitalLocation;
use crate::settlement::{SettlementCalendar, SettlementQuote};
use crate::transfer::{FxRates, ShortfallAsymmetry, TransferCost, TransferCostModel};
use qip_capital::allocation::{AllocationPlan, CapitalAllocator};
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, Duration, Timestamp};
use qip_numerics::lp::{LinearProgram, LpStatus, Sense, solve};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Capital of one kind already sitting at one location.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocationBalance {
    /// Where it is.
    pub location: CapitalLocation,
    /// What kind of demand it can meet.
    pub kind: DemandKind,
    /// How much, in the location's currency.
    pub on_hand: Decimal,
}

impl LocationBalance {
    /// Record a balance.
    pub fn new(location: CapitalLocation, kind: DemandKind, on_hand: Decimal) -> Result<Self> {
        if on_hand.is_negative() {
            return Err(Error::invalid(format!(
                "a balance at {location} cannot be negative; a deficit is demand, and belongs \
                 in a forecast"
            )));
        }
        Ok(Self {
            location,
            kind,
            on_hand,
        })
    }
}

/// Everything the planner needs to decide.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositioningRequest {
    /// Where the free capital currently sits.
    pub source: CapitalLocation,
    /// How much of it may be moved, in the source currency.
    pub mobile_capital: Decimal,
    /// What is already in place, per location and kind.
    pub balances: Vec<LocationBalance>,
    /// Where demand is expected.
    pub forecasts: Vec<DemandForecast>,
    /// The rates every amount is converted through to face a limit.
    pub rates: FxRates,
}

impl PrePositioningRequest {
    /// Start a request.
    pub fn new(source: CapitalLocation, mobile_capital: Decimal, rates: FxRates) -> Result<Self> {
        if mobile_capital.is_negative() {
            return Err(Error::invalid("mobile capital cannot be negative"));
        }
        Ok(Self {
            source,
            mobile_capital,
            balances: Vec::new(),
            forecasts: Vec::new(),
            rates,
        })
    }

    /// Add a balance already in place.
    pub fn with_balance(mut self, balance: LocationBalance) -> Self {
        self.balances.push(balance);
        self
    }

    /// Add a forecast to plan against.
    pub fn with_forecast(mut self, forecast: DemandForecast) -> Self {
        self.forecasts.push(forecast);
        self
    }

    /// What is already at a location for a kind of demand.
    ///
    /// Balances are summed rather than the first one taken, so two accounts at
    /// the same custodian are one pool — which is what they are.
    pub fn on_hand(&self, location: &CapitalLocation, kind: DemandKind) -> Decimal {
        self.balances
            .iter()
            .filter(|b| &b.location == location && b.kind == kind)
            .map(|b| b.on_hand)
            .sum()
    }
}

/// Why a lane was not pre-positioned into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The interval's lower bound is already covered by what is on hand.
    NoConfidentDemand,
    /// The benefit at the lower bound does not cover the cost at the upper.
    CostExceedsBenefit,
    /// The capital would arrive after the demand it was sent for.
    SettlesTooLate,
    /// The destination venue's allocation limit has no room.
    VenueLimit,
    /// The headroom left in the live allocation is not enough for this move.
    BudgetExhausted,
    /// A currency, calendar or rate the lane needs is missing.
    Unpriceable,
}

impl RefusalReason {
    /// A stable label for logs and metrics.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NoConfidentDemand => "no_confident_demand",
            Self::CostExceedsBenefit => "cost_exceeds_benefit",
            Self::SettlesTooLate => "settles_too_late",
            Self::VenueLimit => "venue_limit",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Unpriceable => "unpriceable",
        }
    }
}

/// A lane the planner declined, with the numbers behind the decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    /// Where the capital would have gone.
    pub location: CapitalLocation,
    /// What kind of demand it would have met.
    pub kind: DemandKind,
    /// Which test the lane failed.
    pub reason: RefusalReason,
    /// The figures, in a sentence.
    pub detail: String,
}

impl Refusal {
    /// A line for an approval log.
    pub fn describe(&self) -> String {
        format!(
            "{} at {} refused ({}): {}",
            self.kind.as_str(),
            self.location,
            self.reason.as_str(),
            self.detail
        )
    }
}

/// One transfer the plan proposes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositionMove {
    /// Where the capital comes from.
    pub from: CapitalLocation,
    /// Where it goes.
    pub to: CapitalLocation,
    /// What kind of demand it is for.
    pub kind: DemandKind,
    /// How much, in the destination currency.
    pub amount: Decimal,
    /// The same amount in the base currency — the figure that faces the limits.
    pub amount_base: Decimal,
    /// When the instruction must be given.
    pub instruct_at: Timestamp,
    /// When the demand is expected.
    pub needed_by: Timestamp,
    /// What the calendar says about timing.
    pub settlement: SettlementQuote,
    /// The itemised cost of moving it.
    pub cost: TransferCost,
    /// The shortfall avoided, computed at the interval's lower bound.
    pub benefit_lower_bound: Decimal,
    /// The cost charged, at its upper bound.
    pub cost_upper_bound: Decimal,
    /// The carry of holding the balance at the destination until it is used.
    pub funding_cost: Decimal,
    /// Benefit less both costs, in the destination currency.
    pub net_value: Decimal,
    /// The same, in the base currency.
    pub net_value_base: Decimal,
    /// Net value per unit of capital committed. A ratio, hence `f64`.
    pub value_density_stat: f64,
}

impl PrePositionMove {
    /// A line an operator can approve or query.
    pub fn describe(&self) -> String {
        format!(
            "move {} to {} for {} by {}: benefit {} at the forecast's lower bound against {} \
             transfer and {} funding, net {}",
            self.amount,
            self.to,
            self.kind.as_str(),
            self.needed_by.to_rfc3339(),
            self.benefit_lower_bound,
            self.cost_upper_bound,
            self.funding_cost,
            self.net_value,
        )
    }
}

/// Everything the planner knew about one lane, kept so a plan can be scored
/// after the fact.
///
/// Recorded for refused lanes as well as accepted ones. A backtest that only
/// sees the transfers that happened cannot tell a forecaster that declined
/// correctly from one that never looked, and those are the two cases worth
/// telling apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaneContext {
    /// The lane.
    pub location: CapitalLocation,
    /// The kind of demand.
    pub kind: DemandKind,
    /// What was already there.
    pub on_hand: Decimal,
    /// The forecast's lower bound.
    pub forecast_lower: Decimal,
    /// The forecast's point estimate.
    pub forecast_point: Decimal,
    /// The forecast's upper bound.
    pub forecast_upper: Decimal,
    /// When the demand was expected.
    pub needed_by: Timestamp,
    /// How long the lane would be short if capital were sourced reactively.
    pub reactive_lag: Duration,
    /// How long capital sent to this lane is tied up before the demand resolves.
    pub committed_for: Duration,
    /// What the plan actually sent, in the destination currency.
    pub positioned: Decimal,
    /// What sending it cost, in the destination currency. Zero where nothing
    /// was sent, including where a priced candidate lost its budget contest.
    pub transfer_cost: Decimal,
}

/// A set of transfers, the lanes declined, and the budget they were taken
/// against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrePositioningPlan {
    /// When the plan was built.
    pub at: Timestamp,
    /// The rates the plan was built against.
    ///
    /// Carried rather than referenced so a plan can be scored afterwards
    /// against the rates it was actually decided on. Scoring it against later
    /// rates would score a different plan.
    pub rates: FxRates,
    /// The headroom the plan was allowed to spend, in the base currency.
    pub budget: Decimal,
    /// The transfers, in the order they were taken.
    pub moves: Vec<PrePositionMove>,
    /// The lanes declined, each naming its figures.
    pub refusals: Vec<Refusal>,
    /// Every lane considered, for scoring afterwards.
    pub lanes: Vec<LaneContext>,
    /// The value the fractional relaxation of this allocation would reach.
    ///
    /// An upper bound on what any all-or-nothing plan could have achieved
    /// against the same budget, from [`qip_numerics::lp`]. `None` where the
    /// problem was too large to bound cheaply or the solver did not reach an
    /// optimum. Reported so the greedy's gap is visible rather than assumed to
    /// be zero; it is a diagnostic statistic, hence `f64`.
    pub relaxation_bound_stat: Option<f64>,
}

impl PrePositioningPlan {
    /// The currency limits, budgets and scores are stated in.
    pub fn base_currency(&self) -> Currency {
        self.rates.base()
    }

    /// Exact sum of everything committed, in the base currency.
    pub fn committed(&self) -> Decimal {
        self.moves.iter().map(|m| m.amount_base).sum()
    }

    /// Whether the plan respects the headroom it was built against.
    ///
    /// Exact fixed point, so this is an invariant rather than a tolerance.
    pub fn is_within_budget(&self) -> bool {
        self.committed() <= self.budget
    }

    /// Headroom left unspent.
    pub fn unspent(&self) -> Decimal {
        self.budget - self.committed()
    }

    /// Expected net value of the plan, in the base currency.
    pub fn expected_net_value(&self) -> Decimal {
        self.moves.iter().map(|m| m.net_value_base).sum()
    }

    /// Committed at one venue, in the base currency.
    pub fn for_venue(&self, venue: &VenueId) -> Decimal {
        self.moves
            .iter()
            .filter(|m| &m.to.venue == venue)
            .map(|m| m.amount_base)
            .sum()
    }

    /// Moved into one location for one kind of demand, in that currency.
    pub fn moved_into(&self, location: &CapitalLocation, kind: DemandKind) -> Decimal {
        self.moves
            .iter()
            .filter(|m| &m.to == location && m.kind == kind)
            .map(|m| m.amount)
            .sum()
    }

    /// The refusals of one kind.
    pub fn refusals_because(&self, reason: RefusalReason) -> impl Iterator<Item = &Refusal> {
        self.refusals.iter().filter(move |r| r.reason == reason)
    }

    /// A summary for an approval log.
    pub fn describe(&self) -> String {
        format!(
            "{} transfer(s) committing {} of {} available, {} refused, expected net value {}",
            self.moves.len(),
            self.committed(),
            self.budget,
            self.refusals.len(),
            self.expected_net_value(),
        )
    }
}

/// A lane that cleared the hurdle, before the budget and limits are applied.
#[derive(Clone, Debug)]
struct Candidate {
    proposed: PrePositionMove,
    lane: usize,
}

/// Decides what to pre-position.
#[derive(Clone, Debug)]
pub struct PrePositioningPlanner {
    allocator: CapitalAllocator,
    transfer: TransferCostModel,
    calendar: SettlementCalendar,
    shortfall_buffer_cap: f64,
}

impl PrePositioningPlanner {
    /// The most the asymmetry may add above the confident demand anchor.
    ///
    /// A quarter. Enough that a contractual shortfall — where the fractile sits
    /// near 0.9 — buys a visible buffer, and little enough that the buffer
    /// cannot become the transfer. The anchor is the number this crate is
    /// willing to defend; everything above it is a hedge against being wrong
    /// about the anchor, and a hedge larger than the position is not a hedge.
    pub const DEFAULT_SHORTFALL_BUFFER_CAP: f64 = 0.25;

    /// The most candidates the relaxation bound will be computed over.
    ///
    /// The simplex is exact and bounded but not free, and this runs on the path
    /// of a funding decision. Past this the bound is simply not reported.
    const RELAXATION_CANDIDATE_LIMIT: usize = 48;

    /// Build a planner around a live allocator, a cost model and a calendar.
    pub fn new(
        allocator: CapitalAllocator,
        transfer: TransferCostModel,
        calendar: SettlementCalendar,
    ) -> Self {
        Self {
            allocator,
            transfer,
            calendar,
            shortfall_buffer_cap: Self::DEFAULT_SHORTFALL_BUFFER_CAP,
        }
    }

    /// Change how much buffer the asymmetry may buy above the anchor.
    pub fn with_shortfall_buffer_cap(mut self, cap: f64) -> Result<Self> {
        if !cap.is_finite() || !(0.0..=1.0).contains(&cap) {
            return Err(Error::invalid(
                "the shortfall buffer cap is a fraction of the confident demand and must lie \
                 in [0, 1]",
            ));
        }
        self.shortfall_buffer_cap = cap;
        Ok(self)
    }

    /// The allocator whose limits every plan is checked against.
    pub fn allocator(&self) -> &CapitalAllocator {
        &self.allocator
    }

    /// The settlement calendar in force.
    pub fn calendar(&self) -> &SettlementCalendar {
        &self.calendar
    }

    /// Build a pre-positioning plan against the live allocation.
    ///
    /// Draws no random numbers and reads no clock: `at` is the instant being
    /// reasoned about, and the same inputs produce the same plan on any machine.
    pub fn plan(
        &self,
        request: &PrePositioningRequest,
        live: &AllocationPlan,
        at: Timestamp,
    ) -> Result<PrePositioningPlan> {
        let budget = self.headroom(request, live)?;

        let mut lanes: Vec<LaneContext> = Vec::with_capacity(request.forecasts.len());
        let mut refusals: Vec<Refusal> = Vec::new();
        let mut candidates: Vec<Candidate> = Vec::new();

        // Lanes are considered in a stable order so that two runs on the same
        // inputs record the same lane indices, and a plan diffs cleanly against
        // the previous one.
        let mut ordered: Vec<&DemandForecast> = request.forecasts.iter().collect();
        ordered.sort_by(|a, b| {
            a.location
                .cmp(&b.location)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.needed_by().cmp(&b.needed_by()))
        });

        for forecast in ordered {
            let lane_index = lanes.len();
            let asymmetry = ShortfallAsymmetry::for_kind(forecast.kind)?;
            let on_hand = request.on_hand(&forecast.location, forecast.kind);
            let interval = forecast.interval();
            let needed_by = forecast.needed_by();

            // What waiting would cost: the lag between the demand appearing and
            // capital instructed at that moment actually landing.
            let reactive = self.calendar.quote(needed_by)?;
            let reactive_lag = reactive.available_at.since(needed_by);

            let lane = LaneContext {
                location: forecast.location.clone(),
                kind: forecast.kind,
                on_hand,
                forecast_lower: interval.lower(),
                forecast_point: interval.point(),
                forecast_upper: interval.upper(),
                needed_by,
                reactive_lag,
                committed_for: needed_by.since(at),
                positioned: Decimal::ZERO,
                transfer_cost: Decimal::ZERO,
            };

            let confident_gap = (interval.lower() - on_hand).max(Decimal::ZERO);
            if confident_gap.is_zero() {
                refusals.push(Refusal {
                    location: forecast.location.clone(),
                    kind: forecast.kind,
                    reason: RefusalReason::NoConfidentDemand,
                    detail: format!(
                        "{on_hand} on hand already covers the forecast's {} lower bound; \
                         the {} point estimate is not enough to move capital on",
                        interval.lower(),
                        interval.point()
                    ),
                });
                lanes.push(lane);
                continue;
            }

            let quote = self.calendar.quote(at)?;
            if !quote.arrives_by(needed_by) {
                refusals.push(Refusal {
                    location: forecast.location.clone(),
                    kind: forecast.kind,
                    reason: RefusalReason::SettlesTooLate,
                    detail: format!(
                        "instructed {} the cut-off, {} settlement makes the capital usable \
                         at {}, which is {:.2} day(s) after it is needed at {}",
                        if quote.made_cutoff { "inside" } else { "after" },
                        self.calendar.convention().as_str(),
                        quote.available_at.to_rfc3339(),
                        quote.lateness(needed_by).as_days_f64(),
                        needed_by.to_rfc3339(),
                    ),
                });
                lanes.push(lane);
                continue;
            }

            let holding = needed_by.since(quote.available_at);
            let amount = self.size(confident_gap, &asymmetry)?;
            let cost = self.transfer.price(
                amount,
                &request.source,
                &forecast.location,
                &quote,
                holding,
            )?;

            // The benefit is taken on the confident gap alone. The buffer above
            // it is a hedge and is not allowed to justify itself.
            let benefit = asymmetry.shortfall_penalty(confident_gap, reactive_lag);
            let funding_cost = asymmetry.surplus_penalty(amount, holding);
            let hurdle = cost.upper + funding_cost;

            if benefit <= hurdle {
                refusals.push(Refusal {
                    location: forecast.location.clone(),
                    kind: forecast.kind,
                    reason: RefusalReason::CostExceedsBenefit,
                    detail: format!(
                        "a benefit of {benefit} at the forecast's {} lower bound does not cover \
                         a transfer cost of {} plus a funding cost of {funding_cost}, {hurdle} \
                         in total",
                        interval.lower(),
                        cost.upper,
                    ),
                });
                lanes.push(lane);
                continue;
            }

            let amount_base = request.rates.to_base(amount, forecast.location.currency)?;
            if !amount_base.is_positive() {
                refusals.push(Refusal {
                    location: forecast.location.clone(),
                    kind: forecast.kind,
                    reason: RefusalReason::Unpriceable,
                    detail: format!(
                        "{amount} {} converts to {amount_base} in the {} base, which cannot \
                         be charged against a limit",
                        forecast.location.currency,
                        request.rates.base()
                    ),
                });
                lanes.push(lane);
                continue;
            }
            let net_value = benefit - hurdle;
            let net_value_base = request
                .rates
                .to_base(net_value, forecast.location.currency)?;

            lanes.push(lane);
            candidates.push(Candidate {
                proposed: PrePositionMove {
                    from: request.source.clone(),
                    to: forecast.location.clone(),
                    kind: forecast.kind,
                    amount,
                    amount_base,
                    instruct_at: at,
                    needed_by,
                    settlement: quote,
                    cost,
                    benefit_lower_bound: benefit,
                    cost_upper_bound: hurdle - funding_cost,
                    funding_cost,
                    net_value,
                    net_value_base,
                    value_density_stat: net_value_base.to_f64() / amount_base.to_f64(),
                },
                lane: lane_index,
            });
        }

        // Highest value per unit of capital first. For a single budget
        // constraint this is exactly optimal in the fractional relaxation and
        // near-optimal in the all-or-nothing problem the fabric actually faces;
        // the gap is reported rather than hidden, in `relaxation_bound_stat`.
        //
        // Deliberately not solved as a floating-point linear program and rounded
        // back into `Decimal`. That rounding step is where a budget quietly
        // overshoots by a few units per transfer, which is the bug class this
        // crate's budget invariant exists to exclude.
        candidates.sort_by(|a, b| {
            b.proposed
                .value_density_stat
                .total_cmp(&a.proposed.value_density_stat)
                .then_with(|| a.proposed.to.cmp(&b.proposed.to))
                .then_with(|| a.proposed.kind.cmp(&b.proposed.kind))
        });

        let venue_headroom = self.venue_headroom(&candidates, live);
        let relaxation_bound_stat = self.relaxation_bound(&candidates, budget, &venue_headroom);

        let mut remaining = budget;
        let mut used_at_venue: BTreeMap<VenueId, Decimal> = BTreeMap::new();
        let mut moves: Vec<PrePositionMove> = Vec::new();

        for candidate in candidates {
            let move_ = candidate.proposed;
            if move_.amount_base > remaining {
                refusals.push(Refusal {
                    location: move_.to.clone(),
                    kind: move_.kind,
                    reason: RefusalReason::BudgetExhausted,
                    detail: format!(
                        "the transfer needs {} but only {remaining} of the {budget} allocation \
                         headroom is left; a partial transfer pays the whole fixed cost for \
                         part of the cover, so it is refused rather than clipped",
                        move_.amount_base
                    ),
                });
                continue;
            }

            let limit = self.allocator.limits().venue_limit(&move_.to.venue);
            let already = live.for_venue(&move_.to.venue)
                + used_at_venue
                    .get(&move_.to.venue)
                    .copied()
                    .unwrap_or(Decimal::ZERO);
            let would_be = already + move_.amount_base;
            if would_be > limit {
                refusals.push(Refusal {
                    location: move_.to.clone(),
                    kind: move_.kind,
                    reason: RefusalReason::VenueLimit,
                    detail: format!(
                        "venue {} already carries {already} and this transfer of {} would take \
                         it to {would_be} against the allocator's {limit} limit",
                        move_.to.venue.as_str(),
                        move_.amount_base,
                    ),
                });
                continue;
            }

            // Decremented before the move is recorded, so a later transfer
            // cannot be sized against headroom this one has taken. Exact
            // subtraction is what makes the budget invariant an invariant.
            remaining -= move_.amount_base;
            *used_at_venue
                .entry(move_.to.venue.clone())
                .or_insert(Decimal::ZERO) += move_.amount_base;
            if let Some(lane) = lanes.get_mut(candidate.lane) {
                lane.positioned = move_.amount;
                lane.transfer_cost = move_.cost.total;
            }
            moves.push(move_);
        }

        Ok(PrePositioningPlan {
            at,
            rates: request.rates.clone(),
            budget,
            moves,
            refusals,
            lanes,
            relaxation_bound_stat,
        })
    }

    /// The headroom a plan may spend, in the base currency.
    ///
    /// The smaller of the capital that can physically move and the room left in
    /// the live allocation. Taking the allocator's *drawdown-adjusted* budget
    /// rather than its headline one is deliberate: when the drawdown schedule
    /// has cut the book in half, the fabric stops pre-positioning too. A layer
    /// that kept moving capital around a book that is being taken off would be
    /// routing around the risk response.
    fn headroom(&self, request: &PrePositioningRequest, live: &AllocationPlan) -> Result<Decimal> {
        let mobile = request
            .rates
            .to_base(request.mobile_capital, request.source.currency)?;
        let ceiling = live.budget.min(live.total_budget);
        let allocation_headroom = (ceiling - live.allocated()).max(Decimal::ZERO);
        Ok(mobile.min(allocation_headroom))
    }

    /// The size to move: the confident gap, plus a bounded asymmetry buffer.
    fn size(&self, confident_gap: Decimal, asymmetry: &ShortfallAsymmetry) -> Result<Decimal> {
        let tilt = ((asymmetry.critical_fractile() - 0.5) * 2.0).clamp(0.0, 1.0);
        let factor = Decimal::from_f64(1.0 + tilt * self.shortfall_buffer_cap)
            .ok_or_else(|| Error::numeric("the shortfall buffer factor was not representable"))?;
        confident_gap
            .checked_mul(factor)
            .ok_or_else(|| Error::numeric("the pre-positioned amount overflowed"))
    }

    /// Remaining room at every venue a candidate would land at.
    fn venue_headroom(
        &self,
        candidates: &[Candidate],
        live: &AllocationPlan,
    ) -> BTreeMap<VenueId, Decimal> {
        let mut headroom = BTreeMap::new();
        for candidate in candidates {
            let venue = &candidate.proposed.to.venue;
            headroom.entry(venue.clone()).or_insert_with(|| {
                (self.allocator.limits().venue_limit(venue) - live.for_venue(venue))
                    .max(Decimal::ZERO)
            });
        }
        headroom
    }

    /// The value the fractional relaxation would reach against the same limits.
    ///
    /// Returns `None` rather than an error on any difficulty: this is a
    /// diagnostic, and a plan must not fail to be produced because a bound could
    /// not be computed for the report attached to it.
    fn relaxation_bound(
        &self,
        candidates: &[Candidate],
        budget: Decimal,
        venue_headroom: &BTreeMap<VenueId, Decimal>,
    ) -> Option<f64> {
        if candidates.is_empty() || candidates.len() > Self::RELAXATION_CANDIDATE_LIMIT {
            return None;
        }
        let n = candidates.len();
        // The simplex minimises, so the objective is the negated value.
        let objective: Vec<f64> = candidates
            .iter()
            .map(|c| -c.proposed.net_value_base.to_f64())
            .collect();
        let weights: Vec<f64> = candidates
            .iter()
            .map(|c| c.proposed.amount_base.to_f64())
            .collect();

        let mut program = LinearProgram::minimise(objective);
        program = program
            .subject_to(weights.clone(), Sense::LessOrEqual, budget.to_f64())
            .ok()?;
        for (venue, room) in venue_headroom {
            let row: Vec<f64> = candidates
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if &c.proposed.to.venue == venue {
                        weights[i]
                    } else {
                        0.0
                    }
                })
                .collect();
            program = program
                .subject_to(row, Sense::LessOrEqual, room.to_f64())
                .ok()?;
        }
        for index in 0..n {
            let mut row = vec![0.0; n];
            row[index] = 1.0;
            program = program.subject_to(row, Sense::LessOrEqual, 1.0).ok()?;
        }

        let solution = solve(&program).ok()?;
        if solution.status == LpStatus::Optimal {
            Some(-solution.objective)
        } else {
            None
        }
    }
}
