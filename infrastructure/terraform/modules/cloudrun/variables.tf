variable "project_id" {
  type = string
}

variable "project_number" {
  description = "The project's numeric id. Cloud Run's deterministic URL for a service is built from it, and with the service resource in Config Connector's hands (ADR 0036) the URL is computed here rather than read back."
  type        = number
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

variable "name" {
  description = <<-EOT
    The workload's own name, without the `qip-` prefix or the environment
    suffix. `market-adapter`, `intent-netting`, `evidence-sealer`.

    One name, used for the service account, the buckets, the label a bill is
    read by, and — in the manifest — the Cloud Run resource. Two names for
    one workload is how a cost report and an incident timeline end up
    describing different things.
  EOT

  type = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,30}[a-z0-9]$", var.name))
    error_message = "A workload name is lower case, starts with a letter and contains only letters, digits and hyphens."
  }
}

variable "plane" {
  description = <<-EOT
    Which plane of the blueprint's §41.6 catalogue this workload belongs to.

    Recorded as a label, which is what makes "what does cognition cost" and
    "what did the ledger plane do during the incident" answerable from the
    bill and the log rather than from somebody's memory of which service is
    which.

    `execution` is deliberately not in the list. The execution node is a
    dedicated Compute Engine machine running bare under systemd — see ADR
    0020 — and a workload that believes it is on the execution path while
    running on a substrate that scales to zero is the exact confusion this
    refusal exists to prevent.
  EOT

  type = string

  validation {
    condition = contains([
      "ingestion",
      "cognition",
      "valuation",
      "intelligence",
      "optimisation",
      "capital-and-risk",
      "ledger-and-treasury",
      "wallet-and-inventory",
      "registries",
      "experience-and-identity",
      "data-and-observability",
      "control-fabric",
    ], var.plane)
    error_message = "The plane must be one of the twelve this module can name. `execution` is not one of them: the execution node is a Compute Engine machine, not a Cloud Run workload."
  }
}

variable "trust_zone" {
  description = <<-EOT
    The trust zone this workload sits in, if it is not the plane's own.

    The blueprint's §46.1 gives thirteen zones with default deny between them.
    Most workloads sit in the zone named after their plane, so this defaults to
    null and resolves to the plane — a value that must be written twice is a
    value that will eventually disagree with itself.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.trust_zone == null || can(regex("^[a-z][a-z0-9-]{1,40}[a-z0-9]$", var.trust_zone))
    error_message = "A trust zone is a lower-case token: letters, digits and hyphens."
  }
}

variable "traffic_class" {
  description = <<-EOT
    Whose traffic this workload carries. There is no default, on purpose.

      * `customer` — traffic that arrives from a person through the public
        edge: the console's API, sign-in, anything a browser reaches.
      * `trading`  — the decision and order path, and anything that may read a
        venue credential.
      * `platform` — everything else: internal control, data, observability.

    Customer traffic and trading traffic never share a load balancer, an
    identity, a credential or a route. This module gives every workload its
    own service account, so the identity half of that separation is
    structural; the ingress half is the manifest's, and the parity test
    refuses a trading workload on any posture but internal.

    Not defaulted, because every default here would be a guess about a
    security boundary. A caller that has not decided which class a workload is
    has not finished designing it.
  EOT

  type = string

  validation {
    condition     = contains(["customer", "trading", "platform"], var.traffic_class)
    error_message = "traffic_class is customer, trading or platform. Say which; there is no safe guess."
  }
}

variable "container_port" {
  description = "The port the service listens on. The collector's scrape document names it; the manifest publishes it."
  type        = number
  default     = 8080

  validation {
    condition     = var.container_port > 0 && var.container_port <= 65535
    error_message = "A container port must be a port."
  }
}

variable "env" {
  description = <<-EOT
    Non-secret configuration, as environment variables, exactly as the
    manifest must set them.

    Non-secret is enforced rather than assumed. A variable whose name reads
    like a credential is refused unless it ends in `_FILE`, which is the
    indirection `qip_core::secret` reads: the environment carries the path,
    the file carries the value. A key in the environment is a key in
    `/proc/<pid>/environ`, in every child process and in every crash dump, and
    the `_FILE` convention exists so that the convenient thing and the safe
    thing are the same thing.

    The `_FILE` variables for mounted secrets are generated from
    `secret_mounts` and must not be repeated here. The merged result is the
    `environment` output, which the parity test compares the manifest to.
  EOT

  type    = map(string)
  default = {}

  validation {
    condition = alltrue([
      for key in keys(var.env) : can(regex("^[A-Z][A-Z0-9_]*$", key))
    ])
    error_message = "An environment variable name is upper case with underscores."
  }

  validation {
    condition = alltrue([
      for key in keys(var.env) :
      endswith(key, "_FILE") || !can(regex("(TOKEN|SECRET|CREDENTIAL|PASSWORD|PRIVATE_KEY|_KEY)$", key))
    ])
    error_message = "That name reads like a credential. Mount it with secret_mounts and let the environment carry the path, not the value."
  }
}

variable "secret_env" {
  description = <<-EOT
    Secrets this workload reads as environment values, projected by Cloud Run
    from a Secret Manager version at container start.

    Permitted for a vendored image only (ADR 0031), and that restriction is
    the point. Every binary this platform compiles reads credentials through
    `qip_core::secret`, which takes a path; a built workload reaching for this
    input would be choosing the easier one, and the day that is possible the
    rule against secrets in the environment stops meaning anything. The
    exception exists for a binary that cannot read a file. OpenObserve is the
    one in this catalogue: its image carries no shell, so no entrypoint can
    bridge a mount, and no symbol in it offers `_FILE` indirection for the
    credential.

    The refusal used to be a precondition on the service resource, reading
    the image's source. The resource is a manifest now (ADR 0036) and so is
    the image, so the parity test carries the refusal: a `secret_env` on a
    manifest whose image is one `deploy.yml` builds fails the build. What
    stays here is the grant, because without it Cloud Run has no instance at
    all — see the resource.

    The value is never in this repository, a plan or the state file: what
    Terraform carries is the secret's name.
  EOT

  type = map(object({
    secret_id = string
    version   = optional(string, "latest")
  }))
  default = {}

  validation {
    condition = alltrue([
      for key in keys(var.secret_env) : can(regex("^[A-Z][A-Z0-9_]*$", key))
    ])
    error_message = "An environment variable name is upper case with underscores."
  }
}

variable "secret_mounts" {
  description = <<-EOT
    Secrets this workload reads, projected as files.

    Keyed by a short mount name. Each entry names an existing Secret Manager
    secret, the version to project, the file it appears as, and the
    environment variable that points at it.

    Files, never values in the environment — the same rule the Secret Manager
    CSI driver enforced on GKE, kept when the substrate changed. Cloud Run
    mounts one volume per directory and two volumes may not share a mount
    path, so each secret gets its own directory under the platform's usual
    `/var/run/secrets/qip`. The path the process reads is the one this module
    writes into the `_FILE` variable (`secret_file_paths`), and the manifest
    mounts the same directory; the parity test compares the two.

    The module grants this workload's own service account
    `roles/secretmanager.secretAccessor` on exactly these secrets and nothing
    else. A secret not listed here is one this workload cannot read, and that
    is the whole point of an account per workload.

    Mounting is the half a manifest guarantees; opening the file is the
    workload's half, and a mount is not evidence the credential arrived. A
    built workload reads the path through `qip_core::secret`, which is why
    the variable must be named `QIP_…_FILE`.
  EOT

  type = map(object({
    secret_id         = string
    version           = optional(string, "latest")
    file_name         = string
    env_file_variable = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for key in keys(var.secret_mounts) : can(regex("^[a-z][a-z0-9-]{0,30}$", key))
    ])
    error_message = "A mount key is a short lower-case token; it becomes a directory name."
  }

  validation {
    condition = alltrue([
      for mount in values(var.secret_mounts) : can(regex("^[a-z0-9][a-z0-9._-]*$", mount.file_name))
    ])
    error_message = "A file name is a file name, not a path: no slashes, no traversal."
  }

  validation {
    condition = alltrue([
      for mount in values(var.secret_mounts) : can(regex("^QIP_[A-Z0-9_]*_FILE$", mount.env_file_variable))
    ])
    error_message = "The environment variable pointing at a mounted secret is named QIP_…_FILE, which is what qip_core::secret reads."
  }

  validation {
    condition = alltrue([
      for mount in values(var.secret_mounts) : can(regex("^(latest|[0-9]+)$", mount.version))
    ])
    error_message = "A secret version is `latest` or a version number."
  }
}

variable "config_files" {
  description = <<-EOT
    Configuration this workload reads as a file, and the committed bytes it
    is made of.

    Keyed by a short mount name. Each entry carries the file's content — read
    by the root with `file()` from a committed path, so what a revision reads
    is what the reviewer read — the name it appears under, and the
    environment variable that points at it. The module publishes every entry
    to a bucket of this workload's own, under a directory named by the hash
    of the content; the manifest mounts that bucket read-only at `/etc/qip`
    and the path reaches the process in the named variable.

    This is not a secret and is deliberately not shaped like one: the
    variable must end in `_PATH`, never `_FILE`, because `_FILE` is the
    indirection `qip_core::secret` reads and a catalogue under that name
    would be a credential to every reader of the configuration. A file that
    is confidential belongs in `secret_mounts`.
  EOT

  type = map(object({
    content           = string
    file_name         = string
    content_type      = optional(string, "application/octet-stream")
    env_file_variable = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for key in keys(var.config_files) : can(regex("^[a-z][a-z0-9-]{0,30}$", key))
    ])
    error_message = "A configuration file key is a short lower-case token; it names the object and the output entry."
  }

  validation {
    condition = alltrue([
      for file in values(var.config_files) : can(regex("^[a-z0-9][a-z0-9._-]*$", file.file_name))
    ])
    error_message = "A configuration file name is a file name, not a path: no slashes, no traversal."
  }

  validation {
    condition     = length(distinct([for file in values(var.config_files) : file.file_name])) == length(var.config_files)
    error_message = "Two configuration files share a file name; they are mounted in one directory and the second would hide the first."
  }

  validation {
    condition = alltrue([
      for file in values(var.config_files) : can(regex("^QIP_[A-Z0-9_]*_PATH$", file.env_file_variable))
    ])
    error_message = "The environment variable pointing at a configuration file is named QIP_…_PATH. A _FILE name is what qip_core::secret reads, and this is not a secret; a file that is one belongs in secret_mounts."
  }

  validation {
    condition = alltrue([
      for file in values(var.config_files) : length(file.content) > 0
    ])
    error_message = "A configuration file has no content. An empty catalogue mounted at the path the process reads is a process that starts with nothing and reports nothing wrong."
  }
}

variable "network_tags" {
  description = <<-EOT
    The network tags the workload's VPC interface carries in the manifest.

    One tag, normally: the trust zone's, from `modules/trust-zones`. Every
    firewall rule that module writes targets a tag, so an instance whose
    interface carries none is an instance those rules never see. Recorded
    here, beside the identity, so the root's `cloud_run_services` output and
    the parity test read the zone's tag from one place; the root refuses a
    catalogue entry whose zone is not declared, which is where the empty case
    is actually caught.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for tag in var.network_tags : can(regex("^[a-z][a-z0-9-]{0,61}[a-z0-9]$", tag))
    ])
    error_message = "A network tag is lower case, starts with a letter, contains only letters, digits and hyphens, and is at most 63 characters."
  }
}

variable "egress_sidecar" {
  description = <<-EOT
    The TLS-terminating egress proxy this workload runs beside it, or null
    for a workload that reaches nothing outside the VPC.

    The object is `modules/egress-proxy`'s `sidecar` output, passed through
    unchanged: the mirrored Envoy image by digest, the bucket and object the
    one committed bootstrap is published to, the destination listener ports,
    and the health listener. The sidecar container itself is in the
    manifest; what this module does with the object is grant the workload's
    identity the read on the bootstrap bucket, and answer `has_egress_proxy`
    and `egress_endpoints` so the parity test can hold the manifest to the
    catalogue.

    Null is the default and the safe one. The fast path carries none,
    deliberately: ADR 0008, consequence 3, is that nothing on the hot path
    consults a model, and port 9102 on this proxy is a route to one.
  EOT

  type = object({
    image            = string
    bootstrap_bucket = string
    bootstrap_object = string
    ports            = list(string)
    health_port      = number
  })

  default = null

  validation {
    condition     = var.egress_sidecar == null || can(regex("^[a-z0-9][a-z0-9._/-]*[a-z0-9]@sha256:[a-f0-9]{64}$", var.egress_sidecar.image))
    error_message = "The egress proxy image must be pinned by digest, as repository@sha256:<64 hex>. A tag is a name someone can move after the attestation was signed."
  }

  validation {
    condition     = var.egress_sidecar == null ? true : (var.egress_sidecar.health_port > 0 && var.egress_sidecar.health_port <= 65535 && length(var.egress_sidecar.ports) > 0)
    error_message = "The egress proxy names a health port and at least one destination listener; a proxy with no listener proxies nothing and reads as a route."
  }
}

variable "collector_image_digest" {
  description = <<-EOT
    The managed-Prometheus collector to run beside this workload, pinned by
    digest, or null for a workload nothing scrapes.

    Null is the default and the closed one: no digest, no scrape document,
    no bucket, and `metrics_collected` answers false, so nothing downstream
    can read "a collector is declared" as "a scrape has happened". Set, it
    must be the full `repository@sha256:<64 hex>` of the copy `vendor.yml`
    mirrored into the environment's own registry and attested; the root
    composes the value from the registry prefix and a bare digest, so the
    upstream repository cannot be named here at all. The sidecar container
    is the manifest's; the document it reads is published here.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.collector_image_digest == null || can(regex("^[a-z0-9][a-z0-9._/-]*[a-z0-9]@sha256:[a-f0-9]{64}$", var.collector_image_digest))
    error_message = "The metrics collector image must be pinned by digest, as repository@sha256:<64 hex>, or left null for no collector. A tag is a name someone can move after the attestation was signed."
  }
}

variable "deployer_service_account" {
  description = <<-EOT
    The account that creates this service's revisions and must therefore be
    able to act as the service's identity: Config Connector's, under ADR
    0036. Granted on this one account and no other; null grants nobody,
    which is a service nothing can move.
  EOT

  type    = string
  default = null
}
