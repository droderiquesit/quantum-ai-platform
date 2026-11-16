# ADR 0689: Four risk rows of lane 44 are not built, and the decision each needs

- **Status**: **Proposed 2026-11-15, lane 44 of wave b52.** Records decisions a
  person must take; builds nothing.
- **Date**: 2026-11-15
- **Rows**: R12-REFLEX-7061, R12-GOV-6118, R12-GOV-6126, R12-MESH-5062.

## Decisions needed

1. **R12-REFLEX-7061 (refuse past `valid_for`).** Refusing new orders once the
   applied payload is outside its window is restrictive, but it contradicts
   `qip-edge/tests/telemetry.rs::a_cell_whose_centre_has_stopped_makes_the_same_decision_as_one_still_served_and_differs_only_by_the_stated_narrowing`,
   which applies a payload at t=100 (valid_for 300 s) and asserts the orphaned
   cell still places an order at t=500, and ADR 0008 (cells decide alone). A lane
   may not weaken a test. A person decides: either that test and ADR 0008's
   reading change, or the requirement's check is read as "narrows to the floor",
   which the code already does. Provisional restrictive choice if the first is
   taken: refuse new exposure only, keep withdrawals and fill confirmation, at
   exactly `valid_for`, no grace (a grace would be an invented figure).
2. **R12-GOV-6118 (edge-node calls `admit_enablement`).** `qip-edge-node` cannot
   depend on `qip-compliance`: that crate reaches `qip-ai`, and
   `architecture.rs::no_edge_cell_can_reach_a_language_model` forbids it for
   every edge crate. Needed: split `eligibility.rs` into a dependency-light lib
   with no `qip-ai` edge, then load the seed at the node root behind a variable.
   Not built: it changes the crate graph and a protected guard's inputs.
3. **R12-GOV-6126 (more manipulation patterns).** Marking the close, momentum
   ignition and quote stuffing each need a window and a threshold that nobody
   has given. Outcome NEEDS_FIGURE. Also open: a production caller of
   `OrderManager::cancel`.
4. **R12-MESH-5062 (reserve against `RegionTable` capital).** The saga's grant
   is `LegAuthority::max_size`; the region share lives in `qip-edge`'s `Cell`.
   Bounding the grant by the region's remaining capital needs a port from the
   cell to the wire thread that ADR 0116 forbids the decision loop from waiting
   on. Needs a design by a person; not built.

## What it costs

Nothing is built, so four requirements stay open and the cost is the exposure they leave. A cell whose centre has stopped keeps placing new orders past `valid_for`, narrowed to the floor but not refused, until a person settles the contradiction with ADR 0008. The edge node cannot call `admit_enablement`, so the enablement check is not enforced there until the `qip-compliance` split is done. The three manipulation patterns the third row names (marking the close, momentum ignition, quote stuffing) get no detector, and `OrderManager::cancel` still has no production caller. The region share does not bound a saga grant. None of these has a measured size; the cost is unmeasured. Each also leaves a person with a decision, a library split or a design to deliver.

## What would make this wrong

A person taking the first decision and finding the existing narrowing already satisfies the requirement would show the row was not a gap. Someone supplying the missing windows and thresholds, or a port design that ADR 0116 admits, would remove the reason for withholding those rows, so waiting is wrong from then on. An orphaned cell found to have placed exposure after `valid_for` in a deployment would show that deferring the restrictive choice left real exposure open. A `qip-compliance` split that keeps `qip-ai` out of every edge crate would unblock the second row.

## Paper boundary

No edit touched any of the three layers.
