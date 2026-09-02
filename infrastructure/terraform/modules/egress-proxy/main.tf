# The egress proxy, published once and attached wherever it is needed.
#
# `qip_transport::http` has no TLS stack and refuses `https` by name, so
# every outbound adapter in the workspace needs a plaintext `http://` address
# beside it that terminates TLS onward. The Kubernetes chart described an
# Envoy proxy for that and never ran it — the Deployment was committed
# commented out, and ADR 0020's corrections record that no proxy pod ever
# existed. This module is the first proxy on the platform that does.
#
# It is deliberately not a Cloud Run service of its own. A Cloud Run service
# answers only at an HTTPS `run.app` name, which is the one scheme the client
# cannot speak, so a standalone proxy would have needed an internal load
# balancer per listener in front of it. Instead the proxy is co-located: this
# module publishes the one committed bootstrap (`infrastructure/egress/
# envoy.yaml`) to a bucket, and `modules/cloudrun` mounts it read-only into
# an Envoy sidecar that shares the workload's loopback. The execution node
# carries the same bootstrap as a systemd unit. One file, every rendering.
#
# What this module holds, once, for every rendering:
#
#   * the bootstrap, read with `file()` — there is no template and no second
#     copy, so the allowlist cannot fork;
#   * a plan-time gate that the bootstrap dials exactly the hosts the caller
#     named and binds every listener to loopback, so a widened file fails the
#     plan rather than a review;
#   * the image, pinned by digest and read out of the vendored-images list
#     the pipeline attests from, so the proxy that runs is the one that was
#     mirrored and signed.

locals {
  bootstrap = file("${path.module}/../../../egress/envoy.yaml")

  # Every upstream the bootstrap dials: a cluster's socket address on 443.
  # Listener binds are on 127.0.0.1 and never on 443, so the port is what
  # tells a dial from a bind here.
  dialled = distinct([
    for match in regexall("socket_address: \\{ address: ([a-z0-9.-]+), port_value: 443 \\}", local.bootstrap) :
    match[0]
  ])

  # Every listener, as name and port, read out of the file rather than
  # declared twice. The admin interface is not a listener and is matched
  # separately below.
  listeners = {
    for match in regexall("- name: ([a-z-]+)\\n\\s+address:\\n\\s+socket_address: \\{ address: ([0-9.]+), port_value: ([0-9]+) \\}", local.bootstrap) :
    match[0] => {
      address = match[1]
      port    = tonumber(match[2])
    }
  }

  admin_binds = [
    for match in regexall("admin:\\n(?:\\s+#[^\\n]*\\n)*\\s+address:\\n\\s+socket_address: \\{ address: ([0-9.]+), port_value: ([0-9]+) \\}", local.bootstrap) :
    match[0]
  ]

  destination_listeners = {
    for name, listener in local.listeners : name => listener
    if name != "health"
  }

  # The mirrored image, by digest, from the same list vendor.yml mirrors and
  # attests. The line is `<source@digest> <destination-path> <tag>`; the
  # destination lands under the environment's own registry prefix, and the
  # digest survives the copy — the workflow proves that before it signs.
  vendored = [
    for line in split("\n", file("${path.module}/../../../egress/vendored-images.txt")) :
    split(" ", trimspace(line))
    if trimspace(line) != "" && !startswith(trimspace(line), "#")
  ]
  envoy_entries = [for entry in local.vendored : entry if entry[1] == "vendor/envoy"]
  envoy_digest  = length(local.envoy_entries) == 1 ? split("@", local.envoy_entries[0][0])[1] : ""
  image         = "${var.image_prefix}/vendor/envoy@${local.envoy_digest}"
}

# Where every rendering reads the bootstrap from.
#
# A bucket rather than a secret: the allowlist is not confidential — it is
# reviewed in a diff — and `modules/secrets` refuses to write any secret
# value from Terraform. A bucket object is the one Cloud Run volume type that
# carries a file Terraform wrote, and versioning keeps every allowlist that
# ever ran readable after the next one replaces it.
resource "google_storage_bucket" "bootstrap" {
  project  = var.project_id
  name     = "qip-egress-${var.environment}-${var.project_id}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false

  versioning {
    enabled = true
  }

  labels = var.labels
}

# Named by its content's hash. A changed allowlist is therefore a new object
# beside the old one rather than a replacement of it, so publishing never
# needs `storage.objects.delete` — the permission the infra account
# deliberately lacks, because an identity that can delete from a bucket can
# delete from the evidence bucket — and every allowlist that ever ran stays
# readable under the name the revision that ran it mounted.
resource "google_storage_bucket_object" "bootstrap" {
  bucket       = google_storage_bucket.bootstrap.name
  name         = "envoy-${substr(sha256(local.bootstrap), 0, 16)}.yaml"
  content      = local.bootstrap
  content_type = "application/yaml"

  # The gate. Every precondition here is evaluated at plan time against the
  # file as committed, so a widened allowlist or a listener bound to an
  # interface address stops the plan and names the line — before any
  # rendering picks it up.
  lifecycle {
    precondition {
      condition     = length(local.dialled) > 0 && length(local.listeners) > 1
      error_message = "The bootstrap at infrastructure/egress/envoy.yaml parsed to ${length(local.dialled)} upstream(s) and ${length(local.listeners)} listener(s); the file has been reshaped and this module is publishing something it cannot read."
    }

    precondition {
      condition     = toset(local.dialled) == toset(var.allowed_upstreams)
      error_message = "The bootstrap dials ${join(", ", sort(local.dialled))} and the allowlist this deployment declares is ${join(", ", sort(var.allowed_upstreams))}. A destination added in one place and not the other is a destination nobody reviewed."
    }

    precondition {
      condition     = alltrue([for listener in values(local.listeners) : listener.address == "127.0.0.1"])
      error_message = "A listener binds something other than loopback: ${join(", ", [for name, listener in local.listeners : "${name}=${listener.address}" if listener.address != "127.0.0.1"])}. The proxy is co-located with the workload it serves and reachable from nowhere else; an interface bind is an address every neighbour can reach."
    }

    precondition {
      condition     = length(local.admin_binds) == 1 && local.admin_binds[0] == "127.0.0.1"
      error_message = "The Envoy admin interface is not bound to loopback (${join(", ", local.admin_binds)}). It serves /quitquitquit and a dump of every upstream."
    }

    precondition {
      condition     = contains(keys(local.listeners), "health")
      error_message = "The bootstrap has no `health` listener, so the sidecar's startup probe and the node's health check have nothing to hit that is not the admin interface."
    }

    precondition {
      condition     = length(local.envoy_entries) == 1 && can(regex("^sha256:[a-f0-9]{64}$", local.envoy_digest))
      error_message = "infrastructure/egress/vendored-images.txt names ${length(local.envoy_entries)} entry for vendor/envoy; exactly one, pinned by sha256 digest, is what vendor.yml mirrors and attests and what this module runs."
    }
  }
}
