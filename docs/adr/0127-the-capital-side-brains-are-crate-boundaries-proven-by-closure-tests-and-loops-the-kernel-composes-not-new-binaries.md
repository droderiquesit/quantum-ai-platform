# ADR 0127: The capital-side brains are crate boundaries proven by closure tests, and loops the kernel composes, not new binaries

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It moves code between in-tree crates
  and admits no third-party crate, no binary, no service and no paid
  resource.
- **Date**: 2026-10-18
- **Extends**: `.claude/rules/architecture/00-boundaries.md` (dependencies
  point inward; `qip-kernel` is the only place that composes services into a
  cycle), and `qip-acceptance`'s `survival_independence.rs` (R12-CAPITAL-7204,
  R12-RISK-7205).
- **Keeps**: ADR 0008 (cells decide alone), ADR 0024 (three central Cloud Run
  binaries plus `qip-ledgerd`), ADR 0123 (the ledger's operation topic).

## Context

The blueprint names six capital-side domains as "first-class autonomous
domains with explicit contracts": the Asset Brain, the Capital Brain, the
Risk Brain, the Treasury Bank, the Ledger Brain and the Settlement Brain
(R12-CAPITAL-1095 to 1100), and asks that Asset and Treasury be separate
(R12-COVERAGE-7157) and that no alpha component can construct, relax or
bypass a survival decision (R12-CAPITAL-7204). In the tree today:

- Treasury is a module of `qip-capital` (`grep -n 'pub mod treasury' backend/crates/services/qip-capital/src/lib.rs`),
  beside `bank`, `funding`, `reservation`, `survival` and thirteen others;
  `qip-capital-fabric` (corridors, custody, settlement projection) depends on
  `qip-capital`.
- The Asset Brain has no home of its own: holdings and lifecycle live in
  `qip-portfolio-engine` (which depends on `qip-optimization-engine`) and
  `qip-lifecycle`.
- The Risk Brain is `qip-risk` (lib) and `qip-risk-engine` (service).
- The Ledger Brain is the `qip-ledgerd` binary over `qip_contracts::ledger`
  and `ledger_operation`.
- The Settlement Brain is split between `qip-capital-fabric`'s settlement
  projection and the cell's gate in `qip-edge/src/settlement.rs`.
- The survival kernel is three modules of the runtime, `qip-kernel/src/survival.rs`,
  `confidence.rs` and `hedge.rs`. `survival_independence.rs` proves by
  closure that `qip-capital`, `qip-risk` and `qip-risk-engine` cannot reach
  an alpha crate, but for the kernel modules it can only scan their `use`
  lines, because a closure test cannot separate one module of `qip-kernel`
  from the rest of it. `survival.rs` and `hedge.rs` import
  `crate::platform::StressReport`, and `confidence.rs` imports
  `qip_cost_router`.

"Autonomous loop" in the rows has two possible readings: its own process, or
its own decision step with its own contract. The first would add binaries the
ADR 0024 catalogue has refused to add on cost (R12-ARCH-9012 is an owner
question for exactly that reason) and would put a network hop between the
cycle and its limits. The second is what the boundary rules already allow.

## Decision

1. **A brain is a crate boundary with a typed contract and a closure test,
   and its loop is a step the kernel composes.** Each brain owns a crate (or
   a named set of crates) whose public types are its contract; the kernel
   calls its step in the stage that owns the decision; no brain gets a
   binary, a thread or a network hop by this record. "Autonomous" means no
   other brain can construct, relax or bypass its decision, and that is
   proven by dependency closure, not by process isolation.
2. **The six homes**, each named so a closure test can address it:
   - **Capital Brain**: `qip-capital`, less `treasury`, `bank` and
     `funding`; a closure test (R12-CAPITAL-1096).
   - **Treasury Bank**: a new `services/qip-treasury` extracted from
     `qip-capital::{treasury, bank, funding}`. `qip-capital-fabric`'s
     corridors and custody depend on it; it depends on no capital-brain
     module except through `qip-contracts` types (R12-COVERAGE-7157,
     R12-CAPITAL-1098).
   - **Asset Brain**: `qip-portfolio-engine` and `qip-lifecycle`, with the
     holding and lifecycle contracts they expose; no extraction, a closure
     test (R12-CAPITAL-1095).
   - **Risk Brain**: `qip-risk` and `qip-risk-engine`, unchanged
     (R12-CAPITAL-1097).
   - **Ledger Brain**: `qip-ledgerd` over `qip_contracts::ledger` and
     `ledger_operation` — postings, balances and operations are the
     contract, ADR 0123's topic is its inbox (R12-CAPITAL-1099).
   - **Settlement Brain**: `qip-capital-fabric`'s settlement module and
     `qip-edge`'s settlement gate, sharing `qip_contracts` settlement terms;
     the two halves stay where they are because one runs in the cell and one
     in the centre, and a crate that held both would put the cell's closure
     behind the centre's (R12-CAPITAL-1100).
3. **The survival kernel leaves the runtime** (R12-CAPITAL-7204).
   `survival.rs`, `confidence.rs` and `hedge.rs` move to a new
   `services/qip-survival`. `StressReport` moves with them (it is their
   input, and the platform builds it); `confidence.rs`'s use of
   `qip_cost_router` is replaced by the measurement types it actually reads,
   or the cost router's types move to `qip-contracts`, whichever the slice
   finds smaller. `qip-survival` may depend on libs, `qip-capital` and
   `qip-risk` and on nothing that generates alpha; the kernel composes it.
   `survival_independence.rs`'s closure test then covers it by closure,
   and its source scan of the kernel modules is deleted in the same commit
   rather than left asserting a property of files that no longer exist.
4. **The order is treasury, then the survival kernel, then closure tests for
   the four brains that need no move.** Treasury first because it is the one
   the register names twice (COVERAGE-7157, CAPITAL-1098) and its extraction
   has no dependency on the others; the survival kernel second because it is
   the one whose property is currently only a source scan.
5. **The acceptance test for a brain** asserts three things: its crates'
   closure reaches no alpha crate (the list `survival_independence.rs`
   already keeps); no other brain's crate depends on it except through
   `qip-contracts`; and the kernel is the only crate depending on two
   brains at once.

## Consequences

- Each brain's independence becomes a dependency-graph fact a person can
  check with `cargo tree`, which is the evidence the rows ask for.
- `qip-capital` shrinks and a new `qip-treasury` and `qip-survival` appear;
  the workspace gains two crates, so any count of crates quoted elsewhere
  goes stale on the day each lands.
- Code slices left: the `qip-treasury` extraction and its arch test
  (COVERAGE-7157, CAPITAL-1098); `qip-survival` and the closure-based
  replacement in `survival_independence.rs` (CAPITAL-7204); a per-brain
  closure test for capital, asset, risk, ledger and settlement
  (CAPITAL-1096, 1095, 1097, 1099, 1100). None changes behaviour; each must show
  the workspace suite unchanged before and after.

## What it costs

- Two crate extractions touch many `use` lines across the kernel and
  `qip-capital-fabric`, in files parallel lanes are editing; each is a
  merge-conflict surface for a day.
- Moving `StressReport` out of `platform.rs` changes a type the platform's
  public API returns; callers in tests and apps follow it.
- "Autonomous loop" is read as a composed step, not a process. A reader of
  the blueprint who meant a process will find this record narrower than the
  sentence, and the owner question on separately deployed services
  (R12-ARCH-9012) stays open.

## What would make this wrong

- If a brain's step needed to run on a cadence the cycle cannot give it (for
  example a treasury sweep that must run while the cycle is halted), it needs
  its own loop, and that is a deployment decision, not a crate boundary.
- If the closure tests could be satisfied while a brain still consults alpha
  through a trait object handed in by the kernel, the property would hold in
  the graph and fail at run time; the test would then need to inspect
  constructor signatures, and this record's "proven by closure" would be
  too weak.

## Rows

R12-COVERAGE-7157, R12-CAPITAL-1095, R12-CAPITAL-1096, R12-CAPITAL-1097,
R12-CAPITAL-1098, R12-CAPITAL-1099, R12-CAPITAL-1100, R12-CAPITAL-7204. Each
stays PARTIAL until its slice lands with a mutated arch test.
