output "service_account_email" {
  description = "The cell's workload identity."
  value       = google_service_account.cell.email
}

output "kubernetes_service_account" {
  description = "The Kubernetes service account the workload identity binding names."
  value       = "qip-edge-${var.cell_id}"
}

output "subnet_id" {
  description = "The cell's subnet."
  value       = google_compute_subnetwork.cell.id
}

output "node_tag" {
  description = <<-EOT
    The network tag every rule in this module targets.

    Whoever creates the cell's node pool applies this tag to it. A firewall
    rule targeting a tag nothing carries is a rule that does nothing, and it
    does nothing silently.
  EOT

  value = local.node_tag
}

output "venues" {
  description = "The venue identifiers this cell may reach, for the Kubernetes network policy to name."
  value       = sort(keys(var.venues))
}
