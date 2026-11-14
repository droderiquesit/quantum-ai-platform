# ADR 0506: A reflex forecast is a contract whose promotion target is a one-variant type, issued by the cell from its own observed outcomes and read by nothing central

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR), session t50-40. The delegation's limits hold:
  no live-order path, no real capital, no weakened test, guard or limit, no
  new dependency. The numbers it introduces — a 256-outcome window, a
  20-outcome warm-up, 1,024 series, the 10th/50th/90th percentiles — are
  parameters of a frequency estimator, not limits: nothing sizes, vetoes,
  routes or trades on what it issues.
- **Date**: 2026-11-13
- **Rows**: R12-WORLD-3084. Builds the code slice ADR 0133 decision 2 left
  open; keeps ADR 0195.

## Context

R12-WORLD-3084: "The Microseconds-ms horizon is served by reflex/
microstructure models producing fill probability, queue movement, adverse
selection, executable edge, and its outputs are promoted only to Local
Reflex only." Its verification: "A Microseconds-ms model targeting any lane
other than Local Reflex only is refused at admission."

The admission half was already true on the central side:
`qip-world-model::lattice` admits a microsecond-row model only aimed at
`PromotionTarget::LocalReflexOnly` and refuses to issue the row from the
central lane. The row itself had no producer. ADR 0133 decision 2 said the
node issues it under a `ReflexForecast` contract in `qip-contracts`; no such
contract existed and nothing issued one.

ADR 0195, accepted later, says of the reflex lane's *decision* forecast:
"No other forecast form is added for the reflex lane", and that its
calibration ships on a promoted model and is "never a number a cell adjusts
on its own". Building ADR 0133's contract and a node issuer has to sit
inside that.

Building it surfaced one fact the earlier record could not have known:
`qip_core::Timestamp` is written to the wire as RFC 3339 at millisecond
precision, so a 500 µs forecast issued and expiring within one millisecond
decoded with its expiry equal to its issue instant and was refused by its
own decoder.

## Decision

1. **`ReflexForecast` is a `Validated` contract in
   `qip-contracts::reflex_forecast`.** Target (one of the four the row names),
   venue, instrument, horizon in microseconds, issued-at and expires-at, a
   distribution, the issuing model's calibration, the model's name, and the
   promotion target. It refuses a horizon of zero or of a second and over
   (the seconds-minutes row's territory, whose promotion target differs), an
   expiry not after issue or after the horizon has passed, and a
   distribution whose shape does not match the target: events (fill
   probability, adverse selection) as a categorical over exactly `occurred`
   and `not_occurred`, magnitudes (queue movement, executable edge) as
   quantiles.
2. **The promotion target is a one-variant enum, `ReflexPromotion::
   LocalReflexOnly`.** A record naming any other target does not decode.
   That is the row's verification made structural on the reflex side: a
   label the consumer must check is replaced by a type that cannot name the
   alternative.
3. **Its two instants travel as integer nanoseconds.** `Timestamp` already
   decodes that form; only the serializer is chosen per field. The general
   `Timestamp` encoding is left alone, because changing it changes every
   hash-chained record that holds one.
4. **The node's issuer is a bounded frequency model, `reflex-frequency/1`,
   in `qip-edge::reflex_forecast`.** Events are stated as the Laplace-smoothed
   frequency over the last 256 outcomes per venue, instrument and target —
   never 0 or 100 % — and magnitudes as that window's 10th, 50th and 90th
   percentiles. It states nothing before 20 outcomes, and refuses a 1,025th
   series rather than evicting a live one. Every forecast can be resolved
   against what happened; the issuer keeps a Brier score and a ten-bin
   reliability table for events and per-quantile coverage for magnitudes,
   and the next forecast carries the measured score as its calibration. It
   resolves only forecasts it issued.
5. **A reflex forecast is a record and evidence, not a decision input.**
   This is how decision 4 coexists with ADR 0195: the issuer adjusts its own
   numbers, so nothing in the cell's pass reads them — no gate, no sizing, no
   venue choice. ADR 0195's distillate stays the only forecast the cell
   decides on. `qip-acceptance`'s `reflex_forecast_boundary` refuses any
   production source outside `qip-contracts`, `qip-edge` and
   `qip-edge-node` naming the contract, and inside `qip-edge` any file but
   the issuer and the module declaration. The centre's after-the-fact
   scoring (ADR 0133) will enter that list in a reviewed diff when it lands.

## Consequences

- The row's four outputs exist as a contract and are issued and scored by
  the node, with a calibration test that is mutation-verified.
- **Still open, and the reason the row stays PARTIAL:** the issuer is not
  yet fed from `Cell::work` or hosted by `qip-edge-node`. Its observations
  are the cell's own fills, queue positions and post-fill moves, and wiring
  those from the pass into the issuer (and choosing where the node keeps its
  forecasts) is the next slice. The centre's re-scoring from outcomes that
  reach it, and the `qip_lattice_brier{horizon="microseconds_ms"}` series
  that waits on it, are open too.

## What it costs

- A second representation of "a forecast" in `qip-contracts` beside
  `Forecast` (§9.14). It is kept separate because `Forecast` has no
  promotion target and states its instants at millisecond precision.
- A window of 256 outcomes estimates a 30 % fill rate to about ±3 percentage
  points standard error. A sharper model has to beat this one on the Brier
  score to replace it, which is the point of having a baseline.

## What would make this wrong

- A measured need for the cell to *decide* on a microsecond forecast. Then
  it comes through ADR 0195's path, as a promoted distillate whose
  calibration ships on the model, and the boundary suite's file list is
  widened by that ADR, not here.
