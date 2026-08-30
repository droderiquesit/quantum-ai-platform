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
//!
//! # Putting a strategy through a market that is not behaving
//!
//! A backtest over bars answers whether a signal predicts. It cannot answer
//! whether the trade could have been done, because a bar has no depth, no
//! venue and no feed. [`market::MarketSimulator`] does: it holds a book with
//! price-time priority ([`venue::SimBook`]), executes orders against it
//! ([`execution::ExecutionReport`]), and lets any number of
//! [`conditions::MarketCondition`]s be injected on top — a flash event, a
//! depth collapse, a crossed market, a delayed or malformed feed, latency and
//! its spikes, and a venue that stops answering part way through an order.
//!
//! Two rules hold across all of it.
//!
//! * **Never more generous than reality.** Fills sweep the book and stop at
//!   the last published level; a fill can never exceed the depth showing; a
//!   crossed touch is charged at its worse side rather than read as free
//!   money. Where the simulator cannot tell, it takes the reading that costs
//!   the strategy money.
//! * **Determinism is the product.** No clock, no ambient RNG. Instants are
//!   parameters, draws come from a stream seeded on the run seed and the
//!   scope, and [`market::SimulationRun::digest`] makes "the same run" a thing
//!   a test compares byte for byte — including under injected chaos.

pub mod backtest;
pub mod clock;
pub mod conditions;
pub mod costs;
pub mod execution;
pub mod harness;
pub mod market;
pub mod montecarlo;
pub mod scenario;
pub mod validation;
pub mod venue;

pub use backtest::{
    BacktestConfig, BacktestResult, BacktestStrategy, Backtester, RejectedOrder, SimulatedFill,
};
pub use clock::{ExecutionAssumptions, PointInTimeView, SimulationClock};
pub use conditions::{ConditionSchedule, ConditionWindow, FeedFault, MarketCondition, Regime};
pub use costs::{CostModel, TradeCost, Unfillable};
pub use execution::{ExecutionPlan, ExecutionReport, FillSlice, FillStatus, PlanReport, SimOrder};
pub use market::{
    InstrumentSpec, MarketSimulator, MarketView, PriceSource, SimStrategy, SimulationRun,
    SyntheticMarket, mark_key,
};
pub use montecarlo::{Distribution, Generator, MonteCarlo};
pub use scenario::{
    FactorExposure, FactorShock, Scenario, ScenarioResult, StressTester, standard_library,
};
pub use validation::{
    DeflatedSharpe, OverfittingReport, PurgedSplit, Split, WalkForward, assess_overfitting,
    deflated_sharpe,
};
pub use venue::{
    BookCondition, ConsumedOrder, Mark, MarkSource, RestingOrder, SimBook, SimLevel, SweepOutcome,
    VenueHealth,
};
