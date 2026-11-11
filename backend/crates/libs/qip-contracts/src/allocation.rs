//! What the capital brain's allocator was bounded by and what it decided,
//! as records another subsystem may read.
//!
//! Moved here from `qip_capital::allocation` by ADR 0385. The fabric that
//! pre-positions capital checks every move against the allocator's venue
//! limits and against the room the live plan leaves, and until this module
//! existed it took those from `qip-capital` directly: the Settlement Brain's
//! crate depended on the Capital Brain's, which ADR 0127 decision 5 forbids
//! except through this crate.
//!
//! What crossed is data: the limits an operator set, the plan the allocator
//! produced, and reads of their own fields (a lookup, a sum, a comparison of
//! two sums). What did not cross is every decision. Sizing on a lower
//! confidence bound, the drawdown schedule and the allocator itself stay in
//! `qip-capital`, so nothing that holds only this crate can produce a plan,
//! only read one.

use crate::signal::StrategyId;
use crate::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
                return Err(Error::invalid(format!(
                    "the {label} limit cannot be negative"
                )));
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
        self.per_cell
            .get(cell)
            .copied()
            .unwrap_or(self.default_per_cell)
    }

    pub fn venue_limit(&self, venue: &VenueId) -> Decimal {
        self.per_venue
            .get(venue)
            .copied()
            .unwrap_or(self.default_per_venue)
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
