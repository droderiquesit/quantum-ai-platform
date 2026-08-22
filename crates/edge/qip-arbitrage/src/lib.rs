//! `qip-arbitrage` — payoff paths across venues and instruments, priced
//! honestly enough to trade.
//!
//! The word the whole crate turns on is *executable*. Finding a set of prices
//! that multiply to more than one is arithmetic and takes a page. Deciding
//! whether that arithmetic survives contact with a book, a fee schedule, a
//! round trip and a venue that can revert your fill is the rest of it, and a
//! path that is profitable on mid prices and unprofitable on the actual book is
//! the default failure mode of every system that skips the difference.
//!
//! So the pipeline is deliberately a series of narrowings, each in its own
//! module, each able only to reject:
//!
//! * [`graph`] holds what converts into what, where, and at what quoted rate.
//!   Cross-venue, triangular and cross-instrument opportunities are all cycles
//!   in the one structure, so there is one search rather than three.
//! * [`search`] finds negative cycles in log space, where a product becomes a
//!   sum and Bellman-Ford applies. Its output is `f64` and is never a decision.
//! * The same module's `confirm_exact` re-multiplies the rates in
//!   [`qip_core::Decimal`] and discards what the floating point invented.
//! * [`pricing`] walks the real book at the real size through
//!   [`liquidity::LiquiditySource`], recording the touch and the sweep
//!   separately so spread and slippage stay distinguishable.
//! * [`netedge`] takes off all seven deductions and refuses any edge that
//!   skipped one.
//! * [`plan`] orders the legs least-reversible-first, works out what inventory
//!   has to be standing there beforehand, and refuses a plan whose residual
//!   exposure exceeds its budget.
//! * [`scan`] runs the lot and keeps the refusals.
//!
//! Two constraints shape all of it. Nothing reads a clock or an ambient random
//! source: `now` is a parameter everywhere it matters, so a replay produces the
//! same opportunities and the same refusals. And every decision is made in
//! exact arithmetic — `f64` appears for logarithms, volatility and scores, is
//! named `_f64` where it does, and never decides anything on its own.
//!
//! The order book is not a dependency. `qip-orderbook` is being built
//! concurrently, and [`liquidity::LiquiditySource`] is the seam it will be
//! wired in behind; [`liquidity::StaticLiquidity`] stands in until then.

mod arith;

pub mod graph;
pub mod liquidity;
pub mod netedge;
pub mod plan;
pub mod pricing;
pub mod scan;
pub mod search;

pub use graph::{
    ArbitrageGraph, ConversionEdge, EdgeKind, Node, PathKind, SyntheticComponent, VenueFacts,
};
pub use liquidity::{LiquiditySource, StaticLiquidity};
pub use netedge::{EdgeAssumptions, NetEdgeCalculator};
pub use plan::{LegPlanner, LegRanking, PlanSettings, PlannedTrade};
pub use pricing::{PathLeg, PathPricing, PricedConversion, price_path};
pub use scan::{
    Opportunity, OpportunityScanner, Rejection, RejectionStage, ScanReport, SizePolicy,
};
pub use search::{
    ExactConfirmation, PathCandidate, SearchSettings, confirm_exact, search_candidates,
};
