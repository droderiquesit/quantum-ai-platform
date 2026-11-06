# ADR 0191: The world model lives in the central brains, versioned per domain on the event log, and is not split into its own service before deployment

- **Status**: **Accepted**, 2026-10-24, decided under the owner's delegation
  of the same day (`docs/blueprint/owner-decisions-2026-10-24.md`). The
  delegation covers design decisions. It does not cover billing or new paid
  infrastructure, and a separately deployed world-model service would be
  both.
- **Date**: 2026-10-24
- **Keeps**: ADR 0024 (warm binaries on Cloud Run, one execution node per
  region), ADR 0008 (cells decide alone; no model on the reflex path), the
  event log as the record.

## Context

R12-WORLD-7023 asks that world-model services be long-running and versioned,
with domain-specific stores, and that no single model sit on the reflex
critical path. The second half is proven:
- `lane_placement` and `arch_001_independent_reflex` show the world-model
  crates are lane three and absent from the edge node's compile set.

The first half is not:
- `qip-world-model` is a library composed into `qip-kernel`'s cycle. Its
  state lives in the memory of whichever central binary runs the cycle
  (`qip-api`, `qip-fastbrain`, `qip-deepbrain`), and it is rebuilt by replay
  after a restart.
- It has no store of its own and no version a reader can cite.
- The `world_model.snapshot` topic is declared in `qip-events` and has no
  producer.

**Correction, 2026-10-31 (lane L4-quant, on a skeptic's reading of the
code).** The first bullet above and decision 1 below say the world model is
rebuilt from the log after a restart. It is not. `Platform::new` starts
from `WorldModel::new()` and rebuilds only the evidence fabric, so a
restarted central brain begins with an empty knowledge graph and causal
graph and relearns them from what arrives. Decision 1 stands as a placement
decision: the world model lives in the central brains. The rebuild it
assumed does not exist, and this record's "What would make this wrong"
names that case. Decision 3's snapshots make it visible: the first snapshot
after a restart carries the next version over the relearned content, and
its digest differs from the one before.

## Decision

1. **The long-running world-model services are the central brains.** A
   world model is rebuilt from the log in each of them, and the deep brain
   holds the longest-lived one (`min_instances = 1` in `catalogue.tf`). No
   separate world-model service is built or deployed before the deployment
   phase. A new Cloud Run service or database is billed infrastructure the
   owner put last.
2. **The domain stores are the event log's records, by journalling stage.**
   Several stages already journal under their own origin:
   - the forecast lattice (`kernel/forecast-lattice`);
   - the federation (`kernel/federation`);
   - the NOW brain (`kernel/now-brain`);
   - feature statistics (`kernel/feature-statistics`);
   - causal-edge retirement (`kernel/retirement`);
   - the symbolic stage (`kernel/symbolic`).
   The world graph also publishes its own `WorldModelUpdated` records. A
   domain held only in memory, and not journalled, is the first case
   decision 3 must snapshot. The log stays the record (product direction:
   "bounded retention, the event log is the record").
3. **Versioning is a per-domain snapshot record on `world_model.snapshot`.**
   This is the code this decision requires. Each snapshot carries the
   domain, a monotone version, the counts and a digest of the domain's
   state, so a reader can cite "the causal graph at version N" and a replay
   can be checked against it. It is written by the central brain that runs
   the cycle, bounded per domain, and never read back as state: the log
   remains the record.
4. **A separate, independently deployed world-model service with its own
   store** is the decision for the deployment phase. It takes its own ADR,
   naming the store and its cost, when the owner enables billing.

## Consequences

- R12-WORLD-7023 stays PARTIAL. Decision 3 is code still to land.
  Decision 4 needs a deploy the owner reserved.
- No reflex path gains a dependency. The snapshot is written centrally and
  the edge node reads nothing new.

## What it costs

- A world model held in three processes is three copies kept consistent
  only by the log. A reader comparing them reads three versions, which is
  what decision 3's version number exists to make visible.
- The per-domain snapshot adds one record per domain per interval, which is
  bounded and in the research P-class.

## What would make this wrong

- A domain whose state cannot be reconstructed from its topic. That would
  be a second source of truth, which the boundaries rule forbids, and it
  would force a store before decision 4.
- A measured cost of replay at start-up large enough to stall a binary's
  readiness. That would argue for a store sooner, and is a measurement to
  take, not a guess.

## Rows

R12-WORLD-7023.
