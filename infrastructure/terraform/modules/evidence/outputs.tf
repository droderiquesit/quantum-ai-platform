output "bucket_name" {
  description = "The evidence bucket's name."
  value       = google_storage_bucket.evidence.name
}

output "bucket_url" {
  description = "The bucket's gs:// URL, for the mesh's evidence configuration."
  value       = google_storage_bucket.evidence.url
}

output "encryption_key_id" {
  description = "The customer-managed key the bucket encrypts with."
  value       = google_kms_crypto_key.evidence.id
}
