# The platform's managed data services.
#
# Section 7 of the charter refuses "one database for everything": each store
# below exists because a specific access pattern justifies it, and the
# rationale travels with the resource rather than living in a design document
# nobody opens. `qip_storage::provider::StorageTarget::rationale` carries the
# same sentences in code, so the two can be compared.
#
# # Everything here is off by default, and that is the honest state
#
# This build implements three storage targets — memory, local files, and the
# in-tree engine — and refuses the six managed ones by name, each with the
# credential or configuration it still needs. Provisioning a database that no
# adapter can open produces a healthy, empty, billable instance and a diagram
# that reads as a working capability. So each service is gated, default false,
# and the flag means "the adapter exists and I have wired it".
#
# # Encryption, and why the keys are separate
#
# Every store that supports a customer-managed key gets its own, rather than
# one key for all data. A key is the smallest revocable unit: one key for
# everything means revoking a compromised credential's access to tick history
# also stops the order book. Separate keys cost nothing and make the blast
# radius match the incident.

locals {
  prefix = "qip-${var.environment}"

  # Private services access is one connection per VPC, and two of the six
  # services below need it. Created once, when either is enabled, rather than
  # by whichever module happened to be written first.
  needs_private_services = var.enable_alloydb || var.enable_memorystore
}

# --- Keys -------------------------------------------------------------------

resource "google_kms_crypto_key" "warehouse" {
  count = var.enable_bigquery ? 1 : 0

  name     = "${local.prefix}-warehouse"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Ninety days. Research history is read for years, so a shorter period
  # multiplies key versions without reducing exposure — the old versions stay
  # live to decrypt old data either way.
  rotation_period = "7776000s"

  lifecycle {
    prevent_destroy = true
  }
}

resource "google_kms_crypto_key" "archive" {
  count = var.enable_cloud_storage ? 1 : 0

  name            = "${local.prefix}-archive"
  key_ring        = var.key_ring_id
  purpose         = "ENCRYPT_DECRYPT"
  rotation_period = "7776000s"

  lifecycle {
    prevent_destroy = true
  }
}

resource "google_kms_crypto_key" "records" {
  count = var.enable_alloydb ? 1 : 0

  name     = "${local.prefix}-records"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Thirty days. This holds orders and positions — the data whose exposure is
  # a reportable incident — so the key turns over faster than the rest.
  rotation_period = "2592000s"

  lifecycle {
    prevent_destroy = true
  }
}

# --- The warehouse: research history ----------------------------------------

resource "google_bigquery_dataset" "research" {
  count = var.enable_bigquery ? 1 : 0

  project    = var.project_id
  dataset_id = replace("${local.prefix}_research", "-", "_")
  location   = var.region
  labels     = var.labels

  description = join(" ", [
    "Columnar scans over research history: attribution, backtest results,",
    "cost-model corrections. Not for transactional writes — a warehouse that",
    "became the order book would be a warehouse nobody could query."
  ])

  # No default expiry. A backtest result whose evidence expired is a promotion
  # decision nobody can re-examine, and the lifecycle ledger keeps references
  # to these rows for the life of the strategy.
  default_table_expiration_ms = null

  default_encryption_configuration {
    kms_key_name = google_kms_crypto_key.warehouse[0].id
  }

  # Terraform owns the schema. A hand-made table in this dataset is a table
  # nobody can reproduce in another environment.
  delete_contents_on_destroy = false
}

# --- Object storage: archives and artifacts ---------------------------------

resource "google_storage_bucket" "archive" {
  count = var.enable_cloud_storage ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-event-archive"
  location = var.region
  labels   = var.labels

  # Uniform access, always. Per-object ACLs are how a bucket ends up with one
  # world-readable object nobody remembers granting.
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  versioning {
    enabled = true
  }

  encryption {
    default_kms_key_name = google_kms_crypto_key.archive[0].id
  }

  # The event log is hash-chained and its value is that it is complete. A
  # retention policy that permitted deletion would let the one record an
  # investigation needs be the one that aged out.
  retention_policy {
    retention_period = var.archive_retention_days * 24 * 60 * 60
    is_locked        = false
  }

  lifecycle_rule {
    condition {
      age = 90
    }
    action {
      type          = "SetStorageClass"
      storage_class = "NEARLINE"
    }
  }

  lifecycle_rule {
    condition {
      age = 365
    }
    action {
      type          = "SetStorageClass"
      storage_class = "COLDLINE"
    }
  }
}

resource "google_storage_bucket" "artifacts" {
  count = var.enable_cloud_storage ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-model-artifacts"
  location = var.region
  labels   = var.labels

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  # Versioned, because a model card references an artifact by name and a
  # replaced artifact under the same name makes every card that points at it
  # a record of something that no longer exists.
  versioning {
    enabled = true
  }

  encryption {
    default_kms_key_name = google_kms_crypto_key.archive[0].id
  }
}

# --- Private services access ------------------------------------------------
#
# AlloyDB and Memorystore are reachable only over VPC peering, never over a
# public address. This is the peering, allocated once for the VPC.

resource "google_compute_global_address" "private_services" {
  count = local.needs_private_services ? 1 : 0

  project       = var.project_id
  name          = "${local.prefix}-private-services"
  purpose       = "VPC_PEERING"
  address_type  = "INTERNAL"
  prefix_length = 16
  network       = var.network_id
}

resource "google_service_networking_connection" "private_services" {
  count = local.needs_private_services ? 1 : 0

  network                 = var.network_id
  service                 = "servicenetworking.googleapis.com"
  reserved_peering_ranges = [google_compute_global_address.private_services[0].name]
}

# --- Transactional records --------------------------------------------------

resource "google_alloydb_cluster" "records" {
  count = var.enable_alloydb ? 1 : 0

  project    = var.project_id
  cluster_id = "${local.prefix}-records"
  location   = var.region
  labels     = var.labels

  network_config {
    network = var.network_id
  }

  encryption_config {
    kms_key_name = google_kms_crypto_key.records[0].id
  }

  # Continuous backup with point-in-time recovery. The window is long enough
  # to cover a bad deployment discovered the following morning, because the
  # failures that need a restore are rarely noticed within the hour.
  continuous_backup_config {
    enabled              = true
    recovery_window_days = 14

    encryption_config {
      kms_key_name = google_kms_crypto_key.records[0].id
    }
  }

  deletion_policy = var.deletion_protection ? "DEFAULT" : "FORCE"

  depends_on = [google_service_networking_connection.private_services]
}

resource "google_alloydb_instance" "primary" {
  count = var.enable_alloydb ? 1 : 0

  cluster       = google_alloydb_cluster.records[0].name
  instance_id   = "${local.prefix}-records-primary"
  instance_type = "PRIMARY"
  labels        = var.labels

  machine_config {
    cpu_count = 4
  }

  # No public IP, ever. The VPC and its egress policy are the access control
  # for every managed store here.
  network_config {
    enable_public_ip = false
  }
}

# --- Tick and order-book history --------------------------------------------

resource "google_bigtable_instance" "timeseries" {
  count = var.enable_bigtable ? 1 : 0

  project       = var.project_id
  name          = "${local.prefix}-timeseries"
  labels        = var.labels
  force_destroy = !var.deletion_protection

  cluster {
    cluster_id   = "${local.prefix}-timeseries-primary"
    zone         = "${var.region}-a"
    storage_type = "SSD"

    # Autoscaling rather than a fixed node count: tick volume on a volatile
    # day is a multiple of a quiet one, and a fixed cluster is either wasted
    # on the quiet days or short on the day it matters.
    autoscaling_config {
      min_nodes      = 1
      max_nodes      = 10
      cpu_target     = 60
      storage_target = 8192
    }
  }
}

# --- Hot cache --------------------------------------------------------------

resource "google_redis_instance" "cache" {
  count = var.enable_memorystore ? 1 : 0

  project        = var.project_id
  name           = "${local.prefix}-cache"
  region         = var.region
  labels         = var.labels
  tier           = "STANDARD_HA"
  memory_size_gb = 4

  authorized_network      = var.network_id
  connect_mode            = "PRIVATE_SERVICE_ACCESS"
  transit_encryption_mode = "SERVER_AUTHENTICATION"
  auth_enabled            = true

  # Nothing here may be the only copy of anything. The rationale in
  # `StorageTarget::Memorystore` is "values that can be recomputed if lost",
  # and this policy is what keeps that true: no persistence, so a cache that
  # survived a restart could never be mistaken for a source of record.
  persistence_config {
    persistence_mode = "DISABLED"
  }

  depends_on = [google_service_networking_connection.private_services]
}

# --- Cross-region transactions ----------------------------------------------

resource "google_spanner_instance" "global" {
  count = var.enable_spanner ? 1 : 0

  project      = var.project_id
  name         = "${local.prefix}-global"
  config       = "regional-${var.region}"
  display_name = "QIP ${var.environment} global"
  labels       = var.labels

  # Processing units rather than nodes: 1000 units is one node, and starting
  # below a node is the difference between an affordable mistake and an
  # expensive one.
  processing_units = 100

  force_destroy = !var.deletion_protection
}

resource "google_spanner_database" "positions" {
  count = var.enable_spanner ? 1 : 0

  project  = var.project_id
  instance = google_spanner_instance.global[0].name
  name     = "positions"

  encryption_config {
    kms_key_name = google_kms_crypto_key.records[0].id
  }

  deletion_protection = var.deletion_protection
}

# --- Access -----------------------------------------------------------------
#
# Per workload, never per project. A component that can read the warehouse
# should not thereby be able to write the archive.

resource "google_bigquery_dataset_iam_member" "writers" {
  for_each = var.enable_bigquery ? toset(var.writer_service_accounts) : toset([])

  project    = var.project_id
  dataset_id = google_bigquery_dataset.research[0].dataset_id
  role       = "roles/bigquery.dataEditor"
  member     = "serviceAccount:${each.value}"
}

resource "google_bigquery_dataset_iam_member" "readers" {
  for_each = var.enable_bigquery ? toset(var.reader_service_accounts) : toset([])

  project    = var.project_id
  dataset_id = google_bigquery_dataset.research[0].dataset_id
  role       = "roles/bigquery.dataViewer"
  member     = "serviceAccount:${each.value}"
}

# Object *creator*, not object admin: a workload that archives an event log
# must be able to add records and must not be able to delete them. That is the
# whole value of an append-only audit trail, and an IAM role is where it is
# either kept or quietly given away.
resource "google_storage_bucket_iam_member" "archive_writers" {
  for_each = var.enable_cloud_storage ? toset(var.writer_service_accounts) : toset([])

  bucket = google_storage_bucket.archive[0].name
  role   = "roles/storage.objectCreator"
  member = "serviceAccount:${each.value}"
}

resource "google_storage_bucket_iam_member" "archive_readers" {
  for_each = var.enable_cloud_storage ? toset(var.reader_service_accounts) : toset([])

  bucket = google_storage_bucket.archive[0].name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${each.value}"
}
