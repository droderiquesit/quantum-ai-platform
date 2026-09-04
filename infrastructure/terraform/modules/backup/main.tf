# Backups for the state that cannot be rebuilt.
#
# `docs/operations/disaster-recovery.md` named the gap precisely: the journal
# is what a node decided and why — the one record that cannot be recomputed
# from the feed, the venue, or anything else the platform holds — and a
# journal on a disk nothing snapshots is a journal that disappears with the
# disk, the project, or the region.
#
# On the GKE runtime this module carried two mechanisms: Backup for GKE,
# selecting the `qip` namespace, and a Compute Engine snapshot schedule for
# the disks the claims left behind. The first left with the cluster (ADR
# 0024). What remains is the second, and it is now the whole answer rather
# than the fallback: the execution node's journal lives on the instance's
# disk, the instance template labels that disk `qip_journal = "true"`, and
# this schedule is attached to it.
#
# What Terraform still cannot do is the attachment itself. A resource policy
# attaches to a *disk*, and the node's disk is created by the managed
# instance group when it builds the instance — after any apply, under a name
# the group chooses. `snapshot_attachment_command` is that step, and the
# runbook carries it. Until it has been run for a given disk, that disk is
# covered by nothing, which NOT-COVERED.md says in as many words.
#
# There is no `enable_backup` flag, and there is not one in disguise: a
# switch whose off position is the gap the runbook already documents would
# leave that gap in place and add a line to the configuration implying
# otherwise.

locals {
  name = "qip-${var.environment}-journal"
}

# The key snapshots are encrypted with. In the platform's key ring, like the
# evidence and secrets keys, rather than in a second ring nobody rotates.
resource "google_kms_crypto_key" "backups" {
  name     = "${local.name}-backups"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Ninety days, matching the platform's other keys.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  # Destroying this key makes every snapshot taken under it unreadable, which
  # is a way of deleting a backup that leaves the snapshot in place.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# The disk-level schedule.
#
# Daily, retained for months, and kept when the disk goes: these are the
# copies that keep covering a journal after the node that wrote it has been
# replaced — a decommissioned node whose decision record somebody may still
# be asked about. That question is a compliance one rather than an
# operational one, so the window is months rather than weeks. Snapshots are
# incremental, so this costs far less than the number suggests for a volume
# that appends.
resource "google_compute_resource_policy" "journal_snapshots" {
  project = var.project_id
  name    = "${local.name}-snapshots"
  region  = var.region

  snapshot_schedule_policy {
    schedule {
      daily_schedule {
        days_in_cycle = 1
        start_time    = var.snapshot_start_time
      }
    }

    retention_policy {
      max_retention_days = var.snapshot_retain_days

      # The line this schedule exists for.
      #
      # `APPLY_RETENTION_POLICY` would delete a disk's snapshots when the disk
      # is deleted, which makes the snapshot useless for the one failure it
      # uniquely covers: a node replaced, its disk auto-deleted with it, and
      # somebody then needing to answer for what that node decided. Keeping
      # them means the record outlives both the instance and the disk.
      on_source_disk_delete = "KEEP_AUTO_SNAPSHOTS"
    }

    snapshot_properties {
      # So a snapshot is as findable as the disk it came from, and so an
      # unlabelled snapshot is visibly not one of these.
      labels = merge(var.labels, {
        qip_journal = "true"
      })

      # `storage_locations` is deliberately not set. Google then stores the
      # snapshot in the multi-region nearest the disk, which is what makes
      # these survive the loss of a single region. Naming one region here
      # would quietly take that away.
    }
  }
}

# The GKE-era mechanism, forgotten rather than deleted.
#
# Backup for GKE left with the cluster under ADR 0024, so these three
# resources are in no configuration any more and the first apply of the Cloud
# Run runtime planned all three for destruction. Two went. The third refused:
#
#   Error: Error when reading or editing BackupPlan: googleapi: Error 400:
#   Resource '"projects/.../backupPlans/qip-dev-journal"' has nested
#   resources. If the API supports cascading delete, set 'force' to true.
#
# The nested resources are backups of the journal — what a cell decided and
# why, the one record §5 says cannot be recomputed. `force = true` is the
# available answer and it is the wrong one: it would delete the evidence to
# unblock a migration, which is the trade this repository does not make. So
# the plan leaves Terraform's management with its backups intact, and stays
# in the project for whatever retention the compliance answer needs.
#
# `google_project_service_identity.gkebackup` is listed for the same reason:
# it destroyed cleanly here (a service identity is a state entry, not a thing
# Terraform deletes), but if this apply is ever replayed against state that
# still holds it, forgetting it is what should happen, not another delete.
#
# What this does not restore: `google_kms_crypto_key_iam_member.backup_agent`
# was destroyed before the plan refused, so the backup service agent no longer
# holds cryptoKeyEncrypterDecrypter on `qip-dev-journal-backups`. The backups
# are still there and the key is still there — `google_kms_crypto_key.backups`
# above carries prevent_destroy — but a restore needs that grant put back
# first. NOT-COVERED.md carries the step. Re-declaring the grant here would
# put a GKE-era resource back into a configuration that has no GKE in it.
removed {
  from = google_gke_backup_backup_plan.journal

  lifecycle {
    destroy = false
  }
}

removed {
  from = google_project_service_identity.gkebackup

  lifecycle {
    destroy = false
  }
}
