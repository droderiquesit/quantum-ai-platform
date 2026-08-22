# The container registry.
#
# The images have to live somewhere, and where they live decides who can
# replace what runs in production. Three properties matter here and each is a
# line below:
#
#   * CI can push and cannot delete. A pipeline that can delete a tag can
#     delete the evidence of what it shipped.
#   * Tags are immutable. The image a deployment pulled is the image the
#     pipeline pushed, not whatever was moved onto the tag afterwards.
#   * Nothing is public. A private registry that anyone can pull from tells an
#     attacker exactly what is running and lets them read it.
#
# No customer-managed key here, deliberately. A container image is not secret
# material — it is the output of a build anyone with the repository can
# reproduce. What matters is that it cannot be replaced or removed, which is
# what immutable tags and the absence of a delete permission give.

resource "google_artifact_registry_repository" "images" {
  project       = var.project_id
  location      = var.region
  repository_id = "qip-${var.environment}"
  description   = "Container images for the qip ${var.environment} platform."
  format        = "DOCKER"

  docker_config {
    # A tag cannot be moved to a different digest. Without this, "the image we
    # tested" and "the image we deployed" can be different bytes under the
    # same name, and nothing in the deployment would show it.
    immutable_tags = true
  }

  # Cleanup policies never actually delete. If a retention policy is added
  # later it reports what it would have removed and removes nothing, so the
  # first time anyone learns a policy is too aggressive is from a log line
  # rather than from a missing image during a rollback.
  cleanup_policy_dry_run = true

  labels = var.labels
}

# The pipeline pushes.
#
# `artifactregistry.writer` grants upload and tag creation. It does not grant
# `artifactregistry.versions.delete` or `artifactregistry.packages.delete` —
# that is `repoAdmin`, which is deliberately granted to nobody here.
resource "google_artifact_registry_repository_iam_member" "ci_push" {
  project    = var.project_id
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.name
  role       = "roles/artifactregistry.writer"
  member     = "serviceAccount:${var.ci_service_account}"
}

# The cluster pulls.
#
# The image pull is performed by the node's identity rather than the pod's, so
# the node service account is the one that needs this. The workload accounts
# are listed as well because a workload reading its own image digest — for the
# provenance an audit asks for — reads it through the same API.
resource "google_artifact_registry_repository_iam_member" "pull" {
  for_each = toset(var.pull_service_accounts)

  project    = var.project_id
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.name
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${each.value}"
}
