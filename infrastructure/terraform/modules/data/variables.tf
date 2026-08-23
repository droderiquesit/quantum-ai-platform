variable "project_id" {
  type = string
}

variable "region" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "key_ring_id" {
  description = "The platform's existing KMS key ring. Data-at-rest keys are created in it."
  type        = string
}

variable "network_id" {
  description = "The VPC every managed instance is reachable from, and only from."
  type        = string
}

# --- What this build can actually reach -------------------------------------
#
# `qip_storage::provider::StorageTarget::is_implemented` returns true for
# exactly three targets: Memory, File and Engine. Every managed target below
# returns a `production_requirement` naming what it still needs, and the
# provider refuses to construct one rather than falling back to local files.
#
# So each of these is default-false, and that is not timidity. Provisioning a
# database no code can open buys a bill, an attack surface and a row in an
# architecture diagram that reads as a working capability. The flag is how a
# deployment says "the adapter now exists and I have wired it", and until
# someone can say that, the honest state of this infrastructure is absent.
#
# Turning one on without writing its adapter will produce a healthy, empty,
# billable instance. That is the failure these defaults exist to prevent.

variable "enable_bigquery" {
  description = <<-EOT
    Research warehouse for attribution, backtest results and cost history.
    Needs `StorageTarget::BigQuery` to gain an adapter first.
  EOT
  type        = bool
  default     = false
}

variable "enable_cloud_storage" {
  description = <<-EOT
    Object storage for event-log archives, model artifacts and reports. The
    closest to usable of the six: `qip_storage::blob` already models the port,
    and the evidence module already provisions its own bucket independently.
  EOT
  type        = bool
  default     = false
}

variable "enable_alloydb" {
  description = <<-EOT
    Transactional records with foreign keys: entities, orders, portfolios.
    Needs `StorageTarget::AlloyDb` to gain an adapter, and this build has no
    Postgres driver — the workspace permits exactly two third-party crates.
  EOT
  type        = bool
  default     = false
}

variable "enable_bigtable" {
  description = <<-EOT
    Tick and order-book history at high write throughput. Needs
    `StorageTarget::Bigtable` to gain an adapter.
  EOT
  type        = bool
  default     = false
}

variable "enable_memorystore" {
  description = <<-EOT
    Hot quotes, feature values and rate limits — values that can be recomputed
    if lost. Needs `StorageTarget::Memorystore` to gain an adapter.
  EOT
  type        = bool
  default     = false
}

variable "enable_spanner" {
  description = <<-EOT
    Only where a transaction must span regions; AlloyDB is cheaper everywhere
    else, which is why this is the last one to turn on rather than the first.
    Needs `StorageTarget::Spanner` to gain an adapter.
  EOT
  type        = bool
  default     = false
}

variable "deletion_protection" {
  description = <<-EOT
    Whether stateful resources refuse to be destroyed. Default true in every
    environment: a `terraform destroy` aimed at dev that reached prod is not a
    hypothetical, and the audit trail these hold is the evidence a regulator
    asks for.
  EOT
  type        = bool
  default     = true
}

variable "archive_retention_days" {
  description = "How long the event-log archive is retained and locked against deletion."
  type        = number
  default     = 2557 # seven years
}

variable "writer_service_accounts" {
  description = "Service account emails permitted to write. Least privilege, per workload."
  type        = list(string)
  default     = []
}

variable "reader_service_accounts" {
  description = "Service account emails permitted to read."
  type        = list(string)
  default     = []
}
