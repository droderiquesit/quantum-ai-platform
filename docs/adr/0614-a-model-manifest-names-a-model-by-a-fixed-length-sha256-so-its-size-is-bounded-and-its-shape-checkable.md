# ADR 0614: A model manifest names a model by a fixed-length SHA-256, so its size is bounded and its shape checkable

- **Status**: **Proposed**, 2026-11-15. Needs the owner's ruling before any
  code: it changes a value pinned digests depend on (ADR 0149 decision 17).
- **Date**: 2026-11-15
- **Extends**: ADR 0110 (signed model packs), ADR 0149 (an added field never
  moves a digest).
- **Row**: R12-FABRIC-7060.

## Context

The requirement: the fabric carries only digests, and a model manifest is
size-bounded with unknown fields refused. `ModelManifest` already refuses
unknown fields and carries no weights. What cannot be enforced is the other
two clauses. The one production producer (`qip-kernel/src/model_serving.rs`)
fills each manifest value with `DistilledModel::digest()`, a `|`-joined
encoding of the model's name, form and every coefficient's bits. It is not a
fixed-length hash and it grows with the model. A shape check (`algo:64-hex`)
would refuse every manifest the kernel issues. A byte bound would cap model
size, not announcement size.

## Decision (proposed)

1. A manifest value is `sha256:<64 lowercase hex>` of the model's canonical
   form, the encoding `DistilledModel::digest()` returns today.
2. The centre, the cell and the release pack's `InlineModel` compute it with
   one function in `qip-contracts`, so the three cannot disagree.
3. `ModelManifest` decoding refuses a value of any other shape and a manifest
   over `MAX_MANIFEST_BYTES`. The figure is the owner's to name; no lane
   chooses it.
4. Pinned digests that today hold the long encoding are migrated in one
   commit. This is the step that touches what ADR 0149 decision 17 keeps
   byte-identical, so it is not taken without the ruling.

## What it costs

The fabric then carries only fixed-length references, and a model's weights
cannot ride a payload by being folded into a digest string. In exchange, the
migration invalidates every signed payload that named the old long form, so
the release pack must be re-signed through the two-person publish, and pinned
digests that depend on the old encoding (ADR 0149 decision 17) change in one
commit that waits on the owner's ruling. The centre, the cell and the release
pack all take on one shared hashing function in `qip-contracts`. Hashing the
canonical form is a step on top of producing it, wherever the digest is
computed, and its cost is unmeasured. Until `MAX_MANIFEST_BYTES` is named and
the migration lands, the size bound and the shape check stay unenforced.

## What would make this wrong

The decision should be revisited if the owner rules that pinned digests must
stay byte-identical and no migration is allowed, since step 4 then cannot be
taken. It should be reopened if the three computing sites cannot share one
function in `qip-contracts` without a dependency inversion, or if a second
manifest producer appears whose model has no canonical form to hash.

## Not decided here

`MAX_MANIFEST_BYTES`; the migration date; the producer and consumer binary and
the object-storage fetch path for the announcement (separate rows).
