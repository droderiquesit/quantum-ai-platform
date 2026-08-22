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

pub mod problem;
pub mod router;

pub use problem::{Objective, PortfolioConstraint, PortfolioProblem, QuboEncoding};
pub use router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver, SolverRun};
