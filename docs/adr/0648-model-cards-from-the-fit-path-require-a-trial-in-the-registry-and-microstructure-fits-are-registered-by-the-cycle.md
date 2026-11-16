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

## What it costs

A fitted card from `register_fit` can no longer be promoted until someone records a completed trial for it, unless the card is `self_fitted` and so gated by its shadow stage. Nothing in production records a trial round, so every microstructure card the deep brain now registers stays unpromoted. That is the intended restrictive state, but it means the trainer produces registry entries that cannot reach a cell yet. A pass registers one card per (venue, family) the folds support, and a new one beside the old only when that family's fold rows have grown; this record holds no measurement of the registry growth or of the cost of the extra fit on the deep brain's learning pass. The gate reads a flag on the card rather than applying to every card, so a card registered by any path other than `register_fit` is not covered, and the fixtures promoting cards that are not training-pipeline fits are left alone by design. A round's evidence remains the caller's sentence, and neither row is complete.

## What would make this wrong

A fitted card that bypasses `register_fit` and reaches decision eligibility with no trial would show `register_fit` is not the one registration every fitted candidate passes through, which this record assumes. If the registry growth from a new card per (venue, family) each time its fold rows grow shows up as a storage or journal-size problem, the version scheme `0.<rows>` is not bounding it well enough. If trial rounds are never wired to event-log fills, so that no microstructure card is ever promoted, the gate would only be blocking a feature and the decision should be revisited. A round's evidence is the caller's sentence today; if cards come to be promoted on rounds recorded that way, before rounds are tied to event-log fills, the requirement protects less than it reads.
