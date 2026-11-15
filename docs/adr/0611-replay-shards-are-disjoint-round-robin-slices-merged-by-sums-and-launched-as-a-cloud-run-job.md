# ADR 0611: Replay shards are disjoint round-robin slices merged by sums, launched as a Cloud Run Job

- **Status**: **Proposed**, 2026-11-15, by lane 04 of wave b52. Not accepted
  until a person accepts it. The Terraform half is a patch, not applied.
- **Date**: 2026-11-15
- **Implements**: R12-TICK-8110 under ADR 0114 Amendment 1 (replay shards run
  as a Cloud Run Job, not on GKE; ADR 0024). **Keeps**: ADR 0003, ADR 0109.

## Decision

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

## Remains

The seam over `score_declined`/`score_filled`, the `qip-deepbrain` entry point,
and the Terraform (`docs/blueprint/lanes/b52-04-replay-job.patch.md`, which
needs `variables.tf`, a protected path). Closing needs a plan and an apply
showing the job; none was run.
