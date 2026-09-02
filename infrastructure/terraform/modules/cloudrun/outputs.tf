output "service_account_email" {
  description = <<-EOT
    The workload's own identity.

    This is what a caller grants against when the workload needs something
    beyond telemetry and its mounted secrets — a bucket, a queue, a key. The
    grant is written where the resource is, named, rather than passed back into
    this module as a list of roles: a role list on a module instantiated
    seventy times is where a wide grant arrives without anyone reading it.
  EOT

  value = google_service_account.workload.email
}

output "name" {
  description = "The deployed resource's name, which is not the workload name: the environment is in it, so two environments in one project cannot collide."
  value       = local.name
}

output "uri" {
  description = <<-EOT
    The service's own URL, or null for a job.

    Present so a caller can attach it to a load balancer's serverless network
    endpoint group. It is not a public address: ingress is internal or
    load-balancer-only under every input this module accepts, so a request
    arriving here from the internet is refused before the container sees it.
  EOT

  value = one(google_cloud_run_v2_service.workload[*].uri)
}

output "ingress" {
  description = "The ingress setting that was actually applied, so a caller can assert on it rather than on the posture it asked for."
  value       = local.ingress
}

output "trust_zone" {
  description = "The zone this workload resolved to — the plane's own unless it was overridden."
  value       = local.trust_zone
}

output "secret_file_paths" {
  description = <<-EOT
    Mount key to the file path the process reads, one entry per mounted secret.

    Exported because the same paths appear in the workload's own configuration,
    and a path that is written twice is a path that will eventually be written
    two ways. A test asserting the deployment and the binary agree reads this.
  EOT

  value = local.secret_files
}

output "egress_endpoints" {
  description = <<-EOT
    The loopback addresses this workload's egress proxy answers on, one per
    destination listener port, or empty for a workload that carries none.

    Exported so a test can assert that every outbound address a workload is
    configured with is one of these — an adapter pointed anywhere else is a
    credential crossing the internet in clear text, or a pod that cannot
    start, and neither is visible from the configuration alone.
  EOT

  value = local.has_egress_sidecar ? [for port in var.egress_sidecar.ports : "http://127.0.0.1:${port}"] : []
}

output "has_egress_proxy" {
  description = "Whether this workload carries the egress proxy sidecar. The fast path must answer false; see `egress_sidecar`."
  value       = local.has_egress_sidecar
}

output "network_tags" {
  description = "The tags the workload's VPC interface carries — the trust zone's, so the zone's firewall rules see this instance."
  value       = var.network_tags
}
