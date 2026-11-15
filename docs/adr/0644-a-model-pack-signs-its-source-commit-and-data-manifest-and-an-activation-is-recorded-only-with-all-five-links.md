# ADR 0644: A model pack signs its source commit and data manifest, and an activation is recorded only with all five links

- **Status**: **Accepted 2026-11-14 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11** (a session decides the in-repository design
  question it meets, by ADR). Wave b52, lane 22. The delegation's limits hold:
  it sets no figure, relaxes no limit, guard or refusal, touches none of the
  three paper-trading layers, and adds no dependency.
- **Date**: 2026-11-14
- **Extends**: ADR 0110 and its Amendment 1 (the two-person `model-pack`
  release). It amends no clause of either.

## Context

R12-MODEL-10180: every model executing in Reflex can be traced to its source
commit, data/evaluation manifest, signature, approval event and activation
epoch. Two of the five had no home. `ModelPack` signed an artifact digest, the
features, calibration, universes, budget, expiry, rollback parent, the
two-person publication and the inline model — and neither the commit that
built the model nor the manifest of the data it was fitted and judged on. A
cell that activated a pack kept a metric, not a record.

## Decision

1. **`ModelPack` gains an optional `provenance`** (`PackProvenance`: a full
   commit id, 40 or 64 lower-case hex, and the SHA-256 of the data/evaluation
   manifest, 64 lower-case hex). It is appended to `signing_payload` behind
   the tag `provenance/v1`, after the inline model, so a pack signed before
   the field existed keeps its bytes and its signature. Malformed values are
   refused by `ModelPack::validate`, never trimmed or padded: an abbreviated
   commit names more than one commit once history grows.
2. **`ActivationRecord::admit` is the only constructor from a pack.** It
   verifies the pack under the `model-pack` key first, refuses a pack with no
   provenance rather than filling placeholder links, and carries the five
   links: source commit, data manifest, signature, the two-person approval,
   and the activation epoch. `validate` refuses a record missing any one and
   names it.
3. **A centre's promotion is not an activation.** The epoch is the policy
   sequence a cell applied; a promotion the centre journals has no epoch and
   makes no record.
4. **No workflow change is needed.** `qip model-pack signing-message` reads
   the pack JSON and `signing_payload` covers the new field, so a pack file
   that carries `provenance` is signed over it with no edit to
   `.github/workflows/model-pack-publish.yml`.

## Consequences

- A pack without provenance still verifies and still releases (backward
  compatible), but no activation record can be made from it.
- Not decided here, and still open: the cell-side journal that writes an
  `ActivationRecord` where `qip-edge-node` admits a manifest (today
  `PackageActivation` is a metric), and the training pipeline that supplies a
  commit and manifest digest to the pack draft.
