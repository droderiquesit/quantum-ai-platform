output "network_id" {
  value = google_compute_network.vpc.id
}

output "subnet_id" {
  value = google_compute_subnetwork.primary.id
}

output "pod_range_name" {
  value = google_compute_subnetwork.primary.secondary_ip_range[0].range_name
}

output "service_range_name" {
  value = google_compute_subnetwork.primary.secondary_ip_range[1].range_name
}

# Named for the deployment that consumes them. Both are null in an environment
# whose console does not reach the platform, which is the honest answer rather
# than an empty string that reads like a configured value.
output "console_egress_subnet" {
  value       = one(google_compute_subnetwork.console_egress[*].name)
  description = "The subnet Cloud Run attaches the console to, or null."
}

output "api_internal_address" {
  value       = one(google_compute_address.api_internal[*].address)
  description = "The internal address qip-api's load balancer answers on, or null."
}
