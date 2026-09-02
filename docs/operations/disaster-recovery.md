# Disaster recovery

**Do not restore positions from a backup.** Reconcile them from the venue. Everything
else on this page is ordinary; that one instruction is the one that matters, and the
reasoning is at the bottom.

The runtime this page describes is the one `infrastructure/terraform/main.tf`
provisions under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md):
the central plane as Cloud Run services, the execution node as one Compute Engine
machine per region. It has never been applied, no node exists in any environment,
and nobody has restored anything from what is described here.

## What is actually irreplaceable

Most of what the platform holds is rebuildable, and treating it as precious wastes the
recovery window on the wrong things.

| State | Where | On loss |
| --- | --- | --- |
| **Event log hash chain** | `ChainArchive` over `QIP_STORAGE_TARGET` | **Irreplaceable.** The audit trail. Cannot be recomputed from anything. |
| **Execution node journal** | `/var/lib/qip/journal` on the node's boot disk, `auto_delete = true` | **Irreplaceable.** What that cell decided and why. Survives the machine only as snapshots. |
| **Evidence bucket** | GCS, versioned, KMS, retention policy | **Irreplaceable.** Promotion evidence a regulator asks for. |
| Books, features, watermarks | memory | Rebuilt from the feed. Deliberately ephemeral. |
| World model, agent state | memory | Rebuilt by running. |
| Positions and open orders | the venue | **Reconciled, not restored.** See below. |
| Model artifacts | Cloud Storage bucket, versioned, when `enable_cloud_storage` is on | Refittable, but expensively. Restore. |

**The central plane holds nothing durable today.** Every Cloud Run service reads
`QIP_STORAGE_TARGET` from `var.storage_target`, whose default is `memory`
(`infrastructure/terraform/variables.tf:302-321`; `catalogue.tf:60,130,176`),
and the variable's own description says why: a Cloud Run instance keeps nothing
across a restart and has no volume to keep it on. The start-up banner says
`NOTHING SURVIVES A RESTART`, which is honest. DR for the audit trail begins
with a storage target that names a real store — `file` and `engine` are the
other two this build implements — and that is an application setting the
Terraform deliberately does not make for you.

## Objectives, and what actually sets them

**RPO for the chain: one cycle.** Records reach the archive at cycle boundaries, never
inside a cycle, because a store's latency on the event path is latency the fast node
exists not to have. A crash mid-cycle loses that cycle's events. Shortening this means
writing through on every append, which is a trade against the whole point of the fast
path.

**RPO for the node journal: the last snapshot.** The journal is on the boot disk
and the disk is deleted with the instance (`modules/execution-node/main.tf:319-331`).
An instance restart keeps it; a replacement — the group's auto-healing after three
failed health checks (`main.tf:274-279`), a rolling replacement from `deploy.yml`,
or a zone loss — is a new machine with a new disk, and what the old node decided
since its last snapshot is gone. The schedule is daily (`modules/backup/main.tf:71-77`,
starting at `snapshot_start_time`, `05:00` UTC by default).

**RTO is bounded by the feed, not by the restore.** A restored process holds no books
and no world model, and rebuilds both by consuming the feed. The restore is minutes; the
warm-up is however long the strategies need history for. `qip-deepbrain` reports
`warming` in its readiness for exactly this reason — until the first cycle lands there
is no world model to consult.

## The gap you have today

Four things, and the first is the one to act on.

**Nothing is covered until the schedule is attached.** `modules/backup` creates a
Compute Engine snapshot schedule and cannot attach it: a resource policy attaches to a
disk, and the node's disk is created by its managed instance group when it builds the
instance — after any apply, under a name the group chose. The agreed handle between
the two halves is the label `qip_journal=true` the instance template stamps on the disk
(`modules/execution-node/main.tf:326-330`). Until the attachment has been run for a
given disk, that disk is covered by nothing (`modules/backup/NOT-COVERED.md`), and the
`journal_backup` output says so as `covers_before_attach = "nothing"`
(`infrastructure/terraform/outputs.tf:217-234`).

[Attaching the schedule](#attaching-the-schedule) below is the procedure.

**No node exists.** `execution_nodes = {}` in every environment, so there is no disk
to attach anything to. This page is written ahead of the first one.

**The central plane is on `memory`.** See above. No Terraform closes this.

**Nobody has restored from any of this.** An untested restore is a belief.
[Restoring a node journal](#restoring-a-node-journal) says as much about itself. Doing
it once, deliberately, onto a machine an operator controls is what turns the paragraphs
above from a configuration into a recovery.

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

## Recovering a node

1. The group does it. An instance that fails its health check three times is replaced
   from the template (`modules/execution-node/main.tf:270-290,449-457`); host
   maintenance terminates rather than migrates and the instance restarts
   (`main.tf:368-377`). The replacement boots the same image, fetches the same
   envelope key, and starts with an **empty** journal.
2. The old journal is in its snapshots, if the schedule was attached. Restore it to
   read, never to resume — see below.
3. Books and features rebuild from the feed. Do not attempt to restore them.
4. The node will not trade until it holds a verified capital envelope, and on this
   runtime no path delivers one (`QIP_MESH_PEER` is unset; ADR 0024). If the envelope
   key is unavailable, the node refuses to start — that is the design, not a fault.

## Snapshotting a node journal

The schedule is `google_compute_resource_policy.journal_snapshots` in
`modules/backup/main.tf:66-105`: daily; retained `snapshot_retain_days` (90 by
default, `variables.tf:632-646`); `on_source_disk_delete = KEEP_AUTO_SNAPSHOTS`, which
is the line the schedule exists for — a node replaced and its disk auto-deleted still
has to be able to account for what it did; snapshots labelled `qip_journal=true`;
`storage_locations` deliberately unset, so Google stores each snapshot in the nearest
multi-region and they survive losing the region. They are encrypted with a key in the
platform's ring (`main.tf:35-55`, `prevent_destroy`).

There is no `enable_backup` flag, and there is not one in disguise: a switch whose off
position is the gap this page documents would leave that gap in place and add a line
implying otherwise (`variables.tf:610-618`).

### Attaching the schedule

After a node's first boot, and again after every replacement, because a replacement is
a new disk:

```sh
terraform -chdir=infrastructure/terraform output -raw journal_snapshot_attachment_command
```

prints a loop over every disk labelled `qip_journal=true` in the region and runs
`gcloud compute disks add-resource-policies` on each
(`modules/backup/outputs.tf:11-38`). Run what it prints. Then check that nothing was
missed — the label key is `qip_journal`, with an underscore:

```sh
gcloud compute disks list --project <project> \
  --filter="labels.qip_journal=true AND -resourcePolicies:*" \
  --format='table(name, zone, labels.environment)'
```

An empty result is the correct state. A disk in that list is covered by nothing.

## Restoring a node journal

Not run against a real project. It is written from the configuration, and the first
person to follow it should expect to correct it.

A snapshot is restored to a new disk, mounted on a machine an operator controls, and
read. **It is never mounted on a node that is serving**: a node that boots from a
restored journal resumes a decision record it did not write, and the hash chain will
say so, which is the correct refusal (`modules/backup/NOT-COVERED.md`, "What a restore
looks like").

1. Find the snapshot: `gcloud compute snapshots list --filter="labels.qip_journal=true"`.
   The labels carry the environment and the platform; the source disk name carries the
   node's group name, `qip-<env>-exec-<id>`.
2. Create a disk from it, in a zone you can attach it in:
   `gcloud compute disks create <name> --source-snapshot <snapshot> --zone <zone>`.
   Snapshots cross regions; this is the one part of the platform's durable state that
   does without an operator decision.
3. Attach it to an operator-controlled instance and mount it read-only. The journal is
   under `var/lib/qip/journal` on that filesystem.
4. **Verify before trusting it.** `verify_continuity` proves the journal is intact
   across restarts and `verify_against` proves this segment follows the digest the
   centre last saw (`backend/crates/edge/qip-edge/src/journal.rs:266,329`). A
   snapshot taken mid-write is exactly what these exist to catch. No operator
   command wraps them today: reading a restored journal means a small program
   against `qip_edge::journal`, and writing that is work this page names and does
   not do.
5. If verification fails, restore an earlier snapshot and verify that. The same rule
   as the chain applies: nothing appends to a journal that failed.

Restoring the journal restores the record of what the node did, not the state it was
in. The replacement node still rebuilds its books from the feed and still will not
trade until it holds a verified capital envelope.

Restoring into a *different* region is a new `execution_nodes` entry there, with its
own subnet, machine and venue decision — see [multi-region](multi-region.md) for what
that costs before there is anywhere to restore to.

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
