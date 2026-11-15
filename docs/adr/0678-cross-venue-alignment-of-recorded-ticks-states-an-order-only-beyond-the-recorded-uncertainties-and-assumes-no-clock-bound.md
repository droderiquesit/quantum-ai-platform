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

## What it costs

Two ticks whose recorded gap is no more than the sum of their recorded uncertainties are reported `Unordered`, so the aligner states fewer orders than a naive timestamp comparison would, and a consumer must handle that answer. Ticks from two different cells must not go through this type; the caller must route them to `CrossRegionAligner` with a stated bound. `VenueTick` names no cell, so nothing in the type stops a caller who forgets, and the error is then understated with no refusal. A lake line with no clock block is refused by the reader that builds `VenueTick` rather than aligned, so such history drops out of a cross-venue read. The aligner holds one latest tick per `(venue, instrument)` key, capped at `MAX_VENUE_PARTITIONS` (1,024), and evicts the earliest beyond that; each `relate` call scans every held key. The scan cost has not been measured. The conversion from `RecordedTick` stays in the caller, so `qip-sequencing` offers none and each consumer writes its own. No production composition root calls the aligner yet, and the row R12-TICK-1124 stays PARTIAL.

## What would make this wrong

A venue's clock error turns out to be larger than its recorded uncertainty even within one cell, for example a feed that stamps arrival rather than capture. Then the stated orders are too confident and a bound argument is needed here as well. A deployment captures several venues through different cells but feeds them to this type, which the type does not prevent. Consumers find `Unordered` so common that the output is unusable, which would show the recorded uncertainties are too wide to relate venues at all. The 1,024-key cap evicts a key a consumer still needs, which would show up as missing alignments for a quiet venue.
