# Managed training and model storage.
#
# # What this does and does not make true
#
# Provisioning this gives the platform somewhere for a training job to run. It
# does not give it the ability to submit one: `qip-training`'s Vertex port has
# no client, no credential and no egress path, and reports itself unavailable
# by design rather than pretending. Infrastructure cannot close that gap, and
# the flag guarding this module exists so nobody mistakes a provisioned
# service for a reachable one.
#
# What *is* real today is `qip_training::local` — ridge regression and
# gradient-boosted stumps fitted in-process, with a held-out tail and a skill
# verdict that a fit cannot assert about itself. That path needs none of this.
#
# # Why the metadata store is the part that matters
#
# The platform's model governance depends on a model card whose evaluation is
# the verdict of the fit it describes — `crates/runtime/qip-kernel/src/central/
# models.rs` refuses to let a caller assert `passed` for exactly that reason. A
# managed training service that recorded runs somewhere the platform's registry
# could not read would split that record in two, and the half a regulator asks
# for would be whichever half is missing.

locals {
  prefix = "qip-${var.environment}"
}

resource "google_kms_crypto_key" "models" {
  count = var.enable_vertex_ai ? 1 : 0

  name     = "${local.prefix}-models"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # A trained model is a derived asset that can be refitted from data the
  # platform keeps, so a shorter rotation costs less here than it does on the
  # stores that hold originals.
  rotation_period = "2592000s"

  lifecycle {
    prevent_destroy = true
  }
}

# Staging for training inputs and outputs.
#
# Separate from the model-artifact bucket in the data module on purpose: this
# one is scratch that a job writes during a run, and the other holds the
# artifact a model card points at. Mixing them means a cleanup policy aimed at
# scratch eventually deletes something a card references.
resource "google_storage_bucket" "training" {
  count = var.enable_vertex_ai ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-training-staging"
  location = var.region
  labels   = var.labels

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  encryption {
    default_kms_key_name = google_kms_crypto_key.models[0].id
  }

  # Scratch, and it says so: a run's inputs are reproducible from the dataset
  # and the seed, both of which the training spec records.
  lifecycle_rule {
    condition {
      age = 30
    }
    action {
      type = "Delete"
    }
  }
}

# Where training runs are recorded.
#
# The platform's own `ModelRegistry` is the authority on whether a model may
# inform a decision; this holds the lineage of the run that produced it. Two
# records of the same fact, deliberately, because the question they answer is
# different: one is "may this model trade", the other is "what produced it".
resource "google_vertex_ai_metadata_store" "training" {
  count = var.enable_vertex_ai ? 1 : 0

  provider = google-beta

  project     = var.project_id
  region      = var.region
  name        = replace("${local.prefix}-training", "-", "_")
  description = "Lineage of every managed training run: dataset, spec, seed, outcome."

  encryption_spec {
    kms_key_name = google_kms_crypto_key.models[0].id
  }
}

# Training runs inside the VPC, never over a public path. A job that could
# reach the internet is a job that could exfiltrate the training data, and the
# training data is the platform's position and fill history.
resource "google_vertex_ai_endpoint" "serving" {
  count = var.enable_vertex_ai ? 1 : 0

  project      = var.project_id
  location     = var.region
  name         = "${local.prefix}-serving"
  display_name = "QIP ${var.environment} model serving"
  labels       = var.labels
  network      = var.network_id

  encryption_spec {
    kms_key_name = google_kms_crypto_key.models[0].id
  }
}

# Create and read, never delete — even on scratch.
#
# `objectAdmin` was the obvious grant and it is the wrong one: a training job
# that can delete objects is a job that can erase the inputs of the run that
# produced a model, and the acceptance suite refuses that role anywhere in this
# configuration for exactly that reason. Nothing is lost by narrowing it,
# because the thirty-day lifecycle rule above is what removes scratch. Deletion
# is the bucket's policy rather than the workload's privilege.
resource "google_storage_bucket_iam_member" "training_writer" {
  count = var.enable_vertex_ai && var.training_service_account != "" ? 1 : 0

  bucket = google_storage_bucket.training[0].name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${var.training_service_account}"
}

resource "google_storage_bucket_iam_member" "training_reader" {
  count = var.enable_vertex_ai && var.training_service_account != "" ? 1 : 0

  bucket = google_storage_bucket.training[0].name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${var.training_service_account}"
}

# `aiplatform.user`, not `aiplatform.admin`: a training workload submits jobs
# and reads its own results. Creating and deleting endpoints is a deployment
# action, and a workload that could do it could also replace the model serving
# live traffic without a deployment anyone reviewed.
resource "google_project_iam_member" "training_user" {
  count = var.enable_vertex_ai && var.training_service_account != "" ? 1 : 0

  project = var.project_id
  role    = "roles/aiplatform.user"
  member  = "serviceAccount:${var.training_service_account}"
}
