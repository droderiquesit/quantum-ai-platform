output "name" {
  value = google_container_cluster.primary.name
}

output "endpoint" {
  value     = google_container_cluster.primary.endpoint
  sensitive = true
}

output "id" {
  description = <<-EOT
    The cluster's fully qualified id, `projects/<p>/locations/<l>/clusters/<n>`.

    The form a Backup for GKE plan names a cluster in. Taken from the resource
    rather than assembled by the caller so the plan cannot end up pointing at a
    cluster that does not exist — a backup plan attached to nothing reports
    healthy and protects nothing.
  EOT
  value       = google_container_cluster.primary.id
}

output "node_pool_bounds" {
  description = <<-EOT
    The autoscaling range, per zone and regionally.

    Surfaced because the two are three apart and confusing them is how a pool
    gets sized at a third of what was meant. A HorizontalPodAutoscaler's
    ceiling is only a policy if the regional maximum can hold it.
  EOT
  value = {
    min_per_zone  = var.min_node_count
    max_per_zone  = var.max_node_count
    min_regional  = var.min_node_count * 3
    max_regional  = var.max_node_count * 3
    initial_total = var.node_count * 3
  }
}

output "confidential_nodes" {
  description = "Whether the node pool's memory is encrypted by an AMD SEV key. See the variable: this is not what crates/libs/qip-confidential provides."
  value       = var.enable_confidential_nodes
}
