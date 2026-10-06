output "network_id" {
  value = google_compute_network.vpc.id
}

output "network_name" {
  value = google_compute_network.vpc.name
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

output "data_vpc_id" {
  value       = google_compute_network.data_vpc.id
  description = "The Data plane's VPC (GCP-051)."
}

output "data_vpc_name" {
  value       = google_compute_network.data_vpc.name
  description = "The Data plane's VPC name."
}

output "engineering_vpc_id" {
  value       = google_compute_network.engineering_vpc.id
  description = "The Engineering plane's VPC (GCP-051)."
}

output "engineering_vpc_name" {
  value       = google_compute_network.engineering_vpc.name
  description = "The Engineering plane's VPC name."
}
