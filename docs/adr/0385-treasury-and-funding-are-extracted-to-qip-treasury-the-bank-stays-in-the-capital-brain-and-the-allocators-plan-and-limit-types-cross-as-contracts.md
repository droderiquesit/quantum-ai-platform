# ADR 0385: Treasury and funding are extracted to `qip-treasury`, the bank stays in the capital brain, and the allocator's plan and limit types cross as contracts

- **Status**: **Accepted 2026-11-11 under the owner's 2026-11-11 instruction
  to the orchestrator** that every in-repository design decision be made so
  that no register row waits on one. Batch l1-d1-01 writes it on the
  orchestrator's ruling. The limits of that instruction hold:
  - it sets no figure: no limit, threshold, rate or cost;
  - it touches no paper-trading layer and adds no order path;
  - it adds no dependency and no crate edge; it removes one;
  - it moves no behaviour into `qip-contracts`.
- **Date**: 2026-11-11
- **Amends**: ADR 0127, decision 2, the Capital Brain and Treasury Bank
  bullets only. ADR 0127's file is not edited. Every other clause of ADR 0127
  stands, decision 5 included, and this record is what makes decision 5 true
  of the Capital Brain.
- **Rows**: R12-CAPITAL-1098, R12-COVERAGE-7157, R12-CAPITAL-1096.

## Context

ADR 0127 decision 2 placed the Treasury Bank in a new `services/qip-treasury`
"extracted from `qip-capital::{treasury, bank, funding}`", and said it
"depends on no capital-brain module except through `qip-contracts` types".
Batch l1-c1-01 read the three files on 2026-11-11 and found the two halves of
that sentence cannot both be built:

- `treasury.rs` names no other `qip-capital` module. `funding.rs` names only
  `treasury`. Those two satisfy the sentence as written.
- `bank.rs` uses four capital-brain types directly: `collateral::Pledge`,
  `dated_ladder::{DatedLadder, EntryKind, LadderEntry}`,
  `reservation::{ReservationLedger, MAXIMUM_RESERVATION_VALIDITY}` and
  `survival::SurvivalReport`. `ReservationLedger` is the thing that makes
  passing a capital check and holding the capital one operation;
  `DatedLadder` decides when a balance may be read. Neither is a record. For
  the bank to move, either those two move to `qip-contracts`, or
  `qip-treasury` depends on `qip-capital`.

The same reading found a second place where ADR 0127 expected something the
code does not have. Decision 2 says "`qip-capital-fabric`'s corridors and
custody depend on" the treasury crate, and R12-CAPITAL-1096's closure test
was left pinning one open edge: `qip-capital-fabric`, the Settlement Brain's
crate, depends on `qip-capital` directly. That edge was never about
treasury. `PrePositioningPlanner` held a `CapitalAllocator` and took an
`AllocationPlan`, and what it read from them was the per-venue limit and the
room the live plan leaves. Nothing in the fabric names a treasury type.

## Decision

1. **`services/qip-treasury` is extracted from `qip-capital::{treasury,
   funding}` and from nothing else.** It depends on `qip-core` and `serde`.
   It depends on no other workspace crate, in any dependency section, and
   `qip-capital` does not depend on it. Treasury needs nothing from the
   capital brain today; if it comes to need something, that crosses as a
   `qip-contracts` data type named by a new ADR.

2. **`bank.rs` stays in `qip-capital`.** `qip-contracts` holds data types.
   Its own crate doc says why: a contract that carries behaviour is a place
   where two subsystems disagree about semantics. `ReservationLedger` and
   `DatedLadder` carry behaviour, so they do not move, and so the bank, which
   drives both, stays with them. ADR 0127 decision 2's Capital Brain bullet is
   amended from "`qip-capital`, less `treasury`, `bank` and `funding`" to
   "`qip-capital`, less `treasury` and `funding`", and its Treasury Bank
   bullet from "`{treasury, bank, funding}`" to "`{treasury, funding}`".

   The alternative, giving `qip-treasury` an edge to `qip-capital`, was
   refused. Treasury answers whether a unit exists to be promised and the
   capital brain answers who is promised it. A treasury that could read the
   allocator could be made to agree with it, and the reserve floor would then
   be a number the thing it bounds can see.

3. **The allocator's limits and its plan are data and move to
   `qip_contracts::allocation`: `AllocationLimits`, `Allocation` and
   `AllocationPlan`.** What moved is what an operator set, what the allocator
   produced, and reads of their own fields: a lookup, a sum, a comparison of
   two sums. What did not move is every decision. `CapitalAllocator`,
   `DrawdownSchedule` and `StrategyProposal` stay in `qip-capital`, so a
   crate holding only `qip-contracts` can read a plan and cannot produce one.
   `qip_capital::allocation` re-exports the three names, so the allocator's
   callers keep one path.

4. **The fabric's direct edge to `qip-capital` is removed, not admitted.**
   `PrePositioningPlanner` now holds `AllocationLimits`, not a
   `CapitalAllocator`; `allocator()` is replaced by `limits()`. The kernel
   hands it the limits. `qip-capital` remains a dev-dependency of the fabric
   so its tests bound the planner by a plan the real allocator produced;
   cargo does not give a dev-dependency to a library target, so fabric code
   cannot name it. ADR 0127 decision 2's phrase "`qip-capital-fabric`'s
   corridors and custody depend on it" is withdrawn: they depend on neither
   crate.

5. **Nothing below the kernel depends on `qip-treasury`.** It is listed as a
   brain in `brain_closure.rs`, so a service holding it beside another brain
   fails the two-brains test.

## What holds it

- `qip-acceptance/tests/treasury_boundary.rs`, a new suite (`architecture.rs`
  is protected and is not edited):
  - `the_treasury_crate_depends_on_qip_core_alone_and_reaches_no_capital_brain_crate`
    pins the dependency set exactly, dev and target sections included;
  - `the_capital_brain_cannot_reach_the_treasury_and_nothing_below_the_kernel_depends_on_it`;
  - `neither_crates_source_names_the_other_and_the_bank_stays_in_the_capital_brain`;
  - `the_manifest_reader_sees_dev_target_table_and_renamed_dependencies`,
    the instrument's own test.
- `brain_closure.rs`:
  `the_capital_brain_reaches_no_alpha_and_no_other_brain_and_nothing_below_the_kernel_depends_on_it`
  asserts the dependent set is empty. It previously asserted
  `{qip-capital-fabric}` and said the edge's removal was the cue to tighten.

## Consequences

- The workspace gains one crate. Any count of crates quoted elsewhere is
  stale from this commit; `CLAUDE.md` says to recount, not to quote.
- ADR 0127 decision 5 holds for the Capital Brain in all three clauses.
- The Treasury Bank has a crate, a typed contract, contract tests and a
  boundary test. It has no composition root. See the next section.

## What it costs

- Three assertions in the protected guard suite `architecture.rs` must
  follow the code: the money-authority classification gains `qip-treasury`;
  `CAPITAL_BRAIN_MODULES` and `CAPITAL_STATE_WRITERS` lose the names that
  left the crate; `SERVICE_TO_SERVICE_EDGES` loses the fabric's edge. A lane
  may not edit that file, so the change ships as
  `docs/blueprint/lanes/l1-d1-01-R12-CAPITAL-1098-architecture.patch` and
  the suite fails on three tests until integration applies it.
- `PrePositioningPlanner::new` changes its first argument and `allocator()`
  becomes `limits()`. One kernel call site and the fabric's own tests
  follow; no other caller exists.
- Two module-level writer checks in `architecture.rs` become a crate
  boundary. That is stronger, and it is also held by a different file, so a
  reader of `architecture.rs` alone sees a shorter list and must follow the
  comment to `treasury_boundary.rs`.
- The bank and the treasury now live in different crates though the
  blueprint names them together. A reader looking for "Treasury Bank" finds
  two homes and this record explaining why.

## What this does not decide

- **The kernel composes no treasury step.** `plan_funding`, `CapitalBook`,
  `LeverageBook`, `InternalFunding` and `FinancingPermissions` had no
  production caller inside `qip-capital` and have none now. Composing them
  needs two things this record may not supply: a reserve floor and leverage
  ceilings, which are figures a person gives; and the central treasury's
  available balance, which the kernel does not hold because the ledger is
  its authority (ADR 0123). R12-CAPITAL-1098 stays PARTIAL on exactly that.
- Whether the bank should one day be split so that its records move and its
  use of the reservation ledger stays. That would be a third ADR.

## What would make this wrong

- If a later reading shows `AllocationLimits` or `AllocationPlan` must carry
  a decision (for example a limit that changes with the drawdown), that
  logic belongs in `qip-capital` and the contract type must stay a record of
  its result. A method added to `qip_contracts::allocation` that takes
  anything but `&self` and a key is the sign.
- If treasury comes to need the reservation ledger's free balance to plan
  funding, the answer is a `qip-contracts` record of that balance the kernel
  passes in, not an edge.
