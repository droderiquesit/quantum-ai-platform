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

output "secondary_image_prefix" {
  description = "The prefix of an image reference in the secondary repository, or null when none is declared."
  # `repository_id` rather than `name`: Google returns the id as the name, and
  # the id is known at plan, so the root harness
  # (tests/secondary-registry.tftest.hcl) can assert the prefix deploy.yml
  # copies to without an apply. It still indexes the planned repository, so
  # with none declared there is no instance to read and the output is null.
  value = var.secondary_region == null ? null : "${var.secondary_region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.images_secondary[0].repository_id}"
}
