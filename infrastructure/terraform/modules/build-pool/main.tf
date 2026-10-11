# A Cloud Build private pool, peered into the Engineering/build VPC (R12
# CICD-9097).
#
# Until this module the platform's images were built on GitHub-hosted
# runners: a machine nobody here provisions, shared with every other GitHub
# customer, holding the pipeline's federated credential while it compiled
# untrusted input. The reproducibility check in `deploy.yml` proved the bytes
# could be rebuilt; it said nothing about where they were built. A private
# pool is a worker this project owns, in a network this project peers, with
# an identity whose only write is one prefix of one staging bucket. The pool
# hands back a saved image and its id; the pipeline loads it, checks the id,
# and pushes, scans, signs and attests as before — so nothing built in the
# pool reaches the registry, let alone an attestation, on its own say-so.
#
# Every name below is a pure function of the project id and the environment,
# because `deploy.yml` constructs them from the committed tfvars exactly as
# it constructs the workload-identity audience — no repository variable, no
# `terraform output` pasted anywhere:
#
#   pool            projects/<project>/locations/<region>/workerPools/qip-<env>-build
#   build identity  qip-build-<env>@<project>.iam.gserviceaccount.com
#   staging bucket  <project>-qip-<env>-build-source
#
# Change one here and the workflow's derivation in the same diff.
#
# No key exists for the build identity and none may: Cloud Build mints its
# token from the metadata server, and the pipeline reaches the pool through
# Workload Identity Federation.

locals {
  pool_name      = "qip-${var.environment}-build"
  build_account  = "qip-build-${var.environment}"
  staging_bucket = "${var.project_id}-qip-${var.environment}-build-source"
}

# --- The peering ---------------------------------------------------------------
#
# A private pool's workers live in a Google-owned producer network, reached
# through a Service Networking peering into a range reserved in ours. The
# range is reserved in the Engineering VPC, which hosts nothing else: no
# workload subnet, no Service Networking connection of `modules/data`'s
# (that one peers the service network), so this connection cannot collide
# with another on the same network.
resource "google_compute_global_address" "pool" {
  project       = var.project_id
  name          = "${local.pool_name}-peering"
  purpose       = "VPC_PEERING"
  address_type  = "INTERNAL"
  prefix_length = var.peering_prefix_length
  network       = var.peered_network_id
  labels        = var.labels
}

resource "google_service_networking_connection" "pool" {
  network                 = var.peered_network_id
  service                 = "servicenetworking.googleapis.com"
  reserved_peering_ranges = [google_compute_global_address.pool.name]
}

resource "google_cloudbuild_worker_pool" "pool" {
  project  = var.project_id
  name     = local.pool_name
  location = var.region

  worker_config {
    machine_type   = var.machine_type
    disk_size_gb   = var.disk_size_gb
    no_external_ip = var.no_external_ip
  }

  network_config {
    peered_network          = var.peered_network_id
    peered_network_ip_range = "/${var.peering_prefix_length}"
  }

  depends_on = [google_service_networking_connection.pool]
}

# --- The build identity --------------------------------------------------------
#
# What a build runs as. It reads the staged source and writes its result
# under `out/` in the staging bucket, and nothing else: no registry write, no
# Secret Manager, no KMS, no attestor. Pushing, signing and attesting stay in
# `deploy.yml`, after the scan, under the pipeline's own identity — a build
# that could publish or attest its own output would make the attestation
# mean only that a build ran.
resource "google_service_account" "build" {
  project      = var.project_id
  account_id   = local.build_account
  display_name = "qip ${var.environment} private-pool builds"
  description  = "Runs builds in ${local.pool_name}. Writes a saved image to its staging bucket; no registry write, no secret, signs nothing. No key."
}

# A build run as a named identity must write its own log: Cloud Build refuses
# a user-specified service account whose build does not set a logging mode,
# and `infrastructure/cloudbuild/build-image.yaml` sets CLOUD_LOGGING_ONLY.
resource "google_project_iam_member" "build_writes_logs" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.build.email}"
}

# --- Where the source is staged ------------------------------------------------
#
# `gcloud builds submit` uploads the checkout as a tarball. Left to itself it
# creates `<project>_cloudbuild`, which needs `storage.buckets.create` on the
# project — a grant the pipeline should not hold to stage a tarball. So the
# bucket is this one, named, and the workflow passes it.
#
# `force_destroy = true`, deliberately and unlike the evidence buckets: what
# it holds is a copy of a commit that is in git, aged out after a week, and
# a teardown that could not delete it would leave a bucket standing for no
# record anybody needs.
#
# Encrypted under a key from the environment ring (SEC-046), not one Google
# holds: what is staged here is the full source tree of a commit, and the
# saved image the build hands back under `out/` is what the pipeline then
# signs. A store of either under a key nobody here can disable or audit is
# the gap `security_controls.rs` refuses for every other bucket.
resource "google_kms_crypto_key" "staging" {
  name     = "qip-${var.environment}-build-source"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Ninety days, matching the platform's other keys.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = var.kms_protection_level
  }

  # Like every key in this tree. A Cloud KMS key cannot be deleted, only
  # have its versions destroyed, and a key of the same name cannot then be
  # created again — so a plan that destroyed this one would make the pool
  # impossible to turn back on. Turning the pool off is a `terraform state
  # rm` of this key, read and done by a person.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# Cloud Storage encrypts with the key as its own service agent, not as the
# caller, so the agent needs the key rather than the build identity.
data "google_storage_project_service_account" "storage" {
  project = var.project_id
}

resource "google_kms_crypto_key_iam_member" "storage_agent" {
  crypto_key_id = google_kms_crypto_key.staging.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:${data.google_storage_project_service_account.storage.email_address}"
}

resource "google_storage_bucket" "staging" {
  project                     = var.project_id
  name                        = local.staging_bucket
  location                    = var.region
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = true
  labels                      = var.labels

  encryption {
    default_kms_key_name = google_kms_crypto_key.staging.id
  }

  # The bucket is created only once the agent can use its key; otherwise the
  # first upload fails on a permission the plan already granted.
  depends_on = [google_kms_crypto_key_iam_member.storage_agent]

  lifecycle_rule {
    condition {
      age = 7
    }
    action {
      type = "Delete"
    }
  }
}

resource "google_storage_bucket_iam_member" "build_reads_source" {
  bucket = google_storage_bucket.staging.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.build.email}"
}

# The build's one write: the saved image and its id, under `out/`. The
# condition keeps it from overwriting the staged source of another build.
resource "google_storage_bucket_iam_member" "build_writes_output" {
  bucket = google_storage_bucket.staging.name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${google_service_account.build.email}"

  condition {
    title       = "build-output-only"
    description = "A private-pool build writes its saved image under out/ and nowhere else."
    expression  = "resource.name.startsWith(\"projects/_/buckets/${local.staging_bucket}/objects/out/\")"
  }
}

# --- What the pipeline may do with it -----------------------------------------
#
# Four grants, each the one permission a `gcloud builds submit
# --worker-pool --service-account --gcs-source-staging-dir` needs, and each
# scoped as narrowly as Google lets it be:
#
#   * creating a build: `cloudbuild.builds.editor`, project-level because a
#     build has no parent narrower than the project;
#   * running it in the pool: `cloudbuild.workerPoolUser`, project-level for
#     the same reason (worker pools carry no IAM policy of their own);
#   * running it as the build identity: `iam.serviceAccountUser` on that one
#     account, never project-wide;
#   * staging the tarball: object create under `source/` only, and the
#     bucket read `gcloud` makes before uploading;
#   * collecting the result: object read on the same bucket, to download
#     the saved image the pool wrote under `out/`.
resource "google_project_iam_member" "ci_submits_builds" {
  project = var.project_id
  role    = "roles/cloudbuild.builds.editor"
  member  = "serviceAccount:${var.ci_service_account}"
}

resource "google_project_iam_member" "ci_uses_the_pool" {
  project = var.project_id
  role    = "roles/cloudbuild.workerPoolUser"
  member  = "serviceAccount:${var.ci_service_account}"
}

resource "google_service_account_iam_member" "ci_runs_builds_as_the_build_identity" {
  service_account_id = google_service_account.build.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.ci_service_account}"
}

resource "google_storage_bucket_iam_member" "ci_reads_staging" {
  for_each = toset([
    "roles/storage.legacyBucketReader",
    "roles/storage.objectViewer",
  ])

  bucket = google_storage_bucket.staging.name
  role   = each.value
  member = "serviceAccount:${var.ci_service_account}"
}

# The pipeline's one write: the checkout tarball, under `source/`, where
# `deploy.yml` stages it (`pool_staging`). Until 2026-10-11 this grant held
# the whole bucket, `out/` included, so the identity that loads the saved
# image and compares its id with the pool's could also have written both —
# the two-build reproducibility check would then have compared the pipeline
# with itself. An allow-list rather than "not out/", so a prefix added later
# is closed to the pipeline until a person opens it.
resource "google_storage_bucket_iam_member" "ci_stages_source" {
  bucket = google_storage_bucket.staging.name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${var.ci_service_account}"

  condition {
    title       = "pipeline-source-only"
    description = "The pipeline stages the checkout under source/ and writes nowhere else, out/ least of all."
    expression  = "resource.name.startsWith(\"projects/_/buckets/${local.staging_bucket}/objects/source/\")"
  }
}
