output "service_account_email" {
  description = <<-EOT
    The workload's own identity.

    This is what a caller grants against when the workload needs something
    beyond telemetry and its mounted secrets — a bucket, a queue, a key. The
    grant is written where the resource is, named, rather than passed back into
    this module as a list of roles: a role list on a module instantiated
    seventy times is where a wide grant arrives without anyone reading it.
    It is also the `serviceAccountRef` the manifest must name.
  EOT

  value = google_service_account.workload.email
}

output "name" {
  description = "The Cloud Run resource's name — and the manifest's `metadata.name`, which is how Config Connector acquires it. The environment is in it, so two environments in one project cannot collide."
  value       = local.name
}

output "uri" {
  description = <<-EOT
    The service's own URL, computed from Cloud Run's deterministic form
    rather than read from a resource, because the resource is Config
    Connector's (ADR 0036). It is not a public address: every catalogue
    workload's ingress is internal, so a request arriving here from the
    internet is refused before the container sees it.
  EOT

  value = local.uri
}

output "trust_zone" {
  description = "The zone this workload resolved to — the plane's own unless it was overridden."
  value       = local.trust_zone
}

output "environment" {
  description = "Every environment variable the manifest must set, name to value: the caller's settings, the `_FILE` path of each mounted secret, the `_PATH` of each configuration file. The parity test compares the manifest to this and to no second list."
  value       = local.environment
}

output "secret_root" {
  description = "The directory every secret volume mounts under, so the manifest and the `_FILE` paths agree by construction."
  value       = local.secret_root
}

output "secret_file_paths" {
  description = <<-EOT
    Mount key to the file path the process reads, one entry per mounted secret.

    Exported because the same paths appear in the manifest's volume mounts,
    and a path that is written twice is a path that will eventually be written
    two ways. A test asserting the manifest and the binary agree reads this.
  EOT

  value = local.secret_files
}

output "config_file_paths" {
  description = <<-EOT
    Mount key to the path the process reads, one entry per configuration
    file, or empty for a workload that reads none.

    Exported for the reason `secret_file_paths` is: the same path is the
    value of the workload's `_PATH` variable in the manifest, and the
    hash-named directory in it changes when the committed file does.
  EOT

  value = local.config_files
}

output "config_file_hashes" {
  description = <<-EOT
    Mount key to the sha256 of the content this workload was given, one entry
    per configuration file.

    The object under `/etc/qip` is named by this hash, so it is the answer to
    "which catalogue did that revision read" — from the plan, not from a
    shell on the instance.
  EOT

  value = local.config_file_hashes
}

output "config_files_bucket" {
  description = "The bucket the manifest mounts read-only at `/etc/qip`, or null for a workload that reads no configuration file."
  value       = one(google_storage_bucket.config_files[*].name)
}

output "collector_config_bucket" {
  description = "The bucket the collector sidecar's scrape document is published to, or null where no collector is declared."
  value       = one(google_storage_bucket.collector_config[*].name)
}

output "egress_endpoints" {
  description = <<-EOT
    The loopback addresses this workload's egress proxy answers on, one per
    destination listener port, or empty for a workload that carries none.

    Exported so a test can assert that every outbound address a workload is
    configured with is one of these — an adapter pointed anywhere else is a
    credential crossing the internet in clear text, or an instance that cannot
    start, and neither is visible from the configuration alone.
  EOT

  value = local.has_egress_sidecar ? [for port in var.egress_sidecar.ports : "http://127.0.0.1:${port}"] : []
}

output "has_egress_proxy" {
  description = "Whether this workload carries the egress proxy sidecar. The fast path must answer false; see `egress_sidecar`."
  value       = local.has_egress_sidecar
}

output "metrics_collected" {
  description = <<-EOT
    Whether a managed-Prometheus collector is declared beside this workload.

    False unless `collector_image_digest` named one. Declared is the whole
    of what this answers: it says a sidecar is in the manifest, not that a
    scrape has happened, and `workload_metrics_exist` in the root stays a
    separate fact a person flips on evidence of ingestion.
  EOT

  value = local.has_metrics_collector
}

output "network_tags" {
  description = "The tags the workload's VPC interface carries in the manifest — the trust zone's, so the zone's firewall rules see this instance."
  value       = var.network_tags
}
