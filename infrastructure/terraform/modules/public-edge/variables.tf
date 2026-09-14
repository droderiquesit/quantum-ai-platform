variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "labels" {
  type = map(string)
}

variable "hostnames" {
  description = <<-EOT
    The hostnames the public edge answers on, and the switch for the whole
    module.

    Empty by default, and empty means **nothing here is created at all** — no
    security policy, no certificate, no bucket, no address, no forwarding
    rule. That is the correct state for an environment with no customer
    surface, which is every environment today: a load balancer with a public
    address, created because a module was instantiated, is an internet-facing
    endpoint nobody decided to open.

    Google issues the managed certificate for these names and will not do so
    until each resolves to the address this module allocates, so a name here
    is a commitment to a DNS record somebody has to make. A name nobody
    intends to point is a certificate that provisions forever and a load
    balancer that serves nothing on 443.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for host in var.hostnames :
      can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$", host))
    ])
    error_message = "A hostname is a lower-case DNS name with at least one dot — app.example.com. A wildcard is refused: Google's managed certificates do not issue for one, and a wildcard here would name hosts nobody enumerated."
  }

  validation {
    # Google's limit is 100 domains per managed certificate, and a list long
    # enough to hit it is a list nobody reviewed.
    condition     = length(var.hostnames) <= 20
    error_message = "At most twenty hostnames. A public edge answering on more names than a reviewer can hold in mind is one whose surface nobody can describe."
  }

  validation {
    condition     = length(distinct(var.hostnames)) == length(var.hostnames)
    error_message = "A hostname is listed twice. Google refuses a duplicate domain on a managed certificate, and the plan should say so before the certificate does."
  }
}

variable "application_backend" {
  description = <<-EOT
    The Cloud Run service the authenticated application APIs are served from,
    as a service name and the trust zone the catalogue places it in, or null
    for an edge that serves only the static shell.

    The zone is not decoration and it is not documentation. Blueprint §40.5
    is explicit that customer traffic and trading traffic never share a load
    balancer, an identity, a credential or a route, and §40.14 lists what a
    client may never reach: Spanner, Pub/Sub, a node in any region, a venue,
    IBM, custody key material. This module enforces the first clause of that
    by refusing, at plan time, to put a backend in front of any zone but the
    two §46.1 marks client-reachable — `public-edge` and
    `application-identity`. A backend naming `execution`, `ledger`,
    `treasury-write` or `optimisation` is refused rather than scoped, because
    scoping it is a review comment and refusing it is a plan that stops.
  EOT

  type = object({
    service_name = string
    trust_zone   = string
  })

  default = null

  validation {
    condition     = var.application_backend == null || can(regex("^[a-z]([a-z0-9-]*[a-z0-9])?$", try(var.application_backend.service_name, "")))
    error_message = "A Cloud Run service name is lower case, starts with a letter, and contains only letters, digits and hyphens."
  }
}

variable "region" {
  description = "The region the serverless network endpoint group is created in; it must be the region the Cloud Run service runs in, because a serverless NEG is regional and cannot name a service in another."
  type        = string
}

variable "static_shell_retention_days" {
  description = <<-EOT
    How long a superseded object in the static shell bucket survives.

    The shell is a build artefact: the durable record of what was served is
    the commit and the pipeline run that published it, not the object. Thirty
    days is long enough to serve a client holding a stale index and short
    enough that the bucket is not an archive nobody prunes.
  EOT

  type    = number
  default = 30

  validation {
    condition     = var.static_shell_retention_days >= 1 && var.static_shell_retention_days <= 365
    error_message = "Retention is between a day and a year. Zero would delete the object the moment it is superseded, while a client is still fetching it."
  }
}

variable "rate_limit_requests_per_minute" {
  description = <<-EOT
    How many requests one client address may make in a minute before Cloud
    Armor throttles it.

    §40.14 requires the edge to enforce a rate limit; this is that number, and
    it is a required input with a conservative default rather than something
    the module leaves off. A public edge with no rate limit is a public edge
    whose cost and whose availability both belong to whoever finds it first.
  EOT

  type    = number
  default = 600

  validation {
    condition     = var.rate_limit_requests_per_minute >= 1 && var.rate_limit_requests_per_minute <= 100000
    error_message = "The rate limit is at least one request a minute and at most a hundred thousand. A limit of zero refuses every client, and a limit large enough not to bind is a control that reads as protection and cannot fire."
  }
}

variable "permitted_regions" {
  description = <<-EOT
    The ISO 3166-1 alpha-2 country codes a client may arrive from, or an empty
    list for no geographic restriction.

    Empty by default. An allowlist that was guessed is worse than none: it
    locks out a desk travelling and reads, in the console, as a policy
    somebody decided. Name the countries the desk actually operates from, or
    leave this empty and say in the tfvars that geography is not a control
    here.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for code in var.permitted_regions : can(regex("^[A-Z]{2}$", code))
    ])
    error_message = "A region is an upper-case ISO 3166-1 alpha-2 country code — GB, US, DE. Cloud Armor matches on exactly that, and a lower-case or three-letter code matches nothing while reading as a restriction."
  }
}
