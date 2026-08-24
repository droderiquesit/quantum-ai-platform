# One edge cell.
#
# ADR 0008 puts the hot execution path in cells that sit next to the venues
# they trade and decide without asking the central plane. Seven of them are
# planned. This module is instantiated once per cell, so adding the next one is
# an entry in a map rather than a copy of a directory — seven copies of a
# network policy is seven places for one of them to be wrong, and the wrong one
# is the one nobody reads.
#
# A cell gets three things of its own and no more:
#
#   * a subnet in its own region, so its traffic is separable in a flow log and
#     its address range is not shared with a cell in another jurisdiction;
#   * a service account, so a compromised cell holds one cell's permissions;
#   * a workload identity binding, so it authenticates without a key on disk.
#
# What it does *not* get is the thing that would make it dangerous: any path
# out except to its own venues and the central plane. The firewall below is the
# network half of that; `allow-edge-egress` in the Kubernetes manifests is the
# pod half. Two controls rather than one, because the first is a configuration
# flag and configuration flags get changed.

locals {
  name = "qip-${var.environment}-${var.cell_id}"

  # The tag the cell's nodes carry, and therefore the tag every rule below
  # targets. Exported so whoever creates the cell's node pool applies the same
  # one — a rule targeting a tag nothing carries is a rule that does nothing.
  node_tag = "qip-edge-${var.cell_id}"
}

resource "google_compute_subnetwork" "cell" {
  project = var.project_id
  name    = local.name
  region  = var.region
  network = var.network_id

  ip_cidr_range = var.subnet_cidr

  # Reaching Google APIs without a public address, the same as the primary
  # subnet.
  private_ip_google_access = true

  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = var.pod_cidr
  }

  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = var.service_cidr
  }

  # A cell trades on its own authority. "What did it talk to" is a question
  # somebody will ask after an incident, and half a percent is enough to
  # answer it.
  log_config {
    aggregation_interval = "INTERVAL_5_SEC"
    flow_sampling        = 0.5
    metadata             = "INCLUDE_ALL_METADATA"
  }
}

resource "google_service_account" "cell" {
  project = var.project_id
  # A service account id is 6 to 30 characters. `qip-edge-` is 9, so a cell id
  # and environment totalling more than 21 fails — at apply, after the network
  # and cluster exist. The precondition moves that to plan time.
  account_id = "qip-edge-${var.cell_id}-${var.environment}"

  lifecycle {
    precondition {
      condition     = length("qip-edge-${var.cell_id}-${var.environment}") <= 30
      error_message = "The derived service account id qip-edge-${var.cell_id}-${var.environment} is ${length("qip-edge-${var.cell_id}-${var.environment}")} characters; Google allows 30. Shorten the cell id."
    }
  }
  display_name = "qip edge cell ${var.cell_id} (${var.environment})"
  description  = "Workload identity for the ${var.cell_id} edge cell."
}

resource "google_service_account_iam_member" "workload_identity" {
  service_account_id = google_service_account.cell.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[qip/qip-edge-${var.cell_id}]"
}

resource "google_project_iam_member" "telemetry" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.cell.email}"
}

resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.cell.email}"
}

# The cell verifies the signature on every capital envelope it is handed. The
# key it verifies against is a secret resource for its *integrity* rather than
# its confidentiality: somebody who can replace it can mint envelopes, which is
# the one way a cell's bound can be widened without the central plane.
resource "google_secret_manager_secret_iam_member" "capital_envelope_key" {
  project   = var.project_id
  secret_id = var.capital_envelope_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.cell.email}"
}

# A cell writes evidence and cannot read or remove it. See the evidence module
# for why the role is this narrow one.
resource "google_storage_bucket_iam_member" "evidence" {
  bucket = var.evidence_bucket
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${google_service_account.cell.email}"
}

resource "google_artifact_registry_repository_iam_member" "pull" {
  project    = var.project_id
  location   = var.registry_location
  repository = var.registry_repository
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${google_service_account.cell.email}"
}

# --- what the cell may reach ------------------------------------------------

# Everything out is denied. Declared at a priority below the allows so that a
# venue rule that is deleted leaves the cell unable to trade rather than able
# to reach anything.
resource "google_compute_firewall" "deny_egress" {
  project = var.project_id
  name    = "${local.name}-deny-egress"
  network = var.network_id

  direction = "EGRESS"
  priority  = 65000

  deny {
    protocol = "all"
  }

  destination_ranges = ["0.0.0.0/0"]
  target_tags        = [local.node_tag]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# One rule per venue, named after the venue. A single rule listing every venue
# range and every venue port would permit each venue's port to every venue's
# address, which is a cross product nobody asked for and nobody would notice.
resource "google_compute_firewall" "venue" {
  for_each = var.venues

  project = var.project_id
  name    = "${local.name}-venue-${lower(each.key)}"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(each.value.port)]
  }

  destination_ranges = [each.value.cidr]
  target_tags        = [local.node_tag]
}

# The central plane: capital envelopes in, evidence and exposure out. A cell
# that has lost this keeps trading inside the envelope it already holds, which
# is the property ADR 0008 exists to provide — so this is a path the cell uses,
# not one it depends on.
resource "google_compute_firewall" "central_plane" {
  project = var.project_id
  name    = "${local.name}-central-plane"
  network = var.network_id

  direction = "EGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = ["443", "8080"]
  }

  destination_ranges = var.central_plane_ranges
  target_tags        = [local.node_tag]
}
