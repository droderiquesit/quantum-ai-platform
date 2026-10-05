output "worker_service_account_email" {
  description = "The identity the Job runs as."
  value       = google_service_account.worker.email
}

output "worker_roles" {
  description = "Every role the worker holds, project-level and bucket-level. The fence of ADR 0102 decision 3: this list is the whole of it."
  value = concat(
    [for m in google_project_iam_member.worker : m.role],
    [google_storage_bucket_iam_member.worker.role],
  )
}

output "bucket_name" {
  description = "The fleet bucket."
  value       = google_storage_bucket.fleet.name
}

output "bucket_uniform_access" {
  description = "Read off the bucket, not the variable."
  value       = google_storage_bucket.fleet.uniform_bucket_level_access
}

output "halt_object_uri" {
  description = "Creating this object halts dispatch and every worker's next call. Terraform never creates it."
  value       = "gs://${google_storage_bucket.fleet.name}/HALT"
}

output "job_name" {
  description = "The Cloud Run Job, or null while no image has been given."
  value       = try(google_cloud_run_v2_job.fleet[0].name, null)
}

output "job_parallelism" {
  description = "Read off the Job."
  value       = try(google_cloud_run_v2_job.fleet[0].template[0].parallelism, null)
}

output "job_max_retries" {
  description = "Read off the Job; must be 0."
  value       = try(google_cloud_run_v2_job.fleet[0].template[0].template[0].max_retries, null)
}

output "image_repository" {
  description = "Where the worker image is pushed: <this>/worker."
  value       = "${google_artifact_registry_repository.fleet.location}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.fleet.repository_id}"
}
