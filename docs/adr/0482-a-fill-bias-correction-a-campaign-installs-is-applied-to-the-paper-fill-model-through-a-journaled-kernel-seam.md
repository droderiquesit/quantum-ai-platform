# ADR 0482: A fill-bias correction a research campaign installs is applied to the paper fill model through a journaled kernel seam, scored under on the venue's paper fills and reverted on withdrawal

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  (decision 28 of 2026-11-11)**, session t50-28 (wave580). It tightens:
  it adds a write seam with a fail-closed guard and a journaled record,
  removes no refusal, and touches no paper-trading layer. The correction
  shifts a diagnostic the research loop reads; it moves no money, sizes no
  order, withdraws and releases no venue, and reaches no live path.
- **Date**: 2026-11-12
- **Reads**: ADR 0142 (the shadow/paper window order and the `Installation`
  record a campaign leaves), ADR 0006 (a classical baseline always — the
  uncorrected twin is the champion the correction must beat), the kernel's
  `microstructure_model` (the pattern this follows: a fact journaled before
  the state moves, the state the left fold of its own log records).
- **Rows**: R12-EXPAND-6050 (PARTIAL → PARTIAL; the slice lands and the gap
  narrows — see below).

## Context

R12-EXPAND-6050 asks for reproducible research across historical, simulated,
paper and shadow environments. ADR 0142 built the campaign that runs the four
in order and leaves an `Installation` record when its shadow window beats the
uncorrected twin. But the installation was **a record on the campaign result
and nothing acted on it**. The deep brain reads the platform behind a shared
`&Platform`, so `run_fill_bias_campaign` partitioned the platform's *existing*
residuals into a paper window and scored them, and `carry_out` counted the
installation. "Installed in the paper fill model" therefore named the instant
the paper window opened, not a correction the model applied — the paper
window's fills were the platform's own pre-existing fills, not fills produced
under the correction.

That is this repository's recurring failure: a control that reads as an effect
while changing nothing (`MaxExpectedShortfall`, a limit that could never fire).
The grade stayed PARTIAL on exactly this gap.

## Decision

A fill-bias correction is **applied to the paper fill model through a journaled
kernel seam**, and the deep brain hands the install and withdrawal to the
platform the way it already hands over a concluded microstructure delta.

1. **The kernel owns the installed set and the act.**
   `qip-kernel`'s new `fill_bias_installation` module holds `InstalledFillBiases`
   — per venue, the correction in force. `Platform::install_fill_bias` journals
   a `FillBiasInstallationRecord` **before** the state moves and inserts;
   `Platform::withdraw_fill_bias` journals the withdrawal and removes. The
   installed set is the left fold of its own log records, and
   `fill_bias_installation::resume` rebuilds it from the log alone, so **a
   replay reproduces the installed correction** — the row's reproducibility
   bar, met on the act and not only on the campaign record.

2. **The correction bites where a paper fill is scored.** `Platform::score_filled`
   subtracts the installed bias from the signed basis-point residual it records
   on the venue's `FillScore` (`trade_error_bps = raw − bias`). That residual is
   the paper fill the *next* campaign reads, so the paper window's fills are now
   produced under the correction and revert to raw the cycle after it is
   withdrawn. The correction shifts only that recorded residual: the twin's own
   fill calibration (R12-TICK-2080) keeps the **raw** error, because that path
   is where the correction's own bias was measured and folding it back would
   count the same bias twice.

3. **Fail closed.** `install_fill_bias` refuses a non-finite bias and a venue no
   registered strategy trades — a correction for a venue nobody produces fills
   at would score nothing, so it is refused rather than stored, exactly as a
   microstructure delta for such a venue is.

4. **The deep brain hands off, it does not reach in.** A sweep holds `&Platform`,
   so `carry_out` records the decision as a `FillBiasAction` on the round — an
   install when the shadow window cleared and the paper window held, a
   withdrawal when the paper window refused the correction (removing whatever an
   earlier sweep installed at the venue, a no-op otherwise). The node's
   `research_pass`, which holds `&mut Platform`, applies them through
   `ResearchRound::admit_fill_bias`, beside `admit_deltas`, reporting any
   refusal rather than swallowing it.

## What would make this wrong

- **A second writer of the installed set.** The set must be the fold of its log
  records alone; a path that changed it without journaling would be a state
  `resume` could not rebuild. Only `install_fill_bias` and `withdraw_fill_bias`
  write it, each journaling first.
- **The correction reaching a decision.** It shifts `trade_error_bps`, which the
  replay-divergence hold and the research loop read. That is intended: the
  twin-as-corrected is what both should see. But it must never size a trade,
  move money, or widen a cap — it subtracts from a prediction, never from a
  price anything trades at. A wrong correction makes the next campaign's
  residuals wrong and is caught there, not on a realised line.
- **Double-counting into the fill calibration.** Folding the corrected residual
  back into the calibration that measured the bias would arm a feedback loop.
  The calibration keeps the raw error on purpose.

## What it costs

Another journal topic group that the log never evicts — every install and
withdraw record is retained so `resume` can rebuild the installed set, and a
venue that installs and withdraws often grows that history. The cost is bounded
the same way the microstructure folds are: one record per act, on a fact that
changes rarely. A second cost is coupling the replay-divergence hold and the
research loop to a correction the research loop itself produced; the correction
is deliberately confined to a diagnostic residual so that the coupling cannot
reach money, a size or a venue withdrawal, and a wrong correction surfaces as a
wrong next-campaign residual rather than a bad fill.

## Consequences

The installation is now an act on the paper fill model, journaled and
replayable, and the paper window's fills are produced under the correction.
The slice lands the write seam, the apply point, the withdrawal revert, the
deep-brain hand-off and tests mutation-verified on both sides. The row stays
PARTIAL: the parked skeptic and independent-review round is not this session's
to close, and the correction is a constant per-venue entry bias — the simplest
fill model the fit supports, not a term structure. Those remain tracked.
