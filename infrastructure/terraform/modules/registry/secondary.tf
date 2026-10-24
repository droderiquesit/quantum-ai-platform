# The secondary image repository (R12-RES-9177).
#
# On an Artifact Registry region loss the nodes keep the images they already
# pulled and a rollout pauses; what must not be lost is the release artifact
# itself. This is a second DOCKER repository in another region with the
# primary's discipline: immutable tags, cleanup never deletes, CI may push and
# may not delete. The pipeline copies a promoted digest here before prod is
# promoted, and a prod promotion checks the digest is present — that check
# lives in the release pipeline, not here.
resource "google_artifact_registry_repository" "images_secondary" {
  count = var.secondary_region == null ? 0 : 1

  project       = var.project_id
  location      = var.secondary_region
  repository_id = "qip-${var.environment}-secondary"
  description   = "Replicated release images for the qip ${var.environment} platform, in a region other than the primary's."
  format        = "DOCKER"

  docker_config {
    immutable_tags = true
  }

  cleanup_policy_dry_run = true

  labels = var.labels
}

resource "google_artifact_registry_repository_iam_member" "ci_push_secondary" {
  count = var.secondary_region == null ? 0 : 1

  project    = var.project_id
  location   = google_artifact_registry_repository.images_secondary[0].location
  repository = google_artifact_registry_repository.images_secondary[0].name
  role       = "roles/artifactregistry.writer"
  member     = "serviceAccount:${var.ci_service_account}"
}
