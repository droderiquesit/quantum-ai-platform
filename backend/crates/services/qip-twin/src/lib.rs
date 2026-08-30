//! `qip-twin` — the counterfactual digital twin, and the outcome capture behind it.
//!
//! The platform can say what it earned. The audit's finding was that it could
//! not say what it *declined* and what that would have earned, and a decision
//! system that only records its trades cannot tell a well-calibrated gate from
//! a gate that is simply shut. This crate is the record that closes that gap:
//! every order, fill, cancel, rejection, missed opportunity and risk ruling,
//! each carrying a trace id, and for each of them the nine things the platform
//! could have done instead, priced.
//!
//! Pricing an alternative is where a system like this normally starts lying to
//! itself, in three specific ways. All three are answered with structure rather
//! than with discipline.
//!
//! ## A counterfactual can never be reported as an actual
//!
//! [`capture::RealisedOutcome`] and [`counterfactual::SimulatedOutcome`] are
//! unrelated types with no conversion between them, and every money figure a
//! simulation produces is wrapped in [`value::Simulated`], which has no
//! accessor returning the value it holds. `Decimal + Simulated<Decimal>` does
//! not exist and — by Rust's orphan rules — cannot be added by any crate but
//! this one. The taint also survives serialization, so a simulated P&L will not
//! deserialize into a real one.
//!
//! ```compile_fail
//! # use qip_core::Decimal;
//! # use qip_twin::value::Simulated;
//! let realised = Decimal::from_int(100);
//! let hypothetical = Simulated::of(Decimal::from_int(40));
//! // There is no `Add<Simulated<Decimal>> for Decimal`, and there is no way to
//! // unwrap the right-hand side. A quarter's P&L cannot absorb an alternative
//! // the platform did not take.
//! let flattering_total = realised + hypothetical;
//! ```
//!
//! Adding two simulated figures together is fine, because the result is still
//! simulated:
//!
//! ```
//! # use qip_core::Decimal;
//! # use qip_twin::value::Simulated;
//! let a = Simulated::of(Decimal::from_int(40));
//! let b = Simulated::of(Decimal::from_int(2));
//! assert_eq!((a + b).as_f64_for_statistics(), 42.0);
//! ```
//!
//! And there is no conversion out:
//!
//! ```compile_fail
//! # use qip_core::Decimal;
//! # use qip_twin::value::Simulated;
//! let hypothetical = Simulated::of(Decimal::from_int(40));
//! let laundered: Decimal = hypothetical.into();
//! ```
//!
//! ## A counterfactual is evaluated against data available at decision time
//!
//! An alternative is fixed in two phases. [`counterfactual::CounterfactualEngine::plan`]
//! sees only an [`asof::DecisionView`], on which **no accessor takes a
//! timestamp** — every read is as of the view's own instant, so there is no
//! call that reaches a price which had not printed.
//! [`counterfactual::CounterfactualEngine::settle`] sees the realised path but
//! receives the plan already fixed, so nothing it reads can change what the
//! alternative was. Choosing the size after seeing the move is not a mistake to
//! catch in review; the two functions are separated by a value.
//!
//! The view is also exclusive. [`asof::TwinMarket::view_at`] takes `&mut self`,
//! so holding a view of the decision instant and a view of the horizon at the
//! same time does not compile:
//!
//! ```compile_fail
//! # use qip_core::Timestamp;
//! # use qip_twin::asof::TwinMarket;
//! fn two_instants_at_once(market: &mut TwinMarket) {
//!     let at_the_decision = market.view_at(Timestamp::from_secs(1_000)).ok();
//!     let at_the_horizon = market.view_at(Timestamp::from_secs(2_000)).ok();
//!     let _ = (at_the_decision, at_the_horizon);
//! }
//! ```
//!
//! One at a time is what the twin actually needs, and that compiles:
//!
//! ```
//! # use qip_core::Timestamp;
//! # use qip_twin::asof::TwinMarket;
//! fn one_instant_then_the_next(market: &mut TwinMarket) {
//!     let at_the_decision = market.view_at(Timestamp::from_secs(1_000)).ok();
//!     drop(at_the_decision);
//!     let at_the_horizon = market.view_at(Timestamp::from_secs(2_000)).ok();
//!     let _ = at_the_horizon;
//! }
//! ```
//!
//! Where a caller could still get it wrong — handing the engine a view of a
//! later instant than the decision — the engine refuses at runtime with an
//! error that names the leak.
//!
//! ## The fill model is the simulator's
//!
//! Counterfactual prices come from
//! [`qip_simulation_engine::clock::SimulationClock::fill_price`] and
//! counterfactual costs from [`qip_simulation_engine::costs::CostModel`],
//! including its refusal to price an order beyond the participation the
//! square-root impact law was calibrated for. There is no second fill model in
//! this crate. A more generous one would make every alternative beat the action
//! taken, which is the failure mode that turns a counterfactual twin into a
//! machine for regret nobody should feel.

pub mod asof;
pub mod capture;
pub mod counterfactual;
pub mod record;
pub mod regret;
pub mod value;

pub use asof::{DecisionView, Liquidity, TwinMarket};
pub use capture::{
    Action, CapturedOutcome, Decision, OutcomeCapture, RealisedOutcome, TraceChain, TraceLink,
};
pub use counterfactual::{
    ActualTrade, Alternative, AlternativeMenu, Counterfactual, CounterfactualEngine,
    CounterfactualPlan, CounterfactualSet, SimulatedFill, SimulatedOutcome, VenueOption,
};
pub use record::{
    AgentOutput, CounterfactualSummary, FillSummary, LearningRecord, MarketOutcome, MarketState,
    RiskSummary, WorldState,
};
pub use regret::{AlternativeRegret, RegretAnalysis};
pub use value::Simulated;
