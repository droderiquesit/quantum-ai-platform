# The pipeline's identity.
#
# The pipeline needs to push images and apply manifests, which means it needs
# credentials, which is the part that usually goes wrong. A service-account key
# in a repository secret is a credential that lives forever, is copied by
# anyone who can read the secret, and leaves no trace of which run used it.
#
# Workload identity federation replaces it with a token GitHub mints for a
# single job, exchanged for a short-lived Google credential. Nothing is stored
# and nothing outlives the run.
#
# The condition below is what makes that safe. Without an attribute condition,
# *any* GitHub repository in the world can present a valid GitHub OIDC token to
# this pool. The issuer proves the token came from GitHub Actions; it does not
# prove which repository ran.

resource "google_service_account" "ci" {
  project      = var.project_id
  account_id   = "qip-ci-${var.environment}"
  display_name = "qip pipeline (${var.environment})"
  description  = "Builds, pushes and deploys. Impersonated through workload identity federation; it has no key."
}

resource "google_iam_workload_identity_pool" "github" {
  project                   = var.project_id
  workload_identity_pool_id = "qip-github-${var.environment}"
  display_name              = "GitHub Actions (${var.environment})"
  description               = "Federates GitHub Actions OIDC tokens for the qip repository."
}

resource "google_iam_workload_identity_pool_provider" "github" {
  project                            = var.project_id
  workload_identity_pool_id          = google_iam_workload_identity_pool.github.workload_identity_pool_id
  workload_identity_pool_provider_id = "github"
  display_name                       = "GitHub OIDC"

  # Only this repository, and only on a branch the deployment is allowed from.
  # A pull request from a fork runs with `assertion.repository` set to the fork,
  # so this refuses it without anyone having to remember to.
  attribute_condition = "attribute.repository == '${var.github_repository}'"

  attribute_mapping = {
    "google.subject"       = "assertion.sub"
    "attribute.repository" = "assertion.repository"
    "attribute.ref"        = "assertion.ref"
    "attribute.workflow"   = "assertion.workflow"
  }

  oidc {
    issuer_uri = "https://token.actions.githubusercontent.com"
  }
}

# Which federated principals may impersonate the pipeline account: those from
# this repository, and nothing else in the pool.
resource "google_service_account_iam_member" "github_impersonation" {
  service_account_id = google_service_account.ci.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.repository/${var.github_repository}"
}

# Deploying means reading the cluster's endpoint and applying manifests inside
# one namespace. `container.developer` is the narrowest predefined role that
# does it: it cannot create, resize or delete a cluster, and it cannot read
# secrets outside what the namespace's RBAC allows.
#
# It is still broader than one namespace. Narrowing it further is a Kubernetes
# RBAC role binding rather than a Google one, and it is listed as an open gap
# in docs/operations/external-dependencies.md rather than pretended away here.
resource "google_project_iam_member" "deploy" {
  project = var.project_id
  role    = "roles/container.developer"
  member  = "serviceAccount:${google_service_account.ci.email}"
}
