output "plan_name" {
  description = "The backup plan's name, for `gcloud beta container backup-restore backups list`."
  value       = google_gke_backup_backup_plan.journal.name
}

output "plan_id" {
  description = "The plan's fully qualified id, which is what a restore plan names."
  value       = google_gke_backup_backup_plan.journal.id
}

output "protected_pod_count" {
  description = <<-EOT
    How many pods the plan's last run actually captured, as Google reports it.

    Worth reading rather than assuming. A plan whose namespace selector matches
    nothing succeeds, reports a healthy state, and protects zero pods — which
    is what a backup gap looks like from the console.
  EOT
  value       = google_gke_backup_backup_plan.journal.protected_pod_count
}

output "coverage" {
  description = <<-EOT
    What the journal backups cover and where each mechanism stops, in the form
    an operator reading a disaster-recovery runbook needs.

    Two entries because there are two mechanisms and they fail in different
    places. The GKE backup plan needs nobody to remember anything and stops
    covering a journal the moment its claim is deleted. The snapshot schedule
    keeps covering the disk after that and protects nothing until somebody has
    attached it — see `snapshot_attachment_command`.

    NOT-COVERED.md is the rest of the answer, including the state that is
    deliberately never backed up.
  EOT
  value = {
    gke_backup_plan = {
      covers               = "every PersistentVolumeClaim and Kubernetes object in ${join(", ", var.namespaces)}, while the claim exists"
      volume_data          = true
      kubernetes_secrets   = false
      schedule             = var.backup_paused ? "paused" : var.backup_schedule
      retained_days        = var.retain_days
      undeletable_for_days = var.delete_lock_days
      stored_in            = var.backup_location
      # False whenever the backups sit in the cluster's own region, which is
      # the default. The disk snapshots below are what covers this meanwhile.
      survives_region_loss = var.backup_location != var.cluster_region
      needs_manual_step    = false
    }
    disk_snapshot_schedule = {
      covers        = "each journal disk it has been attached to, including after its claim is deleted"
      schedule      = "daily at ${var.snapshot_start_time} UTC"
      retained_days = var.snapshot_retain_days
      # Snapshots are stored in the multi-region nearest the disk because
      # `storage_locations` is deliberately unset.
      survives_region_loss   = true
      kept_when_disk_deleted = true
      # The honest one. A schedule attached to no disk protects nothing, and
      # nothing in Terraform can attach it: the disks are named `pvc-<uuid>`
      # and created when a cell's pod is first scheduled.
      needs_manual_step = "attach it — see snapshot_attachment_command, and run it again after adding a cell"
    }
  }
}

output "snapshot_schedule_name" {
  description = "The Compute Engine snapshot schedule the journal disks are labelled for. It protects nothing until it is attached to them."
  value       = google_compute_resource_policy.journal_snapshots.name
}

output "snapshot_attachment_command" {
  description = <<-EOT
    The command that attaches the snapshot schedule to the journal disks.

    This is an output rather than a resource because it cannot be one. A
    Compute Engine resource policy attaches to a *disk*, and the journal disks
    are named `pvc-<uuid>` and created by the CSI driver when a cell's pod is
    first scheduled — after any apply, with a name nothing could have
    predicted. `infrastructure/kubernetes/base/journal-storage.yaml` labels them
    `qip-journal=true` so that they can be found; this is the finding.

    Run it after a cell's first pod is running, and again after adding a cell.
    Until it has been run for a given disk, that disk is covered by the GKE
    backup plan and by nothing else — which is enough right up until somebody
    deletes the claim.

    docs/operations/disaster-recovery.md carries this as a numbered step so it
    is somewhere an operator already looks.
  EOT

  value = join(" ", [
    "gcloud compute disks list --project ${var.project_id}",
    "--filter=\"labels.qip-journal=true AND labels.qip-environment=${var.environment} AND -resourcePolicies:${google_compute_resource_policy.journal_snapshots.name}\"",
    "--format=\"value(name,zone)\"",
    "| while read -r name zone; do",
    "gcloud compute disks add-resource-policies \"$name\" --project ${var.project_id}",
    "--zone \"$zone\" --resource-policies ${google_compute_resource_policy.journal_snapshots.name};",
    "done",
  ])
}
