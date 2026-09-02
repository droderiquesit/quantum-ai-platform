# Disaster recovery

**Written for the retired runtime.** The GKE backup plan this page names left with the cluster under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md); what backs up an execution node's journal now is the disk snapshot schedule in `infrastructure/terraform/modules/backup`, and a Cloud Run service holds nothing to back up. The first instruction on this page — reconcile positions from the venue, never from a backup — is unchanged. The rest needs rewriting for the snapshot schedule and is open work.

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

This section used to say the journal claims were `Retain`, that retained is not backed
up, and that there was no snapshot schedule on those volumes. There is one now, and a
second mechanism beside it. What is left is narrower, and most of it is one step a
person has to take.

**The journals are backed up two ways, and only one is automatic.**
`infrastructure/terraform/modules/backup` provisions both:

* A **Backup for GKE plan** over the `qip` namespace — daily, including volume data,
  retained 35 days, undeletable for the first 7. It selects by namespace, so it needs
  nobody to remember anything and a cell added later is covered from its first backup.
  It captures the Kubernetes objects as well, so a restore puts back a StatefulSet, a
  claim and the knowledge of which replica owned which disk, rather than a bare block
  device.

* A **Compute Engine snapshot schedule** — daily, retained 90 days, with
  `on_source_disk_delete = KEEP_AUTO_SNAPSHOTS`. This is the half that covers a journal
  *after* its claim is deleted, which is exactly what `qip-journal`'s `reclaimPolicy:
  Retain` leaves behind and what the backup plan stops seeing. Its snapshots are stored
  in a multi-region, so they survive losing the region too.

**The snapshot schedule has to be attached to each disk by hand.** This is the live gap
now. Terraform creates the schedule and cannot attach it: a resource policy attaches to
a disk, and the journal disks are named `pvc-<uuid>` and created by the CSI driver when
a cell's pod is first scheduled, long after any apply. The agreed handle between the two
halves is the label `qip-journal=true`, which the StorageClass stamps on every journal
disk.

[Attaching the schedule](#attaching-the-schedule) below is the procedure. The command
with this deployment's project and schedule name already filled in is:

```sh
terraform -chdir=infrastructure/terraform output -raw journal_snapshot_attachment_command
```

and the check that nothing was missed — run it after adding a cell, because that is when
a new row appears:

```sh
gcloud compute disks list --project <project> \
  --filter="labels.qip-journal=true AND -resourcePolicies:*" \
  --format='table(name, zone, labels.qip-environment)'
```

An empty result is the correct state. A disk in that list is still covered by the backup
plan — until somebody deletes its claim, after which it is covered by nothing.

**By default the backup plan does not survive losing the region.** Its backups are
stored where the plan is, which is the cluster's own region. `backup_location` moves
them, at the cost of cross-region transfer on every backup and a slower restore. The
`journal_backup` Terraform output reports which of the two this deployment has, as
`survives_region_loss`. The disk snapshots survive it either way.

**With `QIP_STORAGE_TARGET=memory` there is still no chain to recover.** That is the
default, and the start-up banner says `NOTHING SURVIVES A RESTART` — which is honest, and
means DR for the audit trail begins with choosing `engine` and a real volume. No
Terraform closes this; it is an application setting. Once it names a real volume in the
`qip` namespace, the backup plan above covers that volume too with no further change.

**Nobody has restored from any of this.** An untested restore is a belief, and
[Restoring a cell journal](#restoring-a-cell-journal) says as much about itself. Doing
it once, deliberately, into a scratch namespace is what turns the paragraphs above from
a configuration into a recovery.

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

## Snapshotting a cell journal

The Kubernetes half of this is in `infrastructure/kubernetes/base/journal-storage.yaml`,
and it is two objects of which only one is live.

**Live: a `StorageClass` named `qip-journal`**, applied by the deploy pipeline,
named by `edge-cell.yaml`'s claim template. It changes two things about a
journal volume and nothing else:

* `reclaimPolicy: Retain`, which is what makes the word "retained" cover the
  disk and not only the claim. The retention policy on the StatefulSet stops
  Kubernetes deleting the *claim*; the class is what stops the disk going with
  it when somebody finally does delete the claim, which is the last step of
  taking a cell out.
* `provisioner: pd.csi.storage.gke.io`, which is what makes the volume
  snapshottable at all. A snapshot is taken by the driver that provisioned the
  disk, so a journal left to an older cluster's in-tree default class cannot be
  snapshotted by any CSI snapshot class — and the way that is discovered is a
  schedule reporting success against volumes it never touched.

**Not live: a `VolumeSnapshotClass`**, in the same file, commented out. It is a
CRD, the cluster module now asserts the PD CSI driver, and the deploy pipeline runs
`kubectl apply --server-side --dry-run=server` over the whole rendered
directory — so an unknown kind fails that gate for every manifest, not just its
own. The preconditions for uncommenting it are listed where it is.

### Before the first cell, not before the first snapshot

```sh
kubectl get csidriver pd.csi.storage.gke.io
kubectl get crd volumesnapshotclasses.snapshot.storage.k8s.io
```

If the first is empty the class above provisions nothing either, the cell's
journal claim sits `Pending`, and the cell never starts. That is the correct
failure and it is much easier to read here than at 3am.

### Attaching the schedule

A Compute Engine snapshot schedule is a resource policy, and a resource policy
attaches to a *disk*. The disks under these claims are named `pvc-<uuid>` and
are created when a cell's pod is first scheduled — after any `terraform apply`,
with a name nothing could have predicted. So the schedule is created by
Terraform and attached by hand, once per cell, after its pods have run:

```sh
gcloud compute disks list --filter="labels.qip-journal=true" \
  --format="value(name,zone)"

gcloud compute disks add-resource-policies <disk> --zone <zone> \
  --resource-policies <schedule>
```

Two replicas per cell means two disks per cell, and the claim behind each is
named `journal-qip-edge-<cell>-<ordinal>` — the claim template's name, the
StatefulSet's name, the ordinal. `kubectl get pvc -l qip.io/cell=<cell>` lists
them, and `kubectl get pv` maps each to the disk the commands above want.

**What this assumes about the Terraform side.** That the schedule is a
`google_compute_resource_policy`, and that the agreed handle between the two
halves is the label `qip-journal=true` that the StorageClass stamps on every
journal disk — not a disk name, which cannot exist before the disk does. If the
Terraform half instead expects to name disks it created itself, then the
journal has to become a statically provisioned volume and the claim template
above is the wrong shape. Check that the two agree before relying on either.

## Restoring a cell journal

Not run against a real project. It is written from the configuration, and the
first person to follow it should expect to correct it.

The cell must be stopped first. A pod holding the claim is a pod writing to it,
and 60 seconds of termination grace is there so a cell cancels its resting
orders before it goes.

```sh
kubectl scale statefulset qip-edge-<cell> --namespace qip --replicas=0
```

**From a Compute Engine snapshot** — the path the schedule above produces:

1. Create a disk from the snapshot, in a zone the cell can schedule into. The
   cell is pinned to its region by a node selector, and the disk must be zonal
   within it.
2. Delete the claim that the restored disk replaces
   (`journal-qip-edge-<cell>-<ordinal>`). Under `qip-journal` this leaves the
   old PersistentVolume `Released` rather than deleting it — deliberately, so a
   restore that turns out to be the wrong snapshot has not destroyed the thing
   it was replacing.
3. Create a PersistentVolume over the restored disk with a
   `csi` source naming `pd.csi.storage.gke.io`, and a claim of the same name
   bound to it.
4. Scale the StatefulSet back up. The ordinal takes the claim by name.

**From a `VolumeSnapshot`** — once the snapshot class is uncommented, steps 1
to 3 collapse into creating the claim with a `dataSource` naming the snapshot,
under the name the ordinal expects, before scaling back up.

Both paths restore into the cluster the cell already runs in. Restoring into a
*different* region is a different exercise and a much larger one — Compute
Engine snapshots cross regions, and nothing else here does. See
[multi-region](multi-region.md) for what the second region would have to be
before there were anywhere to restore to.

Then, and this is the step that is not optional:

5. **Verify before trusting it.** `verify_continuity` proves the journal is
   intact across restarts and `verify_against` proves this segment follows the
   digest the centre last saw. A restore from a snapshot taken mid-write is
   exactly what these exist to catch. The same rule as the chain applies: if
   verification fails, do not let the cell append to it — restore an earlier
   snapshot and verify that.

6. The cell still rebuilds its books from the feed and still will not trade
   until it holds a verified capital envelope. Restoring the journal restores
   the record of what it did, not the state it was in.

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
