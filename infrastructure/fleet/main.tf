# The development and research fleet (ADR 0102), first slice: one Cloud Run
# Job, the one identity it runs as, and the one bucket it reads packets from
# and writes outputs to.
#
# This is a root of its own, with its own state, on purpose. ADR 0098 asked for
# the fleet's resources to sit where `terraform plan` on the platform's root
# cannot propose to change them and the reverse. The project boundary ADR 0098
# wanted is gone (the owner chose the dev project), so the fence is what the
# worker identity is *not* able to do, and that is checkable by listing its
# bindings: `output.worker_roles` is that list and `tests/fleet.tftest.hcl`
# fails if it grows.
#
# Nothing here touches the trading platform. No Secret Manager, KMS, Artifact
# Registry write, Cloud Run, Binary Authorization or git right is granted to the
# worker, so a worker cannot start a worker, read the venue credential, or push.

terraform {
  required_version = ">= 1.9.0"

  # State lives in the fleet's own bucket (`<project>-fleet-tfstate`), passed
  # at init with `-backend-config`. Not the platform's state bucket: an
  # identity that plans the fleet then needs no access to the platform's
  # state, and a plan here cannot see the platform's resources at all.
  backend "gcs" {
    prefix = "fleet"
  }

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

locals {
  labels = {
    env         = var.environment
    service     = "fleet"
    owner       = var.owner
    cost-center = var.cost_center
  }

  # The whole of the worker's project-level authority. Vertex AI, and nothing
  # else. Object access to the bucket is bound on the bucket below, never on the
  # project, so it cannot reach another bucket.
  worker_project_roles = var.worker_may_call_models ? ["roles/aiplatform.user"] : []
  worker_bucket_role   = "roles/storage.objectUser"

  # Vertex AI, Cloud Run, the bucket and the identity. Enabling is a human act
  # by default (`enable_apis = false`); see README.md.
  apis = ["aiplatform.googleapis.com", "run.googleapis.com", "storage.googleapis.com", "iam.googleapis.com", "artifactregistry.googleapis.com"]
}

resource "google_project_service" "api" {
  for_each = var.enable_apis ? toset(local.apis) : toset([])

  service            = each.value
  disable_on_destroy = false
}

resource "google_service_account" "worker" {
  account_id   = "fleet-worker"
  display_name = "Fleet worker (ADR 0102): Vertex AI and one bucket, nothing else"
}

# Absent on purpose: google_service_account_key. The credential is the metadata
# server's short-lived token (01-security-and-safety.md).

resource "google_project_iam_member" "worker" {
  for_each = toset(local.worker_project_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.worker.email}"
}

resource "google_storage_bucket" "fleet" {
  name                        = "${var.project_id}-fleet"
  location                    = var.region
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  labels                      = local.labels

  # Bounded retention. HALT is deliberately outside every prefix named here,
  # because a kill switch an age rule can delete is a switch that quietly
  # un-halts the fleet a month later.
  lifecycle_rule {
    condition {
      age            = var.retention_days
      matches_prefix = ["packets/", "output/", "ledger/"]
    }
    action {
      type = "Delete"
    }
  }
}

resource "google_storage_bucket_iam_member" "worker" {
  bucket = google_storage_bucket.fleet.name
  role   = local.worker_bucket_role
  member = "serviceAccount:${google_service_account.worker.email}"
}

# The one repository the worker image is pushed to. The worker identity holds
# no role on it: Cloud Run's own service agent pulls the image, so a worker
# can neither push an image nor read another one.
resource "google_artifact_registry_repository" "fleet" {
  repository_id = "fleet"
  location      = var.region
  format        = "DOCKER"
  description   = "The fleet worker image (ADR 0102). Pushed by a person or CI, never by a worker."
  labels        = local.labels

  depends_on = [google_project_service.api]
}

# Created only once an image exists. The first apply has to create the
# repository before anything can be pushed to it, and a Job naming an image
# that is not there fails at create with a message about the image, not about
# the order.
resource "google_cloud_run_v2_job" "fleet" {
  count = var.image == null ? 0 : 1

  name                = "fleet"
  location            = var.region
  labels              = local.labels
  deletion_protection = false

  template {
    # One packet per task. The dispatcher sets the task count per run; the
    # parallelism is the ladder's current rung and is capped at 40 by variable.
    parallelism = var.parallelism
    task_count  = 1
    labels      = local.labels

    template {
      service_account = google_service_account.worker.email
      # The platform default retries a task blindly, and retrying a wrong
      # answer buys a differently wrong answer at full price (ADR 0102 §2).
      max_retries = 0
      timeout     = "${var.task_timeout_seconds}s"

      containers {
        image = var.image

        resources {
          limits = {
            cpu    = "1"
            memory = "512Mi"
          }
        }

        env {
          name  = "FLEET_BUCKET"
          value = google_storage_bucket.fleet.name
        }
        env {
          name  = "FLEET_DAILY_USD_CEILING"
          value = tostring(var.daily_usd_ceiling)
        }
        env {
          name  = "FLEET_SLOT_USD_CAP"
          value = tostring(var.per_slot_usd_cap)
        }
      }
    }
  }

  lifecycle {
    # The dispatcher sets these per run.
    ignore_changes = [client, client_version]
  }

  depends_on = [google_project_service.api]
}
