# ADR 0140: Every material forecast carries the forecast contract, and a number that reaches a size is read off one

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). Contract changes inside the tree; no
  dependency, no paid resource.
- **Date**: 2026-10-18
- **Extends**: ADR 0005 (confidence is arithmetic), ADR 0088 (a belief is a
  distribution with evidence and its own TTL), ADR 0125 (staleness may
  narrow a size and an expired forecast vetoes).
- **Keeps**: `Decimal` for money and `f64` for statistics, with the crossing
  stated where it happens.

## Context

`qip_contracts::forecast::Forecast` (`Validated<ForecastFields>`) is the
platform's forecast contract: id, model and version, target, issued-at,
horizon, outcome distribution, uncertainty decomposition, tail mass and the
number of models consulted, refused at construction and at decode when any
is missing. R12-WORLD-10187 asks that every material forecast use it. Three
do not:

- `qip_contracts::feature::ForecastLattice` holds a feature key, an instant
  and a `Vec<Distribution>`, and computes disagreement from their medians.
  Its members name no model, horizon or expiry, so a disagreement cannot be
  traced to the models that disagreed.
- `qip_capital_fabric::forecast::DemandForecast` holds a location, a kind, an
  as-of and a horizon in its own shape.
- The thesis a size is computed from: `qip_portfolio_engine::construction::ApprovedThesis`
  carries `conviction` and `expected_return` as bare `f64`s "computed
  upstream from the thesis, never chosen here" — from what forecast, the
  type does not say.

Locate them with
`grep -n 'pub type Forecast\|pub struct ForecastFields' backend/crates/libs/qip-contracts/src/forecast.rs`,
`grep -n 'pub struct ForecastLattice' backend/crates/libs/qip-contracts/src/feature.rs`,
`grep -n 'pub struct DemandForecast' backend/crates/services/qip-capital-fabric/src/forecast.rs` and
`grep -n 'pub struct ApprovedThesis' backend/crates/services/qip-portfolio-engine/src/construction.rs`.

## Decision

1. **"Material" has one definition**: a forecast is material when it reaches
   a size, a capital plan, a published record or a promotion decision. A
   number used only inside one function to rank candidates before any of
   those is not.
2. **The lattice's members are `Forecast`s.** `ForecastLattice::new` takes
   `Vec<Forecast>` for one target and horizon and refuses members whose
   target or horizon differ; disagreement is computed from their
   distributions as today, and each member names its model, so the lattice's
   disagreement record names who disagreed.
3. **A demand forecast is a `Forecast` with a capital target.**
   `DemandForecast` keeps its location and kind and holds a `Forecast` whose
   target is spelled from them; its own as-of and horizon fields go, so the
   two cannot disagree.
4. **A thesis carries the forecast it is sized on.** `ApprovedThesis` gains
   the `Forecast` of the instrument's return over the thesis horizon;
   `expected_return` is read off its distribution and `conviction` is
   computed from it by a stated function, in the kernel, with the
   `f64`-to-`Decimal` crossing commented where it happens. A thesis without
   a forecast is refused at `validate`, and an expired one is vetoed under
   ADR 0125.
5. **Old records stay readable.** Journaled lattices, demand forecasts and
   theses written before the change decode with the forecast absent and are
   never re-sized from; nothing rewrites a sealed record.

## Consequences

- Every number that moves a size can be traced to a model, a version and a
  horizon from the log alone, which is ADR 0007's attribution applied to the
  input of a decision as well as its outcome.
- Code slices left: lattice members as `Forecast` with a refusal test for a
  mismatched target; `DemandForecast` holding a `Forecast` with a round-trip
  test; `ApprovedThesis` carrying a `Forecast` with a test that a thesis
  without one is refused and that `expected_return` equals the
  distribution's mean (WORLD-10187). Each needs its mutation.

## What it costs

- Every producer of the three types changes, including tests that build
  theses by hand; a test that cannot state a forecast's model and horizon
  now has to.
- A forecast record is larger than an `f64`; theses and lattices in the log
  grow accordingly.
- `qip-portfolio-engine` gains a direct dependency on `qip-contracts` if it
  does not have one, which is an inward edge and allowed.

## What would make this wrong

- If a material decision ever rested on a quantity that is not a forecast (a
  deterministic schedule, a contractual amount), forcing it into the contract
  would invent a distribution nobody estimated; such a quantity needs its
  own typed record, not a `Forecast` with zero variance.
- If the forecast contract's fields proved too narrow for a family of models
  (for example a multivariate joint forecast), the contract needs a version,
  and this record's "is a `Forecast`" would then mean "is a version of it".

## Rows

R12-WORLD-10187. It stays PARTIAL until its slices land with a mutated test.
