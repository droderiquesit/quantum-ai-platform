output "worker_service_account_email" {
  description = "The identity the Job runs as."
  value       = google_service_account.worker.email
}

output "worker_roles" {
  description = "Every role the worker holds, project-level and bucket-level. The fence of ADR 0102 decision 3: this list is the whole of it."
  value = concat(
    [for m in google_project_iam_member.worker : m.role],
    [
      google_storage_bucket_iam_member.worker_reads.role,
      google_storage_bucket_iam_member.worker_writes.role,
    ],
  )
}

output "worker_bucket_conditions" {
  description = "The IAM condition on each of the worker's bucket bindings, read off the resources. The narrowing of ADR 0102's amendment, item 3."
  value = {
    reads  = google_storage_bucket_iam_member.worker_reads.condition[0].expression
    writes = google_storage_bucket_iam_member.worker_writes.condition[0].expression
  }
}

output "job_egress" {
  description = "Read off the Job: ALL_TRAFFIC means every destination is routed through the fleet VPC, whose firewall admits only Google APIs. Null while there is no Job."
  value       = try(google_cloud_run_v2_job.fleet[0].template[0].template[0].vpc_access[0].egress, null)
}

output "egress_allowed_ranges" {
  description = "The destinations the fleet VPC's allow rule admits."
  value       = google_compute_firewall.allow_google_apis_egress.destination_ranges
}

output "egress_denied_ranges" {
  description = "The destinations the fleet VPC's deny rule refuses: everything the allow rule did not admit first."
  value       = google_compute_firewall.deny_all_egress.destination_ranges
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
