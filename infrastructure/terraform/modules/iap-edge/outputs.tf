output "address" {
  description = "The reserved global address. Create the A record for the hostname pointing here, at the registrar — this is the one step this repository cannot perform, because algorik.ai answers from nameservers outside this project."
  value       = google_compute_global_address.edge.address
}

output "hostname" {
  description = "The name this front door answers for, for an operator to type and for a test to hold the manifest to."
  value       = var.hostname
}

output "url" {
  description = "Where the fronted service is reachable once the A record resolves and the certificate has provisioned. https, because there is no listener on 80."
  value       = "https://${var.hostname}"
}

output "certificate_id" {
  description = "The managed certificate the proxy attaches. It stays in PROVISIONING until the A record resolves to the address above; more than about fifteen minutes there means the record is missing or points elsewhere."
  value       = google_compute_managed_ssl_certificate.edge.id
}

output "backend_service_id" {
  description = "The IAP-protected backend service. Named here so an operator can see which resource the project-level roles/iap.httpsResourceAccessor grant admits them to — this module holds no access list of its own (ADR 0094 decision 3)."
  value       = google_compute_backend_service.service.id
}
