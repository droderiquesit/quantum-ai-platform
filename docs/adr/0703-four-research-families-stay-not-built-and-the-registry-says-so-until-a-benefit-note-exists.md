# ADR 0703: Four research families stay not built, and the foundry keeps saying so, until a committed benefit note exists

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-QUANT-1078, R12-QUANT-1079, R12-QUANT-1080, R12-QUANT-1081

## Context

Four rows ask the Quantum Foundry to register a benchmarked workload family
each: feature/ensemble selection (1078), quantum kernels and variational
models (1079), quantum reservoir and extreme-learning research (1080), and
scenario exploration (1081). Each row's check is the same: the foundry
registers the family with an encoding, a classical baseline and a benchmark,
and its status is not "not built".

Today `Foundry::platform` in `qip-quantum/src/foundry.rs` registers one
family and records the other four as "research family not built: no encoding,
baseline or benchmark exists in-tree". That sentence is true, and it is the
foundry's honest answer.

ADR 0150 (Accepted 2026-10-19, on the owner's approval) fixes the scope. Only
the portfolio QUBO and the bounded in-tree constraint solver are built. Any
other quantum family may have its encoding, baseline and benchmark written
only after a committed note shows two things: the instance sizes the platform
actually poses lie above the exact-enumeration limit, and they lie at or below
the provider's qubit ceiling. Decision A2 of that record also keeps
feature/ensemble selection not built on purpose. ADR 0190 (2026-10-24)
confirmed those gates and kept the parked research families parked. No such
note exists.

A patch holding all four families, each with a classical baseline, is parked
at `docs/blueprint/lanes/L4-quant-research-families-adr0150.patch`. It is not
landed, and this record does not land it.

## Decision

1. **R12-QUANT-1078, feature/ensemble selection: decline for now.** The
   family stays "not built" in the registry. Landing the encoding would break
   ADR 0150 A2, which names this family. The row stays open and is graded
   against the declined clause. It reopens when a committed benefit note
   meets the two conditions above for feature-selection instances.
2. **R12-QUANT-1079, kernels and variational models: decline for now.** The
   family stays "not built". A kernel or variational model on the platform's
   data sizes is classically tractable today, and no note says otherwise, so
   ADR 0150's gate is unmet. Registering it with a benchmark would imply a
   measured advantage that nobody has measured.
3. **R12-QUANT-1080, reservoir and extreme-learning research: decline for
   now.** Same reason and same gate as decision 2. The classical echo-state
   and autoregressive comparators are the standing answer until a note shows
   a size where they do not suffice.
4. **R12-QUANT-1081, scenario exploration: decline for now.** Selection under
   a budget at the scenario counts the platform poses is within exact
   enumeration or the in-tree bounded solver, which ADR 0150 already
   provides. The family stays "not built" until a note shows instance sizes
   beyond that.

For all four: the registry keeps recording "not built" and is not edited to
read otherwise. The parked patch stays parked. No row is marked done. The
row text may be reworded only by the owner; this record does not reword it.

## What it costs

Four P3 capability rows stay MISSING, and the foundry demonstrates one
family, not five. A reader comparing the foundry to the blueprint's
"benchmarked workload families" will find a gap. That is the intended
outcome: the gap is stated in the registry itself, and the cost is a missing
demonstration, not a missing control. The parked patch's work is carried
unused until a note releases it, and it may drift from the tree meanwhile.

## What would make this wrong

A committed note showing instance sizes the platform poses above the
exact-enumeration limit and at or below the provider's qubit ceiling for any
one of these families would make that family's decision wrong, and its row
would be re-decided then, with the patch rebased and re-tested first. The
same applies if a wider qubit ceiling is obtained (billed infrastructure and
the owner's choice), or if the owner rewords a requirement. It would also be
wrong if the "not built" status were found to be hiding a use that the desk
relies on today; no evidence of that is in the tree.
