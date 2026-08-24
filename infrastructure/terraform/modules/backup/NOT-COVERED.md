# What this backup does not protect

This module covers the edge cell journals, which is what
`docs/operations/disaster-recovery.md` called out as the live gap. It is not a
backup of the platform, and the difference matters more here than in most
places, because the runbook's central instruction is that some state must
**not** be restored at all.

There are two mechanisms and they stop in different places, so "covered" is not
one answer:

| | GKE backup plan | Disk snapshot schedule |
| --- | --- | --- |
| Selects by | namespace | the disk, once attached |
| Needs a manual step | no | **yes, once per disk** |
| Covers a journal whose claim was deleted | no | yes |
| Captures the Kubernetes objects too | yes | no |
| Survives losing the region | only if `backup_location` says so | yes |
| Retention | 35 days | 90 days |

The row that matters most is the manual step. `snapshot_attachment_command`
prints it, the runbook carries it as a numbered step, and until it has been run
for a given disk that disk has one mechanism rather than two.

## Not covered, on purpose

**Positions and open orders.** Never backed up and never restored. The runbook's
first line says so and the reasoning is at the bottom of it: a restored position
book asserts a belief about the world that was true at the moment of the backup
and is not true now, while the venue holds a real position nobody is managing.
These are reconciled from the venue. There is nothing here for them because
there should be nothing anywhere for them.

**Books, features, watermarks and world model.** `emptyDir` and memory, by
design. Rebuilt from the feed. Backing them up would produce state nobody
reconciled, which is the previous item wearing a different name.

**Kubernetes Secrets.** `include_secrets = false`. There are none in the
manifests — every credential is in Secret Manager, created empty by
`modules/secrets` and written out of band — so this excludes nothing that
exists and prevents the first copy from being made. A backup artefact holding
credential material is a credential store with different access control, a
different retention and a different set of people who can read it.

**The evidence bucket.** Protected by the bucket rather than by a backup:
versioned, customer-managed key, `public_access_prevention` enforced, and a
retention policy that is locked by default. A backup of it would be a second
copy of the write-once record living somewhere with weaker deletion controls
than the original, which lowers rather than raises the floor.

## Not covered, and you should know

**The event-log hash chain, when `QIP_STORAGE_TARGET=memory`.** There is
nothing to back up, because there is nothing durable. The start-up banner says
`NOTHING SURVIVES A RESTART`. This is an application setting, not an
infrastructure one, and no Terraform in this repository can close it — choosing
`engine` and a real volume is what closes it, and then this plan covers that
volume too because it is a claim in the `qip` namespace.

**A lost region, by the GKE backup plan, unless `backup_location` says
otherwise.** Its backups are stored in the location the plan names, and that
defaults to the cluster's own region, so a region becoming unavailable takes
the cluster and those backups together. The `coverage` output reports this
directly as `survives_region_loss`.

Changing it is one variable and two real costs: cross-region transfer on every
backup, and a restore that pulls across regions while everybody is waiting.

The disk snapshots do survive it. `storage_locations` is deliberately unset on
the resource policy, so Google stores each snapshot in the multi-region nearest
the disk. That is a genuine second answer to regional loss and it is only as
good as the attachment step — a schedule attached to no disk survives a region
loss perfectly and restores nothing.

**Any cluster other than this one, and any region other than this one.**
Backup for GKE is per-cluster, and a Compute Engine resource policy can only be
attached to a disk in its own region. This configuration builds exactly one
cluster, and the edge cell workloads run in its `qip` namespace — the edge-cell
module gives a cell a subnet, an identity and firewall rules, not a cluster of
its own. If a deployment ever gives a cell its own cluster, that cluster needs
its own plan and its own regional snapshot schedule, and the journals on it are
unprotected until it has both. Nothing here will notice.

**Any journal disk nobody attached the schedule to.** Said twice on purpose.
This is the one gap in this module that a person closes rather than a
configuration, it has to be closed again every time a cell is added, and its
failure mode is silence.

**Whatever the namespace selector does not match.** `namespaces = ["qip"]`. A
workload deployed somewhere else is not in these backups. `all_namespaces`
would be the wrong fix — it makes the contents of every backup a thing nobody
decided — and adding a namespace to the list is the right one.

## The two checks worth running

`protected_pod_count` is an output for a reason. A plan whose selector matches
nothing succeeds, reports healthy, and protects zero pods. That is
indistinguishable from a working backup until somebody needs one.

And the disks, which is the check for the manual step:

    gcloud compute disks list --project <project> \
      --filter="labels.qip-journal=true AND -resourcePolicies:*" \
      --format='table(name, zone, labels.qip-environment)'

Every row it returns is a journal with no snapshot schedule attached. An empty
result is the correct state. Run it after adding a cell, because that is when a
new row appears.

## And the check neither of these is

Nobody has restored from any of this. An untested restore is a belief, and the
first place a belief gets tested should not be an incident. The runbook carries
the procedure; performing it once, deliberately, into a scratch namespace is
what turns this module from a configuration into a recovery.
