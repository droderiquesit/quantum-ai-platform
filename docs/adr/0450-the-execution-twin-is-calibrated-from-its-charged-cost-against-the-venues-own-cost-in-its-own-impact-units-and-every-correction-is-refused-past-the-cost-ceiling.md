# ADR 0450: The execution twin is calibrated from its charged cost against the venue's own cost, in its own impact units, and every correction is refused past the cost ceiling

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31
  delegation** (in-repository design decisions), session t50-12 (wave580).
  It touches no paper-trading layer: the calibration prices counterfactuals
  and narrows sizing through the existing misfit cap, and reaches no venue.
- **Date**: 2026-11-12
- **Reads**: ADR 0003 (the "live telemetry" is the platform's own paper
  fills), ADR 0062 (the fill error is diagnostic and withdraws nothing),
  ADR 0109 (no dependency is added).
- **Rows**: R12-TICK-2080 (stays PARTIAL; the gap is named below).

## Context

`FillCalibration` (`qip-twin/src/calibration.rs`) moved a per-venue spread
bias and, since lane L3b, an impact slope. The 2026-10-31 skeptic refuted the
row's COMPLETE bar on five counts, each confirmed by reading the code:

1. `score_filled` fed the twin's **printed entry mark against the fill** —
   the venue's slippage — which no change to the twin's cost model can
   shrink. With a base model near the venue, the fit added the slippage on
   top of a cost already charged, and never closed.
2. The slope was fitted in `sqrt(quantity / one bar's volume)`, while the
   twin prices impact at `sqrt(quantity / twenty-bar mean volume)` scaled by
   `sigma / sigma_ref`. A slope in the wrong unit is a coefficient
   correction of the wrong size.
3. The slope had no upper bound. A correction carrying the coefficient past
   `MAX_TRADE_COST_BPS` left the venue `Unpriceable`; an unpriceable trade
   is never scored, so the venue was never calibrated again.
4. `twin_misfit` (R12-RES-7104) read `bias_bps` alone, so a size-dependent
   misfit hid in the slope.
5. Queue and latency have no calibration input at all.

## Decision

1. **The observation is a cost error against the uncalibrated model.**
   `FillCalibration::observe_cost(venue, base, error_bps, impact_unit)`
   takes `base`'s charged cost for the venue's own fill — the quantity the
   venue filled, at the twin's entry mark, against the liquidity the twin
   priced its trade arm with — less the venue's cost: its slippage from the
   order's arrival price plus the fees it billed, over the notional it
   filled. Measured against `base` and not against the calibrated model,
   the target is stationary; the fitted law is `base − venue`, and
   `cost_model`'s existing subtraction lands on the venue.
2. **The twin's impact unit is stated once.** `CostModel::impact_unit(p,
   sigma)` in `qip-simulation-engine` returns the factor the coefficient is
   multiplied by, so `impact_bps == impact_coefficient_bps * impact_unit`.
   `SimulatedOutcome` now carries the `Liquidity` its entry was priced
   against (`entry_liquidity`, `#[serde(default)]`, so earlier records read
   `None`), and the kernel derives the unit from it rather than estimating
   the liquidity a second time from the bars.
3. **Every correction is refused past the ceiling, not clamped.** A fitted
   slope that would carry `base`'s impact coefficient to
   `MAX_TRADE_COST_BPS` or past it is refused and the previous slope kept;
   so is a bias that would carry `base`'s half-spread there. Each refusal is
   counted on `VenueCalibration::refusals`, reported on the returned
   `Observed`, and surfaced by `score_filled` as a LEARN problem.
4. **The misfit reads both terms.** `VenueCalibration::correction_bps` is
   the larger of `|bias|` and `|slope × the venue's mean impact unit|`, not
   their net — a twin wrong twice in opposite directions is not right once —
   and `twin_misfit` compares that with `TWIN_MISFIT_BIAS_BPS`.
5. **A cell's fill is calibrated from the twin's mark, for now.** Its
   report carries neither the arrival price its order was sent at nor the
   fees the venue billed, so its "cost" is its slippage from the twin's
   entry mark with no fees — the price error and the cost error cannot be
   separated there. Dropping cell fills was tried first and broke
   R12-RES-7104's hold at a cell's venue
   (`central.rs::a_strategy_trading_at_a_cell_venue_whose_twin_misprices_its_fills_is_held`
   reads that venue's calibration), so they stay in, and the limitation is
   named below rather than hidden.

`qip_venue_fill_error_bps` keeps its meaning: it charts the twin's mark
against the fill, a price-model diagnostic, and reads nothing that decides.

## Evidence

- `qip-kernel/tests/learning.rs`
  `after_fills_with_systematic_cost_error_the_twins_spread_and_impact_converge_on_the_venues_true_values`:
  on a tape with a volatility near the reference and four fill sizes, the
  desk's simulated venue (3 bp half-spread, 1 bp commission, and no impact,
  since the kernel supplies it no liquidity statistics) moves the twin from
  2.5 bp and 40 bp to within 0.2 bp of 3 bp and under 2 bp of coefficient.
  Freezing `observe_cost` fails it at `40 -> 40`; feeding the mark error
  fails it at `40 -> 39.99…`.
- `a_fill_the_twin_mispriced_moves_that_venues_calibrated_spread_toward_the_venues_cost`:
  the bias is one fifth of the hand-worked cost error (−0.5 bp), not of the
  −1,002 bp mark error; feeding the mark error fails it at −200.5.
- `a_twin_wrong_only_about_size_is_a_misfit_twin`, and in `qip-twin`
  `an_impact_slope_that_would_price_past_the_cost_ceiling_is_refused_and_the_old_slope_kept`
  and `a_twin_wrong_only_about_size_reports_a_correction_at_the_venues_own_size`;
  in `qip-simulation-engine`
  `the_impact_unit_times_the_coefficient_is_the_impact_the_model_charges`.

## What remains (R12-TICK-2080 stays PARTIAL)

- **A cell's fill cost.** Until `CellReport` carries each order's arrival
  price and the venue's billed fees, a cell's venue is calibrated against
  the twin's mark (decision 5), which is the flaw decision 1 removes for
  the desk.

- **Queue and latency.** The simulated venue does not time
  acknowledgements, and the twin's `CostModel` has no queue term, so
  neither has a calibration input. Closing it needs an acknowledgement
  instant on the simulated fill and a queue term the twin prices; both are
  contract changes to `qip-execution-engine` and `qip-simulation-engine`.
- **A non-zero true impact on the default tape.** The desk's venue charges
  no impact because the kernel never calls `SimulatedBroker::set_liquidity`;
  convergence is proven on the coefficient's correct value of zero, and on a
  non-zero synthetic law in `qip-twin`'s own tests, not on a non-zero law
  the kernel's venue produces.
- **A crisis volatility.** The ceiling is checked at the reference
  volatility; a measured volatility far above it can still scale a sane
  coefficient past the ceiling, as it could before calibration existed.

## What it costs

- A second pricing of every scored desk fill under the base model, in
  `fill_cost_error`: one `cost_of` call, arithmetic only, inside LEARN's
  existing per-cycle cap.
- One `Option<Liquidity>` on every `SimulatedOutcome`. It is
  `#[serde(default)]`, so journaled records written before it read `None`
  and none is rewritten; a counterfactual journal grows by three numbers per
  traded alternative.
- `observe_with_participation` is removed rather than kept beside
  `observe_cost`: two ways to feed the fit, in two units, is the defect this
  decision corrects.
- A venue whose fills demand a correction past the cost ceiling keeps a
  correction short of what its fills state, and says so on every refused
  observation, rather than becoming unpriceable.

## What would make this wrong

A venue whose arrival price is not the price its cost is measured from —
a venue that fills against a book the order never saw — would make the
venue's "cost" a price move, and the spread term would learn the market.
