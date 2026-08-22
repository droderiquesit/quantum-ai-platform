output "repository_id" {
  description = "The repository's fully qualified id."
  value       = google_artifact_registry_repository.images.id
}

output "repository_name" {
  description = "The repository's short name, for an image reference."
  value       = google_artifact_registry_repository.images.name
}

output "image_prefix" {
  description = "The prefix an image reference starts with."
  value       = "${var.region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.images.name}"
}
