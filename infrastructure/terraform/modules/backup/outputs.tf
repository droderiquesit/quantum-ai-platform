output "snapshot_schedule_name" {
  description = "The disk snapshot schedule's name, for the attachment command below and for an operator checking what covers a disk."
  value       = google_compute_resource_policy.journal_snapshots.name
}

output "encryption_key_id" {
  description = "The customer-managed key the snapshots are encrypted with."
  value       = google_kms_crypto_key.backups.id
}

output "snapshot_attachment_command" {
  description = <<-EOT
    The command that attaches the journal snapshot schedule to every journal
    disk in the region, and the reason it is an output instead of a resource.

    A resource policy attaches to a disk. The execution node's disk is created
    by its managed instance group when the instance is built — after any
    apply, under a name the group chose — and the instance template labels it
    `qip_journal=true` for exactly this reason. Terraform cannot name a disk
    that does not exist yet, so the attachment is a step an operator runs
    after a node's first boot, and again after a node is replaced.

    Until it has been run for a given disk, that disk is covered by nothing.
    `docs/operations/disaster-recovery.md` carries it as a numbered step.
  EOT

  value = join(" ", [
    "for disk in $(gcloud compute disks list",
    "--project=${var.project_id}",
    "--filter='labels.qip_journal=true AND zone:${var.region}-'",
    "--format='value(name,zone.basename())' | tr '\\t' ':'); do",
    "gcloud compute disks add-resource-policies \"$${disk%%:*}\"",
    "--project=${var.project_id}",
    "--zone=\"$${disk##*:}\"",
    "--resource-policies=${google_compute_resource_policy.journal_snapshots.name};",
    "done",
  ])
}

output "coverage" {
  description = "What the schedule covers and where that stops, for an operator reading the plan rather than the runbook."
  value = {
    mechanism            = "compute-engine-disk-snapshots"
    retain_days          = var.snapshot_retain_days
    survives_region_loss = true
    attached_by          = "the snapshot_attachment_command output, run after a node's first boot and after every replacement"
    covers_before_attach = "nothing"
  }
}
