# ADR 0388: The kernel holds the treasury and names the source of every planned placement, and an undeclared treasury permits no financing

- **Status**: **Accepted 2026-11-11 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Batch l1-d2-01 writes it.
  The limits of that delegation hold:
  - it sets no figure: no reserve floor, no leverage ceiling, no permission;
  - it touches no paper-trading layer and moves no money;
  - it changes no requirement's wording and reverses no accepted refusal;
  - it adds no third-party dependency, and deletes no test and no refusal.
- **Date**: 2026-11-11
- **Builds on**: ADR 0385 (the extraction of `services/qip-treasury`), whose
  file is not edited. It amends nothing in it: ADR 0385 says the treasury and
  the capital brain "meet in the kernel or not at all", and this is where they
  meet.
- **Rows**: R12-CAPITAL-1098, R12-COVERAGE-7157.

## Context

ADR 0385 extracted the Treasury Bank's books and the funding planner into
`qip-treasury`. The extraction made a fact visible that had been true all
along: no binary runs that code. Inside `qip-capital` it was linked and
called by nothing. As a crate of its own it is linked by nothing, and
`runtime_reachability::every_non_test_crate_is_reachable_from_an_app` fails
on it:

    crates no app links, directly or transitively: ["qip-treasury"]

An exemption in `NOT_A_RUNTIME_CRATE` would turn the gate green and keep the
fact. That list holds the test crate only.

The row's gap named two inputs nobody has given: a reserve floor and leverage
ceilings, which are figures and the owner's; and the central treasury's
available balance, which the kernel does not hold because the ledger is its
authority (ADR 0123).

The kernel does hold one funding question with every input present. Each
DECIDE builds a pre-positioning plan (`Platform::pre_position`): how much to
move and to which venue, inside a `budget` that is the allocator's
drawdown-adjusted headroom. The plan is journalled
(`PrePositioningPlanned`). Nothing says which source each unit comes from,
which is the question `qip_treasury::funding::plan_funding` answers
(CAPITAL-001).

## Decision

1. **The kernel holds the treasury.** `qip-kernel` depends on `qip-treasury`
   and `qip_kernel::treasury::Treasury` holds its leverage book and
   financing permissions, the reserve floor if one is declared, and the
   entity and jurisdiction a financing permission is keyed on. `Platform`
   holds one. No other crate gains an edge: `qip-capital` and
   `qip-capital-fabric` still cannot name `qip-treasury`, which
   `treasury_boundary.rs` already holds, and this is a runtime-to-service
   edge, so `SERVICE_TO_SERVICE_EDGES` does not grow.

2. **DECIDE asks it to name the source of every planned move.**
   `Platform::plan_pre_positioning` calls `Treasury::fund_plan`. One
   requirement per move, for the move's `amount_base`. One source: the
   central treasury's own capital, holding the plan's `budget`. The balance
   is taken from the plan, so the plan and its funding cannot disagree about
   it, and the kernel claims no treasury balance of its own. A move the
   treasury refuses is a stage problem. The stage sentence carries the
   funding.

3. **An undeclared treasury permits no financing and says why.** The
   platform is assembled with `Treasury::undeclared`. With no reserve floor
   declared, every financing source offered is left out before anything is
   summed, whatever permission or ceiling is recorded, and the funding names
   the absence (`TreasuryFunding::financing_withheld`). Borrowed money cannot
   be judged against a reserve nobody stated. This is fail-closed and it
   invents no figure.

4. **An absent floor is not a floor of zero.** The undeclared treasury
   applies no reserve floor to own capital and the funding says so in those
   words. Own capital stays bounded by the plan's budget, which is the limit
   that already governs it. Refusing every own-capital placement until a
   floor exists was considered and not taken: the plan is a paper record
   (ADR 0003, ADR 0021), the refusal would be reported on every cycle of
   every platform, and a problem raised on every cycle is one nobody reads.

5. **With a declaration, three refusals apply to a whole requirement.**
   `Treasury::declared` takes a `TreasuryDeclaration`: an optional floor,
   leverage decisions and permissions, all absent by default. Then a
   requirement that draws on financing must pass `LeverageBook::check` with
   the requirement as notional and its own-capital draws as capital; a
   requirement whose own-capital draws would breach the floor is refused by
   `CapitalBook::commit`; and an unpermitted source is dropped by the
   planner. A refused requirement is taken out and the rest are planned
   again, so its draws return to the sources. Nothing is trimmed to fit.

6. **No funding record is journalled.** The funding is a pure function of
   the journalled plan and the declaration the platform was assembled with.
   A second record would be a second claim about the same plan (decision 28,
   fourth preference).

7. **No financing source is offered, and that arm is declared absent.**
   Nothing in the tree reports a balance at a financing counterparty, so
   `fund_plan` offers own capital alone (decision 28, fifth preference). The
   leverage and permission refusals are held by test through
   `Treasury::fund`, and reach a deployed process only when a producer of
   such a balance exists.

## What stays with the owner

- The reserve floor, each leverage ceiling, and each financing permission.
  No composition root reads a declaration today, because there is none to
  read. When the owner gives one, it enters as a `TreasuryDeclaration` at a
  composition root, and how it is authenticated and journalled is a decision
  of that change.
- Whether own-capital placements should be refused until a floor is
  declared (decision 4 chose not to).

## Consequences

- `qip-treasury` is in the closure of every binary that links `qip-kernel`,
  and `plan_funding`, `LeverageBook::check`, `CapitalBook::commit` and
  `FinancingPermissions::permit` each have a production call site in
  `qip-kernel/src/treasury.rs`.
- The grant path (`RequirementKind::Grant`) is not composed. Capital grants
  to cells are sized in `central::regions`; naming their source needs the
  central treasury's balance, which the ledger holds and the kernel does
  not. R12-CAPITAL-1098 stays PARTIAL on that and on the absent
  declaration.

## What it costs

- One more edge out of the kernel, and `qip-treasury` in the closure of
  every binary that links it.
- One more clause in DECIDE's sentence on every cycle that forecasts a lane,
  saying each time that no financing is counted and no floor is applied. It
  is true each time, and it is long.
- A treasury in every deployed process that refuses all financing. Until the
  owner declares a floor, the leverage and permission paths run in tests
  only.
- The funding is recomputed from the plan by whoever wants it, not read from
  the log as a record of its own.

## Evidence

- `backend/crates/runtime/qip-kernel/src/treasury.rs`
- `backend/crates/runtime/qip-kernel/tests/treasury.rs`: five tests.
- `backend/crates/runtime/qip-kernel/tests/platform_outcomes.rs`
  `decide_names_the_source_of_every_planned_move_and_says_why_no_financing_is_counted`
- Mutations that fired, each restored byte for byte (`cmp`): the financing
  filter made to admit every source; the leverage check skipped; the floor
  commit skipped; the negative-floor refusal removed; the source balance
  doubled away from the plan's budget; the stage sentence dropped.

## What would make this wrong

- A producer of a financing balance appears and `fund_plan` still offers own
  capital alone: decision 7's absence would then be a silence.
- A declaration is read from somewhere a reviewer does not see, or a default
  is supplied for an absent field.
