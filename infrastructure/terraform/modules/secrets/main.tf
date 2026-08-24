# Secrets, keys and identities.
#
# The rule this module exists to enforce: no secret value is ever in Terraform.
# Every secret is created empty and its value written out of band, so a state
# file that leaks contains the shape of the deployment and none of its
# credentials.
#
# The venue credential is the one that matters most. It is readable only where
# the autonomy ceiling permits live trading, so a paper-trading environment
# holding one cannot use it — not because the application declines, but
# because the IAM binding does not exist.

terraform {
  required_providers {
    google = {
      source = "hashicorp/google"
    }
    # `google_project_service_identity` only. Enabling an API does not always
    # create its service agent; this resource forces the agent into existence
    # so a grant to it does not race a lazily created account. The first real
    # apply lost that race twice — GKE Backup's agent and Pub/Sub's grant.
    google-beta = {
      source = "hashicorp/google-beta"
    }
  }
}

resource "google_kms_key_ring" "platform" {
  project  = var.project_id
  name     = "qip-${var.environment}"
  location = var.region
}

resource "google_kms_crypto_key" "node_encryption" {
  name     = "qip-${var.environment}-node-encryption"
  key_ring = google_kms_key_ring.platform.id

  # Ninety days. Short enough that a compromised key has a bounded window,
  # long enough that rotation is not itself a source of incidents.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  # Destroying the key that encrypts etcd destroys the cluster's data.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# The GKE service agent encrypts etcd with the node-encryption key as itself,
# not as any workload — so the agent needs the key, exactly like the storage
# agent in modules/evidence. Without this the cluster fails its precondition
# with MISSING_IAM_PERMISSIONS_ON_CRYPTO_KEY, which is how the first real
# apply died.
resource "google_project_service_identity" "container" {
  provider = google-beta
  project  = var.project_id
  service  = "container.googleapis.com"
}

resource "google_kms_crypto_key_iam_member" "gke_robot" {
  crypto_key_id = google_kms_crypto_key.node_encryption.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@container-engine-robot.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.container]
}

resource "google_kms_crypto_key" "secrets" {
  name            = "qip-${var.environment}-secrets"
  key_ring        = google_kms_key_ring.platform.id
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# The nodes' identity.
#
# Separate from every workload account on purpose. The kubelet pulls images and
# writes node telemetry as this account; a pod authenticates as its own through
# workload identity. Reusing a workload's account for the node pool would mean a
# node compromise yields that workload's permissions, which is precisely the
# sharing the accounts below exist to avoid.
resource "google_service_account" "nodes" {
  project      = var.project_id
  account_id   = "qip-nodes-${var.environment}"
  display_name = "qip nodes (${var.environment})"
  description  = "The node pool's identity. Pulls images and writes node telemetry; runs no workload."
}

resource "google_project_iam_member" "node_telemetry" {
  for_each = toset([
    "roles/monitoring.metricWriter",
    "roles/logging.logWriter",
    "roles/stackdriver.resourceMetadata.writer",
  ])

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.nodes.email}"
}

# One service account per deployable. A compromised component has only its own
# permissions, which is the entire argument for not sharing one.
resource "google_service_account" "workload" {
  for_each = var.service_accounts

  project      = var.project_id
  account_id   = "${each.value}-${var.environment}"
  display_name = "qip ${each.key} (${var.environment})"
  description  = "Workload identity for the qip ${each.key} deployable."
}

# The secrets themselves, created without values.
# Where Secret Manager announces that a rotation is due.
#
# This is the one Pub/Sub topic in the platform, and it is not a contradiction
# of ADR 0011. That decision replaced Pub/Sub as the *data* bus — the path
# carrying state deltas and capital envelopes between cells and the central
# plane, which is now the in-tree mesh in `qip-transport`. This carries no
# platform data: it is a control-plane notification from a Google service to
# an operator, and Secret Manager will not accept a rotation schedule without
# one.
# Pub/Sub encrypts the topic with the key as its own service agent, so the
# agent needs the key before the topic can exist. The agent itself is created
# lazily; the service identity forces it so the grant has someone to land on.
resource "google_project_service_identity" "pubsub" {
  provider = google-beta
  project  = var.project_id
  service  = "pubsub.googleapis.com"
}

resource "google_kms_crypto_key_iam_member" "pubsub_agent" {
  crypto_key_id = google_kms_crypto_key.secrets.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@gcp-sa-pubsub.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.pubsub]
}

resource "google_pubsub_topic" "rotation" {
  project = var.project_id
  name    = "qip-secret-rotation-${var.environment}"
  labels  = var.labels

  # The message says a rotation is due and names the secret. It never carries
  # the value, so the topic's own encryption is defence in depth rather than
  # the thing keeping the credential safe.
  kms_key_name = google_kms_crypto_key.secrets.id

  depends_on = [google_kms_crypto_key_iam_member.pubsub_agent]
}

# Secret Manager publishes as its own service agent, so the grant is to that
# agent rather than to any workload identity.
resource "google_pubsub_topic_iam_member" "rotation_publisher" {
  project = var.project_id
  topic   = google_pubsub_topic.rotation.name
  role    = "roles/pubsub.publisher"
  member  = "serviceAccount:service-${var.project_number}@gcp-sa-secretmanager.iam.gserviceaccount.com"
}

resource "google_secret_manager_secret" "platform" {
  for_each = toset(var.secret_names)

  project   = var.project_id
  secret_id = "${each.value}-${var.environment}"

  replication {
    user_managed {
      replicas {
        location = var.region
        customer_managed_encryption {
          kms_key_name = google_kms_crypto_key.secrets.id
        }
      }
    }
  }

  # A secret with no rotation schedule is a secret nobody rotates — and a
  # schedule with nowhere to announce itself is one nobody acts on, which is
  # why Secret Manager requires the topic rather than accepting the timer
  # alone. Terraform never writes the new value; the notification is what tells
  # an operator that writing one is now due.
  rotation {
    next_rotation_time = "2026-12-01T00:00:00Z"
    rotation_period    = "7776000s"
  }

  topics {
    name = google_pubsub_topic.rotation.id
  }

  labels = var.labels

  # Terraform creates the secret. It never creates a version, because a
  # version has a value and a value in state is a leaked credential.
  lifecycle {
    ignore_changes = [labels]
  }
}

# The API reads the tokens that authenticate its callers.
resource "google_secret_manager_secret_iam_member" "api_tokens" {
  for_each = toset([
    for name in var.secret_names : name
    if startswith(name, "qip-token-")
  ])

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform[each.value].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload["api"].email}"
}

# The capital-envelope key, readable by every central-plane workload.
#
# The API signs envelopes with it and the two brains verify and sign against
# it; a cell verifies grants against the same key, and gets its own grant in
# `modules/edge-cell`. Until this existed only the cells could read it — so the
# process that *mints* the grants could not read the key it mints them under,
# and the pod would have stopped at `ContainerCreating` when the CSI driver
# failed to project a secret its identity was not allowed to fetch.
#
# One key rather than a signing key and a verification key: the envelope is
# authenticated with an HMAC, which is symmetric, so the two are the same
# bytes. Splitting the variable in two would produce a deployment where the
# centre signs under one value and the cells verify under another, and the
# failure — every grant rejected — reads as a mesh fault rather than a
# configuration one.
resource "google_secret_manager_secret_iam_member" "capital_envelope_key" {
  for_each = var.service_accounts

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform["qip-capital-envelope-key"].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload[each.key].email}"
}

# The venue credential is readable only where live trading is permitted at all.
#
# This is the infrastructure half of the live-trading control. The application
# refuses to send a live order below a live autonomy level; this makes the
# credential unreadable in an environment that could not use it anyway, so a
# misconfigured application in a paper environment still cannot authenticate to
# a venue.
resource "google_secret_manager_secret_iam_member" "venue_credential" {
  count = var.venue_credential_readable ? 1 : 0

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform["qip-venue-credential"].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.workload["fastbrain"].email}"
}

# The workload identity bindings are deliberately NOT here. The pool they
# name — `<project_id>.svc.id.goog` — exists only once a cluster with
# workload identity has been created, and the cluster consumes this module's
# node-encryption key, so a binding here would need the cluster to exist
# before the thing the cluster depends on. They live in the root, after
# `module.cluster`. The first real apply proved the cycle the hard way:
# "Identity Pool does not exist (…svc.id.goog)".

# The minimum each deployable needs beyond its secrets: write telemetry.
resource "google_project_iam_member" "telemetry" {
  for_each = var.service_accounts

  project = var.project_id
  role    = "roles/monitoring.metricWriter"
  member  = "serviceAccount:${google_service_account.workload[each.key].email}"
}

resource "google_project_iam_member" "logging" {
  for_each = var.service_accounts

  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = "serviceAccount:${google_service_account.workload[each.key].email}"
}
