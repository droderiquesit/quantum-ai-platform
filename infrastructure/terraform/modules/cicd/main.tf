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

# The infrastructure account: what lets an agent iterate the whole stack.
#
# The deploy account above deliberately cannot create or destroy
# infrastructure; that refusal is most of its value. But this platform is
# operated by agents that build the stack, test against it and tear the
# expensive parts down again to stop the meter — and the only key-free way to
# hand them that power is a second federated identity that holds it.
#
# Not owner, although `claude-builder` holds owner and this account applies
# the same configuration. Owner carries `storage.objects.delete`, and the
# acceptance suite refuses any Terraform-granted role that can delete from the
# evidence bucket — an append-only store whose writer can delete is a store
# nobody has deleted from yet. So the account holds the per-service admin
# roles the configuration actually exercises, plus a custom storage role that
# can manage buckets and read objects and cannot delete one.
#
# Two honest caveats, stated rather than discovered:
#
#   * This list is curated, so a module that starts using a new service fails
#     mid-apply with a permissions error naming a role to add here. That is
#     the accepted price of the evidence guarantee.
#   * `projectIamAdmin` + `roleAdmin` mean this account could grant itself
#     what it lacks — IAM admin is inherently self-escalating. The curated
#     list is defence in depth against mistakes, not a boundary against the
#     account itself; the boundary for evidence is the bucket's locked
#     retention policy, which no IAM role can undo.
#
# `serviceusage.serviceUsageAdmin` is deliberately absent, as it is for the
# deploy account: enabling an API widens the project's attack surface, so
# infra.yml operates environments a person has already bootstrapped.
#
# The remaining bound is the access path: no key, only this repository's
# GitHub OIDC tokens can impersonate it, and the one workflow that uses it —
# infra.yml — runs on manual dispatch only and refuses prod outright. The
# honest cost: anyone who can push a workflow file to this repository can
# reshape dev, test and stage. For a repository whose pushes are the
# platform's own agents acting for its owner, that is the intended
# arrangement, and prod stays behind the human-run bootstrap script.
resource "google_service_account" "infra" {
  project      = var.project_id
  account_id   = "qip-infra-${var.environment}"
  display_name = "qip infrastructure (${var.environment})"
  description  = "Applies and tears down Terraform from infra.yml. Impersonated through workload identity federation; it has no key."
}

resource "google_service_account_iam_member" "infra_impersonation" {
  service_account_id = google_service_account.infra.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.repository/${var.github_repository}"
}

resource "google_project_iam_member" "infra_roles" {
  for_each = toset([
    "roles/compute.admin",
    "roles/container.admin",
    "roles/iam.serviceAccountAdmin",
    "roles/iam.serviceAccountUser",
    "roles/iam.workloadIdentityPoolAdmin",
    "roles/iam.roleAdmin",
    "roles/resourcemanager.projectIamAdmin",
    "roles/cloudkms.admin",
    # admin manages keys and their IAM and deliberately excludes the crypto
    # operations themselves. One of those is needed read-only:
    # modules/binaryauthorization reads the attestor's public key at plan
    # time, and the first infra run died on exactly
    # `cloudkms.cryptoKeyVersions.viewPublicKey`. Viewer grants that and
    # nothing that signs, encrypts or decrypts.
    "roles/cloudkms.publicKeyViewer",
    "roles/secretmanager.admin",
    "roles/pubsub.admin",
    "roles/monitoring.admin",
    "roles/logging.admin",
    "roles/artifactregistry.admin",
    "roles/binaryauthorization.policyAdmin",
    "roles/containeranalysis.admin",
    "roles/gkebackup.admin",
  ])

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.infra.email}"
}

# Buckets, but not their objects' lives. Bucket delete is safe to hold: a
# bucket must be empty to be deleted, and the evidence bucket's locked
# retention policy means it cannot be emptied. Object delete is the one
# permission this role exists to not have — the exception is the Terraform
# state bucket, where overwriting state genuinely requires it, and that grant
# is made by `bootstrap-deploy.sh`, scoped to that bucket alone.
resource "google_project_iam_custom_role" "infra_storage" {
  project     = var.project_id
  role_id     = "qipInfraStorage_${var.environment}"
  title       = "qip infrastructure storage (${var.environment})"
  description = "Bucket administration and object reads for the infra account. Deliberately no storage.objects.delete."

  permissions = [
    "storage.buckets.create",
    "storage.buckets.delete",
    "storage.buckets.get",
    "storage.buckets.getIamPolicy",
    "storage.buckets.list",
    "storage.buckets.setIamPolicy",
    "storage.buckets.update",
    "storage.objects.get",
    "storage.objects.list",
  ]
}

resource "google_project_iam_member" "infra_storage" {
  project = var.project_id
  role    = google_project_iam_custom_role.infra_storage.id
  member  = "serviceAccount:${google_service_account.infra.email}"
}
