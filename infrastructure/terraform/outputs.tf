# Outputs.
#
# Nothing here is a secret. The endpoint and the service-account emails are
# needed to deploy; the credentials they authenticate with are in Secret
# Manager and never in state or output.

output "cluster_name" {
  description = "The cluster's name."
  value       = module.cluster.name
}

output "cluster_endpoint" {
  description = "The private control-plane endpoint."
  value       = module.cluster.endpoint
  # Not a secret, but not something to print in a CI log either.
  sensitive = true
}

output "service_account_emails" {
  description = "The workload identity service accounts, one per deployable."
  value       = module.secrets.service_account_emails
}

output "autonomy_ceiling" {
  description = <<-EOT
    The highest autonomy level this environment's platform may reach.

    Surfaced as an output so an operator can answer "could this cluster trade
    live" from the infrastructure rather than by reading a config map.
  EOT
  value       = var.autonomy_ceiling
}

output "live_capable" {
  description = "Whether this environment is permitted to reach a real venue at all."
  value       = var.autonomy_ceiling != "paper_trading"
}

output "image_prefix" {
  description = <<-EOT
    The prefix every image reference starts with.

    Needed by the pipeline to tag what it pushes and by the manifests to name
    what they pull, so it comes from the infrastructure rather than being
    written down twice.
  EOT

  value = module.registry.image_prefix
}

output "evidence_bucket" {
  description = "The write-once evidence bucket, for the mesh's evidence configuration."
  value       = module.evidence.bucket_name
}

output "workload_identity_provider" {
  description = <<-EOT
    Set this as the GitHub repository variable GCP_WORKLOAD_IDENTITY_PROVIDER.

    Until it is set, the deploy workflow cannot authenticate and says so. That
    is the intended failure: the alternative is a service-account key in a
    repository secret, which is a credential that never expires and leaves no
    record of which run used it.
  EOT

  value = module.cicd.workload_identity_provider
}

output "deploy_service_account" {
  description = "Set this as the GitHub repository variable GCP_DEPLOY_SERVICE_ACCOUNT."
  value       = module.cicd.service_account_email
}

output "edge_cells" {
  description = <<-EOT
    Each cell's identity, subnet, node tag and permitted venues.

    The node tag matters: every firewall rule constraining a cell targets it,
    and a rule targeting a tag nothing carries does nothing silently.
  EOT

  value = {
    for id, cell in module.edge_cell : id => {
      service_account            = cell.service_account_email
      kubernetes_service_account = cell.kubernetes_service_account
      subnet_id                  = cell.subnet_id
      node_tag                   = cell.node_tag
      venues                     = cell.venues
    }
  }
}
