//! `qip-cost-router` — what a decision cost, and the cheapest intelligence that
//! could have made it.
//!
//! Two capabilities that are really one. The cost engine says what reaching a
//! decision cost; the router uses that to decide how much intelligence a
//! decision is worth. Neither is much use alone: a router with no cost model
//! has nothing to be frugal about, and a cost model nothing consults is a
//! report.
//!
//! # The cost engine
//!
//! [`qip_contracts::NetEdge`] takes nine deductions off a gross figure. Seven
//! are charged by the market. Two — [`qip_contracts::DeductionKind::ComputeCost`]
//! and [`qip_contracts::DeductionKind::DataCost`] — are charged by the platform
//! to itself, and this crate is what produces them.
//!
//! They belong in the deduction list rather than in a separate budget for one
//! reason: **an opportunity that earns less than it cost to find is not an
//! opportunity.** Keeping those costs outside the edge calculation is how a
//! platform runs profitably on paper and loses money in aggregate — every trade
//! clears its spread and its fees, the inference bill arrives monthly against
//! none of them, and nothing in the per-trade arithmetic ever showed a loss.
//!
//! * [`ComputeLedger`] accumulates what a decision cost as it is made, rung by
//!   rung, and composes [`qip_agents::BudgetLedger`] so an agent's own
//!   allowance still refuses the call that would breach it.
//! * [`DataCostModel`] amortises a subscription over the reads it actually got.
//!   A source read once a day and one read a million times cost the same to
//!   licence and are wildly different per decision.
//! * [`CostEngine::cost_deductions`] hands back the two deductions.
//!
//! # The router
//!
//! **Select the cheapest intelligence level capable of making the decision.**
//! The ladder is [`IntelligenceTier`]: deterministic Rust, a statistical model,
//! a tiny model, a specialist model, an agent, a panel of agents, a deep model,
//! a simulation, a solver. [`Router::select`] walks it from the bottom and
//! stops at the first rung that is affordable, fast enough and strong enough.
//!
//! Two rules are properties of the types rather than checks in the code.
//!
//! * **A decision requiring determinism can never reach a model.** Risk checks
//!   and execution decisions are in that class. [`Router::select`] returns a
//!   [`Routing::Deterministic`] for them, holding a [`DeterministicRouting`]
//!   that has no field a [`ModelTier`] fits in — and [`Router::escalate`] takes
//!   a [`JudgedRouting`], which only the other branch can produce. There is no
//!   path, not merely no code taking one.
//! * **A rung that costs more than the decision is worth is refused by name.**
//!   [`TierVerdict::Unaffordable`] carries the cost and the ceiling, and the
//!   refusal says both.
//!
//! [`ReputationBook`] then answers *which* model, and answers it per context:
//! a model is scored by asset class, region, market regime, volatility regime
//! and horizon, and its record in one cell is not evidence about another.
//! Shrinkage is [`qip_contracts::signal::Conviction`]'s, so a model with no
//! observations in a regime reads as a coin flip there.
//!
//! Nothing here reads a clock or draws a random number, so the same decision
//! routes the same way on every run and a replay reproduces the reasoning as
//! well as the result.
//!
//! # Pricing a decision end to end
//!
//! ```
//! use qip_agents::Budget;
//! use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
//! use qip_core::{Decimal, Duration, dec};
//! use qip_cost_router::{
//!     Conditions, CostEngine, ComputeLedger, DataCostModel, DataReads, DataSource,
//!     DecisionContext, Determinism, Horizon, MarketRegime, Region, Router, Routing,
//!     VolatilityRegime,
//! };
//! use qip_financial::asset_class::AssetClass;
//!
//! # fn main() -> qip_core::error::Result<()> {
//! let conditions = Conditions::new(
//!     AssetClass::Equity,
//!     Region::new("us-east"),
//!     MarketRegime::Trending,
//!     VolatilityRegime::Normal,
//!     Horizon::Intraday,
//! );
//! let context = DecisionContext::new(
//!     "is this dislocation real",
//!     dec!("400"),
//!     Duration::from_secs(30),
//!     0.85,
//!     Determinism::NotRequired,
//!     conditions,
//! );
//!
//! // The cheapest rung that reaches 0.85 confidence inside thirty seconds.
//! let router = Router::default();
//! let routing = router.select(&context)?;
//!
//! // Charge what it used, against an agent budget that can refuse.
//! let mut ledger = ComputeLedger::new("is this dislocation real", Budget::research_default())?;
//! ledger.charge_all(&routing.tiers())?;
//!
//! // Amortise the licences the decision read.
//! let engine = CostEngine::new(DataCostModel::new().with_source(DataSource::new(
//!     "tick-feed",
//!     dec!("30000"),
//!     Duration::from_days(30),
//!     6_000_000,
//! )?));
//! let reads = DataReads::new().record("tick-feed", 400);
//!
//! let (compute, data) = engine.cost_deductions(&ledger, &reads)?;
//! assert_eq!(compute.kind, DeductionKind::ComputeCost);
//! assert_eq!(data.kind, DeductionKind::DataCost);
//! # Ok(())
//! # }
//! ```

pub mod context;
pub mod data;
pub mod engine;
pub mod ledger;
pub mod reputation;
pub mod router;
pub mod tier;

pub use context::{
    Conditions, DecisionContext, Determinism, Horizon, MarketRegime, Region, VolatilityRegime,
};
pub use data::{DataCostModel, DataReads, DataSource};
pub use engine::CostEngine;
pub use ledger::{ComputeLedger, TierCharge};
pub use reputation::{ModelReputation, Rated, Record, ReputationBook};
pub use router::{
    DeterministicRouting, Escalation, EscalationLimits, JudgedRouting, Router, Routing,
    RoutingPolicy, TierVerdict,
};
pub use tier::{IntelligenceTier, ModelTier};
