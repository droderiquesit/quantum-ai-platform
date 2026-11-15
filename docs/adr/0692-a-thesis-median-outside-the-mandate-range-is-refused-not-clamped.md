# ADR 0692: A thesis median outside the mandate range is refused, not clamped

- **Status**: Accepted, 2026-11-15, by lane 46 of wave b52 under the wave's
  delegation. Restrictive only: it removes an admission path and adds none.
- **Date**: 2026-11-15
- **Keeps**:
  - ADR 0140: a thesis is sized only on a validated `Forecast`.
  - ADR 0003: paper trading. The three layers (the Terraform refusal in
    `variables.tf`, `AutonomyLevel::deployable` at the composition roots, the
    `Cell` constructor) are not touched.

## Context

R12-WORLD-10187's last reader named an open defect: the median of a thesis
forecast was `(anomaly.expected - anomaly.observed).clamp(-0.5, 0.5)`. The
workspace principle is "refuse rather than guess; validate inputs, do not
clamp them". A clamped median is recorded in the forecast and its lineage as
what the model argued, although the measurement was outside the range. The
forecast contract's distribution then looked ordinary and the sizing took it
at face value.

## Decision

1. `qip-kernel/src/thesis_median.rs::reversion_median` returns the difference
   unchanged when it is finite and within +/-0.5, and refuses otherwise.
   `Platform::thesis_from` calls it with `?`, so a refused median means no
   thesis and no capital committed for that opportunity.
2. The bound 0.5 is **provisional**: it is the old clamp's bound, kept so no
   figure that was admissible before is refused for being near it and none
   that was clamped is admitted.
3. The refusal is restrictive only. A thesis that is not built is not sized.

## Consequences and what is not claimed

- Nothing was compiled or run when this was written (owner instruction of
  2026-11-15). The pipeline is the first to build it.
- **Unit risk, stated plainly.** `Anomaly.observed` and `.expected` are in the
  detector's own series unit (`detector.rs` sets `observed: latest` for price
  levels, and volatility or probability for other kinds). Where a detector
  reports price levels, `expected - observed` is a price difference, not a
  return, and the old clamp was hiding that by pinning it to +/-0.5. Refusing
  will now surface it as a refused thesis for those anomalies. That is the
  honest outcome, and a person should decide whether the median should become
  a relative return (`(expected - observed) / observed`), which is a model
  change this ADR does not make.
- Not addressed: `DemandForecast` is still not a `Forecast`; one expired
  thesis still refuses a whole construction batch; epistemic variance is still
  `variance / n`.

## What would make this wrong

If the pipeline shows the cycle producing no theses on its committed fixtures,
the anomaly's unit, not this refusal, is the defect; fix it with an ADR that
states the return the median is.
