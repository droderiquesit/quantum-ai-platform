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

output "deploy_attribute_condition" {
  description = <<-EOT
    The CEL condition the workload identity pool provider is planned with —
    the whole of what decides which GitHub repository may mint a token for this
    project.

    Read off `google_iam_workload_identity_pool_provider.github` rather than
    rebuilt from `var.github_repository`, so that an assertion on it is false
    the moment the variable stops reaching the provider. A condition rebuilt
    from the variable would agree with itself however the provider was
    configured, which is the failure a reviewer cannot see in a diff: a
    hardcoded repository and a threaded one read identically.

    It is a string a person may read and no credential. The repository name is
    public and the condition contains no token, key or account identifier.
  EOT

  value = google_iam_workload_identity_pool_provider.github.attribute_condition
}
