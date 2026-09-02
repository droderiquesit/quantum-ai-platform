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
# Secret Manager's own agent — third of the lazily created service agents,
# found the same way as the first two: a real apply naming an account that
# "does not exist". It needs to exist for the publisher grant below, and it
# needs the secrets key because every secret here is customer-managed
# encrypted, which Secret Manager performs as this agent.
resource "google_project_service_identity" "secretmanager" {
  provider = google-beta
  project  = var.project_id
  service  = "secretmanager.googleapis.com"
}

resource "google_kms_crypto_key_iam_member" "secretmanager_agent" {
  crypto_key_id = google_kms_crypto_key.secrets.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@gcp-sa-secretmanager.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.secretmanager]
}

resource "google_pubsub_topic_iam_member" "rotation_publisher" {
  project = var.project_id
  topic   = google_pubsub_topic.rotation.name
  role    = "roles/pubsub.publisher"
  member  = "serviceAccount:service-${var.project_number}@gcp-sa-secretmanager.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.secretmanager]
}

resource "google_secret_manager_secret" "platform" {
  for_each = toset(var.secret_names)

  project   = var.project_id
  secret_id = "${each.value}-${var.environment}"

  # The CMEK below is performed by Secret Manager's agent, so the secret
  # cannot exist before the agent holds the key.
  depends_on = [google_kms_crypto_key_iam_member.secretmanager_agent]

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

# The venue credential is readable only where live trading is permitted at all.
#
# This is the infrastructure half of the live-trading control. The application
# refuses to send a live order below a live autonomy level; this makes the
# credential unreadable in an environment that could not use it anyway, so a
# misconfigured application in a paper environment still cannot authenticate to
# a venue.
#
# The reader is the fast brain's Cloud Run identity, passed in by the root
# from the catalogue rather than created here: every workload's account is
# created by `modules/cloudrun` beside the workload, and this module holds
# only the one grant whose condition is the environment's ceiling. `count`
# on the root's predicate is the shape three acceptance tests evaluate rung
# by rung; keep it.
resource "google_secret_manager_secret_iam_member" "venue_credential" {
  count = var.venue_credential_readable ? 1 : 0

  project   = var.project_id
  secret_id = google_secret_manager_secret.platform["qip-venue-credential"].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${var.venue_credential_reader}"
}

# Every other grant a workload needs — its mounted secrets, telemetry,
# logging — is made by `modules/cloudrun` on the identity it creates, in the
# file where the mount is declared. There is no per-deployable account here
# any more: the GKE runtime's `workload` accounts and their workload-identity
# bindings left with it (ADR 0024), and an account created here for a
# workload created elsewhere would be an identity with nothing attached.

# --- The console's identity (ADR 0018) ---------------------------------------

# The account the portal runs as.
#
# Its own account, and the reason is what it replaces: the portal was deployed
# under `<project-number>-compute@developer.gserviceaccount.com`, the project's
# default compute identity. That account is shared by anything in the project
# that does not name one, it accumulates grants nobody attributes to a
# particular workload, and a grant given to it for the console is a grant given
# to everything else that defaults to it. Naming the identity is what makes the
# next line a statement about the console rather than about the project.
resource "google_service_account" "console" {
  count        = var.console_enabled ? 1 : 0
  project      = var.project_id
  account_id   = "qip-${var.environment}-console"
  display_name = "The portal, reading the platform as viewer"
  description  = "Runs the Cloud Run portal. Reads qip-token-viewer only; see ADR 0018."
}

# The console reads the platform as `viewer`, and holds no other platform
# credential.
#
# Viewer is the whole entitlement and it is worth naming what it excludes:
# `POST /api/v1/cycle` is `analyst`, and both directions of
# `/api/v1/kill-switch` are `operator`. The console renders what the platform
# decided; it does not run a cycle and it cannot halt one. A console compromise
# is therefore a disclosure of what this deployment already shows a signed-in
# operator, not a control of it.
resource "google_secret_manager_secret_iam_member" "console_viewer_token" {
  count     = var.console_enabled ? 1 : 0
  project   = var.project_id
  secret_id = google_secret_manager_secret.platform["qip-token-viewer"].secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.console[0].email}"
}

# The console writes the agreements a user accepted onto their account
# (ADR 0019), and reads them back at sign-in.
#
# A custom role, because every predefined role that can do this can do far
# more. `roles/identitytoolkit.admin` and `roles/firebaseauth.admin` both carry
# `firebaseauth.users.delete`, `firebaseauth.configs.getSecret` — the project's
# own signing configuration — and, in the first case,
# `identitytoolkit.tenants.setIamPolicy`, which is the permission to decide who
# else administers identity. The console reads an account and updates an
# account. Two permissions is what that is, and widening a grant to make an
# error go away is exactly the move the infrastructure rules refuse.
resource "google_project_iam_custom_role" "console_profile_claims" {
  count       = var.console_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "qip_${var.environment}_console_profile_claims"
  title       = "Console profile claims"
  description = "Read and update Identity Platform custom claims. Cannot delete an account, read the project's identity configuration, or change who administers it."

  permissions = [
    # Read the profile at sign-in.
    "firebaseauth.users.get",
    # Write it at sign-up. An account created without one is refused a
    # session, so this is not optional and its absence is not silent.
    "firebaseauth.users.update",
  ]
}

resource "google_project_iam_member" "console_profile_claims" {
  count   = var.console_enabled ? 1 : 0
  project = var.project_id
  role    = google_project_iam_custom_role.console_profile_claims[0].id
  member  = "serviceAccount:${google_service_account.console[0].email}"
}
