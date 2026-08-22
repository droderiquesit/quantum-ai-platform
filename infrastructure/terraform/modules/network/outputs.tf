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
