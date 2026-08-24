variable "project_id" {
  type = string
}

variable "project_number" {
  description = "The project's numeric id. Backup for GKE's service agent is named by number, and the CMEK grant cannot infer it."
  type        = number
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "key_ring_id" {
  description = "The platform's existing KMS key ring. The backup key is created in it rather than in a second ring nobody rotates."
  type        = string
}

variable "cluster_id" {
  description = "The cluster to back up, fully qualified. From the cluster module's output, so the plan cannot name a cluster that does not exist."
  type        = string
}

variable "backup_location" {
  description = <<-EOT
    Where the backups are stored.

    The cluster's own region by default, which is the cheap answer and the
    honest limit of it: a backup held in the same region as the cluster does
    not survive losing the region. It covers a failed disk, a deleted
    PersistentVolume, a corrupted journal and an operator error — which is four
    of the five things the disaster-recovery runbook lists and not the fifth.

    Setting this to another region buys the fifth, and costs cross-region
    transfer on every backup and a slower restore. It is a deployment's call
    rather than a default, because the platform that needs it is one whose
    regulator asks about regional loss, and the platform that does not is
    paying for a scenario it has decided to accept.

    Whichever is chosen, `NOT-COVERED.md` states which of the two it is.
  EOT
  type        = string
}

variable "namespaces" {
  description = <<-EOT
    Which namespaces are backed up.

    `qip` only, which is where the edge cell StatefulSets and their journal
    claims live. Deliberately a list rather than `all_namespaces`: a backup
    that silently grows to cover whatever else is scheduled in the cluster is a
    backup whose contents, size and cost nobody chose, and restoring it puts
    that back too.
  EOT
  type        = list(string)
  default     = ["qip"]

  validation {
    condition     = length(var.namespaces) > 0
    error_message = "A backup plan covering no namespace is a plan that reports healthy and protects nothing."
  }
}

variable "backup_schedule" {
  description = <<-EOT
    When a backup is taken, as a cron expression in UTC.

    Daily. A volume backup here is a persistent disk snapshot — incremental
    after the first, and taken without pausing the writer — so this does not
    need a quiet window and is not constrained by market hours the way a node
    upgrade is.

    The minute is not zero on purpose. Everything scheduled on the hour in a
    Google Cloud project contends with everything else scheduled on the hour,
    and a backup that is merely late is a backup whose completion time nobody
    can predict.

    Shortening this does not shorten the runbook's stated RPO for the journal,
    which is the shipping interval to the cell's mirror. This is the durable
    copy behind that, not a replacement for it.
  EOT
  type        = string
  default     = "17 3 * * *"
}

variable "backup_paused" {
  description = <<-EOT
    Whether the schedule is suspended.

    False. There is no `enable_backup` flag in this module and this is not one
    in disguise — it is the switch for a cluster that is genuinely holding
    nothing worth keeping, and it leaves the plan, the key and the retention in
    place so that resuming is one field rather than a new plan with no history.

    The reason there is no on/off flag: the disaster-recovery runbook records
    the absence of a snapshot schedule as a gap the platform has. A flag whose
    default is off would leave that gap exactly where it was and add a line to
    the configuration claiming otherwise, which is worse than the gap. The same
    argument the Binary Authorization module makes for not being optional.
  EOT
  type        = bool
  default     = false
}

variable "delete_lock_days" {
  description = <<-EOT
    How long a backup cannot be deleted by anyone, including whoever holds the
    permission to delete it.

    Seven days. The window in which an operator error, or an account acting on
    somebody else's behalf, cannot also remove the evidence of what it did. It
    must not exceed `retain_days`, and a backup under lock also prevents the
    plan itself from being deleted.

    Raising it hardens that window and removes the ability to clean up a
    backup taken by mistake — of a namespace that should not have been in
    scope, for instance — for the same number of days.
  EOT
  type        = number
  default     = 7

  validation {
    condition     = var.delete_lock_days >= 0 && var.delete_lock_days <= 90
    error_message = "The delete lock must be between 0 and 90 days."
  }
}

variable "retain_days" {
  description = <<-EOT
    How long a backup is kept before it ages out.

    Thirty-five days: five weeks, so a corruption noticed at a month-end
    reconciliation still has a clean copy behind it.

    Deliberately not seven years. The evidence bucket holds the record a
    regulator asks for, under a locked retention policy, and that is the
    long-horizon store. These are copies of a live volume for getting the
    platform running again, and keeping every daily copy of a 16Gi journal for
    seven years would buy a very large bill and no additional recoverability
    that the chain in the evidence bucket does not already provide.
  EOT
  type        = number
  default     = 35

  validation {
    condition     = var.retain_days >= 1 && var.retain_days <= 365
    error_message = "Retention must be between 1 and 365 days."
  }

  validation {
    condition     = var.retain_days >= var.delete_lock_days
    error_message = "Retention is shorter than the delete lock, which asks Google to both keep a backup and remove it. The API refuses this at apply."
  }
}

variable "cluster_region" {
  description = <<-EOT
    The region the cluster runs in.

    Passed separately from `cluster_id` rather than parsed out of it, and used
    for one thing: deciding whether `backup_location` is somewhere else. That
    comparison is what the `coverage` output reports as
    `survives_region_loss`, and a runbook that gets it wrong is a runbook that
    promises a recovery nobody can perform.
  EOT
  type        = string
}

variable "snapshot_start_time" {
  description = <<-EOT
    When the disk-level snapshot schedule runs, as `HH:MM` in UTC.

    Offset from `backup_schedule` on purpose. Two snapshot mechanisms reading
    the same disks in the same minute is avoidable I/O on a volume a cell is
    actively journalling to, and neither of them is urgent enough to be worth
    contending for it.

    Google accepts hourly boundaries and some half-hours; a value it does not
    accept is refused at apply.
  EOT
  type        = string
  default     = "05:00"

  validation {
    condition     = can(regex("^([01][0-9]|2[0-3]):(00|30)$", var.snapshot_start_time))
    error_message = "The start time is HH:MM in UTC, on the hour or the half hour."
  }
}

variable "snapshot_retain_days" {
  description = <<-EOT
    How long a disk snapshot is kept.

    Longer than the GKE backup plan's retention, and deliberately so. These are
    the copies that keep covering a journal after its claim has been deleted —
    a cell taken out of service, whose disk is `Released` and whose record
    somebody may still have to answer for. The window in which that question
    gets asked is a compliance one rather than an operational one, so it is
    measured in months rather than weeks.

    Snapshots are incremental against the previous one, so this is much cheaper
    than the number suggests for a journal that appends.
  EOT
  type        = number
  default     = 90

  validation {
    condition     = var.snapshot_retain_days >= 1 && var.snapshot_retain_days <= 365
    error_message = "Snapshot retention must be between 1 and 365 days."
  }
}
