# Secrets, keys and identities.
#
# The rule this module exists to enforce: no secret value is ever in Terraform.
# Every secret is created empty and its value written out of band, so a state
# file that leaks contains the shape of the deployment and none of its
# credentials.
#
# The venue credential is the one that matters most. It is readable only where
# the autonomy ceiling permits live trading, so a paper-trading environment
# holding one cannot use it — not because the application declines, but
# because the IAM binding does not exist.

resource "google_kms_key_ring" "platform" {
  project  = var.project_id
  name     = "qip-${var.environment}"
  location = var.region
}

resource "google_kms_crypto_key" "node_encryption" {
  name     = "qip-${var.environment}-node-encryption"
  key_ring = google_kms_key_ring.platform.id

  # Ninety days. Short enough that a compromised key has a bounded window,
  # long enough that rotation is not itself a source of incidents.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  # Destroying the key that encrypts etcd destroys the cluster's data.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

resource "google_kms_crypto_key" "secrets" {
  name            = "qip-${var.environment}-secrets"
  key_ring        = google_kms_key_ring.platform.id
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# One service account per deployable. A compromised component has only its own
# permissions, which is the entire argument for not sharing one.
resource "google_service_account" "workload" {
  for_each = var.service_accounts

  project      = var.project_id
  account_id   = "${each.value}-${var.environment}"
  display_name = "qip ${each.key} (${var.environment})"
  description  = "Workload identity for the qip ${each.key} deployable."
}

# The secrets themselves, created without values.
resource "google_secret_manager_secret" "platform" {
  for_each = toset(var.secret_names)

  project   = var.project_id
  secret_id = "${each.value}-${var.environment}"

  replication {
    user_managed {
      replicas {
        location = var.region
        customer_managed_encryption {
          kms_key_name = google_kms_crypto_key.secrets.id
        }
      }
    }
  }

  # A secret with no rotation schedule is a secret nobody rotates.
  rotation {
    next_rotation_time = "2026-12-01T00:00:00Z"
    rotation_period    = "7776000s"
  }

  labels = var.labels

  # Terraform creates the secret. It never creates a version, because a
  # version has a value and a value in state is a leaked credential.
  lifecycle {
    ignore_changes = [labels]
  }
}

# The API reads the tokens that authenticate its callers.
resource "google_secret_manager_secret_iam_member" "api_tokens" {
  for_each = toset([
    for name in var.secret_names : name
    if startswith(name, "qip-token-")
  ])

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform[each.value].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload["api"].email}"
}

# The venue credential is readable only where live trading is permitted at all.
#
# This is the infrastructure half of the live-trading control. The application
# refuses to send a live order below a live autonomy level; this makes the
# credential unreadable in an environment that could not use it anyway, so a
# misconfigured application in a paper environment still cannot authenticate to
# a venue.
resource "google_secret_manager_secret_iam_member" "venue_credential" {
  count = var.venue_credential_readable ? 1 : 0

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform["qip-venue-credential"].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload["fastbrain"].email}"
}

# Workload identity bindings, so a pod authenticates as its service account
# without a key file on disk.
resource "google_service_account_iam_member" "workload_identity" {
  for_each = var.service_accounts

  service_account_id = google_service_account.workload[each.key].name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[qip/${each.value}]"
}

# The minimum each deployable needs beyond its secrets: write telemetry.
resource "google_project_iam_member" "telemetry" {
  for_each = var.service_accounts

  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.workload[each.key].email}"
}

resource "google_project_iam_member" "logging" {
  for_each = var.service_accounts

  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.workload[each.key].email}"
}
