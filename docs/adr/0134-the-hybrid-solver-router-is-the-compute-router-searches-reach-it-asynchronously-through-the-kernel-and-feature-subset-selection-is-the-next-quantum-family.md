# ADR 0134: The hybrid solver router is the compute router; searches reach it asynchronously through the kernel, and feature-subset selection is the next quantum family

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It uses the simulated quantum
  provider and the classical solvers already in the tree; it admits no solver
  crate, no accelerator, no hosted QPU and no credential.
- **Date**: 2026-10-18
- **Extends**: ADR 0006 (a classical baseline for every quantum result),
  the quantum foundry's family register (`qip-quantum/src/foundry.rs`).
- **Keeps**: `.claude/rules/architecture/00-boundaries.md` (a service does not
  reach into another service; the kernel composes), ADR 0099 C5 (Z3 refused),
  ADR 0122 (accelerators only in the offline research lane), ADR 0125 (an
  advisory result may narrow, never enlarge).

## Context

- **R12-RISK-9049** asks for hedge search "using the hybrid solver router"
  and for unwind plans. `qip_risk::hedge_search::search` scores candidates
  and reports `HedgeMeasures` with `liquidity_decay_days` and a boolean
  `unwind_feasible`; no staged plan exists, and the register found "no
  SolverRouter exists in the workspace". One does, under another name:
  `qip_optimization_engine::router::ComputeRouter`, which the kernel drives
  in `construct_from`, with `ComputeRouter::solve_classical` (exact
  enumeration, else annealing) as the baseline every quantum candidate is
  verified against, and `qip-kernel/src/compute_routing.rs` deciding which
  arms a deployment attaches.
- **R12-COVERAGE-7183** asks that quantum research assist symbolic or
  combinatorial search. `JobClass::HypothesisSearch` is admitted in the
  foundry and has no production caller. `qip-reasoning-engine` cannot call
  the router directly: it is a service, and services do not depend on each
  other.
- **R12-EXPAND-6036** asks that graph search, feature/ensemble selection and
  kernel/variational experiments produce experiment and benchmark records.
  `Foundry::platform` registers one family, `PortfolioAllocation`, with
  its baseline and threshold, and records every other `FamilyKind` as not
  built; the kernel's benchmark constant names that one family.

Locate them with
`grep -n 'pub struct ComputeRouter' backend/crates/services/qip-optimization-engine/src/router.rs`,
`grep -n 'pub fn search\|pub unwind_feasible' backend/crates/libs/qip-risk/src/hedge_search.rs`,
`grep -n 'pub enum JobClass\|pub enum FamilyKind\|pub fn platform' backend/crates/libs/qip-quantum/src/foundry.rs`.

## Decision

1. **The "hybrid solver router" is `ComputeRouter`.** No second router is
   built. Every search that may use a non-classical arm goes through it, so
   the arm selection, the classical verification and the benchmark record
   stay in one place, and ADR 0006 holds by construction rather than by each
   caller remembering it.
2. **Searches reach it through the kernel, asynchronously.** The kernel
   composes `hedge_search` (a lib) and `qip-reasoning-engine` (a service) with
   the router, as it composes everything else. A hedge search or a hypothesis
   search submits a job of its `JobClass`; the classical answer is computed in
   the same cycle and is what the cycle acts on; a non-classical candidate
   that returns later is verified classically and recorded, and may replace
   the classical answer only in a later cycle and only if it beats it by the
   family's declared threshold. Nothing waits on a quantum arm.
3. **A hypothesis-search result is advisory** (COVERAGE-7183). It may propose
   a candidate hypothesis or veto one under ADR 0125's two powers; a result
   without its classical validation is refused, not acted on.
4. **A hedge result carries an unwind plan** (RISK-9049). `HedgeMeasures`
   gains an `UnwindPlan`: per leg, the staged quantity per day at the allowed
   share of stressed volume, summing to the leg's quantity, from which
   `liquidity_decay_days` and `unwind_feasible` are then derived rather than
   computed separately. A plan whose stages do not sum to the position is
   refused at construction.
5. **The next foundry family is `FeatureEnsembleSelection`** (EXPAND-6036):
   choosing a feature subset as a QUBO, solved on the simulated provider,
   with the classical baseline a greedy forward selection scored on the same
   objective, a declared promotion threshold, and an experiment record per
   run (the row's `QuantumExperiment`, a type this slice adds; none exists). It is chosen first because its classical baseline is
   cheap and exact to state, its objective is already computed by the
   training lane, and it needs no dependency. Graph search and
   kernel/variational families stay recorded as not built; this record
   decides their order (graph search next) but not their encodings.

## Consequences

- One router holds every non-classical arm, so a benchmark comparing quantum
  and classical is always the same comparison, keyed by family.
- Code slices left: the kernel submitting hedge and hypothesis jobs and the
  later-cycle replacement rule, with a test that a candidate without
  classical validation is refused (COVERAGE-7183, RISK-9049); `UnwindPlan` in
  `hedge_search.rs` with tests that stages sum and that an infeasible plan
  reads infeasible (RISK-9049); the `FeatureEnsembleSelection` declaration,
  its encoding and baseline, with a `solvers.rs` test asserting the
  experiment record and the baseline (EXPAND-6036).

## What it costs

- A quantum candidate can only help a later cycle, never the one that asked;
  for a hedge in a fast market that may be too late to matter, and the
  record accepts that rather than putting a quantum arm on the cycle's path.
- More jobs on the simulated provider add CPU to the deep brain's budget; the
  existing per-sweep compute budget bounds it, and a family that exhausts it
  is skipped, not queued.
- A second family doubles the benchmark records a reader must interpret, and
  a family that never beats its baseline is still run and recorded until
  someone retires it.

## What would make this wrong

- If a family's classical baseline turned out not to be the strongest
  reasonable classical method (greedy selection beaten by an obvious
  alternative), every "quantum beat classical" record for it would be
  misleading; the baseline must then be strengthened before the family's
  results are read.
- If a search needed its result inside the cycle (a hedge that must be placed
  now), decision 2's asynchrony is wrong for it and that search should use
  the classical arm alone, not a synchronous quantum one.

## Rows

R12-COVERAGE-7183, R12-RISK-9049, R12-EXPAND-6036. Each stays PARTIAL until its
slice lands with a mutated test.
