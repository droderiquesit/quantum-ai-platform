# ADR 0724: The Meta Brain measures models by regime from the forecaster tournament's own record

- **Status**: Proposed 2026-11-22, for the owner
- **Date**: 2026-11-22
- **Rows**: R12-AMBIENT-3024

## Context

R12-AMBIENT-3024 asks for a Meta Brain that measures "which brains/models are
reliable in which regimes". ADR 0466 built the brains half: research memory
carries the regime of every charged call, and the Meta Brain reads the
per-(agent, regime) record. Under "Not decided here" it left the models half
open, on the stated ground that the tournament and the deep brain's registry
hold no regime key.

Half of that ground does not hold. The forecaster tournament
(`qip_learning_engine::tournament::Tournament`) has always filed every scored
forecast under the regime its question was opened in; `Tournament::scorecards`
already reports one card per (forecaster, regime). What was missing was a path
from that record to the specialist. The deep brain's registry of production
models is a different matter and still holds no regime key.

A first change on 2026-11-22 supplied the path by writing the record to the
desk's model-registry slot and granting the Meta Brain `read_model_registry`.
Two independent readers refused it. Decision 2 of ADR 0466 says the Meta
Brain's manifest grants `read_research_memory` "and nothing else", and that
gate opens the whole `ModelsView` — forecaster health, drift, everything the
model-failure probe reads — to an agent that needed one list.

## Decision

1. **Models are measured by regime from the tournament's own record.**
   `Tournament::regime_briers` returns, per (forecaster, regime), the mean
   Brier over the forecasts scored in that regime and how many there were. A
   cell with no scored forecast is absent, never a zero: a Brier of zero is a
   perfect record.
2. **The record is handed, not gated.** `Platform::stage_reason` calls
   `hand_the_arbiters_the_models_record` before the panel is convened; it
   passes the cells to `Organisation::hand_model_records`, and the second
   dispatch phase puts them on the `PanelView` every arbiter receives
   (`PanelView::model_records`). No desk facility holds them, no panel agent
   sees them, and no capability is introduced or widened. **Decision 2 of
   ADR 0466 stands unamended**: the Meta Brain's manifest grants
   `read_research_memory` and nothing else, and a test pins the set.
3. **What the Meta Brain reports.** One `model_brier:{model}@{regime}` and one
   `model_scored:{model}@{regime}` fact per cell; `model_regime_reliability_cells`,
   which evidences `Scope::RegimeReliability` beside the analysts' cells;
   and `models_measured_in_regime` / `models_unmeasured_in_regime` for the
   regime the brief names. A model with no scored forecast in that regime is
   counted unmeasured, never read at its record in another.
4. **Point in time.** Only questions LEARN has already resolved are in the
   record, and REASON runs before LEARN in a cycle, so the hand-off carries
   nothing the REASON reading it could not have known.
5. **Nothing acts on it.** The finding stays neutral at zero conviction, as
   decision 6 of ADR 0466 has it. No sizing, routing or order path reads a
   model cell.

## What it costs

- The organisation holds state between a hand-off and a dispatch. A caller
  that dispatches without handing gives its arbiters the previous list, or
  none. The kernel rebuilds it on every REASON; another caller must too.
- "Models" here means the tournament's forecasters: detectors, analysts as
  forecasters, and fitted cards entered in it. A production model in the deep
  brain's registry that has never forecast a tournament question has no cell.
  The register's clause is met for what the platform scores, not for every
  artefact it stores.
- The regime is the one the question was *opened* in. A question that
  straddles a turn is filed under the regime that no longer holds when it
  resolves.
- The finding grows by two facts per cell, bounded by the tournament's own
  `MAX_FORECASTERS` and `MAX_REGIMES`.

## Not decided here

- Whether REASON should use the per-regime model record — to discount a
  forecaster in a regime it has failed in — remains open, as ADR 0466 left
  the use of the resolution open. The verifier society's model-risk critic
  already reads the meta-model's predicted failure by regime; this record
  does not join the two.
- A regime key on the deep brain's production-model registry.

## What would make this wrong

- If an arbiter other than the Meta Brain is rostered and should not see
  model records: every arbiter receives the same `PanelView`. The hand-off
  would then need to be per arbiter, or gated after all.
- If the owner holds that a typed hand-off is a capability in substance, and
  that anything an agent reads beyond its brief and the panel's findings must
  appear on its manifest. Then decision 2 of ADR 0466 needs amending and the
  right grant is a new, narrow capability over this list alone, not
  `read_model_registry`.
- If the tournament's regime key and `Platform::regime_context` ever diverge
  in form: the count in force compares the two as strings, and every model
  would read unmeasured. The kernel test that turns a tape illiquid and
  compares the Meta Brain's cells with `Tournament::scorecards` is the guard.
