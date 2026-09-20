output "address" {
  description = "The reserved global address. Create the A records for both hostnames pointing here, at the registrar — this is the one step this repository cannot perform, because algorik.ai answers from nameservers outside this project."
  value       = google_compute_global_address.gitops.address
}

output "certificate_id" {
  description = "The managed certificate the Gateway attaches. It stays in PROVISIONING until the A records resolve to the address above."
  value       = google_compute_managed_ssl_certificate.gitops.id
}

output "hostnames" {
  description = "The names this front door answers for, for the HTTPRoutes to match and for an operator to type."
  value = {
    argocd = var.argocd_hostname
    kargo  = var.kargo_hostname
  }
}

output "iap_members" {
  description = "Who may pass IAP today. Empty means the front door admits nobody, which is a posture rather than a failure."
  value       = var.iap_members
}
