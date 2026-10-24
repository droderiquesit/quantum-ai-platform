# The secondary image repository (R12-RES-9177).
#
# On an Artifact Registry region loss the nodes keep the images they already
# pulled and a rollout pauses; what must not be lost is the release artifact
# itself. This is a second DOCKER repository in another region with the
# primary's discipline: immutable tags, cleanup never deletes, CI may push and
# may not delete. The pipeline copies a promoted digest here before prod is
# promoted, and a prod promotion checks the digest is present — that check
# lives in the release pipeline, not here.
#
# It names a customer-managed key (SEC-046). As first declared it named none,
# so the copy of the release kept for a region loss would have been encrypted
# under a key Google holds: nobody here could disable it, and no audit log of
# this platform's would record its use.
# `security_controls::no_data_store_beyond_those_the_register_records_is_left_without_a_customer_managed_key`
# refused it. Artifact Registry takes a key only from the repository's own
# location (see `key_ring_id`), and the environment's ring is in `var.region`,
# so the secondary has its own ring in `var.secondary_region`, under the same
# gate. With `secondary_region` null, which it is everywhere, none of the
# three below exists.
resource "google_kms_key_ring" "registry_secondary" {
  count = var.secondary_region == null ? 0 : 1

  project  = var.project_id
  name     = "qip-${var.environment}-registry-secondary"
  location = var.secondary_region
}

resource "google_kms_crypto_key" "registry_secondary" {
  count = var.secondary_region == null ? 0 : 1

  name            = "qip-${var.environment}-registry-secondary"
  key_ring        = google_kms_key_ring.registry_secondary[0].id
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = var.kms_protection_level
  }

  # As the primary's: destroying the key leaves every replicated image listed
  # and unreadable, which is deletion that no delete permission was needed for.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

resource "google_kms_crypto_key_iam_member" "artifact_registry_agent_secondary" {
  count = var.secondary_region == null ? 0 : 1

  crypto_key_id = google_kms_crypto_key.registry_secondary[0].id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@gcp-sa-artifactregistry.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.artifact_registry]
}

resource "google_artifact_registry_repository" "images_secondary" {
  count = var.secondary_region == null ? 0 : 1

  project       = var.project_id
  location      = var.secondary_region
  repository_id = "qip-${var.environment}-secondary"
  description   = "Replicated release images for the qip ${var.environment} platform, in a region other than the primary's."
  format        = "DOCKER"
  kms_key_name  = google_kms_crypto_key.registry_secondary[0].id

  docker_config {
    immutable_tags = true
  }

  cleanup_policy_dry_run = true

  labels = var.labels

  # The agent must hold the key before the repository is created under it, or
  # the create fails at apply on a permission the plan never mentioned.
  depends_on = [google_kms_crypto_key_iam_member.artifact_registry_agent_secondary]
}

resource "google_artifact_registry_repository_iam_member" "ci_push_secondary" {
  count = var.secondary_region == null ? 0 : 1

  project    = var.project_id
  location   = google_artifact_registry_repository.images_secondary[0].location
  repository = google_artifact_registry_repository.images_secondary[0].name
  role       = "roles/artifactregistry.writer"
  member     = "serviceAccount:${var.ci_service_account}"
}
