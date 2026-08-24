terraform {
  required_providers {
    google = {
      source = "hashicorp/google"
    }
    # For `google_project_service_identity` only — see modules/secrets for the
    # reasoning; the GKE Backup agent was the account whose lazy creation the
    # first real apply raced and lost.
    google-beta = {
      source = "hashicorp/google-beta"
    }
  }
}

# Backups for the state that cannot be rebuilt.
#
# `docs/operations/disaster-recovery.md` named this as a live gap, and named it
# precisely: the edge cell journal claims are `Retain`, and **retained is not
# backed up**. `Retain` is an instruction to Kubernetes not to delete a disk
# when the claim goes away. It says nothing about a disk that fails, a project
# that is deleted, an operator who removes the wrong PersistentVolume, or a
# region that becomes unavailable. In every one of those the journal is gone,
# and the journal is what a cell decided and why — the one record that cannot
# be recomputed from the feed, the venue, or anything else the platform holds.
#
# There are two mechanisms here, and that is deliberate rather than belt and
# braces. They fail in different places, and the gap between them is the exact
# state the runbook's own "take a cell out" step produces.
#
# ## Backup for GKE — the one that needs nobody to remember anything
#
# It selects by **namespace**, so it covers every journal claim in `qip`
# including ones a cell created after this was written, with no per-disk step.
# It also captures the Kubernetes objects that make a volume meaningful — the
# StatefulSet, the claim, which replica owned which disk — rather than a bare
# block device somebody then has to work out how to reattach.
#
# Its limit is that it sees claims. A journal whose PersistentVolumeClaim has
# been deleted is no longer in the namespace, so it stops being backed up from
# that moment, even though the disk still exists and still holds the record.
#
# ## A Compute Engine snapshot schedule — the one that outlives the claim
#
# `infrastructure/kubernetes/base/journal-storage.yaml` sets the journal
# StorageClass to `reclaimPolicy: Retain` precisely so that deleting a claim
# leaves the disk. It also labels every disk it provisions —
# `qip-journal=true` — and says why: a resource policy attaches to a *disk*,
# the disks are named `pvc-<uuid>` and created when a cell's pod is first
# scheduled, long after any apply, so Terraform can create the schedule and
# cannot attach it. The label is how somebody finds them.
#
# This is the Terraform half of that arrangement. The schedule below exists to
# be attached; `snapshot_attachment_command` in the outputs is the one-liner
# that does it, and docs/operations/disaster-recovery.md carries it as a step.
#
# The attachment being manual is a real weakness and is stated rather than
# glossed: an unattached schedule protects nothing. It is worth having anyway,
# because it is the only thing that keeps covering a `Released` volume — an
# orphaned disk that nothing reclaims, that bills, and that holds what a
# decommissioned cell decided. Backup for GKE has already stopped looking at it.
#
# Its other advantage is incidental and useful: persistent disk snapshots are
# stored in a multi-region by default, so they survive losing the region the
# disk is in, which the GKE backup plan does not unless `backup_location` says
# otherwise.
#
# What neither covers is in NOT-COVERED.md, and the list is short and real.

locals {
  name = "qip-${var.environment}-journal"
}

# The key the backups are encrypted with.
#
# In the platform's existing key ring, like the evidence and model keys, rather
# than a second ring nobody rotates. A backup of the journal holds exactly what
# the journal holds, so anything less than the protection on the original would
# make the backup the easier thing to read.
resource "google_kms_crypto_key" "backups" {
  name     = "${local.name}-backups"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Ninety days, matching the node and secret keys. Rotation re-encrypts new
  # backups; existing ones stay readable under the version that wrote them,
  # which is why the key must never be destroyed while a backup is retained.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  # Destroying this key destroys every backup taken under it, silently: the
  # backups stay listed and become unrestorable. That is the worst failure mode
  # available here — a disaster recovery plan that reports coverage and cannot
  # deliver it.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# Backup for GKE encrypts as its own service agent, so the grant is to that
# agent rather than to any workload identity — the same arrangement, and the
# same reasoning, as the Secret Manager agent that publishes rotation notices
# in modules/secrets.
#
# Enabling `gkebackup.googleapis.com` does NOT reliably create the agent —
# the first real apply failed with "service-…@gcp-sa-gkebackup… does not
# exist" on a project where the API was on. The service identity below forces
# the agent into existence; being applied after modules/services is necessary
# but not sufficient.
resource "google_project_service_identity" "gkebackup" {
  provider = google-beta
  project  = var.project_id
  service  = "gkebackup.googleapis.com"
}

resource "google_kms_crypto_key_iam_member" "backup_agent" {
  depends_on = [google_project_service_identity.gkebackup]

  crypto_key_id = google_kms_crypto_key.backups.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@gcp-sa-gkebackup.iam.gserviceaccount.com"
}

resource "google_gke_backup_backup_plan" "journal" {
  project  = var.project_id
  name     = local.name
  location = var.backup_location
  labels   = var.labels

  # Named from the cluster module's output rather than assembled here. A plan
  # pointing at a cluster that does not exist is accepted by the API and
  # protects nothing.
  cluster = var.cluster_id

  backup_config {
    # The whole point. Without this the plan captures Kubernetes objects and
    # none of the journal, which is a backup of the shape of the deployment
    # rather than of the thing that cannot be rebuilt.
    include_volume_data = true

    # No Kubernetes Secrets in the backup, and there are none to take: every
    # credential in this platform lives in Secret Manager and is read at
    # start-up, which is why `no_credential_appears_in_a_kubernetes_manifest`
    # passes. Setting this true would create the first copy of that rule being
    # broken — a backup artefact holding credential material, in a store with
    # different access control and a different retention, restored by whoever
    # can restore a backup.
    include_secrets = false

    # `qip` only. Not `all_namespaces`, because a backup that quietly grows to
    # cover whatever else lands in the cluster is a backup whose size, cost and
    # contents nobody decided.
    selected_namespaces {
      namespaces = var.namespaces
    }

    encryption_key {
      gcp_kms_encryption_key = google_kms_crypto_key.backups.id
    }
  }

  backup_schedule {
    cron_schedule = var.backup_schedule
    paused        = var.backup_paused
  }

  retention_policy {
    # A backup cannot be deleted for this many days after it is taken, by
    # anyone, including whoever holds the permission to delete it. This is the
    # window in which an operator error or a compromised account cannot also
    # remove the evidence of what it did — the same argument as the evidence
    # bucket's retention policy, at a shorter horizon because these are copies
    # of a live volume rather than the write-once record.
    backup_delete_lock_days = var.delete_lock_days

    # After this, a backup ages out. Long enough that a corruption noticed
    # weeks later still has a clean copy behind it; not so long that the
    # platform accumulates an unbounded second copy of everything.
    backup_retain_days = var.retain_days

    # Not locked, and this is the one place this module is deliberately less
    # strict than the evidence bucket.
    #
    # Locking a retention policy is irreversible and freezes both numbers
    # above. On the evidence bucket that is the control itself: the lock is
    # what makes the record undeletable. Here the numbers are an operational
    # estimate that will be wrong at least once — the first real restore is
    # what tells anyone whether thirty-five days was right — and a locked plan
    # cannot be corrected, only abandoned and replaced, leaving the backups
    # taken under it stranded behind a plan nobody maintains.
    #
    # `backup_delete_lock_days` still protects each individual backup, which is
    # the protection that actually matters against a bad day.
    locked = false
  }

  # There is deliberately no `prevent_destroy` here, and the provider pinned in
  # this repository has no deletion-protection field for a backup plan.
  #
  # What protects it is `backup_delete_lock_days` above: Google refuses to
  # delete a plan while any backup taken under it is still inside its lock
  # window, so the plan cannot be removed ahead of the backups it holds.
  #
  # A `prevent_destroy` would add nothing to that and would take something
  # away: several fields here — `backup_location` most obviously — force
  # replacement, so the lifecycle rule would make correcting the location
  # impossible rather than merely deliberate. The KMS key above carries
  # `prevent_destroy` because destroying it strands every backup; the plan does
  # not, because recreating a plan loses a schedule and no data.
}

# The snapshot schedule the journal disks are labelled for.
#
# Regional, and in the cluster's region, because a resource policy can only be
# attached to a disk in the same region. Every journal disk this platform
# creates is in the primary cluster's region: the edge-cell module gives a cell
# a subnet, an identity and firewall rules, not a cluster, so the cells' pods
# and their volumes live in the one cluster this configuration builds.
resource "google_compute_resource_policy" "journal_snapshots" {
  project = var.project_id
  name    = "${local.name}-snapshots"
  region  = var.cluster_region

  snapshot_schedule_policy {
    schedule {
      daily_schedule {
        days_in_cycle = 1
        # Offset from the GKE backup plan rather than alongside it. Two
        # snapshot mechanisms reading the same disks at the same minute is
        # avoidable I/O on a volume a cell is writing its journal to.
        start_time = var.snapshot_start_time
      }
    }

    retention_policy {
      max_retention_days = var.snapshot_retain_days

      # The line this schedule exists for.
      #
      # `APPLY_RETENTION_POLICY` would delete a disk's snapshots when the disk
      # is deleted, which makes the snapshot useless for the one failure it
      # uniquely covers: somebody removing a cell, deleting its `Retain`ed
      # claim, and then needing to answer for what that cell decided. Keeping
      # them means the record outlives both the claim and the disk.
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
      # these survive the loss of a single region. Naming one region here would
      # quietly take that away.
    }
  }
}
