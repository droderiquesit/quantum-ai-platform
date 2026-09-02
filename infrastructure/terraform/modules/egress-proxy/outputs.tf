output "sidecar" {
  description = <<-EOT
    What `modules/cloudrun` needs to attach the proxy to a service: the
    mirrored image by digest, the bucket and object the bootstrap is read
    from, the destination listener ports, and the health listener's port for
    the sidecar's startup probe.

    Everything here is derived from the one committed bootstrap and the one
    vendored-images entry; nothing is typed a second time.
  EOT

  value = {
    image            = local.image
    bootstrap_bucket = google_storage_bucket.bootstrap.name
    bootstrap_object = google_storage_bucket_object.bootstrap.name
    ports            = sort([for listener in values(local.destination_listeners) : tostring(listener.port)])
    health_port      = local.listeners["health"].port
  }
}

output "endpoints" {
  description = <<-EOT
    The address each listener answers on, keyed by listener name, as the
    `http://127.0.0.1:<port>` a co-located adapter is configured with.

    Every value is `http`, and that is the contract: `qip_transport::http`
    refuses `https` by name, so an operator who "fixes" one of these to point
    straight at the vendor gets a construction error naming the variable —
    which is the correct failure, because the alternative is an API token
    crossing the internet in clear text.
  EOT

  value = {
    for name, listener in local.destination_listeners : name => "http://127.0.0.1:${listener.port}"
  }
}

output "dialled_upstreams" {
  description = "The hosts the published bootstrap dials, sorted. The whole external surface of the proxy in one list, which is the form a review is done on."
  value       = sort(local.dialled)
}

output "image" {
  description = "The Envoy image every rendering runs, by digest, in the environment's own registry."
  value       = local.image
}
