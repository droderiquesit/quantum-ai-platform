output "training_bucket" {
  description = "Training staging bucket, or null when Vertex AI is not enabled."
  value       = var.enable_vertex_ai ? google_storage_bucket.training[0].name : null
}

output "metadata_store_id" {
  description = "Training metadata store, or null when Vertex AI is not enabled."
  value       = var.enable_vertex_ai ? google_vertex_ai_metadata_store.training[0].id : null
}

output "serving_endpoint_id" {
  description = "Model serving endpoint, or null when Vertex AI is not enabled."
  value       = var.enable_vertex_ai ? google_vertex_ai_endpoint.serving[0].id : null
}

# Stated as an output rather than left implicit, because the difference between
# "provisioned" and "reachable" is the whole hazard this module documents.
output "reachable_by_this_build" {
  description = "Whether this build's code can submit a job to what was provisioned."
  value       = false
}
