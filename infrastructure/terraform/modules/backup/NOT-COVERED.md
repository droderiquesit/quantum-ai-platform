# What the journal backups do not cover

Read this before citing `modules/backup` as evidence that anything is
recoverable.

## Nothing is covered until the schedule is attached

A Compute Engine snapshot schedule is a resource policy, and a resource
policy protects a disk only once it has been attached to that disk. The
execution node's disk is created by its managed instance group when the
instance is built — after any apply, under a name the group chose — so
Terraform cannot attach the policy at apply time. The `snapshot_attachment_command`
output is the attachment, and it has to be run after a node's first boot and
again after every blue-green replacement, because the replacement is a new
instance with a new disk.

Until it has been run for a given disk, **that disk is covered by nothing**.
A plan that shows the schedule existing is not a plan that shows a journal
being backed up.

## Backup for GKE left with the cluster

This module used to carry a Backup for GKE plan selecting the `qip` namespace,
which covered every journal claim with no per-disk step. That mechanism has
no meaning without a cluster and was removed under ADR 0024. The trade is
stated plainly: the namespace-scoped plan needed nobody to remember anything,
and the disk schedule needs an operator to run one command per node. The
runbook step is the mitigation and it is a weaker one.

## Positions and open orders are never restored

By design. `docs/operations/disaster-recovery.md` is explicit that positions
and open orders are reconciled from the venue and never from a backup: a
restored position that the venue does not hold is a position the platform
believes it has and does not, which is worse than an empty book. The journal
is the record of what was decided; it is not the state to resume from.

## Regional loss

`storage_locations` is deliberately unset on the snapshot properties, so
Google stores each snapshot in the multi-region nearest the disk. That is what
lets these survive the loss of a single region, and it is the only part of the
platform's durable state that does without an operator decision.

## What a restore looks like

A snapshot is restored to a new disk, mounted on a machine an operator
controls, and read. It is never mounted on a node that is serving: a node
that boots from a restored journal resumes a decision record it did not
write, and the hash chain will say so, which is the correct refusal.
