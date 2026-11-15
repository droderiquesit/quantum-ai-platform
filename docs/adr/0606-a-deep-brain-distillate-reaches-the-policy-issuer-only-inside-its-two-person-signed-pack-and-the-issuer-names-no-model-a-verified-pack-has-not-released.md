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

## What it costs

- A distillate the deep brain promotes waits for two people to sign a release
  pack before any cell can see it. There is no faster path by design, so the
  delay between promotion and use is whatever the two-person release takes, and
  it is unmeasured.
- Every promotion a cell is meant to run needs a `qip-cli model-pack` run
  against its artifact digest. The command refuses a promotion with no
  permitted locations, and a `ModelPromotion` reads its locations off the
  registry card and never from the caller, so such a promotion stays
  unreleased until its card names them.
- The pack's signed contract grows (`ModelPack::signing_payload`). A pack
  signed before this change carries no distilled entry, so releasing a
  distillate takes a new pack. Whether an earlier pack keeps its bytes is the
  implementation's to hold: the payload's two existing optional tails are
  appended only when present for that reason.
- The distilled entry (`DistilledManifestEntry`, a name and a digest) and the
  permitted locations travel inside signed bytes, so `qip-api` names no
  distillate to a cell that the two signers did not release.
- The row stays PARTIAL until a deployed cell installs one; no node is
  deployed. `qip-cli model-pack` does not yet take a promotion and `ModelPack`
  holds no distilled entry, so the mutations the implementation must show
  firing are still owed: admit a promotion without a verified pack; drop the
  permitted locations from the signed payload.

## What would make this wrong

- A second consumer, such as a research cell, needs promotions before a
  release. A read-only channel would then be a new decision, never a widening
  of this one.
- The two signers sign packs without reading the distillate, which would make
  the two-person control nominal.
- The wait between promotion and a signed pack proves long enough that
  promotions go stale before a cell can use them.
- The payload change cannot be made so that packs signed before it keep their
  bytes, and every earlier pack has to be signed again.
