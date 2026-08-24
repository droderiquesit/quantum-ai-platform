# Disaster recovery

**Do not restore positions from a backup.** Reconcile them from the venue. Everything
else on this page is ordinary; that one instruction is the one that matters, and the
reasoning is at the bottom.

## What is actually irreplaceable

Most of what the platform holds is rebuildable, and treating it as precious wastes the
recovery window on the wrong things.

| State | Where | On loss |
| --- | --- | --- |
| **Event log hash chain** | `ChainArchive` over `QIP_STORAGE_TARGET` | **Irreplaceable.** The audit trail. Cannot be recomputed from anything. |
| **Edge cell journal** | per-replica 16Gi claim, `Retain` | **Irreplaceable.** What that cell decided and why. |
| **Evidence bucket** | GCS, versioned, KMS, retention policy | **Irreplaceable.** Promotion evidence a regulator asks for. |
| Books, features, watermarks | `emptyDir` | Rebuilt from the feed. Deliberately ephemeral. |
| World model, agent state | memory | Rebuilt by running. |
| Positions and open orders | the venue | **Reconciled, not restored.** See below. |
| Model artifacts | Cloud Storage bucket, versioned | Refittable, but expensively. Restore. |

## Objectives, and what actually sets them

**RPO for the chain: one cycle.** Records reach the archive at cycle boundaries, never
inside a cycle, because a store's latency on the event path is latency the fast node
exists not to have. A crash mid-cycle loses that cycle's events. Shortening this means
writing through on every append, which is a trade against the whole point of the fast
path.

**RPO for the edge journal: the shipping interval.** The journal ships to its mirror on
a schedule; a cell lost between shipments loses what it decided since.

**RTO is bounded by the feed, not by the restore.** A restored process holds no books
and no world model, and rebuilds both by consuming the feed. The restore is minutes; the
warm-up is however long the strategies need history for. `qip-deepbrain` reports
`warming` in its readiness for exactly this reason — until the first cycle lands there
is no world model to consult.

## The gap you have today

**The journal claims are `Retain`, and retained is not backed up.** `Retain` means
Kubernetes will not delete the disk when the claim goes away. It does nothing about a
failed disk, a deleted project, or a region becoming unavailable. There is no snapshot
schedule on those volumes.

**With `QIP_STORAGE_TARGET=memory` there is no chain to recover.** That is the default,
and the start-up banner says `NOTHING SURVIVES A RESTART` — which is honest, and means
DR for the audit trail begins with choosing `engine` and a real volume.

Closing both needs a snapshot schedule on the journal volumes and a durable storage
target for the central plane. Neither is provisioned.

## Recovering the chain

1. Stand up the workload against the same store. `ChainArchive::open` adopts what is
   there and continues.
2. **Verify before trusting it.** `ChainArchive::verify` walks the linkage; the CLI's
   `verify` subcommand does the same. A restore that lost records in the middle fails
   here rather than looking complete.
3. If verification fails, the restore is incomplete. Do not append to it — a chain
   extended past a break records the break permanently and makes the later entries
   unprovable too. Restore an earlier copy and verify that.

The archive keeps its own dense position and linkage rather than the source log's, so a
run that was archived twice or a run that is missing shows up as a break rather than as
a plausible sequence.

## Recovering a cell

1. The StatefulSet gives the replacement pod the same claim, so the journal is there.
2. `verify_continuity` proves the journal is intact across restarts, and
   `verify_against` proves this segment follows the digest the centre last saw.
3. Books and features rebuild from the feed. Do not attempt to restore them.
4. The cell will not trade until it holds a verified capital envelope. If the envelope
   key is unavailable, the cell refuses to start — that is the design, not a fault.

## Why positions are reconciled and never restored

A backup of the platform's position book is, by definition, a picture of what the
platform believed at some earlier moment. Restoring it asserts that belief as current.

Between the backup and the recovery, orders were working. Some filled. The venue knows
which; the backup cannot. Restoring it produces a book that is confidently wrong, and
every subsequent risk check, exposure calculation and hedge is computed against a
fiction — while the venue holds a real position nobody is managing.

The platform already takes this position in code. `qip-brokers`' REST adapter documents
that the venue's records, not the adapter's, are the reconciliation of record; it
persists nothing locally, and an order whose outcome it could not read becomes
`Unknown` rather than being assumed. `reconciliation-break.md` is the runbook for when
the two disagree in normal operation, and after a disaster **every** position is in that
state until proven otherwise.

So: bring the platform up with an empty book, hold it below live autonomy, query the
venue, and let reconciliation populate what is real. It is slower than a restore and it
is the only version that ends with a book that matches the world.
