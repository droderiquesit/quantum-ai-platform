# Everything `.github/workflows/image.yml` needs to find, so the workflow
# derives each name from the configuration rather than spelling it a second
# time. Two spellings of one name is how a rule ends up targeting a tag no
# instance carries.

output "payload_bucket" {
  description = "The bucket the bake stages its payload in."
  value       = google_storage_bucket.payload.name
}

output "builder_service_account_email" {
  description = "The identity the throwaway builder machine runs as. It reads the staging bucket and holds no other grant, and it has no key."
  value       = google_service_account.builder.email
}

output "builder_subnet" {
  description = "The subnet the builder machine attaches to. No external address is ever assigned on it."
  value       = google_compute_subnetwork.builder.name
}

output "builder_tag" {
  description = "The network tag the builder must carry. The deny-egress and the Google-APIs allow both target it, so a machine created without it has no rules and reaches whatever the VPC's implied allow-all permits."
  value       = local.builder_tag
}
