# ADR 0678: Cross-venue alignment of recorded ticks states an order only beyond the recorded uncertainties and assumes no clock bound

- **Status**: **Provisional, 2026-11-15**, written by lane 39 of wave b52
  under the owner's instruction of the same date. Nothing in it was compiled
  or run. It adds no dependency, no figure and no live path, and touches none
  of the three paper-trading layers.
- **Date**: 2026-11-15
- **Rows**: R12-TICK-1124 (stays PARTIAL until the pipeline has run it).

## Context

`qip_sequencing::align` orders two stamps only beyond both clocks'
uncertainty and has no "simultaneous" answer. `CrossRegionAligner` relates
ticks captured by different cells and takes a stated per-cell clock bound as
an argument. Nothing related ticks of one instrument printed on two venues and
read back from the Tick Lake: the only in-tree cross-venue computation,
`qip_feature_dag`'s `CrossVenueSpread`, compares fresh mids with no clock
uncertainty, and `lake_history` returns each tick's clock block
(`RecordedTick::clock`) to nobody who aligns.

## Decision

1. `qip_sequencing::venues::CrossVenueAligner` relates a `VenueTick` to the
   latest tick of every other venue on the same instrument, through
   `align`. An order is stated only beyond the sum of the two recorded
   uncertainties; anything closer is `Order::Unordered`.
2. **It assumes no clock bound.** Venues captured by one cell share that
   cell's clock, so the recorded uncertainty is the whole error. Venues
   captured by different cells must go through `CrossRegionAligner`, which
   takes a bound as an argument because no figure for one has been stated.
   `CrossVenueAligner` has no such argument; feeding it ticks from two cells
   would understate the error, and the type's documentation says so.
3. A tick whose lake line carries no clock block is refused by the reader that
   builds `VenueTick`, not placed at the venue's own instant: its clock is
   unknown, not exact.
4. The module lives in `qip-sequencing` as pure data and arithmetic, with no
   dependency on `qip-storage`, so the regional cell's closure still cannot
   link the lake (`lane0_tick_lake`). The conversion from `RecordedTick` is the
   caller's, and is written in the acceptance test.

## What this does not do

No production composition root calls it yet. `qip-deepbrain`'s tick desk is
off in every committed deployment (`QIP_DEEPBRAIN_TICK_EVERY` unset) and runs
one simulated venue; wiring the aligner into the desk's round, and a gitops
change enabling the desk, are separate and the second is the owner's decision.

## Provisional

No number is chosen here. The 100 ns and 200 ns uncertainties and the 250 ns
shift in `tick_replay.rs` are test fixtures that make both `Unordered` and
ordered answers occur, not statements about any venue's clock.
