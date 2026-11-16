# ADR 0723: Tick label leakage stays guarded by the refusals already in qip-training, and the tick desk stays off until the deep brain reads a real lake

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-TICK-2066 (stays PARTIAL)

## Context

R12-TICK-2066 states that labels are never available as inputs at or before
the instant they describe, and that a window the series does not cover is
refused rather than shortened. Its check: a horizon past the series end is
refused; a dataset whose feature timestamp exceeds its label anchor is
refused; the check fails if either is accepted.

The register records what exists. `LabelBuilder` refuses an uncovered horizon
and a test names it. The deep brain's learning desk assembles features from
bars up to index i and targets the span i to i+1. `LeakageAudit` is a declared
record on strategy evidence. The register's gap, written 2026-10-25, records
that the tick desk's off-by-default guard was kept: the desk reads the deep
brain's own lake namespace, that namespace holds no market segments by
default, and so a default-on desk would replay an empty lake every round and
`build_examples` would never run. The register's notes record a second
finding: a refusal of an observation knowable after the anchor, placed in
`build_examples`, cannot fire there, because that function sorts by knowable
instant first. A control that cannot fire reads as protection and is not.

ADR 0151 states that the tick desk stays off by default. ADR 0416 covers the
connector's tick-lake capture and the deep brain's retention right. This
record amends neither.

## Decision

1. **R12-TICK-2066: the guard stays; the refusals that already exist stand as
   the row's evidence for the uncovered-horizon half.** The tick desk remains
   off by default. Turning it on would add no capability, since it would read
   an empty lake, and a switch that changes nothing observable is not
   evidence for the row. This is a decline of default-on wiring, with the
   reason stated.
2. **No refusal is added to `build_examples` for an observation knowable after
   the anchor.** After the sort by knowable instant, such a refusal has no
   input on which it could fire. The decision is not to add a control that
   cannot fire.
3. **The feature-after-anchor half of the check is owed at the point where
   tick labelling has a production caller.** When the tick desk builds
   `MicroExamples` from a lake the deep brain can read, the refusal belongs
   on the field that carries the instant at the seam where examples are built
   from unsorted input, and its test must be shown to fail when the refusal is
   removed. Until then the row stays PARTIAL and says so.
3a. **The lake is the precondition, and its shape is a choice for the
   owner.** The deep brain needs a lake it can read on its default path. Two
   routes are open: the edge node's segments shipped to a store both read, or
   the deep brain's own capture on by default under a licensed or simulated
   dataset. The first adds a transfer path between planes; the second needs a
   dataset whose licence posture is evaluated before use. Both touch data
   licensing or infrastructure cost, so neither is chosen here.
4. **Superseding the guard waits on that lake.** An ADR superseding ADR 0151's
   statement that the desk stays off is written only when the default desk
   reads real segments, and it must name this row.

## What it costs

The row stays PARTIAL, and the feature-after-anchor refusal has no
production seam to live in yet. The tick desk produces nothing by default,
so a reader of the platform sees no tick-derived examples. That absence is
the honest state: the alternative is a desk that runs and learns from nothing.

## What would make this wrong

- A production path that builds tick examples from unsorted input appears
  without this record's refusal, which would let a later instant reach a
  feature unchecked.
- The lake is filled by default for a reason unrelated to this row, and the
  desk is switched on without an ADR superseding ADR 0151.
- A reader treats the sort in `build_examples` as the leakage guard. It orders
  the input; it does not refuse anything, and the first half of the check is
  held by `LabelBuilder` alone.
- The owner decides the tick desk is not wanted at all, in which case the row
  should be declined rather than held PARTIAL.
