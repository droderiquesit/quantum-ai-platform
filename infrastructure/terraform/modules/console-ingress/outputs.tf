output "address" {
  description = "The reserved address Argo CD is published on. Empty when the consoles are not published."
  value       = var.enabled ? google_compute_global_address.console[0].address : ""
}

output "address_name" {
  description = "The name Argo CD's Ingress references through kubernetes.io/ingress.global-static-ip-name."
  value       = var.enabled ? google_compute_global_address.console[0].name : ""
}

output "kargo_address" {
  description = "The reserved address Kargo is published on. Empty when the consoles are not published."
  value       = var.enabled ? google_compute_global_address.kargo[0].address : ""
}

output "kargo_address_name" {
  description = "The name Kargo's Ingress references through kubernetes.io/ingress.global-static-ip-name."
  value       = var.enabled ? google_compute_global_address.kargo[0].name : ""
}

output "kargo_hostname" {
  description = "The name Kargo's certificate is issued for, derived from its own address exactly as Argo CD's is."
  value       = var.enabled ? "kargo.${replace(google_compute_global_address.kargo[0].address, ".", "-")}.nip.io" : ""
}

output "hostname" {
  description = <<-EOT
    The name the certificate is issued for and a browser asks for.

    nip.io resolves any address embedded in a name back to that address, which
    is what makes a Google-managed certificate possible before algorik.ai is
    delegated: the certificate authority's check is that the name resolves to
    the load balancer, and this name does, by construction. When the real
    domain lands this output is replaced by a record in it and nothing else
    about the arrangement changes.
  EOT
  value       = var.enabled ? "argocd.${replace(google_compute_global_address.console[0].address, ".", "-")}.nip.io" : ""
}
