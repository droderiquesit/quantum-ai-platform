# The evidence store.
#
# `qip_mesh::ports::EvidenceStore` is write-once by construction: it has no
# `delete`, no overwrite, and refuses a second write of different bytes to an
# existing key. That guarantee is worth exactly as much as the bucket underneath
# it, so this module's job is to make the bucket agree.
#
# The property to preserve is narrower than "secure". It is that **the people
# who run the platform cannot revise its evidence**. An append-only store whose
# writer holds a delete permission is not append-only; it is a store nobody has
# deleted from yet.
#
# Four things enforce it:
#
#   * A retention policy, locked, so a deletion is refused by the storage
#     service rather than by an IAM binding somebody can change.
#   * Object versioning, so an overwrite that somehow happens leaves the
#     original readable rather than replacing it.
#   * Uniform bucket-level access, so a per-object ACL cannot quietly grant
#     what the bucket policy refuses.
#   * `roles/storage.objectCreator` and nothing wider. Not `objectUser`, not
#     `objectAdmin`, not `storage.admin` — each of those carries
#     `storage.objects.delete`.

resource "google_kms_crypto_key" "evidence" {
  name     = "qip-${var.environment}-evidence"
  key_ring = var.key_ring_id

  # Ninety days, matching the platform's other keys.
  rotation_period = "7776000s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  # Destroying this key makes every object in the bucket unreadable, which is
  # a way of deleting evidence that leaves the objects in place.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# Cloud Storage encrypts with the key as its own service agent, not as the
# caller, so the agent needs the key rather than the workload.
data "google_storage_project_service_account" "storage" {
  project = var.project_id
}

resource "google_kms_crypto_key_iam_member" "storage_agent" {
  crypto_key_id = google_kms_crypto_key.evidence.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:${data.google_storage_project_service_account.storage.email_address}"
}

resource "google_storage_bucket" "evidence" {
  project = var.project_id

  # Project ids are globally unique, so this name is too, without a random
  # suffix that would make the bucket unnameable from a runbook.
  name     = "qip-evidence-${var.environment}-${var.project_id}"
  location = var.region

  # Per-object ACLs cannot override the bucket policy.
  uniform_bucket_level_access = true

  # Not even by accident, and not even briefly.
  public_access_prevention = "enforced"

  # `terraform destroy` will refuse rather than empty the bucket first.
  force_destroy = false

  versioning {
    enabled = true
  }

  # The storage service refuses a delete inside the retention window. This is
  # the control that does not depend on an IAM binding staying correct.
  #
  # Locking is irreversible: a locked policy can be lengthened and never
  # shortened or removed, for the life of the bucket. That is the point, and it
  # is also the reason this is the one setting worth reading twice before the
  # first apply.
  retention_policy {
    retention_period = var.retention_days * 24 * 60 * 60
    is_locked        = var.retention_locked
  }

  encryption {
    default_kms_key_name = google_kms_crypto_key.evidence.id
  }

  labels = var.labels

  lifecycle {
    prevent_destroy = true
  }

  depends_on = [google_kms_crypto_key_iam_member.storage_agent]
}

# Writers create objects and can do nothing else to them.
#
# `roles/storage.objectCreator` is `storage.objects.create` alone. It carries
# no read, no list, no overwrite and no delete. A workload that writes evidence
# and cannot read it back is not an inconvenience — it is the shape of the
# guarantee.
resource "google_storage_bucket_iam_member" "writers" {
  for_each = toset(var.writer_service_accounts)

  bucket = google_storage_bucket.evidence.name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${each.value}"
}

# Readers read. Separate accounts from the writers on purpose: the component
# that produces evidence and the component that serves it to an auditor should
# not be the same identity.
resource "google_storage_bucket_iam_member" "readers" {
  for_each = toset(var.reader_service_accounts)

  bucket = google_storage_bucket.evidence.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${each.value}"
}
