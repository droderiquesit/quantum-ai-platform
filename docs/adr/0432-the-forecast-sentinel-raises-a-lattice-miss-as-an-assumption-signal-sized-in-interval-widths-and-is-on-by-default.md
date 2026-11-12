# ADR 0432: The forecast sentinel raises a lattice miss as an assumption signal sized in interval widths, and is on by default

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-03. It adds an advisory
  sentinel to the deep brain's research sweep; nothing it raises reaches a
  thesis, a size or an order, and it touches no paper-trading layer.
- **Date**: 2026-11-12
- **Reads**: ADR 0006 (nothing here is a quantum path), the forecast
  lattice's own module doc (`qip-kernel/src/lattice_stage.rs`, "Advisory, and
  structurally so").
- **Rows**: R12-AMBIENT-3066 (PARTIAL; see "What stays open").

## Context

R12-AMBIENT-3066 reads: "The ambient mesh includes forecast models over
multiple horizons that maintain expected future states over multiple
horizons, from milliseconds to years." Its check: the sentinel runs in
production, its output reaches the attention router, and disabling it
removes its signals.

The models already exist. The kernel's forecast lattice
(`qip_world_model::lattice`, run every cycle by `Platform::understand_now`)
holds a calibrated forecast per instrument at the four central horizons —
seconds-minutes, hours-days, weeks-months, quarters-years — re-issues each
when it goes stale, and scores each against reality when it falls due. What
it did not do was tell anyone. A forecast that missed was a row in a
scorecard and a `covered: false` on a journaled `ScoreRecord`; nothing woke.
So the lattice maintained expected future states and was not a sentinel.

## Decision

1. **The finding is a resolution outside the forecast's own 80% interval.**
   `LatticeStage::run` keeps each such resolution as a `ForecastMiss` — the
   quantiles it stated and the outcome reality gave — in a queue bounded at
   `LATTICE_MISSES_KEPT` (256), read by sequence number with
   `misses_since(seen)`, the same shape as the NOW Brain's surprise queue. A
   reader that falls behind is told how many it lost. The miss is the
   comparison of an expected state with reality, which is what a sentinel
   over expected states can notice; a forecast alone is not a deviation.
2. **It is an assumption signal, routed to a reflex-model update.** The model
   that expected otherwise is the assumption that failed, so the class is
   `SignalClass::Assumption`, which the standard routing policy sends to
   `Pathway::ReflexModelUpdate`. It goes through the sweep's one
   `AttentionRouter` with the same budget as every other signal, so a storm
   of misses is deferred or shed on the record rather than waking everything.
3. **It is sized in widths of its own interval past the bound.** Severity is
   `outside / (q90 − q10)` on the 10 000 bp scale, saturating at the top at
   one whole width. A calibrated lattice misses its 80% interval one
   resolution in five by construction and most of those land near the bound,
   so the miss alone would be a steady stream of noise; the router's
   materiality line (1 000 bp) admits a miss more than a tenth of a width
   past the bound. Sizing from the median instead would rate a miss that
   barely left the interval as half the scale. No figure here was stated by
   the owner; the tenth-of-a-width line is the router's existing materiality
   read through this scale, and the ADR is the place to change it.
4. **A miss that cannot be sized is refused, not repaired.** An interval of
   non-positive or non-finite width, a non-finite outcome or median, or an
   outcome inside the interval is refused by `forecast_miss_signal` and
   recorded on the round as `forecast_misses_refused` with why.
5. **On by default; `QIP_DEEPBRAIN_FORECAST_SENTINEL=off` turns it off;
   anything else stops start-up.** Off, the sweep still reads the queue
   through and raises nothing, so switching it back on does not replay a
   backlog stamped as fresh. A value other than `on` or `off` is refused by
   `ResearchConfig::from_lookup`, because a typo that silently left the
   switch in either position is a switch nobody holds.

## What stays open (dated 2026-11-12)

- **The millisecond row.** `LatticeHorizon::MicrosecondsMs` is the local
  reflex's lane and `qip_world_model::lattice::issue` refuses the central
  lane there; no reflex-side forecaster populates it in any composition root.
  The sentinel covers seconds to years, not milliseconds.
- **Production proof.** The deep brain's `main.rs` builds its research loop
  from `ResearchConfig::from_lookup`, so the sentinel is on the default path,
  but no `qip-deepbrain` is deployed; "runs in production" is met in the
  composition root and not in a deployment.

## Consequences

- A forecast miss is now an `AmbientSignal` with trigger `forecast_miss`,
  journaled with every other routed signal by the ambient journal.
- `ResearchRound` gains `forecast_misses`, `forecast_misses_lost` and
  `forecast_misses_refused`.
- Proven by `qip-deepbrain/tests/forecast_sentinel.rs`, each test
  mutation-verified.

## What it costs

- One more signal source on the sweep's single attention budget. In a
  regime break every central horizon of every instrument can miss at once —
  up to four misses per instrument per cycle — and those compete with NOW
  surprises and limit warnings for the same activations. The router defers
  or sheds the excess on the record, so the cost is attention, not silence.
- The miss queue holds at most 256 entries in the kernel; a sweep that falls
  further behind loses the oldest and reports how many.
- The severity scale is this ADR's, not the owner's: a different line would
  change what wakes, and nothing has measured the right one.

## What would make this wrong

- A measured run in which forecast-miss signals dominate the attention
  budget while the lattice stays calibrated (scorecards inside their
  objective): then misses of a tenth of a width are noise and the scale, or
  the materiality line it is read against, should move.
- A reflex-side forecaster populating the millisecond row: its misses belong
  to the cell's own loop, and this sentinel should not be stretched to read
  them through the centre.
- A consumer of `Pathway::ReflexModelUpdate` that acts on the model: today
  the activation is advisory and recorded; anything that retrains on it
  needs its own decision.
