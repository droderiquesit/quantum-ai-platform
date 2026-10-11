# Later environments read the one build registry (R12 CICD-9112).
#
# There is one build registry: `deploy.yml` builds each commit once into the
# build environment's `qip-<env>` and promotes that digest to every later
# environment, which therefore pulls — and scans, and attests — from here.
# Each later environment needs two principals on this repository: its CI
# account (to describe, scan and attest the digest) and its Cloud Run
# service agent (to pull it). Both live in another project, so this
# environment's configuration cannot derive them and the tfvars name them.
#
# Empty by default and in every environment today, because no later
# environment is provisioned and IAM refuses a grant to an account that does
# not exist. Reader and nothing more, on this repository alone: a later
# environment that could push here could replace the bytes the earlier ones
# tested, which is the failure the single registry exists to remove.

variable "promotion_readers" {
  description = "Principals in later environments granted roles/artifactregistry.reader on the image repository, as IAM members (`serviceAccount:…`). Read only, never writer."
  type        = set(string)
  default     = []
  nullable    = false

  validation {
    condition = alltrue([
      for member in var.promotion_readers :
      can(regex("^serviceAccount:[a-z0-9-]+@[a-z0-9.-]+\\.iam\\.gserviceaccount\\.com$", member))
    ])
    error_message = "Every promotion reader is one service account, written `serviceAccount:<name>@<domain>.iam.gserviceaccount.com`. A user, a group, a domain, allUsers or allAuthenticatedUsers is refused: the build registry is read by pipelines and service agents, not by people."
  }
}

resource "google_artifact_registry_repository_iam_member" "promotion_read" {
  for_each = var.promotion_readers

  project    = var.project_id
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.name
  role       = "roles/artifactregistry.reader"
  member     = each.value
}
