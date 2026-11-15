# ADR 0648: A fitted card requires its trial in the registry, and the cycle registers microstructure fits

- **Status**: Accepted 2026-11-15, lane 24 of wave b52, under the standing
  owner delegation. Narrows what a registry will promote and adds one caller;
  widens no authority, adds no dependency, touches no paper-trading layer.
- **Date**: 2026-11-15
- **Rows**: R12-MODEL-4022 (gap 1), R12-MODEL-7022 (cycle-stage clause).

## Context

ADR 0436 made the kernel's `promote_model_from` refuse an untried
training-pipeline candidate, but `ModelRegistry::promote` and
`promote_artifact` did not read the trial, so a caller holding the registry
reached decision eligibility with none. Moving the gate onto every card
would touch about eighty call sites, nearly all fixtures promoting cards
that are not training-pipeline fits.

ADR 0480 left the microstructure trainer reachable from a test only.

## Decision

1. `ModelCard::trial_required` (default false, absent on old cards).
   `central::models::register_fit`, the one registration every fitted
   candidate passes through, sets it. `ModelRegistry::require_promotable`
   refuses a card with `trial_required` and no completed `TrialRecord`,
   unless the card is `self_fitted` (its shadow stage is its gate).
   Verified against the kernel suites that promote fitted cards:
   `models`, `serving`, `promotion_durability`, `promotion_tests`,
   `regime_boundary`, `release_packs`, `model_tournament`,
   `microstructure_release`: all pass unchanged.
2. `microstructure_training::train_and_register_all` registers one model per
   (venue, family) the log's folds support, at version `0.<rows>` so a pass
   is idempotent. The deep brain's `engine.maybe_learn` calls it through
   `LearningDesk::train_microstructure_models` before the registry is
   journaled. It registers only.
3. A registered microstructure card therefore needs a recorded trial and then
   a verified release pack before any cell is named it. Nothing records a
   trial for one yet, so none is promoted: that is the intended restrictive
   state, not an oversight.

## What remains

Nothing records trial rounds in production, and a round's evidence is the
caller's sentence; tying rounds to event-log fills is bar part 4. No publish
workflow signs a microstructure pack, and the fit reads folded residuals, not
fills. Neither row is complete.
