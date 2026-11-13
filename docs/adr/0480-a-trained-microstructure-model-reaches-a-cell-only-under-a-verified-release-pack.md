# ADR 0480: A trained microstructure model reaches a cell only under a verified release pack

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**,
  session t50-27. It adds a trainer and narrows what the manifest names. It
  widens no authority, adds no dependency, and touches no paper-trading
  layer.
- **Date**: 2026-11-13
- **Rows**: R12-MODEL-7022 (stays PARTIAL; see "What remains").

## Context

R12-MODEL-7022 reads: "Microstructure training outputs are packaged through
the Model Registry into warm/reflex-compatible artifacts", verified by "a
trained microstructure model reaches the edge only as a registry-signed
ModelPack".

Before this decision the kernel kept a running microstructure *estimate*
per venue (`qip_kernel::microstructure_model`, R12-MODEL-1198), folded in
LEARN from the cells' concluded deltas and journaled as fold records. It
was a statistic, not a model: nothing fitted weights from it, nothing
registered them, and nothing a cell could compute was traceable to a
registry card. Separately, ADR 0110 gates the `trained_models` slot on
release packs — but only once a composition root admits some. With none
admitted every promoted distillate is named to the cells, which is the
pre-release reading kept for shared-key cells.

## Decision

1. **A trainer, over the log.** `qip_kernel::microstructure_training` builds
   one dataset per (venue, residual family) from the `MicrostructureFold`
   records LEARN journals — the cell's modelled figure as the single input
   `modelled`, the venue's observed figure as the target, ordered by window
   end — and fits it with the same `LocalTrainer` linear family every other
   candidate uses. `Platform::train_microstructure_model` reads the folds
   from the platform's own log, so a fit is reproducible from the log alone.
   Only fill probability, slippage and impact are trained, the three the
   estimate folds. `Decimal` crosses to `f64` once, in `dataset`, and a
   residual with no finite form is refused rather than dropped.
2. **Registered under a purpose the promotion reads.** `register` runs the
   platform's `register_fit` (the skill bar and the card) and sets the
   card's purpose to `MICROSTRUCTURE_PURPOSE`. `distil` turns the linear
   teacher into the `DistilledModel` a cell evaluates inline — exactly, since
   a one-feature linear fit already is such a function.
3. **The trainer promotes nothing.** Promotion is `Platform::promote_model`
   with every gate it already has: the four promotion tests, a completed
   trial, the verifier society, the model desk.
4. **The release pack is required, always, for this purpose.** A promotion
   of a card whose purpose is `MICROSTRUCTURE_PURPOSE` carries
   `RELEASE_PACK_REQUIRED_STAGE` among its signed stages — read off the
   card, never the caller. `Platform::model_manifest_targeted` withholds such
   a promotion, counted as unreleased, whenever no release pack is admitted;
   with packs admitted, ADR 0110's rule applies unchanged (a verified pack
   must name the artifact and the inline model). The requirement lives in
   the signed stages, so it survives a restart and a record edited to drop
   it no longer carries the centre's signature.

## What it costs

The deep brain now has a registration step it must run before a
microstructure estimate can inform a cell, and a desk that promotes one
carries a release-pack requirement that no other distillate carries — a
publish-workflow signature per venue and family. A deployment that admits no
packs, which is every deployment today, cannot name a microstructure model at
all; that is the point, but it means the estimate keeps narrowing venues
(R12-MODEL-1198) while the trained model waits for a signed release. The
trainer reads the whole fold log each time, which is linear in the records
LEARN has journalled.

## Consequences

- A microstructure model can reach a cell only beside a two-person pack
  signed under the `model-pack` key; a bundle cell additionally verifies that
  pack itself (`ModelManifest::check_releases`), so a compromised issuer
  naming the model without its pack is refused at the cell.
- A deployment that has admitted no release packs still names every other
  distillate as before. Only this purpose is narrowed.

## Evidence

- `cargo test -p qip-kernel --test microstructure_release` — the centre
  withholds the trained model with no pack, with a pack the verifier
  refuses, and with a verified pack for another artifact; names it with the
  pack beside it under a verified release. Three mutations killed.
- `cargo test -p qip-acceptance --test microstructure_model_pack` — the
  kernel-issued manifest installs the trained model in a bundle cell, which
  evaluates the fit; the same model named without its pack is refused by the
  cell. Two mutations killed.

## What remains

- No stage of the cycle calls the trainer: LEARN folds residuals, and a desk
  must call `train_microstructure_model`, `register`, the promotion tests,
  the trial and `promote_model` itself. No composition root does yet.
- The trainer fits from the folds only, not from cell fills directly; one
  row per residual, unweighted by its sample count.
- Nothing has been deployed, and no publish workflow has signed a
  microstructure pack.

## What would make this wrong

A second producer of microstructure distillates that registers under another
purpose would bypass decision 4. The purpose is the boundary; a reviewer
adding such a producer must route it through `register` here.
