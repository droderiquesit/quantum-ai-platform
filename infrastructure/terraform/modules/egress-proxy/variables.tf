variable "project_id" {
  type = string
}

variable "region" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "image_prefix" {
  description = <<-EOT
    The environment's own registry prefix, from `modules/registry`.

    The Envoy image runs from here and never from Docker Hub: `vendor.yml`
    mirrors the reviewed digest into this registry and attests it with the
    platform's attestor, which is what lets Binary Authorization's single
    REQUIRE_ATTESTATION rule admit it without an exemption pattern. The
    digest itself is read out of `infrastructure/egress/vendored-images.txt`
    so the plan and the list the workflow mirrors from cannot disagree.
  EOT
  type        = string
}

variable "allowed_upstreams" {
  description = <<-EOT
    The hosts the proxy may dial, and no others.

    This is the allowlist as a value a deployment declares, checked at plan
    time against the hosts the committed bootstrap actually dials. The two
    must be the same set: a host in the file and not here is a destination
    somebody added without declaring it, and a host here and not in the file
    is an adapter that will get a 404 from a proxy nobody suspects. Widening
    the proxy is therefore two edits and a reviewer who sees both, which is
    the property an allowlist exists to have.

    No default. A deployment that has not written down what its proxy may
    reach has not decided.
  EOT
  type        = list(string)

  validation {
    condition = alltrue([
      for host in var.allowed_upstreams : can(regex("^[a-z0-9][a-z0-9.-]*[a-z0-9]$", host)) && !strcontains(host, "*")
    ])
    error_message = "Each allowed upstream is one host name. A wildcard is a family of hosts, and an allowlist stops being a list the moment one entry matches a family."
  }

  validation {
    condition = alltrue([
      for host in var.allowed_upstreams : !strcontains(host, "venue") && !strcontains(host, "broker") && !strcontains(host, "exchange")
    ])
    error_message = "An allowed upstream names a venue, broker or exchange. A route to a venue is a live-order path whatever the ceiling says; it is not added through the egress allowlist."
  }
}
