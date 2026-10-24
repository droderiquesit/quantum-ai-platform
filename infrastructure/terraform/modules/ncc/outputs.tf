output "latency_hub_id" {
  description = "The mesh-topology hub's id, or null when no latency spoke is named."
  value       = length(google_network_connectivity_hub.latency) > 0 ? google_network_connectivity_hub.latency[0].id : null
}

output "services_hub_id" {
  description = "The star-topology hub's id, or null when no star spoke is named."
  value       = length(google_network_connectivity_hub.services) > 0 ? google_network_connectivity_hub.services[0].id : null
}
