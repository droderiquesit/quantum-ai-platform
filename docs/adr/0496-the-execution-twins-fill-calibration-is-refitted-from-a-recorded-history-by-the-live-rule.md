# ADR 0496: The execution twin's fill calibration is refitted from a recorded history by the live rule

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-35. It sets no figure the owner has not stated, changes no
  requirement's wording, adds no dependency, changes no sealed body's schema
  and touches no paper-trading layer. Nothing sizes, vetoes or trades on what
  it adds.
- **Date**: 2026-11-13
- **Rows**: R12-MODEL-4008.

## Context

R12-MODEL-4008 asks that the execution twin's models be "calibrated to
Algorik telemetry and historical replay", checked by "twin parameters are
refit from recorded fills and the calibrated twin's fill-error distribution
narrows; a test shows a recalibration changing twin output".

The LEARN stage refits the twin live. Since ADR 0450 (R12-TICK-2080),
`Platform::score_filled` hands `FillCalibration::observe_cost` the
uncalibrated model's charged cost for the venue's fill less what the venue
actually cost, in the twin's impact units. That calibration lives only in
memory, and nothing showed that calibrating brings the twin's costs closer
to the venue's — the tests proved the parameters move, not that the twin
then errs less.

This session first built the refit on the cycle journal's
`CounterfactualJournal::fill_outcomes`, replaying each fill's
`trade_error_bps` once per order id. ADR 0450 landed on the integration
branch meanwhile and showed that quantity is the wrong one: it is the
entry-price error, which "charged every cost twice" when fitted. The journal
does not carry the cost error the calibration now learns from — `FillScore`
has no field for it, and `CycleJournalEntry` is a sealed body locked by
`qip-kernel/tests/body_schema_lock.rs`. The journal refit was withdrawn
before it was merged rather than ship a replay that teaches the twin a
quantity the live loop stopped teaching it.

## Decision

1. `FillCalibration::replay(base, history)` absorbs a recorded history,
   oldest first, by the **same** rule a live fill takes
   (`observe_cost`, ceiling refusals included), against the same
   uncalibrated `base`. One rule, not two that can drift: replaying what the
   live loop saw rebuilds what it held. `from_history` does the same from an
   empty calibration. A `RecordedFill` is the venue, the cost error and the
   impact unit — the inputs of `observe_cost`, and explicitly not
   `FillScore::trade_error_bps`.
2. A replay is **all or nothing**: every record passes the one check a live
   fill also passes (`check_observation`, now shared) before any is
   absorbed, and a refusal names the record's position. A twin fitted to the
   readable prefix of a record is indistinguishable from one fitted to all
   of it, and so is worse than no refit.
3. The narrowing the row asks for is measured on the twin's **charged cost**
   against reality's on held-out fills, which is the quantity ADR 0450
   calibrates. The charted `qip_venue_fill_error_bps` is the entry-price
   error and is not expected to narrow.

## What it costs

Nothing in the kernel produces `RecordedFill`s yet. Making the cycle journal
the source needs the cost error and impact unit recorded per scored fill, a
schema-version bump of a sealed body shared by fifty lanes; and a restart
path calling `from_history` needs a composition-root change. Both are the
row's remaining gap, not done here.

## What would make this wrong

- The live rule changing again without `replay` following it. The test
  asserting a replayed calibration equals the live one catches this only if
  it is updated to feed the new live rule, so the two are kept side by side
  in `calibration.rs`.
- A schema change that records the cost error: then the journal refit this
  ADR withdrew becomes correct and should be restored over the new field.
