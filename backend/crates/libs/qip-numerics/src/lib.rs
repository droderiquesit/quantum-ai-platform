//! `qip-numerics` — linear algebra, statistics and optimisation, implemented
//! in-tree so numerical behaviour is auditable and reproducible.
//!
//! Everything here is deterministic given its inputs: no threading, no
//! platform-dependent reductions, no reliance on FMA contraction. Two runs on
//! the same data produce identical bits, which is what makes backtests and risk
//! numbers comparable across time and across machines.

pub mod anneal;
pub mod distributions;
pub mod hmm;
pub mod interpolate;
pub mod lp;
pub mod matrix;
pub mod qp;
pub mod search;
pub mod stats;
pub mod vector;

pub use matrix::Matrix;
pub use vector::Vector;

/// Absolute tolerance used by default for convergence checks.
pub const DEFAULT_TOL: f64 = 1e-10;
/// Iteration ceiling for the iterative solvers.
pub const DEFAULT_MAX_ITERS: usize = 10_000;
