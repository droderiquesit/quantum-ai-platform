# ADR 0679: A tick-stage test written without running asserts reproducibility, equality with an independent path and sensitivity, and pins no literal digest

- **Status**: **Provisional, 2026-11-15**, written by lane 39 of wave b52
  under the owner's instruction of the same date, which withholds cargo
  build, test and clippy from the lane. It adds no dependency and touches no
  paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-TICK-1121, R12-TICK-1125, R12-TICK-1127 (each stays PARTIAL
  until the pipeline has run its test).

## Context

`RECORDED_DIGEST` in `tick_replay.rs` is computed in memory from decode to
book. The lake, the fabric's `ReplaySession` and `qip_kernel::fusion::fuse`
sit outside it, so a mutation of any of them moved no digest the suite held.
An earlier lane draft for these rows did not compile (E0505, `ticks` moved
while borrowed) and was reverted. A literal digest is a value only a run can
produce; one typed in without running is a guess that fails the first time it
is checked, or worse, is "corrected" by pasting.

## Decision

1. **The lake stage** is checked against the digest that already exists. The
   session's decoded messages are sealed with `Lake::write_market_segment`,
   pinned, read back with `read_market_history`, sequenced and replayed; the
   record must equal the in-memory one and hash to `RECORDED_DIGEST`. The
   decoder's diagnostics are taken from the decode stage, which the lake does
   not hold.
2. **The replay engine and the fusion** pin no literal. Each asserts that a
   second run is equal; that inputs one tick or one fee apart do not share a
   digest; and, where an independent implementation exists, that the stage
   agrees with it (`ReplaySession`'s book against `qip_orderbook::replay` on
   the clean span; the fusion of the lake's replay against the fusion of the
   in-memory replay). Sensitivity of the digest to the stage under test is
   shown by removing one printed message and requiring the digest to move.
3. **The pinned literals are to be added by a lane that has run them**, in a
   commit that says the value was observed, not predicted.
4. A new test in this state says so in its doc comment and in its report
   row: `built_unverified: true`, `mutation: not run`, with the mutation that
   was intended written out so the next lane can run it.

## Consequences

The tests can pass while a stage is wrong in a way both sides of an equality
share. Reproducibility and sensitivity do not prove a stage correct; equality
with an independent path does, and only where one exists. Row 1124's
alignment digest has no independent path and rests on reproducibility,
sensitivity to the recorded uncertainty and the premises that both answers
occur.

## Open finding

A lake segment orders lines by receive time, then `CapturedMarketEvent::sequence`,
then canonical text. One wire packet can decode to two messages (an execution
is a trade and a reduction) sharing the first two keys, so text order decides
which a replay applies first unless the capture path stamps a per-message
sequence. The test stamps decode position; whether the edge capture path does
was not established here.
