# ADR 0496: The execution twin's fill calibration is refitted from the cycle journal by the live rule

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
Algorik telemetry and historical replay", with the check "twin parameters
are refit from recorded fills and the calibrated twin's fill-error
distribution narrows; a test shows a recalibration changing twin output".

The LEARN stage already refits the twin live: `Platform::score_filled`
hands each scored fill's entry error to
`qip_twin::calibration::FillCalibration::observe_with_participation`, and
the next pricing uses `FillCalibration::cost_model`. That calibration lived
only in memory. A restarted platform priced its first counterfactuals with
the uncalibrated model, and nobody could ask what the twin would have learned
from a stretch of history without re-running the platform over it.

The record exists: every cycle's `CounterfactualJournal::fill_outcomes`
carries each scored fill's venue and `trade_error_bps`. Two facts about it
shape the decision.

1. **Each cycle journals the whole retained list**, not that cycle's scores,
   so a fill appears in every later cycle's journal until the retention cap
   drops it.
2. **The kernel's error is against the uncalibrated twin.** It compares the
   twin's entry *price* with the venue's fill, and the calibrated half-spread
   is charged as a cost beside that price rather than moving it. Measured on
   2026-11-13 in `qip-kernel/tests/learning.rs`: eight fills on one tape
   scored an identical error of about -7000.9 bps while the venue's bias moved
   by thousands of basis points. This session first read the error as a
   residual of the calibrated twin and added the applied correction back
   before the fit; the bias then integrated without bound and an existing
   test (`a_persistent_pattern_of_unfavourable_declines_on_one_instrument_narrows_only_that_instruments_sizing`)
   failed. That change was withdrawn.
3. **The record carries no fill participation.** `FillScore` has no field
   for it, and `CycleJournalEntry` is a sealed body locked by
   `qip-kernel/tests/body_schema_lock.rs`; adding one is a schema version
   bump in a body fifty lanes share.

## Decision

1. `FillCalibration::replay` absorbs a recorded history, oldest first, by
   the **same** rule a live fill takes (`observe_with_participation`). One
   rule, not two that can drift: replaying what the live loop saw rebuilds
   what the live loop held. `FillCalibration::from_history` is the same from
   an empty calibration.
2. A replay is **all or nothing**: every record is checked by the one check
   a live fill also passes before any is absorbed, and a refusal names the
   record's position. A twin fitted to the readable prefix of a log is
   indistinguishable from one fitted to all of it, and so is worse than no
   refit.
3. `qip_kernel::fill_replay::recorded_fills` reads the journals' fill
   scores **once per order id**, in the order first scored, skipping a score
   with no entry error. `fill_replay::refit_from_journal` composes it with
   `from_history`.
4. **No participation is replayed**, and the schema is not bumped here. A
   refit therefore moves each venue's spread and not its impact slope: it
   equals the live calibration exactly while the live one has fitted no slope
   (fewer than two sizes of fill), and differs after. This is the remaining
   gap on the row, stated in the register.

## What it costs

A refit from the journal is not yet the restart path: nothing in a
composition root calls `refit_from_journal` on start-up. It is a function a
caller — a restore, or a research desk replaying a window — can call, and it
is proven against a platform's own journal, not wired.

## What would make this wrong

- A change making the twin's entry price move with the calibration. The
  kernel's error would then be a residual, and both the live rule and replay
  would need the correction added back; the narrowing test in
  `qip-twin/tests/futures.rs` measures the all-in entry cost, so it would
  keep passing and would not catch it. The kernel test that failed on
  2026-11-13 is the one that would.
- `fill_outcomes` changing from a cumulative list to a per-cycle one. The
  de-duplication would then be harmless but unnecessary.
