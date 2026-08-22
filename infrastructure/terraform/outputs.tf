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
  value = var.autonomy_ceiling
}

output "live_capable" {
  description = "Whether this environment is permitted to reach a real venue at all."
  value       = var.autonomy_ceiling != "paper_trading"
}
