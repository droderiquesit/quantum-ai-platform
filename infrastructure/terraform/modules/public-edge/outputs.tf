output "enabled" {
  description = "Whether this environment has a public edge at all. False everywhere today; an operator reading the outputs should not have to infer that from an empty address."
  value       = length(var.hostnames) > 0
}

output "address" {
  description = "The global address the hostnames must resolve to before Google will issue the managed certificate. Null where the edge does not exist, which is every environment."
  value       = one(google_compute_global_address.edge[*].address)
}

output "security_policy" {
  description = "The Cloud Armor policy every backend behind this edge is attached to, so that a reviewer can check the attachment rather than assume it."
  value       = one(google_compute_security_policy.edge[*].name)
}

output "static_shell_bucket" {
  description = "The bucket the pipeline publishes the shell to. Null where the edge does not exist; a publish step pointed at null fails rather than creating a bucket of its own."
  value       = one(google_storage_bucket.static_shell[*].name)
}

output "hostnames" {
  description = "The names this edge answers on, echoed so that the DNS records somebody has to create are readable from the outputs rather than only from the tfvars."
  value       = var.hostnames
}
