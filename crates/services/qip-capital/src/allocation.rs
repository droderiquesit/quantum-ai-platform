//! Splitting a risk budget across strategies and cells.
//!
//! The allocator's output is a set of numbers that cells will trade against
//! without asking again, so the arithmetic has to be exact and the constraints
//! have to hold jointly. Four bind at once — per strategy, per cell, per venue
//! and in total — and a strategy is given the smallest of them.
//!
//! Two decisions are worth stating because they are the ones that would
//! otherwise be made implicitly:
//!
//! * **Uncertainty is subtracted, not averaged.** A strategy is sized off
//!   `expected_sharpe - k · standard_error`, a lower confidence bound, rather
//!   than off the point estimate. Two strategies with the same point estimate
//!   and different sample sizes are not the same proposition, and an allocator
//!   that treats them alike systematically over-allocates to the one with
//!   least evidence — which is also the one most likely to be a fluke.
//! * **Capacity caps before the budget does.** Beyond a strategy's modelled
//!   capacity extra notional carries negative expected edge (see
//!   [`crate::capacity`]), so the allocator reduces to capacity and says so,
//!   rather than allocating the indicated size and discovering the impact
//!   afterwards.
//!
//! Sums are exact. Every allocation is capped by a running remainder and
//! subtracted from it in [`Decimal`], so the allocations cannot sum above the
//! budget by a rounding step. That is a real bug class: an allocator that
//! computes shares in floating point and rounds each one independently will
//! quietly over-commit by a few units per strategy, and a few units per
//! strategy across a few hundred strategies is a position nobody approved.

use crate::capacity::CapacityModel;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One strategy asking for capital, with the evidence behind the ask.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyProposal {
    pub strategy: StrategyId,
    /// The edge cell that would run it.
    pub cell: String,
    pub venue: VenueId,
    /// Risk-adjusted expected edge — an annualised Sharpe ratio.
    pub expected_sharpe: f64,
    /// Standard error of that estimate.
    ///
    /// Not optional. A point estimate submitted without one would have to be
    /// treated as certain, and no estimate from a finite sample is.
    pub sharpe_standard_error: f64,
    pub capacity: CapacityModel,
    /// Relative standard error of the capacity estimate, in `[0, 1]`.
    pub capacity_uncertainty: f64,
}

impl StrategyProposal {
    /// The edge the allocator actually sizes on: the point estimate less
    /// `penalty` standard errors, floored at zero.
    ///
    /// Floored rather than allowed negative because a negative score would
    /// otherwise flip the sign of a share of a positive budget.
    pub fn risk_adjusted_edge(&self, penalty: f64) -> f64 {
        let se = if self.sharpe_standard_error.is_finite() {
            self.sharpe_standard_error.abs()
        } else {
            f64::INFINITY
        };
        if !self.expected_sharpe.is_finite() {
            return 0.0;
        }
        (self.expected_sharpe - penalty * se).max(0.0)
    }
}

/// The ceilings an allocation must respect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AllocationLimits {
    /// The whole risk budget. No plan may sum above this.
    pub total_budget: Decimal,
    /// The most any one strategy may hold.
    pub per_strategy: Decimal,
    /// The most any one cell may hold across all its strategies.
    pub default_per_cell: Decimal,
    per_cell: BTreeMap<String, Decimal>,
    /// The most that may sit at any one venue.
    ///
    /// Venue concentration is its own limit rather than a special case of the
    /// cell limit: cells are an operational division and venues are a
    /// counterparty and outage boundary, and the same book can be spread
    /// across every cell and concentrated at one venue.
    pub default_per_venue: Decimal,
    per_venue: BTreeMap<VenueId, Decimal>,
}

impl AllocationLimits {
    pub fn new(
        total_budget: Decimal,
        per_strategy: Decimal,
        default_per_cell: Decimal,
        default_per_venue: Decimal,
    ) -> Result<Self> {
        if !total_budget.is_positive() {
            return Err(Error::invalid("a risk budget must be positive"));
        }
        for (label, limit) in [
            ("per-strategy", per_strategy),
            ("per-cell", default_per_cell),
            ("per-venue", default_per_venue),
        ] {
            if limit.is_negative() {
                return Err(Error::invalid(format!("the {label} limit cannot be negative")));
            }
        }
        Ok(Self {
            total_budget,
            per_strategy,
            default_per_cell,
            per_cell: BTreeMap::new(),
            default_per_venue,
            per_venue: BTreeMap::new(),
        })
    }

    /// Override the limit for one cell.
    pub fn with_cell_limit(mut self, cell: impl Into<String>, limit: Decimal) -> Self {
        self.per_cell.insert(cell.into(), limit);
        self
    }

    /// Override the limit for one venue.
    pub fn with_venue_limit(mut self, venue: VenueId, limit: Decimal) -> Self {
        self.per_venue.insert(venue, limit);
        self
    }

    pub fn cell_limit(&self, cell: &str) -> Decimal {
        self.per_cell.get(cell).copied().unwrap_or(self.default_per_cell)
    }

    pub fn venue_limit(&self, venue: &VenueId) -> Decimal {
        self.per_venue.get(venue).copied().unwrap_or(self.default_per_venue)
    }
}

/// How allocation shrinks as a drawdown deepens.
///
/// A step function, stated rather than smooth, so an operator can read the
/// number that will apply at 11% rather than infer it from a curve.
///
/// The shipped schedule is deliberately steeper than proportional — half the
/// book gone at a 10% drawdown, not 90% of it — for two reasons. First,
/// drawdowns are not independent draws: realised volatility is autocorrelated,
/// so the conditional probability of the next five percent given the last five
/// is higher than the unconditional one, and sizing as though it were not is
/// what turns a bad month into a terminal one. Second, the arithmetic of
/// recovery is asymmetric: a 20% drawdown needs a 25% return to undo and a 50%
/// drawdown needs 100%, so capital preserved deep in a drawdown is worth more
/// than the same capital deployed at the high-water mark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawdownSchedule {
    steps: Vec<(f64, Decimal)>,
}

impl Default for DrawdownSchedule {
    fn default() -> Self {
        Self {
            steps: vec![
                (0.00, Decimal::ONE),
                (0.05, Decimal::from_raw(750_000_000)),
                (0.10, Decimal::from_raw(500_000_000)),
                (0.15, Decimal::from_raw(250_000_000)),
                (0.20, Decimal::ZERO),
            ],
        }
    }
}

impl DrawdownSchedule {
    /// Build a schedule, refusing one that is not monotone.
    ///
    /// A schedule where a deeper drawdown allocates more is not a
    /// conservative schedule with a typo in it; it is a procyclical one, and
    /// the constructor refuses it so the property the tests assert cannot be
    /// broken by configuration.
    pub fn new(steps: Vec<(f64, Decimal)>) -> Result<Self> {
        if steps.is_empty() {
            return Err(Error::invalid("a drawdown schedule needs at least one step"));
        }
        let mut previous_drawdown = f64::NEG_INFINITY;
        let mut previous_multiplier = Decimal::MAX;
        for (drawdown, multiplier) in &steps {
            if !drawdown.is_finite() || *drawdown < 0.0 {
                return Err(Error::invalid("a drawdown step must be a non-negative fraction"));
            }
            if *drawdown <= previous_drawdown {
                return Err(Error::invalid("drawdown steps must ascend"));
            }
            if multiplier.is_negative() || *multiplier > Decimal::ONE {
                return Err(Error::invalid("an allocation multiplier must lie in [0, 1]"));
            }
            if *multiplier > previous_multiplier {
                return Err(Error::invalid(
                    "a deeper drawdown may not allocate more; the schedule must not increase",
                ));
            }
            previous_drawdown = *drawdown;
            previous_multiplier = *multiplier;
        }
        Ok(Self { steps })
    }

    pub fn steps(&self) -> &[(f64, Decimal)] {
        &self.steps
    }

    /// The multiplier in force at a drawdown.
    ///
    /// Below the first step the schedule allocates in full; a drawdown deeper
    /// than the last step takes the last step's multiplier, which in the
    /// shipped schedule is zero.
    pub fn multiplier_at(&self, drawdown: f64) -> Decimal {
        let drawdown = if drawdown.is_finite() { drawdown.max(0.0) } else { 1.0 };
        let mut multiplier = Decimal::ONE;
        for (threshold, step) in &self.steps {
            if drawdown >= *threshold {
                multiplier = *step;
            } else {
                break;
            }
        }
        multiplier
    }
}

/// What one strategy was given, and what stopped it being given more.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Allocation {
    pub strategy: StrategyId,
    pub cell: String,
    pub venue: VenueId,
    pub notional: Decimal,
    /// What its share of the budget alone would have been.
    pub indicated: Decimal,
    /// The lower-confidence-bound edge it was sized on.
    pub risk_adjusted_edge: f64,
    /// Every constraint that cut the indicated size, named.
    pub binding_constraints: Vec<String>,
}

impl Allocation {
    /// Whether the indicated size survived every constraint.
    pub fn is_unconstrained(&self) -> bool {
        self.binding_constraints.is_empty()
    }
}

/// The whole plan, with everything needed to audit it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AllocationPlan {
    pub at: Timestamp,
    /// The budget before the drawdown response.
    pub total_budget: Decimal,
    pub drawdown: f64,
    pub drawdown_multiplier: Decimal,
    /// The budget actually distributed against.
    pub budget: Decimal,
    pub allocations: Vec<Allocation>,
    /// Proposals given nothing, with why.
    pub refusals: Vec<(StrategyId, String)>,
}

impl AllocationPlan {
    /// Exact sum of everything allocated.
    pub fn allocated(&self) -> Decimal {
        self.allocations.iter().map(|a| a.notional).sum()
    }

    /// Whether the plan respects the budget it was built against.
    ///
    /// Checked in exact fixed point, so this is a real invariant rather than a
    /// tolerance.
    pub fn is_within_budget(&self) -> bool {
        self.allocated() <= self.budget && self.allocated() <= self.total_budget
    }

    pub fn for_strategy(&self, strategy: &StrategyId) -> Option<&Allocation> {
        self.allocations.iter().find(|a| &a.strategy == strategy)
    }

    /// Gross allocated to one cell.
    pub fn for_cell(&self, cell: &str) -> Decimal {
        self.allocations
            .iter()
            .filter(|a| a.cell == cell)
            .map(|a| a.notional)
            .sum()
    }

    /// Gross allocated at one venue.
    pub fn for_venue(&self, venue: &VenueId) -> Decimal {
        self.allocations
            .iter()
            .filter(|a| &a.venue == venue)
            .map(|a| a.notional)
            .sum()
    }
}

/// Distributes a risk budget across strategies and cells.
#[derive(Clone, Debug, PartialEq)]
pub struct CapitalAllocator {
    limits: AllocationLimits,
    schedule: DrawdownSchedule,
    uncertainty_penalty: f64,
}

impl CapitalAllocator {
    /// One standard error of penalty by default.
    ///
    /// A one-sided bound at roughly the 84th percentile: enough that a
    /// strategy with a wide estimate is visibly smaller than one with a tight
    /// one, not so much that anything with a short track record is squeezed to
    /// nothing before it can build one.
    pub const DEFAULT_UNCERTAINTY_PENALTY: f64 = 1.0;

    pub fn new(limits: AllocationLimits, schedule: DrawdownSchedule) -> Self {
        Self {
            limits,
            schedule,
            uncertainty_penalty: Self::DEFAULT_UNCERTAINTY_PENALTY,
        }
    }

    /// Change how many standard errors are subtracted from a point estimate.
    pub fn with_uncertainty_penalty(mut self, penalty: f64) -> Result<Self> {
        if !penalty.is_finite() || penalty < 0.0 {
            return Err(Error::invalid(
                "the uncertainty penalty must be a non-negative number of standard errors",
            ));
        }
        self.uncertainty_penalty = penalty;
        Ok(self)
    }

    pub fn limits(&self) -> &AllocationLimits {
        &self.limits
    }

    pub fn schedule(&self) -> &DrawdownSchedule {
        &self.schedule
    }

    /// Build a plan.
    ///
    /// Deterministic: proposals are considered by risk-adjusted edge and then
    /// by strategy id, so the same inputs produce the same plan on any machine
    /// and in any input order.
    pub fn allocate(
        &self,
        proposals: &[StrategyProposal],
        drawdown: f64,
        at: Timestamp,
    ) -> Result<AllocationPlan> {
        let multiplier = self.schedule.multiplier_at(drawdown);
        let budget = self
            .limits
            .total_budget
            .checked_mul(multiplier)
            .ok_or_else(|| Error::numeric("the drawdown-adjusted budget overflowed"))?;

        let mut plan = AllocationPlan {
            at,
            total_budget: self.limits.total_budget,
            drawdown,
            drawdown_multiplier: multiplier,
            budget,
            allocations: Vec::new(),
            refusals: Vec::new(),
        };

        let mut scored: Vec<(f64, &StrategyProposal)> = proposals
            .iter()
            .map(|proposal| (proposal.risk_adjusted_edge(self.uncertainty_penalty), proposal))
            .collect();
        // Descending edge, then ascending id. Sorting on a total order rather
        // than a partial one keeps the plan reproducible when two strategies
        // score identically.
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.strategy.as_str().cmp(b.1.strategy.as_str()))
        });

        let total_score: f64 = scored.iter().map(|(score, _)| score).sum();
        if total_score <= 0.0 || !budget.is_positive() {
            for (_, proposal) in scored {
                plan.refusals.push((
                    proposal.strategy.clone(),
                    if budget.is_positive() {
                        "no proposal has a positive edge after its uncertainty is subtracted"
                            .to_string()
                    } else {
                        format!(
                            "the drawdown response set the budget to {budget} at a {:.1}% drawdown",
                            drawdown * 100.0
                        )
                    },
                ));
            }
            return Ok(plan);
        }

        let mut remaining_total = budget;
        let mut remaining_cell: BTreeMap<String, Decimal> = BTreeMap::new();
        let mut remaining_venue: BTreeMap<VenueId, Decimal> = BTreeMap::new();

        for (score, proposal) in scored {
            if score <= 0.0 {
                plan.refusals.push((
                    proposal.strategy.clone(),
                    format!(
                        "a {:.2} Sharpe with a {:.2} standard error has no edge left after \
                         {:.1} standard error(s) are subtracted",
                        proposal.expected_sharpe,
                        proposal.sharpe_standard_error,
                        self.uncertainty_penalty
                    ),
                ));
                continue;
            }

            let share = Decimal::from_f64(score / total_score)
                .ok_or_else(|| Error::numeric("a strategy's share was not representable"))?;
            let indicated = budget
                .checked_mul(share)
                .ok_or_else(|| Error::numeric("an indicated allocation overflowed"))?;

            let cell_remaining = *remaining_cell
                .entry(proposal.cell.clone())
                .or_insert_with(|| self.limits.cell_limit(&proposal.cell));
            let venue_remaining = *remaining_venue
                .entry(proposal.venue.clone())
                .or_insert_with(|| self.limits.venue_limit(&proposal.venue));
            let capacity = proposal
                .capacity
                .conservative_capacity(proposal.capacity_uncertainty);

            let mut notional = indicated;
            let mut binding = Vec::new();
            for (limit, reason) in [
                (
                    self.limits.per_strategy,
                    format!("the {} per-strategy limit", self.limits.per_strategy),
                ),
                (
                    capacity.notional,
                    format!(
                        "modelled capacity of {} ({}), beyond which its own impact exceeds \
                         its edge",
                        capacity.notional,
                        capacity.binding.as_str()
                    ),
                ),
                (
                    cell_remaining,
                    format!("headroom of {cell_remaining} left at cell {}", proposal.cell),
                ),
                (
                    venue_remaining,
                    format!(
                        "headroom of {venue_remaining} left at venue {}",
                        proposal.venue.as_str()
                    ),
                ),
                (
                    remaining_total,
                    format!("headroom of {remaining_total} left in the total budget"),
                ),
            ] {
                if limit < notional {
                    notional = limit;
                    binding.push(reason);
                }
            }
            let notional = notional.max(Decimal::ZERO);

            if notional.is_zero() {
                plan.refusals.push((
                    proposal.strategy.clone(),
                    binding.last().cloned().unwrap_or_else(|| {
                        "no headroom remained under any limit".to_string()
                    }),
                ));
                continue;
            }

            // Decrement before pushing, so a later proposal cannot be sized
            // against headroom this one has already taken. The subtraction is
            // exact, which is what makes the budget invariant hold rather than
            // hold to a tolerance.
            remaining_total -= notional;
            if let Some(left) = remaining_cell.get_mut(&proposal.cell) {
                *left -= notional;
            }
            if let Some(left) = remaining_venue.get_mut(&proposal.venue) {
                *left -= notional;
            }

            plan.allocations.push(Allocation {
                strategy: proposal.strategy.clone(),
                cell: proposal.cell.clone(),
                venue: proposal.venue.clone(),
                notional,
                indicated,
                risk_adjusted_edge: score,
                binding_constraints: binding,
            });
        }

        Ok(plan)
    }
}
