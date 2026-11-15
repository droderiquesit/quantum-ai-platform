# ADR 0606: A deep-brain distillate reaches the policy issuer only inside its two-person signed pack, and the issuer names no model a verified pack has not released

- **Status**: Proposed, 2026-11-15, by lane 03 of wave b52 to record the owner's
  ruling of 2026-10-31 (decision 14, `docs/blueprint/lanes/wave1031-owner-rulings-2-2026-10-31.json`,
  R12-TICK-2070, `ADR_THEN_IMPLEMENT`). It changes no limit, admits no
  dependency and touches no paper-trading layer.
- **Date**: 2026-11-15
- **Extends**: ADR 0110 (signed release packs hold the `trained_models` slot),
  ADR 0003 (paper only).
- **Tightens only**: nothing a deep brain produces gains a path to a cell that
  it did not have.

## Context

`qip-training::distill::distil` states a student's fidelity to its teacher, and
the deep brain's learning desk distils and journals a `ModelPromotion` into its
own event log. `qip-kernel`'s `resume_model_promotions` reads only the
process's own `EventLog`, and `qip-api` says it "trains and promotes no model".
Outside `qip-kernel`, `ModelPromotion` appears only in `qip-deepbrain`: no
fabric topic or shared store carries it, a shared store is billed and `fabricd`
is not deployed. So a distillate the deep brain promotes has no channel to a
cell, and the obvious one, an automatic one, would let an unreviewed process
name a model a cell runs inline.
`model_manifest_targeted` already withholds a promotion with no matching
release pack; the pack is the existing control.

## Decision

1. **No automatic channel.** A deep-brain promotion reaches `qip-api`'s
   `trained_models` slot only through the two-person release.
2. **The signed pack carries it.** `qip-cli model-pack` takes the deep brain's
   journaled `ModelPromotion` by artifact digest and places its distilled entry
   and permitted locations inside the pack's signed bytes. It refuses a
   promotion with no permitted locations. This changes the pack's signed
   contract (`qip-contracts` `ModelPack::signing_payload`), which is why this
   record exists, in line with R12-MODEL-10180's extension of the same payload.
3. **The issuer admits from a verified pack and from nothing else.** `qip-api`
   admits a promotion into `model_promotions` only from a pack verified in
   `model_trust.rs`. An unreleased model never enters a manifest, and the slot
   is empty before release.
4. **Bytes and activation stay where they are.** Model bytes ride in the compiled
   plan baked into the node image (R12-MODEL-1083); activation stays the pack's
   (R12-MODEL-10180).

## Consequences

- A distillate's stated worst-case cost and fidelity travel inside signed
  bytes, so a cell cannot load one whose bound was not reviewed.
- The row stays PARTIAL until a deployed cell installs one; no node is deployed.
- Mutations the implementation must show firing: admit a promotion without a
  verified pack; drop the permitted locations from the signed payload.

## What would make this wrong

If a second consumer needs promotions before a release (a research cell), a
read-only channel would be a new decision, never a widening of this one.
