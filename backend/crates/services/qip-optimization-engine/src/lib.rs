//! `qip-optimization-engine` — DECIDE.
//!
//! Portfolio optimisation, and the routing between classical and quantum
//! solvers.
//!
//! The rule this crate enforces is that **no quantum result is used without a
//! classical baseline solved on the same problem, and a tie goes to the
//! classical solver**. [`router::ComputeRouter::solve`] always runs the
//! classical path, runs the quantum path only where the problem's structure
//! could justify it, and returns the quantum answer only when it is feasible
//! and better by more than a stated margin. The
//! [`router::RoutingDecision`] carries every run, so a claim of advantage
//! arrives with its evidence or not at all.
//!
//! Two supporting decisions make that comparison honest:
//!
//! * Both paths express the *same* [`problem::PortfolioProblem`]. A quantum
//!   result on a slightly different formulation would be evidence of nothing.
//! * Every candidate is scored against the real problem, including the
//!   constraints its own relaxation dropped. A QUBO answer that ignored the
//!   budget constraint is measured against it anyway and comes back
//!   infeasible, rather than winning on an objective it was not entitled to.
//!
//! The default is classical. A deployment with no quantum backend behaves
//! identically to one whose backend is unreachable.
//!
//! # The decomposition around it
//!
//! The router solves one allocation problem. Blueprint §23.1 decomposes the
//! full job into three levels, and two of them live here beside it:
//!
//! * [`families`] — LEVEL 1. Clusters strategies into families, keyed on
//!   correlation measured inside a stated stress window. The blueprint's own
//!   note on the capability is that calm-market correlation understates stress
//!   correlation, so a family drawn on the full sample separates two
//!   strategies that are one bet in a drawdown. There is no calm fallback:
//!   without a usable stress window the stage refuses.
//! * [`horizons`] — §23.4. Reconciles family budgets against the four capital
//!   pools, so a multi-year commitment is never funded out of the inventory a
//!   market maker needs by the close.
//!
//! LEVEL 2 — allocating across the families — is the problem
//! [`problem::PortfolioProblem`] and the router already express, with each
//! family standing where an asset stands today. LEVEL 3, distributing a
//! family's budget across its members by capacity, is not built.
//!
//! Neither new stage has a production caller. Nothing in this platform holds a
//! per-strategy return corpus to cluster, so no family has been evaluated on
//! real data and none is claimed to have been. Both are complete, refusing,
//! tested units waiting for the population they operate on.

pub mod families;
pub mod horizons;
pub mod problem;
pub mod router;

pub use families::{
    Diagnostics, FamilyAssignment, FamilyClustering, FamilyId, Linkage, StrategyReturns,
    StressCorrelation, StressWindow,
};
pub use horizons::{
    CapitalPools, FamilyBudget, Horizon, HorizonPosition, HorizonReconciliation, ReconciledPlan,
    family_horizons, reconcile,
};
pub use problem::{Objective, PortfolioConstraint, PortfolioProblem, QuboEncoding};
pub use router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver, SolverRun};
