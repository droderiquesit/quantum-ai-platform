# Data-write audit logs on the services that change what runs (R12 SEC-9135,
# R12 CICD-9081; ported from recover/l016-cicd-037, its CICD-037).
#
# Admin-activity logs — a deploy, a cluster update, an instance created or
# deleted — are on by Google's default and cannot be turned off, so they are
# not configured here. The port deliberately drops the `ADMIN_WRITE` entries
# the recovered branch declared: that is not a log type Google accepts
# (`ADMIN_READ`, `DATA_READ` and `DATA_WRITE` are), and the apply would have
# failed on it. What is not on by default is DATA_WRITE: the writes to a
# resource's contents that sit below admin activity. With it, a change an
# agent makes through Cloud Run, the GitOps control plane's Kubernetes API or
# a Compute Engine machine leaves a record of who made it, which is what
# makes it attributable and revertible rather than merely present.
#
# Secret Manager and KMS have their own, wider audit config in
# modules/secrets (DATA_READ as well), and a service may hold only one
# `google_project_iam_audit_config`, so neither is named here.
#
# Unconditional: the infrastructure account holds
# `resourcemanager.projectIamAdmin`, which carries the setIamPolicy an audit
# config is written with, so no grant is widened for this.
resource "google_project_iam_audit_config" "runtime_changes" {
  for_each = toset([
    "run.googleapis.com",
    "container.googleapis.com",
    "compute.googleapis.com",
  ])

  project = var.project_id
  service = each.key

  audit_log_config {
    log_type = "DATA_WRITE"
  }
}

# --- Cloud Asset Inventory: every change to the project's shape, as a feed ------
#
# Audit logs say who called what. They do not say what the project now looks
# like: a firewall rule edited by hand, an IAM binding added in the console,
# a bucket's retention lock removed — each is one line in a log that nothing
# reads back as a before-and-after. Cloud Asset Inventory records the
# resource and its IAM policy as they are, and a feed publishes every change
# to them the moment it happens (R12 SEC-9135). Without the feed the API's
# 35-day history is only something an operator queries after the fact; with
# it, a change nobody planned is a message an operator can be paged on.
#
# Two feeds, because a feed carries one content type: the resource bodies
# and the IAM policies, both for every asset type the project holds. They
# publish to one topic, `asset_changes`. That topic is the second Pub/Sub
# topic in the platform and, like the first (`modules/secrets`' rotation
# notice), it is not a contradiction of ADR 0011: it carries no platform
# data and no state between cells and the centre, only a notice from a
# Google control plane to an operator, which is the one use `blueprint_arch`'s
# allow-list admits. Nothing in the tree subscribes; the subscription is an
# operator's or a SIEM's, out of band, exactly as the rotation topic's is.
#
# The topic is encrypted with its own customer-managed key, as every store
# here is (SEC-046). The key is not `prevent_destroy`: a notice already
# delivered is in whatever consumed it, and a key that outlives its topic
# protects nothing. The rotation key's agent grant lives in `modules/secrets`;
# this one needs the same grant on its own key, to Pub/Sub's service agent,
# before the topic can be created with it.
resource "google_kms_crypto_key" "asset_feed" {
  name            = "qip-${var.environment}-asset-feed"
  key_ring        = module.secrets.key_ring_id
  rotation_period = "7776000s"
  labels          = local.labels

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = var.kms_protection_level
  }
}

# Pub/Sub encrypts the topic as its own service agent, which `modules/secrets`
# forces into existence (`google_project_service_identity.pubsub`) — this
# root reads the module, so the agent exists before this grant lands on it.
resource "google_kms_crypto_key_iam_member" "asset_feed_pubsub_agent" {
  crypto_key_id = google_kms_crypto_key.asset_feed.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${local.project_number}@gcp-sa-pubsub.iam.gserviceaccount.com"

  depends_on = [module.secrets]
}

resource "google_pubsub_topic" "asset_changes" {
  project = var.project_id
  name    = "qip-${var.environment}-asset-changes"
  labels  = local.labels

  kms_key_name = google_kms_crypto_key.asset_feed.id

  depends_on = [google_kms_crypto_key_iam_member.asset_feed_pubsub_agent]
}

# Cloud Asset Inventory publishes as its own service agent, and the agent is
# created lazily: the first real apply in this repository lost that race twice
# with other services' agents (modules/secrets), so the identity is forced
# before anything is granted to it.
resource "google_project_service_identity" "cloudasset" {
  provider = google-beta
  project  = var.project_id
  service  = "cloudasset.googleapis.com"

  depends_on = [module.services]
}

resource "google_pubsub_topic_iam_member" "asset_changes_publisher" {
  project = var.project_id
  topic   = google_pubsub_topic.asset_changes.name
  role    = "roles/pubsub.publisher"
  member  = "serviceAccount:service-${local.project_number}@gcp-sa-cloudasset.iam.gserviceaccount.com"

  depends_on = [google_project_service_identity.cloudasset]
}

# Every asset type, both content types. `asset_types` is left unset on
# purpose: a list would admit exactly the resources somebody thought of, and
# the change that matters is the one nobody did.
resource "google_cloud_asset_project_feed" "changes" {
  for_each = {
    resources    = "RESOURCE"
    iam_policies = "IAM_POLICY"
  }

  project         = var.project_id
  billing_project = var.project_id
  feed_id         = "qip-${var.environment}-${replace(each.key, "_", "-")}"
  content_type    = each.value

  feed_output_config {
    pubsub_destination {
      topic = google_pubsub_topic.asset_changes.id
    }
  }

  depends_on = [
    google_pubsub_topic_iam_member.asset_changes_publisher,
    module.services,
  ]
}
