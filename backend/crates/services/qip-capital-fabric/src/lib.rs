//! `qip-capital-fabric` — pre-position capital before the opportunity exists.
//!
//! [`qip_capital`] decides how much risk the platform may take and issues the
//! grants that let edge cells take it. It answers *how much*, and it answers it
//! for now. This crate answers the question that comes next and that nobody asks
//! until it is too late to act on: **where will that capital have to be, and
//! will it be there in time.**
//!
//! The rule the whole crate implements is one sentence: *do not wait until an
//! opportunity exists to move capital.* Forecast where it will be needed, and
//! pre-position when the expected value of having it there exceeds the cost of
//! putting it there. This is an extension of [`qip_capital`] forward in time,
//! not a replacement for it — the budget, the drawdown response and the venue
//! limits are all still that crate's, composed here rather than restated.
//!
//! # The five pieces
//!
//! * [`forecast`] predicts demand for cash, collateral, FX funding, inventory
//!   and margin per region, currency and venue. Every forecast carries an
//!   [`Interval`], and there is no way to express one that does not: a point
//!   forecast used for pre-positioning is how capital ends up in the wrong place
//!   with confidence.
//! * [`transfer`] prices the move — FX conversion, wire fees, the funding
//!   differential, the opportunity cost of capital in flight — and prices the
//!   thing that gets left out, which is the cost of *not* having it where it
//!   turned out to be needed. [`ShortfallAsymmetry`] refuses to be configured
//!   symmetrically, because a symmetric penalty puts the optimum at the median
//!   and systematically under-positions.
//! * [`settlement`] knows that capital in flight is not capital available.
//!   T+0/T+1/T+2 counted in settlement days, cut-off times, and non-settlement
//!   days — so a plan that assumes Friday afternoon's instruction is Monday
//!   morning's balance is refused rather than approved.
//! * [`plan`] takes the decision. Benefit at the interval's **lower** bound
//!   against cost at its **upper** bound, checked against
//!   [`qip_capital::CapitalAllocator`]'s live limits, with every refusal naming
//!   its figures.
//! * [`mod@evaluate`] scores the plan afterwards against what the world actually
//!   needed, which is what makes the forecaster improvable rather than merely
//!   confident.
//!
//! # What is deliberately not here
//!
//! **Taking capital back.** [`qip_capital::RecallOrder`] already withdraws
//! capital mid-flight, already models the fact that a recall is a request rather
//! than a guarantee, and already has the envelope expiry as its backstop. A
//! second, softer recall path in this crate would be a way to move capital that
//! does not stop when the first one does.
//!
//! **A second budget.** The fabric spends the headroom left in a live
//! [`qip_capital::AllocationPlan`] and checks every destination against the
//! allocator's own per-venue limit. Two systems each holding a budget is two
//! systems each believing they own the same dollar.
//!
//! # The control around movement, without the movement (§37, §38.4)
//!
//! [`destination`], [`corridor`] and [`gate`] are the deterministic half of
//! the blueprint's treasury design, built as ADR 0021 permits it and no
//! further: an allowlist of destinations with who approved each and when, a
//! corridor lifecycle whose transition table refuses every edge not on it, and
//! a seven-check veto-only [`gate::TransferGate`] that returns a
//! [`gate::Approved`] carrying no way to execute. There is no transfer engine behind the gate,
//! no signing, no withdrawal and no call out of the process. That is not a
//! stub awaiting an engine — a gate that refuses is the control, and the
//! control is the half worth having.
//!
//! # Determinism, and the log as the only record
//!
//! Nothing here reads a wall clock or draws a random number. Every entry point
//! takes the [`qip_core::Timestamp`] it is reasoning about, and the single
//! stochastic operation in the crate — [`RealisedDemand::sample`], for
//! backtests — takes a seeded [`qip_core::Xoshiro256`] as an argument. The
//! planner itself is entirely deterministic, so a replay reproduces the same
//! transfers and the same refusals.
//!
//! [`journal`] makes that replay real for the control half: every
//! destination, corridor, wallet and gate decision is written to the
//! hash-chained event log as the command and its outcome, and [`replay`]
//! rebuilds the state from those records alone — re-running each command and
//! refusing, by position, a record that is out of sequence, altered, or
//! claims an outcome the control does not produce.
//!
//! # Deciding one transfer
//!
//! ```
//! use qip_capital::{AllocationLimits, CapitalAllocator, DrawdownSchedule};
//! use qip_capital_fabric::{
//!     CapitalLocation, DemandForecast, DemandKind, FundingCurve, FxRates, Interval,
//!     LocationBalance, PrePositioningPlanner, PrePositioningRequest, Region,
//!     SettlementCalendar, SettlementConvention, TransferCostModel,
//! };
//! use qip_contracts::venue::VenueId;
//! use qip_core::{Currency, Decimal, Duration, Timestamp, dec};
//! use qip_financial::costs::{LiquidityProfile, TransactionCostModel};
//!
//! # fn main() -> qip_core::error::Result<()> {
//! // A Thursday, well inside the cut-off.
//! let now = Timestamp::from_civil(2024, 3, 7).saturating_add(Duration::from_hours(9));
//!
//! let allocator = CapitalAllocator::new(
//!     AllocationLimits::new(
//!         dec!("100000000"),
//!         dec!("30000000"),
//!         dec!("60000000"),
//!         dec!("90000000"),
//!     )?,
//!     DrawdownSchedule::default(),
//! );
//! let planner = PrePositioningPlanner::new(
//!     allocator,
//!     TransferCostModel::new(
//!         TransactionCostModel::listed(1.0),
//!         LiquidityProfile::listed(Decimal::from_int(5_000_000_000_000), 1.0),
//!         FundingCurve::flat(400.0)?,
//!         dec!("25"),
//!         300.0,
//!     )?,
//!     SettlementCalendar::weekday(SettlementConvention::T1)?,
//! );
//!
//! let tokyo = CapitalLocation::new(Region::new("apac"), Currency::JPY, VenueId::new("XTKS"));
//! let treasury =
//!     CapitalLocation::new(Region::new("namr"), Currency::USD, VenueId::new("TREASURY"));
//! let rates = FxRates::new(Currency::USD).with_rate(Currency::JPY, dec!("0.0067"))?;
//!
//! // Margin is contractual: a shortfall is somebody else selling the book.
//! let forecast = DemandForecast::new(
//!     tokyo.clone(),
//!     DemandKind::Margin,
//!     now,
//!     Duration::from_days(3),
//!     Interval::new(dec!("400000000"), dec!("500000000"), dec!("600000000"), 0.80)?,
//!     40,
//! )?;
//!
//! let request = PrePositioningRequest::new(treasury, dec!("20000000"), rates)?
//!     .with_balance(LocationBalance::new(
//!         tokyo.clone(),
//!         DemandKind::Margin,
//!         dec!("100000000"),
//!     )?)
//!     .with_forecast(forecast);
//!
//! // The live allocation: nothing drawn down, nothing yet committed.
//! let live = planner.allocator().allocate(&[], 0.0, now)?;
//! let plan = planner.plan(&request, &live, now)?;
//!
//! // The gap the forecaster is confident about is 300m yen, plus the buffer the
//! // margin asymmetry buys on top of it.
//! assert_eq!(plan.moves.len(), 1);
//! assert!(plan.moved_into(&tokyo, DemandKind::Margin) >= dec!("300000000"));
//! assert!(plan.is_within_budget());
//! # Ok(())
//! # }
//! ```

pub mod corridor;
pub mod custody;
pub mod destination;
pub mod evaluate;
pub mod forecast;
pub mod gate;
pub mod journal;
pub mod location;
pub mod plan;
pub mod replay;
pub mod settlement;
pub mod transfer;
pub mod wallet;

pub use evaluate::{LaneOutcome, PlanScore, RealisedDemand, evaluate};
pub use forecast::{DemandForecast, DemandForecaster, DemandKind, DemandObservation, Interval};
pub use location::{CapitalLocation, Region};
pub use plan::{
    LaneContext, LocationBalance, PrePositionMove, PrePositioningPlan, PrePositioningPlanner,
    PrePositioningRequest, Refusal, RefusalReason,
};
pub use settlement::{SettlementCalendar, SettlementConvention, SettlementQuote};
pub use transfer::{FundingCurve, FxRates, ShortfallAsymmetry, TransferCost, TransferCostModel};
