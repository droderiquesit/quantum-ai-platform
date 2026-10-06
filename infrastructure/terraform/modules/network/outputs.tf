# Three separate networks per environment (GCP-009): Reflex for execution nodes,
# Fabric for control-plane and event-fabric, Service for all other workloads.
output "reflex_network_id" {
  value       = google_compute_network.vpc["reflex"].id
  description = "The Reflex network (execution nodes)."
}

output "reflex_network_name" {
  value = google_compute_network.vpc["reflex"].name
}

output "fabric_network_id" {
  value       = google_compute_network.vpc["fabric"].id
  description = "The Fabric network (control-plane and event-fabric workloads)."
}

output "fabric_network_name" {
  value = google_compute_network.vpc["fabric"].name
}

output "service_network_id" {
  value       = google_compute_network.vpc["service"].id
  description = "The Service network (application and workload zones)."
}

output "service_network_name" {
  value = google_compute_network.vpc["service"].name
}

# Backward compatibility during migration: old single-network outputs now
# reference the Service network where most workloads live.
output "network_id" {
  value       = google_compute_network.vpc["service"].id
  description = "The Service network (backward compatibility). Use reflex_network_id, fabric_network_id, or service_network_id for explicit network selection."
}

output "network_name" {
  value = google_compute_network.vpc["service"].name
}

# Null in an environment whose console does not reach the platform, which is
# the honest answer rather than an empty string that reads like a configured
# value.
output "console_egress_subnet" {
  value       = one(google_compute_subnetwork.console_egress[*].name)
  description = "The subnet Cloud Run attaches the console to, or null."
}

output "console_egress_cidr" {
  value       = one(google_compute_subnetwork.console_egress[*].ip_cidr_range)
  description = "The console subnet's range, or null — the source a firewall rule admitting the console names."
}

output "google_apis_zone" {
  value       = google_dns_managed_zone.googleapis.name
  description = "The private zone that resolves every Google API to the restricted VIP."
}
