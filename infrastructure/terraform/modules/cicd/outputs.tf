output "service_account_email" {
  description = "The pipeline's service account. Set as the GitHub variable GCP_DEPLOY_SERVICE_ACCOUNT."
  value       = google_service_account.ci.email
}

output "workload_identity_provider" {
  description = <<-EOT
    The provider resource name the GitHub action authenticates against. Set as
    the GitHub variable GCP_WORKLOAD_IDENTITY_PROVIDER.

    It contains the project *number*, which is why it is an output rather than
    something a workflow can construct from the project id it already knows.
  EOT

  value = google_iam_workload_identity_pool_provider.github.name
}

output "infra_service_account" {
  description = "The infrastructure account infra.yml impersonates. Set as the GitHub variable GCP_INFRA_SERVICE_ACCOUNT."
  value       = google_service_account.infra.email
}
