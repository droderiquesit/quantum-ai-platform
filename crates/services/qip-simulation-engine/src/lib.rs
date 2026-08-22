//! `qip-simulation-engine` — SIMULATE.
//!
//! Backtesting, Monte Carlo, and the scenario library.
//!
//! The design commitment is that **look-ahead is unrepresentable**. A strategy
//! reads market data only through a [`clock::PointInTimeView`], which borrows
//! from the clock and filters every read against the current instant. Reading
//! tomorrow's price is not something a strategy has to remember not to do; it
//! is something there is no method for. Bars are keyed on close time, so a
//! daily bar stamped with today's date does not exist until the session ends.
//!
//! Three further things this refuses to let a backtest hide:
//!
//! * **The execution assumption.** A decision at time `t` executes at
//!   `t + decision_lag`, and a run that permits same-bar fills says so in its
//!   own caveats. That single assumption is the difference between most
//!   published backtests and reality.
//! * **The cost of being wrong about liquidity.** The impact model refuses to
//!   price an order beyond the participation it was calibrated for, and the
//!   rejected orders appear in the result rather than being silently filled.
//! * **The search.** [`validation::deflated_sharpe`] discounts a result for how
//!   many strategies were tried, and [`validation::PurgedSplit`] builds folds
//!   that do not leak a holding period's worth of the test set into training.

pub mod backtest;
pub mod clock;
pub mod costs;
pub mod montecarlo;
pub mod scenario;
pub mod validation;

pub use backtest::{
    BacktestConfig, BacktestResult, BacktestStrategy, Backtester, RejectedOrder, SimulatedFill,
};
pub use clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
pub use costs::{CostModel, TradeCost, Unfillable};
pub use montecarlo::{Distribution, Generator, MonteCarlo};
pub use scenario::{
    FactorExposure, FactorShock, Scenario, ScenarioResult, StressTester, standard_library,
};
pub use validation::{
    DeflatedSharpe, OverfittingReport, PurgedSplit, Split, WalkForward, assess_overfitting,
    deflated_sharpe,
};
