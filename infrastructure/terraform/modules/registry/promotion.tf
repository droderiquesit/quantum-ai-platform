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

# `var.promotion_readers` is declared in variables.tf with the module's other
# inputs, where `terraform_contract.rs` (and a reader) looks for them.

resource "google_artifact_registry_repository_iam_member" "promotion_read" {
  for_each = var.promotion_readers

  project    = var.project_id
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.name
  role       = "roles/artifactregistry.reader"
  member     = each.value
}
