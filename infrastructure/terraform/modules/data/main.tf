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
  # Both record stores encrypt under this key: AlloyDB's operational records
  # and Spanner's `positions` database. Gated on AlloyDB alone, enabling
  # Spanner by itself planned a database whose key reference indexed an empty
  # tuple, so the ledger could not be planned without a second, unrelated
  # store switched on beside it.
  count = var.enable_alloydb || var.enable_spanner ? 1 : 0

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

  # No default expiry. Research aggregates are source-of-truth (not derived
  # copies), so they are deliberately kept indefinitely. A backtest result
  # whose evidence expired is a promotion decision nobody can re-examine, and
  # the lifecycle ledger keeps references to these rows for the life of the
  # strategy (FINOPS-013, FINOPS-014).
  default_table_expiration_ms = null

  default_encryption_configuration {
    kms_key_name = google_kms_crypto_key.warehouse[0].id
  }

  # Terraform owns the schema. A hand-made table in this dataset is a table
  # nobody can reproduce in another environment.
  delete_contents_on_destroy = false

  # The key above is used by BigQuery's own encryption account, not by the
  # caller, so the dataset cannot be created until that account holds it.
  # Without the edge Terraform is free to create both in parallel, and the
  # dataset is refused for a missing key permission the same apply was about
  # to grant.
  depends_on = [google_kms_crypto_key_iam_member.warehouse_bigquery_agent]
}

# BigQuery encrypts a CMEK dataset as its per-project encryption service
# account (`bq-<number>@bigquery-encryption.iam.gserviceaccount.com`), the
# same arrangement as Cloud Storage's service agent in modules/evidence.
# Until 2026-10-24 this module set `default_encryption_configuration` on the
# research dataset and granted the key to nobody, so the first apply with
# `enable_bigquery = true` (dev) had no way to create the dataset.
data "google_bigquery_default_service_account" "bigquery" {
  count = var.enable_bigquery ? 1 : 0

  project = var.project_id
}

resource "google_kms_crypto_key_iam_member" "warehouse_bigquery_agent" {
  count = var.enable_bigquery ? 1 : 0

  crypto_key_id = google_kms_crypto_key.warehouse[0].id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:${data.google_bigquery_default_service_account.bigquery[0].email}"

  lifecycle {
    # The grant goes to BigQuery's encryption account and to no other. The
    # neighbouring modules grant keys to the Cloud Storage agent with a
    # data source of the same shape, and a copy of that block here would plan
    # cleanly, apply cleanly, and leave the dataset as unable to use its key
    # as it was with no grant at all.
    precondition {
      condition     = endswith(data.google_bigquery_default_service_account.bigquery[0].email, "@bigquery-encryption.iam.gserviceaccount.com")
      error_message = "the warehouse key would be granted to ${data.google_bigquery_default_service_account.bigquery[0].email}, which is not BigQuery's encryption service account; the research dataset encrypts as that account and would still be refused its key."
    }
  }
}

# --- Object storage: archives and artifacts ---------------------------------

resource "google_storage_bucket" "archive" {
  count = var.enable_cloud_storage ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-event-archive"
  location = var.bucket_location
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
  # investigation needs be the one that aged out. The policy is locked so that
  # no cost-response automation can shorten the retention window and delete
  # records (FINOPS-013, FINOPS-014, FINOPS-019).
  retention_policy {
    retention_period = var.archive_retention_days * 24 * 60 * 60
    is_locked        = true
  }

  # Market data: Tick/Quote/Book updates (Transient class in event log).
  # Raw ticks are retained for 30 days in hot storage, then archived to cold storage.
  # This retention period supports replay and training on recent market regimes
  # while expiring raw captures to avoid unbounded growth (TICK-037).
  lifecycle_rule {
    condition {
      age                   = 30
      matches_prefix        = ["lake/class=market/"]
      matches_storage_class = ["STANDARD"]
    }
    action {
      type          = "SetStorageClass"
      storage_class = "COLDLINE"
    }
  }

  # Market data older than 90 days in COLDLINE is deleted (retention policy
  # for transient market history per TICK-037; raw bytes not kept beyond this).
  lifecycle_rule {
    condition {
      age            = 90
      matches_prefix = ["lake/class=market/"]
    }
    action {
      type = "Delete"
    }
  }

  # Internal financial history: orders, fills, verdicts, decisions.
  # Classed as Irreplaceable (Permanent retention, never evicted).
  # Must be retained indefinitely for audit, compliance and replay.
  # Transition to COLDLINE for cost optimization but never delete (TICK-037).
  lifecycle_rule {
    condition {
      age                   = 180
      matches_prefix        = ["lake/class=internal/"]
      matches_storage_class = ["STANDARD"]
    }
    action {
      type          = "SetStorageClass"
      storage_class = "COLDLINE"
    }
  }

  # Metadata: retain indefinitely (segment records, manifests, ledgers).
  # Not prefixed by class=, so stored at lake root or in other paths.
  lifecycle_rule {
    condition {
      age                   = 365
      matches_storage_class = ["STANDARD"]
      matches_prefix        = ["lake/"]
    }
    action {
      type          = "SetStorageClass"
      storage_class = "NEARLINE"
    }
  }

  lifecycle_rule {
    condition {
      age                   = 730
      matches_storage_class = ["NEARLINE"]
      matches_prefix        = ["lake/"]
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
  location = var.bucket_location
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

# The regional segment store: sealed tick and internal replay segments and
# knowledge packs, in the region fabricd runs in, before they are compacted
# into the dual-region archive above. Regional because the writer is regional
# and a segment upload that crosses a continent on the produce path is latency
# the journal pays for. Internal and derived content only — never an external
# corpus — which is why it is not called a landing bucket: that word is one
# of the tokens `qip-acceptance` refuses in a store's name.
#
# It is not the record. fabricd's own journal is, and the archive is the copy
# kept, so this bucket expires what it holds: a regional copy kept forever is
# the uncontrolled duplication the retention rules forbid. Object lifecycle
# cannot move an object to another bucket, so nothing here copies a segment
# into the archive yet; until a transfer does, pointing fabricd's archive at
# this bucket would delete the archive at thirty days. The seven-day
# retention policy is unlocked on purpose; it stops an operator deleting a
# segment compaction has not yet read, and nothing more.
resource "google_storage_bucket" "segments" {
  count = var.enable_cloud_storage ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-segments-${var.region}"
  location = var.region
  labels   = var.labels

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  encryption {
    default_kms_key_name = google_kms_crypto_key.archive[0].id
  }

  retention_policy {
    retention_period = 7 * 24 * 60 * 60
    is_locked        = false
  }

  lifecycle_rule {
    condition {
      age = 30
    }
    action {
      type = "Delete"
    }
  }
}

# Simulation outputs and reproducible campaign datasets, one bucket with a
# retention class per prefix rather than one rule for all three. A campaign's
# dataset is what makes its result reproducible, so it is kept; a run's raw
# output can be regenerated from the dataset and the manifest, so it expires;
# the manifest is small and names exactly what ran, so it is kept. One
# lifecycle for the bucket would either delete the datasets or keep every
# output forever.
resource "google_storage_bucket" "simulation" {
  count = var.enable_cloud_storage ? 1 : 0

  project  = var.project_id
  name     = "${local.prefix}-simulation"
  location = var.bucket_location
  labels   = var.labels

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  encryption {
    default_kms_key_name = google_kms_crypto_key.archive[0].id
  }

  # Thirty days, unlocked: long enough that a result under review cannot be
  # deleted from under the reviewer.
  retention_policy {
    retention_period = 30 * 24 * 60 * 60
    is_locked        = false
  }

  # Run outputs: regenerable from campaign + manifest, so bounded.
  lifecycle_rule {
    condition {
      age            = 90
      matches_prefix = ["outputs/"]
    }
    action {
      type = "Delete"
    }
  }

  # Campaign datasets: kept, colder with age, never deleted.
  lifecycle_rule {
    condition {
      age                   = 90
      matches_prefix        = ["campaigns/"]
      matches_storage_class = ["STANDARD"]
    }
    action {
      type          = "SetStorageClass"
      storage_class = "COLDLINE"
    }
  }

  # Manifests: kept, colder with age, never deleted.
  lifecycle_rule {
    condition {
      age                   = 365
      matches_prefix        = ["manifests/"]
      matches_storage_class = ["STANDARD"]
    }
    action {
      type          = "SetStorageClass"
      storage_class = "NEARLINE"
    }
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

  # A second cluster in another zone of the same region, so a zonal outage
  # leaves the history readable rather than the replay stopped. Replication
  # between the two is eventually consistent; the app profiles below decide
  # which workload may see that and which may not.
  cluster {
    cluster_id   = "${local.prefix}-timeseries-secondary"
    zone         = "${var.region}-b"
    storage_type = "SSD"

    autoscaling_config {
      min_nodes      = 1
      max_nodes      = 10
      cpu_target     = 60
      storage_target = 8192
    }
  }
}

# One app profile per workload, chosen by the consistency that workload needs
# rather than one default profile for all of them. The default profile routes
# with multi-cluster routing to whichever cluster is nearest, so a writer and
# a replay on the same profile can each land on a different cluster and the
# replay reads history the writer has already acknowledged as missing: a
# backtest that silently loses its most recent ticks.
locals {
  bigtable_app_profiles = {
    # The fabric sink writes, and a replay that must see every acknowledged
    # write reads, through the primary only: single-cluster routing is the
    # read-your-writes guarantee replication does not give.
    ingest = {
      description = "Fabric sink writes and read-your-writes replay; pinned to the primary cluster."
      multi       = false
      cluster     = "${local.prefix}-timeseries-primary"
      priority    = "PRIORITY_HIGH"
    }
    # Research and dashboards tolerate replication lag and must not stop on a
    # zonal outage: multi-cluster routing fails over to the surviving cluster.
    serve = {
      description = "Lag-tolerant reads that must survive a zonal outage; multi-cluster routing."
      multi       = true
      cluster     = null
      priority    = "PRIORITY_MEDIUM"
    }
    # Bulk scans for training sets are pinned to the secondary at low
    # priority, so a large scan never competes with the sink for the
    # primary's CPU on the day volume peaks.
    batch = {
      description = "Bulk training-set scans; pinned to the secondary at low priority."
      multi       = false
      cluster     = "${local.prefix}-timeseries-secondary"
      priority    = "PRIORITY_LOW"
    }
  }
}

resource "google_bigtable_app_profile" "timeseries" {
  for_each = var.enable_bigtable ? local.bigtable_app_profiles : {}

  project        = var.project_id
  instance       = google_bigtable_instance.timeseries[0].name
  app_profile_id = each.key
  description    = each.value.description

  multi_cluster_routing_use_any = each.value.multi ? true : null

  dynamic "single_cluster_routing" {
    for_each = each.value.multi ? [] : [each.value.cluster]
    content {
      cluster_id                 = single_cluster_routing.value
      allow_transactional_writes = false
    }
  }

  standard_isolation {
    priority = each.value.priority
  }

  # A profile is deleted only with the instance; ignore_warnings lets a
  # profile be replaced while the instance still serves traffic.
  ignore_warnings = true
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

  authorized_network = var.network_id
  connect_mode       = "PRIVATE_SERVICE_ACCESS"
  # See `memorystore_transit_encryption`: true gives the safer instance that
  # the in-tree plaintext RESP client cannot reach without a proxy; false gives
  # one it can reach, with cache traffic in clear text inside the VPC.
  transit_encryption_mode = var.memorystore_transit_encryption ? "SERVER_AUTHENTICATION" : "DISABLED"
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
  config       = "nam-eur-asea"
  display_name = "QIP ${var.environment} global"
  labels       = var.labels

  # Enterprise Plus is the edition whose multi-region configurations carry
  # the external-consistency guarantee the ledger of record is placed on
  # Spanner for (ADR 0113). Left unset, the provider plans the project's
  # default edition, and a ledger on Standard would read as the same
  # resource in every review while holding a weaker consistency contract;
  # `qip-acceptance`'s infrastructure suite refuses the attribute's absence.
  edition = "ENTERPRISE_PLUS"

  # Managed autoscaling with a floor, rather than a fixed size. A fixed 100
  # processing units was affordable and would also have been the size the
  # ledger had on the close of a volatile day, when reconciliation reads every
  # book at once and a commit that waits on CPU is a balance the desk cannot
  # see. The floor is `spanner_min_processing_units`, which refuses anything
  # below one node; the ceiling bounds the bill.
  autoscaling_config {
    autoscaling_limits {
      min_processing_units = var.spanner_min_processing_units
      max_processing_units = var.spanner_max_processing_units
    }
    autoscaling_targets {
      high_priority_cpu_utilization_percent = 65
      storage_utilization_percent           = 90
    }
  }

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
