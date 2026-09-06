output "cluster_name" {
  description = "The control-plane cluster's name, which `infra.yml`'s bootstrap step derives independently from the environment and checks against this."
  value       = google_container_cluster.control_plane.name
}

output "cluster_location" {
  description = "The region the cluster is in."
  value       = google_container_cluster.control_plane.location
}

output "kcc_service_account_email" {
  description = "The identity Config Connector applies as. The bootstrap writes it into the `ConfigConnector` object; `catalogue.tf` grants it `iam.serviceAccountUser` on each workload's own account."
  value       = google_service_account.kcc.email
}

output "argocd_service_account_email" {
  description = "The identity Argo CD's repo-server and proving hook act as."
  value       = google_service_account.argocd.email
}

output "kargo_service_account_email" {
  description = "The identity Kargo's controller acts as."
  value       = google_service_account.kargo.email
}

output "etcd_key_id" {
  description = "The key etcd is encrypted with, so an operator checking what holds the App keys at rest can name it."
  value       = google_kms_crypto_key.etcd.id
}
