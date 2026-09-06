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
//! # Why neither stage has a production caller
//!
//! Neither does, and the reason recorded here was wrong. It said nothing in
//! this platform holds a per-strategy return corpus. Something does:
//! `qip-kernel`'s `central::realised::RealisedSeries` retains up to
//! `REALISED_SESSIONS` — 252 — daily sessions per `(cell, strategy)` of the
//! P&L the centre's own attribution booked, each over the gross limit of the
//! envelope the fills were made under, written from `CentralPlane::ingest`
//! by way of `record_realised` and read by the demotion monitor. That is a
//! corpus of realised returns from attributed fills, reproducible from the
//! event log, and an agent who believed the old sentence would set out to
//! build one while the one already in the tree went unused.
//!
//! Three things still stand between that corpus and a caller here, and each
//! is a gap in the platform rather than in these stages:
//!
//! * **The calendar cannot be reconstructed.** [`StressCorrelation`] needs
//!   every series aligned to one calendar, and the only exposure of the
//!   corpus — `CentralPlane::live_outcomes` — hands back a `Vec<f64>` with
//!   the day each return belongs to already dropped. Even with the days, a
//!   day on which a strategy settled nothing has no session at all, and the
//!   centre retains only the envelope it holds *now*, so "held a grant and
//!   made nothing" and "held no grant" are indistinguishable after the fact.
//!   The first is a return of zero and the second is not a return; filling
//!   the gap either way invents an observation, and refusing every strategy
//!   without a session on every day of the grid leaves nothing to cluster.
//!   Retaining the grant per session, beside the P&L already retained, is
//!   what would close this.
//! * **There is no stress axis.** [`StressWindow`] takes a benchmark series
//!   or an upstream classifier's verdict, and the platform records neither a
//!   stress classification nor a benchmark a window could be cut from.
//! * **Nothing consumes a family.** [`problem::PortfolioProblem`] is built
//!   per instrument in `qip-portfolio-engine`'s `construction`, and
//!   `qip-capital` sizes per strategy; no seam anywhere takes a
//!   [`FamilyAssignment`]. `qip_lifecycle::StrategyFamily` is not that seam
//!   and must not be mistaken for it: it is a provenance key naming the
//!   sweep a strategy came from, fixed at enrolment, and `TrialBook` refuses
//!   to move a strategy between families precisely so a trial count cannot
//!   be laundered by renaming. A family recomputed from correlation every
//!   cycle cannot be that family.
//!
//! [`horizons`] is further from a caller than [`families`], not nearer.
//! [`CapitalPools`] requires the total split four ways and refuses a split
//! that does not sum exactly; the platform tracks one book — equity, the
//! active reservations, and the unfunded commitments that
//! `Platform::deployable_capital` nets off — and no division into inventory,
//! deployable, unreserved and reserved exists to read. [`family_horizons`]
//! requires a [`Horizon`] per strategy, and nothing records one;
//! `qip_financial::ladder::LiquidationHorizon` is a property of an instrument
//! rung rather than of a strategy, and mapping its seven rungs onto these
//! four buckets would assert a strategy attribute nobody measured.
//!
//! Both stages are complete, refusing, tested units. No family has been
//! evaluated on real data and none is claimed to have been — in particular
//! [`Diagnostics::pairs_calm_would_have_misfiled`], which is what keying on
//! stress rather than on the full sample costs in this population, has never
//! been computed over a real one, and its value for this platform is
//! unknown rather than small.

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
