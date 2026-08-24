# What the platform is actually configured to reach.
#
# Each output is null when its service is disabled, so a caller that renders
# these gets an honest picture rather than a plausible-looking address for
# something that was never created.

output "bigquery_dataset_id" {
  description = "Research warehouse dataset, or null when BigQuery is not enabled."
  value       = var.enable_bigquery ? google_bigquery_dataset.research[0].dataset_id : null
}

output "archive_bucket" {
  description = "Event-log archive bucket, or null when Cloud Storage is not enabled."
  value       = var.enable_cloud_storage ? google_storage_bucket.archive[0].name : null
}

output "artifact_bucket" {
  description = "Model-artifact bucket, or null when Cloud Storage is not enabled."
  value       = var.enable_cloud_storage ? google_storage_bucket.artifacts[0].name : null
}

output "alloydb_instance_uri" {
  description = "Primary AlloyDB instance, or null when AlloyDB is not enabled."
  value       = var.enable_alloydb ? google_alloydb_instance.primary[0].name : null
}

output "bigtable_instance" {
  description = "Bigtable instance, or null when Bigtable is not enabled."
  value       = var.enable_bigtable ? google_bigtable_instance.timeseries[0].name : null
}

output "redis_host" {
  description = "Memorystore private address, or null when Memorystore is not enabled."
  value       = var.enable_memorystore ? google_redis_instance.cache[0].host : null
  sensitive   = true
}

output "spanner_database" {
  description = "Spanner positions database, or null when Spanner is not enabled."
  value       = var.enable_spanner ? google_spanner_database.positions[0].name : null
}

# The gap, as data.
#
# A deployment can render this and get the list of services it has switched on
# that no adapter in this build can open. It is the infrastructure counterpart
# of `StorageTarget::required_configuration`, and it exists so the mismatch is
# something an operator reads at plan time rather than something they infer
# from an empty database and a bill.
output "enabled_without_an_adapter" {
  description = "Managed services enabled here that this build's code cannot yet open."
  value = compact([
    var.enable_bigquery ? "bigquery: StorageTarget::BigQuery has no adapter" : "",
    var.enable_alloydb ? "alloydb: StorageTarget::AlloyDb has no adapter, and this build has no Postgres driver" : "",
    var.enable_bigtable ? "bigtable: StorageTarget::Bigtable has no adapter" : "",
    # Memorystore has an adapter now, so what is reported is the transport
    # mismatch instead: an instance requiring TLS that the in-tree plaintext
    # client cannot reach without a proxy in front of it.
    var.enable_memorystore && var.memorystore_transit_encryption ? "memorystore: the instance requires TLS and qip_storage::redis speaks plaintext; put a TLS-terminating proxy in the VPC or set memorystore_transit_encryption = false" : "",
    var.enable_spanner ? "spanner: StorageTarget::Spanner has no adapter" : "",
  ])
}
