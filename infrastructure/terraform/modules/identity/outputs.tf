output "enabled" {
  description = "Whether identity is active in this environment."
  value       = var.enabled
}

output "authorized_domains" {
  description = "The domains authentication may redirect to, as applied. The frontend's ALGORIK_AUTHORIZED_DOMAINS must match this list exactly — a domain present in one and absent in the other fails only at redirect time, in front of a user."
  value       = var.enabled ? one(google_identity_platform_config.algorik[*].authorized_domains) : []
}

output "frontend_environment" {
  description = "The environment keys the applications read (see packages/shared-types). The API key is intentionally absent: it is created with the project and read from configuration delivery, never round-tripped through Terraform outputs into logs."
  value = var.enabled ? {
    ALGORIK_IDENTITY_PROJECT_ID = var.project_id
    ALGORIK_AUTHORIZED_DOMAINS  = join(",", var.authorized_domains)
  } : {}
}
