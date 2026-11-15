# ADR 0609: A world/market mismatch has one paper-only consumer per kind; a fused digest and replay shards are built as stated below

- **Status**: **Proposed**, 2026-11-15, by lane 04 of wave b52, for the owner
  ruling of 2026-10-31 on R12-TICK-2084 (ADR_THEN_IMPLEMENT). Not accepted
  until a person accepts it.
- **Date**: 2026-11-15
- **Extends**: ADR 0151 decision 1 (an input has a named consumer or is not
  built).
- **Keeps**: ADR 0003 (paper trading), ADR 0006, ADR 0109 (no dependency).

## Context

`qip_kernel::fusion::fuse` names three mismatches between the world model and
the market. The tick desk counted them and dispatched none, so a disagreement
raised nothing.

## Decision

1. An investigation opens a `ResearchQuestion`, handed to the discovery desk's
   scout queue at the rank of the prior it contradicted.
2. An evidence-quality challenge asks discovery for a better source, ranked at
   the quality the evidence lacks. Its only effect on a weight is
   `EvidenceChallenge::lowered`, which is `min`: it may lower, never raise.
3. A relative-value hypothesis enters `falsification::Book` as
   `HypothesisStatus::Proposed`. `ProposedHypothesis` has no quantity, price or
   side, so nothing can size from it; the book dedups on event and instrument.
4. `mismatch::route` returns every mismatch somewhere: the routed total equals
   `FusedState::mismatches().len()`.

## Consequences

The tick desk stays off by default (`QIP_DEEPBRAIN_TICK_EVERY` unset), so no
deployed binary routes a mismatch until it is enabled and entitled. Proposed
hypotheses are not yet evaluated by `Book::review`. Nothing applies
`EvidenceChallenge::lowered` to a live evidence weight yet.

## What would make this wrong

A consumer that derives a size from `ProposedHypothesis`. The type has no field
to size from; adding one needs this record revisited.


---

# Part 2: the per-cell fused digest (R12-TICK-2086)

Folded in from a draft numbered 0610, which belongs to another lane. Implements ADR 0133.

### Decision

1. `qip_contracts::fused_digest::FusedDigest` is the opportunity map addressed
   to one cell in one region, with its own `valid_until`.
2. `qip_kernel::digest::build_for_cell` calls `OpportunityMap::authorise_for`
   first; a region the model card's entitlements do not permit gets no digest.
3. `FusedDigest::read(now, region)` is the only way to take entries out. It
   refuses another region and a stale digest, and drops entries past their own
   expiry.
4. Bounds are arguments, not defaults: `max_entries` and `valid_for` are stated
   by the caller and zero is refused. **Provisional**: no deployment value is
   chosen here; the caller must be given one by the owner, and until then the
   tick desk (off by default) is the only caller candidate.

### Not decided here / remains

- The `fused_digest` slot of `PolicyPayload` is **not** added. It changes the
  signing string and every payload literal, so it is its own versioned change
  (the slot must be off the wire while unproduced, as ADR 0080's slot is).
- No edge crate calls `FusedDigest::read` yet, and the kernel's per-cell
  publication does not call `build_for_cell`.


---

# Part 3: replay shards (R12-TICK-8110)

Folded in from a draft numbered 0611, which belongs to another lane. Under ADR 0114 Amendment 1.

### Decision

1. A replay is a fold over archived segments; a shard takes the segments whose
   index is congruent to its number modulo the shard count
   (`replay_shard::shard_indices`). Disjoint and covering by construction.
2. Shard results are `ComparisonTotals`, sums only, so the merge is
   order-independent and equals the single-process total. Zero shards and an
   out-of-range shard are refused.
3. **Which crate reads `qip-storage`'s `ArchiveReader` into the kernel's
   scoring inputs**: the shard entry point lives in `qip-deepbrain` (an app,
   which may read storage and the kernel), not in `qip-kernel`, which keeps no
   dependency on `qip-storage`. It calls a public seam over
   `Platform::score_declined` / `score_filled` that this lane did **not** add.
4. The job is a Cloud Run Job behind `enable_replay_pool`, sharing no service
   or identity with serving. Cost is the owner's figure (about 105 USD a month
   per the 2026-10-31 ruling, shared with R12-TICK-7020); **provisional**, and
   the variable defaults to false.

### Remains

The seam over `score_declined`/`score_filled`, the `qip-deepbrain` entry point,
and the Terraform (`docs/blueprint/lanes/b52-04-replay-job.patch.md`, which
needs `variables.tf`, a protected path). Closing needs a plan and an apply
showing the job; none was run.
