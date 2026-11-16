# The scout (ADR 0144, R12-DATA-8112): a Cloud Run job that polls the
# catalogued research connectors once per execution under an identity of its
# own, and writes each fetch as an immutable record to a bucket the deep brain
# may only read.
#
# The identity's grants are a closed list and the acceptance suite's
# `scout_identity.rs` holds this file to it: create objects in the scout
# bucket, write logs. No other role is granted to it, in this module or
# anywhere it can be named from. It holds nothing on a trading service, the
# fabric, the ledger, a signing key, or the deep brain's secrets.
#
# Nothing here is applied by being written: the module is not instantiated by
# the root, and an environment gains the job only through the owner's reviewed
# plan.

locals {
  name = "qip-scout-${var.environment}"
}

resource "google_service_account" "scout" {
  project      = var.project_id
  account_id   = local.name
  display_name = "qip scout (${var.environment})"
  description  = "Polls research connectors. Holds no trading role (ADR 0144)."
}

resource "google_storage_bucket" "records" {
  project  = var.project_id
  name     = "${local.name}-records-${var.region}"
  location = var.region
  labels   = var.labels

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
}

# Create-only. objectCreator cannot overwrite or delete, so a compromised scout
# cannot replace a record the deep brain has read; the writer also uses
# ifGenerationMatch=0.
resource "google_storage_bucket_iam_member" "scout_creates" {
  bucket = google_storage_bucket.records.name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${google_service_account.scout.email}"
}

# The deep brain reads and re-validates; it cannot write here.
resource "google_storage_bucket_iam_member" "deepbrain_reads" {
  bucket = google_storage_bucket.records.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${var.deepbrain_service_account_email}"
}

resource "google_project_iam_member" "scout_logs" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.scout.email}"
}

resource "google_cloud_run_v2_job" "scout" {
  project  = var.project_id
  name     = local.name
  location = var.region
  labels   = var.labels

  template {
    template {
      service_account = google_service_account.scout.email
      timeout         = "${var.task_timeout_seconds}s"
      max_retries     = 0

      containers {
        image = var.image
        args  = ["scout"]

        env {
          name  = "QIP_CONNECTOR_SOURCE"
          value = var.connector_sources
        }
        env {
          name  = "QIP_CONNECTOR_BASE_URL"
          value = var.egress_proxy_url
        }
        env {
          name  = "QIP_SCOUT_BUCKET"
          value = google_storage_bucket.records.name
        }
      }
    }
  }
}

# The trigger has its own identity, allowed to run this one job and nothing
# else.
resource "google_service_account" "trigger" {
  count = var.schedule == null ? 0 : 1

  project      = var.project_id
  account_id   = "${local.name}-trigger"
  display_name = "qip scout trigger (${var.environment})"
}

resource "google_cloud_run_v2_job_iam_member" "trigger_runs" {
  count = var.schedule == null ? 0 : 1

  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_job.scout.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:${google_service_account.trigger[0].email}"
}

resource "google_cloud_scheduler_job" "scout" {
  count = var.schedule == null ? 0 : 1

  project  = var.project_id
  region   = var.region
  name     = local.name
  schedule = var.schedule

  http_target {
    http_method = "POST"
    uri         = "https://${var.region}-run.googleapis.com/apis/run.googleapis.com/v1/namespaces/${var.project_id}/jobs/${google_cloud_run_v2_job.scout.name}:run"

    oauth_token {
      service_account_email = google_service_account.trigger[0].email
    }
  }
}
