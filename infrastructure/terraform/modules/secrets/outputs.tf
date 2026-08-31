output "node_encryption_key_id" {
  # Through the GKE agent's grant rather than the key itself, so a cluster
  # consuming this output waits for the grant. The key id string is identical;
  # what changes is the dependency graph — without it the cluster raced the
  # grant and lost.
  value = google_kms_crypto_key_iam_member.gke_robot.crypto_key_id
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

output "key_ring_id" {
  description = <<-EOT
    The platform's key ring.

    Exported so a module that needs a customer-managed key creates it here
    rather than standing up a second key ring nobody rotates.
  EOT

  value = google_kms_key_ring.platform.id
}

output "node_service_account_email" {
  description = "The node pool's identity, which runs no workload."
  value       = google_service_account.nodes.email
}

output "secret_ids" {
  description = "Secret name to the created secret's id, so a module can grant access without reconstructing the name."
  value = {
    for name, secret in google_secret_manager_secret.platform : name => secret.secret_id
  }
}

output "service_account_names" {
  description = <<-EOT
    Deployable name to the service account's full resource name, for the
    workload identity bindings the root creates after the cluster exists.
  EOT
  value = {
    for name, account in google_service_account.workload : name => account.name
  }
}

output "console_service_account_email" {
  description = "The identity the portal runs as, or null where the console has no platform to read."
  value       = one(google_service_account.console[*].email)
}
