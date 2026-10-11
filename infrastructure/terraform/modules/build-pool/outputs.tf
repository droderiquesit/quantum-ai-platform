output "worker_pool_id" {
  description = "The pool `deploy.yml` submits to: projects/<project>/locations/<region>/workerPools/qip-<env>-build."
  value       = google_cloudbuild_worker_pool.pool.id
}

output "build_service_account" {
  description = "The identity builds run as. Reads staged source, writes its saved image under out/; no registry write, no secret."
  value       = google_service_account.build.email
}

output "staging_bucket" {
  description = "Where `gcloud builds submit` stages the checkout, and where the pool leaves the saved image."
  value       = google_storage_bucket.staging.name
}

output "peered_network" {
  description = "The network the pool's workers are peered into, as planned."
  value       = google_cloudbuild_worker_pool.pool.network_config[0].peered_network
}
