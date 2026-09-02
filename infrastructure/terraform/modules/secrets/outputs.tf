output "secrets_key_id" {
  value = google_kms_crypto_key.secrets.id
}

output "key_ring_id" {
  description = <<-EOT
    The platform's key ring.

    Exported so a module that needs a customer-managed key creates it here
    rather than standing up a second key ring nobody rotates.
  EOT

  value = google_kms_key_ring.platform.id
}

output "secret_ids" {
  description = "Secret name to the created secret's id, so a module can grant access without reconstructing the name."
  value = {
    for name, secret in google_secret_manager_secret.platform : name => secret.secret_id
  }
}

output "console_service_account_email" {
  description = "The identity the portal runs as, or null where the console has no platform to read."
  value       = one(google_service_account.console[*].email)
}
