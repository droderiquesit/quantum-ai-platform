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
//! # Who calls [`families`], and what is still not wired
//!
//! [`families`] has a production caller. `qip-kernel`'s `central::structure`
//! builds one aligned series per strategy from the centre's own realised
//! corpus and clusters it, and `Platform::stage_learn` calls it every cycle
//! through `CentralPlane::family_structure`, journalling what it found in the
//! cycle entry. The corpus is `central::realised::RealisedSeries`: up to
//! `REALISED_SESSIONS` — 252 — daily sessions per `(cell, strategy)` of the
//! P&L the centre's own attribution booked, written from
//! `CentralPlane::ingest`, which `qip-api`'s mesh drains cell reports into.
//!
//! Two of the three things that stood in the way are gone, and the third is
//! not:
//!
//! * **The calendar can now be reconstructed.** It could not be, because
//!   [`StressCorrelation`] needs every series aligned to one calendar while
//!   `CentralPlane::live_outcomes` hands back a `Vec<f64>` with the day
//!   dropped — and, worse, a day on which a strategy settled nothing left no
//!   session at all, so "held a grant and made nothing" (a return of zero)
//!   was indistinguishable afterwards from "held no grant" (not a return).
//!   `RealisedSeries::retain_grant` now records the grant on its own, gated
//!   on the envelope being live at the report's instant, and
//!   `CentralPlane::realised_calendar` exposes the closed, granted days keyed
//!   by day. Neither an absence nor a zero is invented: a day the cell said
//!   nothing about still has no session.
//! * **The stress axis is the desk's own book.** The platform records no
//!   regime classification and no volatility index, so the caller cuts
//!   [`StressWindow::worst_quantile`] from the one series it holds — the
//!   desk's attributed daily return over its granted book — at the worst
//!   decile of 120 closed sessions. The argument for it, and the exceedance
//!   bias it carries, are in `central::structure`; the provenance string
//!   travels into [`StressCorrelation::provenance`] so the caveat arrives
//!   with the number.
//! * **Nothing consumes a family, and that is still true.**
//!   [`problem::PortfolioProblem`] is built per instrument in
//!   `qip-portfolio-engine`'s `construction`, and `qip-capital` sizes per
//!   strategy; no seam anywhere takes a [`FamilyAssignment`].
//!   `qip_lifecycle::StrategyFamily` is not that seam and must not be
//!   mistaken for it: it is a provenance key naming the sweep a strategy came
//!   from, fixed at enrolment, and `TrialBook` refuses to move a strategy
//!   between families precisely so a trial count cannot be laundered by
//!   renaming. A family recomputed from correlation every cycle cannot be
//!   that family. So the caller **measures and does not allocate**: it
//!   records the diagnostics and nothing acts on the membership. A decision
//!   keyed on a family nothing consumes would be a gate with no subject.
//!
//! One honest limit on all of that. The corpus records a day's *grant* only
//! where the centre holds a live envelope, and `CentralPlane::issue` has no
//! production caller — every call site is a test. Until a deployed process
//! issues a grant, the calendar is empty, `family_structure` returns `None`,
//! and the stage records nothing. The seam is wired and the thing upstream of
//! it is not yet fed.
//!
//! [`horizons`] now has one too, and it is a narrower one than [`families`]'.
//! `qip_lifecycle::horizon::HorizonAssurance` — attached to a
//! `LifecycleLedger` by whoever composes it — is consulted by
//! `LifecycleLedger::record_promotion` before a strategy may take a rung that
//! holds capital, and reaches this crate at
//! [`HorizonRegister::settle`]/[`HorizonRegister::settle_despite`],
//! [`FamilyBudget::from_money`], [`reconcile`] and
//! [`HorizonReconciliation::into_plan`]. That path is production code:
//! `qip-kernel`'s `central::factory` calls `qip_lifecycle::attempt_promotion`,
//! which calls `record_promotion`.
//!
//! **Two honest limits on that, because the row this closes is one a previous
//! scorecard got wrong in the optimistic direction.** First, the assurance is
//! optional in the same way `TrialBook` was before `aa66c5d`: a ledger with
//! none attached promotes exactly as it did before, and no composition root
//! attaches one yet, so in a deployment the gate is wired and unfed. Second,
//! [`family_horizons`] and [`family_horizons_settled`] still have no caller
//! outside tests — the lifecycle seam reconciles one budget per strategy
//! rather than per correlation family, because [`FamilyAssignment`] has no
//! constructor but [`FamilyClustering::cluster`] and the lifecycle crate holds
//! no correlation matrix. The blueprint's "horizon × family" cross is
//! therefore reconciled on the horizon axis and not yet on the family one.
//!
//! What did *not* exist before and does now is the seam where two claims about
//! one strategy's horizon disagree: [`HorizonRegister`] takes claims that name
//! their [`HorizonSource`], reports a [`HorizonDispute`] rather than letting
//! the last writer win, refuses to settle while one stands, and — where a desk
//! decides anyway — records the decision and the overruled claims in
//! [`SettledHorizons`] so nothing downstream can read the result as agreement.
//! [`CapitalPools`] still requires the total split four ways and still refuses
//! a split that does not sum exactly, and the four figures are the operator's
//! to state: the platform tracks one book — equity, the active reservations,
//! and the unfunded commitments that `Platform::deployable_capital` nets off —
//! and no division into inventory, deployable, unreserved and reserved is
//! derived anywhere. Nothing here infers one, because
//! `qip_financial::ladder::LiquidationHorizon` is a property of an instrument
//! rung rather than of a strategy, and mapping its seven rungs onto these four
//! buckets would assert a strategy attribute nobody measured.
//!
//! Both stages are complete, refusing, tested units. No family has been
//! evaluated on data from a deployment and none is claimed to have been — in
//! particular [`Diagnostics::pairs_calm_would_have_misfiled`], which is what
//! keying on stress rather than on the full sample costs in a population.
//! The caller now computes it every cycle the corpus supports one and puts it
//! in the log, so its value for this platform is answerable rather than
//! unknowable; on today's evidence it has been computed over test populations
//! only, because nothing deployed has issued a grant for a day to be retained
//! against.

pub mod families;
pub mod horizons;
pub mod problem;
pub mod router;

pub use families::{
    Diagnostics, FamilyAssignment, FamilyClustering, FamilyId, Linkage, StrategyReturns,
    StressCorrelation, StressWindow,
};
pub use horizons::{
    CapitalPools, FamilyBudget, Horizon, HorizonDispute, HorizonPosition, HorizonReconciliation,
    HorizonRegister, HorizonSource, ReconciledPlan, SettledHorizons, family_horizons,
    family_horizons_settled, reconcile,
};
pub use problem::{Objective, PortfolioConstraint, PortfolioProblem, QuboEncoding};
pub use router::{ComputeRouter, RoutingDecision, RoutingPolicy, Solver, SolverRun};
