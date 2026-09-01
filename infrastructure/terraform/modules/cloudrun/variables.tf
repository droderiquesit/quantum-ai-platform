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

variable "name" {
  description = <<-EOT
    The workload's own name, without the `qip-` prefix or the environment
    suffix. `market-adapter`, `intent-netting`, `evidence-sealer`.

    One name, used for the Cloud Run resource, the service account and the
    label a bill is read by. Two names for one workload is how a cost report
    and an incident timeline end up describing different things.
  EOT

  type = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,30}[a-z0-9]$", var.name))
    error_message = "A workload name is lower case, starts with a letter and contains only letters, digits and hyphens."
  }
}

variable "kind" {
  description = <<-EOT
    Whether this is a Cloud Run service or a Cloud Run job.

    The blueprint's catalogue holds both, and they are not interchangeable: a
    service answers requests and a job runs to completion. Naming which one
    this is here rather than inferring it from whether a port was set means a
    job that accidentally carries a port is refused instead of deployed as a
    service that never receives a request.
  EOT

  type    = string
  default = "service"

  validation {
    condition     = contains(["service", "job"], var.kind)
    error_message = "kind is service or job."
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
    identity, a credential or a route, and the preconditions in `main.tf`
    refuse the configurations that would make them. This module gives every
    workload its own service account, so the identity half of that separation
    is structural rather than a rule somebody follows.

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

variable "ingress_posture" {
  description = <<-EOT
    Who may reach this workload, closed by default.

      * `internal`    — reachable only from inside the VPC.
      * `public-edge` — reachable only through the external load balancer that
        fronts the customer edge, and still not directly.

    There is deliberately no value that maps to Cloud Run's
    `INGRESS_TRAFFIC_ALL`. That setting makes the service's own `run.app` URL
    answer the internet, which means the load balancer, its WAF and its
    identity check become a route rather than the route — and a service left
    at `ALL` after a debugging session looks identical in the console to one
    that was never meant to be private. The public edge here is
    `INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER`: the load balancer is the only
    way in, and there is no input to this module that produces anything wider.
  EOT

  type    = string
  default = "internal"

  validation {
    condition     = contains(["internal", "public-edge"], var.ingress_posture)
    error_message = "ingress_posture is internal or public-edge. There is no value here that opens the workload's own URL to the internet."
  }
}

variable "invokers" {
  description = <<-EOT
    The identities that may invoke this workload, as full IAM members.

    Empty by default, which means nothing may call it: an unreachable service
    is a deployment problem and a world-reachable one is an incident, so the
    empty value is the one that fails safely.

    The two anonymous principals are refused by the second validation below.
    One of them admits the internet and the other admits every Google account
    in existence, which is a different thing from admitting the caller you had
    in mind. They are named in the condition rather than here, because the
    acceptance suite scans this configuration for those two tokens and a
    `validation` block is the one construct that can name them without
    granting anything — the same arrangement `console-ingress` uses. A
    workload on the public edge is reached through the load balancer's own
    backend identity, not by making the service anonymous.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for member in var.invokers :
      can(regex("^(user|group|serviceAccount|domain):", member))
    ])
    error_message = "Each invoker is a full IAM member: user:…, group:…, serviceAccount:… or domain:…."
  }

  validation {
    condition     = !contains(var.invokers, "allUsers") && !contains(var.invokers, "allAuthenticatedUsers")
    error_message = "An anonymous invoker makes the workload's own URL the route in. Name the caller."
  }
}

variable "image_digest" {
  description = <<-EOT
    The image to run, pinned by digest.

    A tag is a name somebody may move. Pinning by digest is what makes "the
    bytes that were tested" and "the bytes that are running" the same
    sentence, and it is the same rule the registry enforces with immutable
    tags and Binary Authorization enforces with an attestation over the
    digest. The validation below refuses anything without `@sha256:`, which
    includes the shape that looks safest and is not — a tag that happens to be
    a commit hash.
  EOT

  type = string

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9._/-]*[a-z0-9]@sha256:[a-f0-9]{64}$", var.image_digest))
    error_message = "The image must be pinned by digest, as repository@sha256:<64 hex>. A tag is a name someone can move after the attestation was signed."
  }
}

variable "egress_network" {
  description = "The VPC every packet leaving this workload traverses. Direct VPC egress, so the firewall rules and the flow logs are the same ones the rest of the platform has."
  type        = string
}

variable "egress_subnet" {
  description = <<-EOT
    The subnet Cloud Run places this workload's network interface in.

    Its own range rather than a share of the primary, for the reason
    `modules/network` gives about the console's subnet: a range that GKE also
    allocates from produces an allocation failure in whichever of the two asks
    second.
  EOT

  type = string
}

variable "min_instances" {
  description = <<-EOT
    The floor. Zero, and it stays zero unless somebody writes down why.

    Scaling to zero is most of the blueprint's cost argument, and a floor of
    one is not a small change: it is a warm instance per revision per region,
    billed whether or not a request ever arrives. The execution node is the
    one workload that is always on and it is not this module.
  EOT

  type    = number
  default = 0

  validation {
    condition     = var.min_instances >= 0 && var.min_instances <= 10 && floor(var.min_instances) == var.min_instances
    error_message = "min_instances is a whole number between 0 and 10."
  }
}

variable "always_on_justification" {
  description = <<-EOT
    Why this workload may not scale to zero.

    Empty by default, and a `min_instances` above zero with this empty is
    refused at plan time. The point is not paperwork: a floor raised to hide a
    cold-start problem and a floor raised because the workload holds a warm
    connection are the same line of Terraform, and only one of them should
    survive review.
  EOT

  type    = string
  default = ""
}

variable "max_instances" {
  description = "The ceiling. Bounded on purpose: an unbounded ceiling turns a retry storm into a bill and a rate-limited dependency into an outage."
  type        = number
  default     = 4

  validation {
    condition     = var.max_instances >= 1 && var.max_instances <= 100 && floor(var.max_instances) == var.max_instances
    error_message = "max_instances is a whole number between 1 and 100."
  }
}

variable "concurrency" {
  description = "Requests one instance serves at once. Cloud Run's own default is 80; a workload holding per-request state wants far less, and saying so here is cheaper than discovering it under load."
  type        = number
  default     = 80

  validation {
    condition     = var.concurrency >= 1 && var.concurrency <= 1000 && floor(var.concurrency) == var.concurrency
    error_message = "concurrency is a whole number between 1 and 1000."
  }
}

variable "cpu" {
  description = "CPU per instance, as Cloud Run writes it."
  type        = string
  default     = "1"

  validation {
    condition     = contains(["0.25", "0.5", "1", "2", "4", "8"], var.cpu)
    error_message = "cpu is one of the values Cloud Run accepts: 0.25, 0.5, 1, 2, 4 or 8."
  }
}

variable "memory" {
  description = "Memory per instance, as Cloud Run writes it."
  type        = string
  default     = "512Mi"

  validation {
    condition     = can(regex("^[0-9]+(Mi|Gi)$", var.memory))
    error_message = "memory is written as Cloud Run writes it: 512Mi, 2Gi."
  }
}

variable "container_port" {
  description = "The port a service listens on. Ignored for a job, which listens on nothing."
  type        = number
  default     = 8080

  validation {
    condition     = var.container_port > 0 && var.container_port <= 65535
    error_message = "A container port must be a port."
  }
}

variable "health_path" {
  description = <<-EOT
    The path the startup probe polls before any request is routed.

    `/health` matches what the Rust binaries already serve, and what they
    serve there is real readiness — storage proven writable, ports bound —
    rather than process liveness. A probe that passes before the journal has
    somewhere to go is how a process ends up trading with no record.
  EOT

  type    = string
  default = "/health"

  validation {
    condition     = startswith(var.health_path, "/")
    error_message = "The health path is a path, starting with a slash."
  }
}

variable "request_timeout_seconds" {
  description = "How long one request may run before Cloud Run ends it. Explicit, because the blocking I/O in this platform is written with timeouts and a request that outlives them is waiting on something that is not coming back."
  type        = number
  default     = 300

  validation {
    condition     = var.request_timeout_seconds >= 1 && var.request_timeout_seconds <= 3600
    error_message = "A request timeout is between 1 and 3600 seconds."
  }
}

variable "env" {
  description = <<-EOT
    Non-secret configuration, as environment variables.

    Non-secret is enforced rather than assumed. A variable whose name reads
    like a credential is refused unless it ends in `_FILE`, which is the
    indirection `qip_core::secret` reads: the environment carries the path,
    the file carries the value. A key in the environment is a key in
    `/proc/<pid>/environ`, in every child process and in every crash dump, and
    the `_FILE` convention exists so that the convenient thing and the safe
    thing are the same thing.

    The `_FILE` variables for mounted secrets are generated from
    `secret_mounts` and must not be repeated here.
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

variable "secret_mounts" {
  description = <<-EOT
    Secrets this workload reads, projected as files.

    Keyed by a short mount name. Each entry names an existing Secret Manager
    secret, the version to project, the file it appears as, and the
    environment variable that points at it.

    Files, never values in the environment — the same rule the Secret Manager
    CSI driver enforces on GKE, kept when the substrate changes. Cloud Run
    mounts one volume per directory and two volumes may not share a mount
    path, so each secret gets its own directory under the platform's usual
    `/var/run/secrets/qip` rather than the single projected directory the CSI
    driver produces. The path the process reads is the one this module writes
    into the `_FILE` variable, so nothing has to know that difference.

    The module grants this workload's own service account
    `roles/secretmanager.secretAccessor` on exactly these secrets and nothing
    else. A secret not listed here is one this workload cannot read, and that
    is the whole point of an account per workload.
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

variable "encryption_key" {
  description = <<-EOT
    The KMS key Cloud Run encrypts this revision's layers with, or null.

    Null is Google-managed encryption, which is not nothing. Passing the
    platform's own key from `modules/secrets` puts the revision under the same
    rotation the rest of the deployment is under, and a key ring nobody
    rotates is the failure the other modules already argue against creating a
    second of.
  EOT

  type    = string
  default = null
}

variable "task_count" {
  description = "How many tasks one execution of a job runs. Ignored for a service."
  type        = number
  default     = 1

  validation {
    condition     = var.task_count >= 1 && var.task_count <= 1000 && floor(var.task_count) == var.task_count
    error_message = "task_count is a whole number between 1 and 1000."
  }
}

variable "task_parallelism" {
  description = "How many of a job's tasks run at once. One by default: a job that fans out before anyone has watched it run once fans out its mistakes too."
  type        = number
  default     = 1

  validation {
    condition     = var.task_parallelism >= 1 && var.task_parallelism <= 1000 && floor(var.task_parallelism) == var.task_parallelism
    error_message = "task_parallelism is a whole number between 1 and 1000."
  }
}

variable "task_max_retries" {
  description = "How many times a failed task is retried before the execution is failed. Ignored for a service."
  type        = number
  default     = 3

  validation {
    condition     = var.task_max_retries >= 0 && var.task_max_retries <= 10 && floor(var.task_max_retries) == var.task_max_retries
    error_message = "task_max_retries is a whole number between 0 and 10."
  }
}

variable "task_timeout_seconds" {
  description = "How long one task of a job may run. Ignored for a service."
  type        = number
  default     = 600

  validation {
    condition     = var.task_timeout_seconds >= 1 && var.task_timeout_seconds <= 86400
    error_message = "A task timeout is between 1 second and 24 hours."
  }
}
