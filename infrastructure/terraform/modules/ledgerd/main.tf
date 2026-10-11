# qip-ledgerd: the ledger writer (ADR 0100 §2) on one Compute Engine machine.
#
# Why a machine and not a Cloud Run service. The ledger is the sole writer
# of the chain of double-entry postings for fills, journalled to a local
# store under `QIP_LEDGER_ARCHIVE_PATH` and fsynced before it acknowledges.
# A Cloud Run service has no disk that outlives an instance and may run two
# instances during a rollout — two writers of one chain. So: one machine,
# one persistent disk, no instance group (a group's surge creates the
# replacement before it retires the original, which is the property this
# workload must not have).
#
# The fourth paper-trading fence (ADR 0106) is the binary's: it parks any
# fill whose `simulated` flag is not true. Nothing here configures, relaxes
# or can relax it — there is no setting for it to read.
#
# Nothing is created unless the root is given a `ledgerd` object, which is
# null in every environment: no boot image carrying qip-ledgerd exists yet
# (`image.yml` bakes the execution node's image only).

resource "google_service_account" "ledgerd" {
  project      = var.project_id
  account_id   = "qip-${var.environment}-ledgerd"
  display_name = "qip ${var.environment} ledger writer"
  description  = "qip-ledgerd's machine identity: writes its own logs and metrics and nothing else (ADR 0100 §2)."
}

# There is no `google_service_account_key` in this module and there must
# never be one. The machine authenticates as the account above through the
# metadata server.

# Logs and metrics, and nothing else. The ledger reads the fabric over the
# network and writes its own disk; it needs no Google API beyond telemetry.
resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.ledgerd.email}"
}

resource "google_project_iam_member" "metrics" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.ledgerd.email}"
}

# The archive. Its own disk so that the machine can be replaced without the
# postings going with it. Labelled `qip_journal` so the backup module's
# snapshot schedule applies.
#
# Deliberately no `prevent_destroy`: closing the root's flag would then be a
# plan Terraform refuses (the shape infrastructure/CLAUDE.md records for the
# control plane's etcd key). The ledger's postings are derived from the
# fabric's P1 stream, which is the record; a ledger lost with its disk is
# rebuilt by replay, not lost.
resource "google_compute_disk" "archive" {
  project = var.project_id
  name    = "qip-${var.environment}-ledger-archive"
  zone    = var.zone
  type    = "pd-balanced"
  size    = var.archive_disk_size_gb

  labels = merge(var.labels, { component = "ledgerd", qip_journal = "true" })
}

resource "google_compute_instance" "ledgerd" {
  project      = var.project_id
  name         = "qip-${var.environment}-ledgerd"
  zone         = var.zone
  machine_type = var.machine_type

  boot_disk {
    auto_delete = true
    initialize_params {
      image = var.boot_image
      type  = "pd-balanced"
    }
  }

  attached_disk {
    source      = google_compute_disk.archive.id
    device_name = "qip-ledger"
    mode        = "READ_WRITE"
  }

  # No `access_config`: no external address, so nothing outside the VPC can
  # reach the read API and the machine has no route out but the VPC's.
  #
  # The stable internal address is the interface's own `network_ip`, as the
  # broker's is, and not a `google_compute_address`. A reserved regional
  # address was declared here once; in this tree every regional
  # `google_compute_address` is a NAT gateway's external IP, and
  # `security_controls.rs` refuses one held by anything else, because a
  # reserved address on a machine is the shape of internet egress with no
  # gateway in front. An INTERNAL reservation is not that, but it is one
  # attribute away from it, and the machine needs no reservation to keep the
  # address it is created with.
  network_interface {
    network    = var.network_id
    subnetwork = var.subnet_id
    network_ip = var.internal_address
    stack_type = "IPV4_ONLY"
  }

  service_account {
    email  = google_service_account.ledgerd.email
    scopes = ["cloud-platform"]
  }

  metadata = {
    startup-script = templatefile("${path.module}/startup.sh.tftpl", {
      account_id               = var.account_id
      fabric_consumer_group    = var.fabric_consumer_group
      listen_port              = var.listen_port
      deployment               = var.environment
      cas_ca_pool              = var.workload_certificates.ca_pool
      cas_certificate_template = var.workload_certificates.certificate_template
      cas_lifetime_secs        = var.workload_certificates.lifetime_secs
      cas_trust_roots_base64   = base64encode(var.workload_certificates.trust_roots_pem)
      google_tls_roots_path    = var.google_tls_roots_path
    })
    block-project-ssh-keys = "true"
  }

  tags = concat(var.network_tags, ["qip-ledgerd"])

  # The writer of the ledger chain boots shielded or not at all (SEC-070's
  # argument for the broker applies unchanged).
  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  labels = merge(var.labels, { component = "ledgerd" })
}
