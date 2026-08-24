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
# that this build cannot actually reach. It is the infrastructure counterpart
# of `StorageTarget::required_configuration`, and it exists so the mismatch is
# something an operator reads at plan time rather than infers from an empty
# database and a bill.
#
# It is deliberately *two* kinds of gap, because they need different actions.
# A service with no adapter needs code written. A service with an adapter needs
# the deployment to supply what the adapter cannot: a TLS-terminating proxy in
# every case, and a bearer token for the two Google APIs, since minting one
# means RSA-signing a JWT and ADR 0009 forbids in-tree crypto.
#
# This list was itself wrong for a while: it named BigQuery and Memorystore as
# having no adapter after both had gained one. An output whose whole purpose is
# to catch drift is worth nothing if it drifts, so it now distinguishes the two
# cases rather than asserting a single fact that quietly expires.
output "enabled_without_an_adapter" {
  description = "Managed services enabled here for which this build has no adapter at all."
  value = compact([
    var.enable_alloydb ? "alloydb: no adapter — no REST data plane exists, so the route to a row is the PostgreSQL wire protocol" : "",
    var.enable_bigtable ? "bigtable: no adapter — the data plane is gRPC only, needing HTTP/2 and protobuf in-tree" : "",
    var.enable_spanner ? "spanner: no adapter — a REST data plane exists but needs session pooling, transaction selectors and streaming resume tokens" : "",
  ])
}

output "enabled_but_unreachable_without_deployment_support" {
  description = "Managed services enabled here whose adapter exists but which need something the deployment must supply."
  value = compact([
    var.enable_bigquery ? "bigquery: adapter exists; needs QIP_GCP_ENDPOINT pointing at a TLS-terminating proxy and one token source" : "",
    var.enable_cloud_storage ? "cloud_storage: adapter exists; needs QIP_GCP_ENDPOINT pointing at a TLS-terminating proxy and one token source" : "",
    var.enable_memorystore && var.memorystore_transit_encryption ? "memorystore: adapter exists; the instance requires TLS and qip_storage::redis speaks plaintext — put a TLS-terminating proxy in the VPC or set memorystore_transit_encryption = false" : "",
  ])
}
