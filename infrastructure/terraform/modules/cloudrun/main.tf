# One workload from the blueprint's Cloud Run catalogue: its identity, its
# grants, and the files it reads.
#
# ADR 0022 makes the Algorik blueprint the architecture of record and its
# §41.6 names roughly seventy Rust binaries on Cloud Run, every one scaling to
# zero. ADR 0024 provisioned that runtime here, and until ADR 0036 this module
# also declared the `google_cloud_run_v2_service` itself. It no longer does.
# The service is a Config Connector `RunService` manifest under
# `infrastructure/gitops/envs/<env>/`, reconciled by Argo CD on the
# control-plane cluster (`modules/gitops-control-plane`), and this module
# keeps everything a manifest cannot carry and nothing a manifest carries:
#
#   * an identity per workload, created here, holding telemetry and the
#     secrets this workload mounts and nothing else;
#   * the accessor grant on exactly the secrets the workload reads, as files
#     (`secret_mounts`) or — for a vendored image only, ADR 0031 — as values
#     (`secret_env`);
#   * the committed configuration files the workload reads, published to a
#     bucket of its own under a directory named by their hash, so the file a
#     revision mounts is the file the reviewer read;
#   * the read grant on the egress proxy's bootstrap bucket, where the
#     workload carries the proxy;
#   * the collector's scrape document, where a workload carries one.
#
# # Where the service went, and why the cut is here
#
# ADR 0036 decision 5. A `removed` block in the root releases
# `google_cloud_run_v2_service.workload` and `_iam_member.invokers` from
# state without destroying them, and Terraform refuses a `removed` block
# whose address is still declared — even at `count = 0`, with "Removed
# resource still exists". So the resource is gone from this file rather than
# gated, and Config Connector acquires each service by its name on the first
# reconcile. Every precondition that used to sit on the service — internal
# ingress for the trading class, the ADR 0030 posture pairing, a justified
# floor above zero, a collector only on a service — checks nothing here now,
# because there is no service here to check. They are carried by the
# acceptance suite's parity test, which reads each manifest beside its
# catalogue entry; a manifest that relaxes one fails the build rather than
# the plan. What this file still refuses is what it still creates: an
# identity whose name overruns Google's limit, a customer-facing workload
# mounting the venue credential, two mounts on one path, and an `env` key
# colliding with a generated `_FILE` variable.
#
# # Who owns the image
#
# Nobody here. The manifest names the digest; Kargo's promotion commit moves
# it; `.github/workflows/deploy.yml` builds, scans, signs, attests and pushes
# and never touches a service. The `ignore_changes` rule that kept Terraform
# from rolling a deploy back left with the resource it was written for.

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

locals {
  name = "qip-${var.environment}-${var.name}"

  # A trust zone is the plane's own unless the caller says otherwise. See the
  # variable: a value written twice is a value that will disagree with itself.
  trust_zone = coalesce(var.trust_zone, var.plane)

  # Where the mounted secrets appear. The same directory the CSI driver
  # projected into on GKE, so a binary moved from a pod to a service reads the
  # same paths it read before — and the manifest mounts at exactly this root,
  # which the parity test asserts.
  secret_root = "/var/run/secrets/qip"

  # Each secret gets a directory of its own, because Cloud Run mounts a volume
  # at a path and two volumes may not share one. The process never learns
  # that: it reads the path in the generated _FILE variable.
  secret_files = {
    for key, mount in var.secret_mounts :
    key => "${local.secret_root}/${key}/${mount.file_name}"
  }

  # Configuration files, when this workload reads any: the committed bytes,
  # as one read-only directory. Every file's path is written here once and
  # reaches the process in its `_PATH` variable, so nothing has to know the
  # mount point.
  has_config_files = length(var.config_files) > 0
  config_root      = "/etc/qip"
  config_files = {
    for key, file in var.config_files :
    key => "${local.config_root}/${local.config_prefix}/${file.file_name}"
  }
  config_file_hashes = {
    for key, file in var.config_files :
    key => sha256(file.content)
  }

  # The files' directory is the hash of all of them, in key order, for the
  # reason the collector's document is named by its hash: a changed file is a
  # new directory beside the old one, never an overwrite, so publishing needs
  # no `storage.objects.delete` and every catalogue a revision ever read
  # stays readable under the name that revision mounted. One directory for
  # all of them because Cloud Run mounts a volume at a path and the process
  # reads them from one.
  config_prefix = substr(sha256(join("\n", [for key in sort(keys(var.config_files)) : local.config_file_hashes[key]])), 0, 16)

  # The environment the manifest must give the container: the caller's
  # non-secret settings, plus one path per mounted secret and per
  # configuration file. Values never appear here. Exported so the parity
  # test compares the manifest's `env` to this and not to a second list.
  environment = merge(
    var.env,
    {
      for key, mount in var.secret_mounts :
      mount.env_file_variable => local.secret_files[key]
    },
    {
      for key, file in var.config_files :
      file.env_file_variable => local.config_files[key]
    },
  )

  labels = merge(
    var.labels,
    {
      "qip-plane"         = var.plane
      "qip-trust-zone"    = local.trust_zone
      "qip-traffic-class" = var.traffic_class
      "qip-workload"      = var.name
    },
  )

  # Whether this workload carries the egress proxy. The sidecar itself is in
  # the manifest; what is here is the grant that lets it read its bootstrap.
  has_egress_sidecar = var.egress_sidecar != null

  # The managed-Prometheus collector, when this workload carries one.
  #
  # Keyed on the digest alone: a null digest is no bucket, no grant and
  # `metrics_collected = false`. There is no second switch that could declare
  # a collector without naming the bytes it runs.
  has_metrics_collector = var.collector_image_digest != null
  collector_mount       = "/etc/rungmp"

  # What the collector scrapes, as the `RunMonitoring` document the sidecar
  # reads from `/etc/rungmp/config.yaml`. The workload's own port and the
  # path both brains and the API serve their exposition on; thirty seconds
  # with a ten-second timeout, the same cadence the execution node's Ops
  # Agent receiver uses, so the two planes' series are comparable. Written
  # here rather than left to the sidecar's built-in default so that the
  # target and the interval are in a diff, not in an image.
  collector_config = <<-EOT
    apiVersion: monitoring.googleapis.com/v1beta
    kind: RunMonitoring
    metadata:
      name: ${local.name}
    spec:
      endpoints:
        - port: ${var.container_port}
          path: /metrics
          interval: 30s
          timeout: 10s
  EOT

  # The object's directory is its content's hash, for the reason the egress
  # bootstrap is named by its hash: a changed configuration is a new object
  # beside the old one, never an overwrite.
  collector_prefix = substr(sha256(local.collector_config), 0, 16)

  # The service's own URL, as Cloud Run has assigned it deterministically
  # since 2024: the service name, the project number, the region. Computed
  # rather than read from a resource, because the resource is Config
  # Connector's now; `gcloud run services describe` is the authority in the
  # window between a sync and its proof.
  uri = "https://${local.name}-${var.project_number}.${var.region}.run.app"
}

# --- the workload's own identity --------------------------------------------

# One service account per workload, and never the project's default compute
# identity.
#
# The default account is shared by everything in the project that does not name
# one; a grant given to it for one workload is a grant given to all of them,
# and `modules/secrets` already records what that cost when the portal was
# deployed under it. With seventy workloads coming, an account per workload is
# the difference between a compromise that holds one service's permissions and
# one that holds the catalogue's.
#
# It is also half of the rule that customer traffic and trading traffic never
# share an identity: they cannot, because no two workloads share one at all.
resource "google_service_account" "workload" {
  project = var.project_id

  # 6 to 30 characters, enforced by Google at apply time — which is after the
  # image has been built and pushed. The precondition moves that to plan time,
  # exactly as `modules/edge-cell` did for a cell id.
  account_id   = "qip-${var.name}-${var.environment}"
  display_name = "qip ${var.name} (${var.environment})"
  description  = "Runs the ${var.name} service on Cloud Run, in the ${var.plane} plane."

  # Every cross-variable refusal in this module is here rather than in a
  # `validation` block: a validation that reads a second variable is skipped
  # silently, and a precondition fails during plan with the message and no
  # diff.
  lifecycle {
    precondition {
      condition     = length("qip-${var.name}-${var.environment}") <= 30
      error_message = "The derived service account id qip-${var.name}-${var.environment} is ${length("qip-${var.name}-${var.environment}")} characters; Google allows 30. Shorten the workload name."
    }

    # The venue credential belongs to the trading zone and reaches nothing
    # else.
    #
    # `modules/secrets` already makes it unreadable in an environment whose
    # ceiling is paper trading. This is the neighbouring failure: an
    # environment where it is readable, and a customer-facing workload mounting
    # it because the mount list was copied from the service next door. Matched
    # on the secret's own prefix rather than a substring of anything, because
    # this repository has already been bitten by a check that matched a
    # neighbouring token.
    precondition {
      condition = var.traffic_class != "customer" || alltrue([
        for mount in values(var.secret_mounts) :
        !startswith(mount.secret_id, "qip-venue-credential")
      ])
      error_message = "A customer-facing workload may not read the venue credential. Customer traffic and trading traffic share no credential; the workload that needs it is in the trading class and is reached over the VPC."
    }

    # Two mounts producing the same file path silently leave one of the two
    # secrets unreadable, and the process fails later looking like a bad
    # credential rather than a bad mount.
    precondition {
      condition     = length(values(local.secret_files)) == length(distinct(values(local.secret_files)))
      error_message = "Two secret mounts resolve to the same file path. Give each mount its own key and file name."
    }

    # The generated _FILE variables must not collide with the caller's own
    # settings: merge would silently keep one, and the one it keeps would be
    # the caller's plaintext.
    precondition {
      condition = length(setintersection(
        keys(var.env),
        [for mount in values(var.secret_mounts) : mount.env_file_variable],
      )) == 0
      error_message = "An entry in `env` has the same name as a variable generated for a mounted secret. Remove it from `env`; the path is written from secret_mounts."
    }
  }
}

# Who may create a revision as this identity.
#
# Cloud Run refuses to create a revision unless the caller may act as the
# service's account, so the reconciler needs `serviceAccountUser` on this one
# account — and on this one account only. Granted here, per workload, rather
# than project-wide: a project-wide `serviceAccountUser` is the right to act
# as every identity in the project, the infra account included. Under ADR
# 0036 the caller is Config Connector's identity; before it, the pipeline's.
resource "google_service_account_iam_member" "deployer" {
  count = var.deployer_service_account == null ? 0 : 1

  service_account_id = google_service_account.workload.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.deployer_service_account}"
}

# The minimum every workload needs beyond its own secrets, and no more.
#
# A workload that cannot write telemetry is a workload nobody can operate,
# and everything past that is a decision about one particular service rather
# than about all of them. This module deliberately takes no `additional_roles`
# parameter — a list of extra roles on a module instantiated seventy times is
# a place for a wide grant to arrive quietly. A workload that needs to read a
# bucket gets that grant where the bucket is, named, in a file somebody
# reviews.
resource "google_project_iam_member" "telemetry" {
  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.workload.email}"
}

resource "google_project_iam_member" "logging" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.workload.email}"
}

# Read exactly the secrets this workload mounts.
#
# Scoped to the secret rather than granted at the project, which is the
# difference between a workload that can read its own credential and one that
# can read every credential in the deployment. No secret here is created by
# this module and none has a value in Terraform.
resource "google_secret_manager_secret_iam_member" "mounted" {
  for_each = var.secret_mounts

  project   = var.project_id
  secret_id = each.value.secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload.email}"
}

# The same grant for the other way a secret reaches this workload.
#
# ADR 0031 added `secret_env` and this resource did not exist, so a workload
# that took its credential as an environment value was granted nothing. That
# is not a slow failure: Cloud Run resolves `secret_key_ref` *before* it
# starts the instance, so a workload without the grant has no instance at
# all, and the URL answers the load balancer's own 500 rather than anything
# the container wrote. The manifest that mounts the value must be applied
# after this grant exists; Argo CD's sync is ordered by the same fact the
# `depends_on` on the old resource stated, and the first sync of an
# environment is what proves it.
resource "google_secret_manager_secret_iam_member" "env" {
  for_each = var.secret_env

  project   = var.project_id
  secret_id = each.value.secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload.email}"
}

# Read the proxy's bootstrap, when this workload carries the proxy.
#
# `objectViewer` on the one bucket that holds the allowlist. It is granted
# here rather than in `modules/egress-proxy` because it is the sidecar's own
# configuration — a grant for a file this workload cannot start without — and
# because the alternative is a list of reader identities passed back into that
# module from the outputs of this one, which is the shape that turns into a
# module cycle the day either side gains a `depends_on`. The role carries no
# write and no delete: the allowlist is written by Terraform and by nothing
# that runs.
resource "google_storage_bucket_iam_member" "egress_bootstrap" {
  count = local.has_egress_sidecar ? 1 : 0

  bucket = var.egress_sidecar.bootstrap_bucket
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.workload.email}"
}

# --- the collector's configuration ------------------------------------------

# Where the collector reads its scrape configuration from, when this workload
# carries one.
#
# A bucket rather than a secret, for the reason the egress bootstrap is one:
# the document is not confidential, `no_secret_value_appears_in_the_terraform`
# refuses a secret version written from Terraform, and a bucket object is the
# one Cloud Run volume type that carries a file Terraform wrote. One bucket per
# collecting workload, because the object names the workload's own port and a
# shared bucket would put one workload's configuration where another could
# mount it.
resource "google_storage_bucket" "collector_config" {
  count = local.has_metrics_collector ? 1 : 0

  project  = var.project_id
  name     = "qip-metrics-${var.environment}-${var.name}-${var.project_id}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false

  versioning {
    enabled = true
  }

  labels = local.labels

  lifecycle {
    # Google's limit is 63 characters, enforced at apply. Refused at plan
    # instead, naming the length.
    precondition {
      condition     = length("qip-metrics-${var.environment}-${var.name}-${var.project_id}") <= 63
      error_message = "The collector bucket name qip-metrics-${var.environment}-${var.name}-${var.project_id} is ${length("qip-metrics-${var.environment}-${var.name}-${var.project_id}")} characters; Google allows 63. Shorten the workload name."
    }
  }
}

resource "google_storage_bucket_object" "collector_config" {
  count = local.has_metrics_collector ? 1 : 0

  bucket       = google_storage_bucket.collector_config[0].name
  name         = "${local.collector_prefix}/config.yaml"
  content      = local.collector_config
  content_type = "application/yaml"
}

# Read the one file, and nothing else. The same narrow role the egress
# bootstrap is read with; the collector writes to Cloud Monitoring on the
# `roles/monitoring.metricWriter` every workload already holds, so nothing
# is widened for it.
resource "google_storage_bucket_iam_member" "collector_config" {
  count = local.has_metrics_collector ? 1 : 0

  bucket = google_storage_bucket.collector_config[0].name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.workload.email}"
}

# --- configuration files ------------------------------------------------------

# The committed configuration this workload reads, published where a Cloud
# Run volume can carry it. A bucket rather than a secret, as the collector's
# document is: the content is not confidential — it is a file in the
# repository — `no_secret_value_appears_in_the_terraform` refuses a secret
# version written from Terraform, and a bucket object is the one Cloud Run
# volume type that carries a file Terraform wrote. One bucket per workload,
# so one workload's configuration is never where another could mount it.
resource "google_storage_bucket" "config_files" {
  count = local.has_config_files ? 1 : 0

  project  = var.project_id
  name     = "qip-config-${var.environment}-${var.name}-${var.project_id}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false

  versioning {
    enabled = true
  }

  labels = local.labels

  lifecycle {
    # Google's limit is 63 characters, enforced at apply. Refused at plan
    # instead, naming the length.
    precondition {
      condition     = length("qip-config-${var.environment}-${var.name}-${var.project_id}") <= 63
      error_message = "The configuration bucket name qip-config-${var.environment}-${var.name}-${var.project_id} is ${length("qip-config-${var.environment}-${var.name}-${var.project_id}")} characters; Google allows 63. Shorten the workload name."
    }
  }
}

# One object per file, under the hash-named directory, with exactly the
# committed content. The object is the file: what the process reads at
# `/etc/qip/<hash>/<file_name>` is these bytes, and `config_file_hashes` says
# which. The manifest's `_PATH` variable names this directory, so a change to
# a committed file is a new object here and a new path in the manifest,
# reviewed together; the parity test refuses the pair disagreeing.
resource "google_storage_bucket_object" "config_files" {
  for_each = local.has_config_files ? var.config_files : {}

  bucket       = google_storage_bucket.config_files[0].name
  name         = "${local.config_prefix}/${each.value.file_name}"
  content      = each.value.content
  content_type = each.value.content_type
}

# Read the files, and nothing else. The same narrow role the collector's
# document and the egress bootstrap are read with, on this workload's own
# bucket alone.
resource "google_storage_bucket_iam_member" "config_files" {
  count = local.has_config_files ? 1 : 0

  bucket = google_storage_bucket.config_files[0].name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.workload.email}"
}
