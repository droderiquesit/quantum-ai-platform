# ADR 0436: A training-pipeline or quantum-informed candidate is promoted only after a recorded simulation or paper trial

- **Status**: **Accepted 2026-11-12 under the owner's delegation of
  2026-10-31 and decision 28 of 2026-11-11**
  (`docs/blueprint/owner-decisions-2026-10-31.md`). Session t50-05 writes
  it. The limits of that delegation hold:
  - it sets no new figure: the trial's round count is the registry's
    existing `MIN_SHADOW_ROUNDS`, re-exported as `MIN_TRIAL_ROUNDS`;
  - it touches no paper-trading layer and adds no order path — the trial's
    venue type has no arm that could name a live venue;
  - it adds no dependency and no crate edge.
- **Date**: 2026-11-12
- **Extends**: ADR 0083 (the promote stage), ADR 0392 (promotion tests),
  ADR 0006 (classical baseline). It amends no clause of any accepted ADR.
- **Rows**: R12-MODEL-4022.

## Context

R12-MODEL-4022 asks that models be validated in simulation and paper
trading before acceptance, verified as "a model reaches decision
eligibility only after a recorded simulation/paper trial; skipping it
refuses promotion". On 2026-11-12 only the deep brain's self-fitted
candidates had anything like it: `ModelProducer::SelfFitted` requires a
completed shadow stage (R12-EXPAND-6070). `ModelProducer::TrainingPipeline`
— what `Platform::promote_model` names — and `QuantumInformed` went from
held-out statistics straight to production. An earlier lane tried to require
the shadow stage for them and reverted unpushed, because entering shadow
changes the card's stage, and nine tests assert exact stages.

## Decision

1. **The trial is a record beside the stage, not a stage.**
   `qip_ai::registry::TrialRecord` sits on `ModelCard::trial`, written only by
   `ModelRegistry::record_trial_round(reference, venue, evidence, at)`. The
   card keeps the stage its other gates read while the trial accumulates.
   A round is refused for blank evidence, a card in `Production` or
   `Retired`, and a time not strictly after the last round.
2. **The venue is a two-arm type.** `TrialVenue::{Simulation, Paper}`. No
   third arm exists to be chosen, so a live run cannot satisfy the gate
   however a caller labels it (rule 01, principle 4).
3. **The gate is in the kernel's promotion.** `promote_model_from` refuses a
   `TrainingPipeline` or `QuantumInformed` candidate whose card has no trial,
   fewer than `MIN_TRIAL_ROUNDS` rounds, or a last round dated after the
   promotion instant. The refusal is before anything changes; the log gains no
   record. `SelfFitted` keeps its shadow gate unchanged.
4. **The trial is on the signed record.** Its stage line ("trial since …:
   N round(s) in simulation and paper (…)") is pushed onto
   `ModelPromotion::stages`, which the central plane signs, so the trial a
   promotion rested on is in the log and an edit to it does not verify.

## What this does not do

- `ModelRegistry::promote` and `promote_artifact`, called directly, do not
  read the trial. About eighty call sites across the workspace promote cards
  that way in tests and in-process registries; moving the gate into the
  registry is the structural form and is left open, dated, on the row.
- Nothing in production yet records a trial round. `TrainingPipeline` has no
  production caller (the deep brain promotes only `SelfFitted`), so no
  deployed promotion stops; a pipeline that promotes must first record rounds.
- A round's evidence is the caller's sentence. Tying a round to simulated
  fills the event log holds, so a reviewer can re-derive it from the log, is
  the remaining work of bar part 4.

## What would make this wrong

A production promotion path that names `TrainingPipeline` without a producer
of trial rounds, or a third `TrialVenue` arm.
