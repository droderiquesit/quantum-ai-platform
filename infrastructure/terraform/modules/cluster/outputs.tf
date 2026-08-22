output "name" {
  value = google_container_cluster.primary.name
}

output "endpoint" {
  value     = google_container_cluster.primary.endpoint
  sensitive = true
}
