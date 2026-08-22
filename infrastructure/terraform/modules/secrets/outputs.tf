output "node_encryption_key_id" {
  value = google_kms_crypto_key.node_encryption.id
}

output "secrets_key_id" {
  value = google_kms_crypto_key.secrets.id
}

output "service_account_emails" {
  description = "Deployable name to service-account email."
  value = {
    for name, account in google_service_account.workload : name => account.email
  }
}
