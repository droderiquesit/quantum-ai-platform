variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "region" {
  description = <<-EOT
    The region the Cloud NAT and its router live in.

    A Cloud NAT is regional and can translate only for subnets in its own
    region, so a deployment whose zones span regions instantiates this module
    once per region rather than reaching across. The NAT below refuses the
    cross-region case at plan time instead of quietly leaving a zone with an
    egress rule and no translation.
  EOT
  type        = string
}

variable "network_id" {
  description = "The VPC the zone subnets are cut from and every rule below is written in."
  type        = string
}

variable "zones" {
  description = <<-EOT
    The trust zones to create, keyed by zone name.

    The names are fixed: `public-edge`, `application-identity`,
    `ingestion-discovery`, `cognition`, `valuation`, `intelligence`,
    `optimisation`, `control-fabric`, `execution`, `ledger`, `wallet-read`,
    `treasury-write`, `management`. A name outside that set is refused rather
    than created, because a zone this module does not know is a zone with no
    sanctioned paths, no sanctioned egress and no place in the model — and the
    failure mode of accepting it is a subnet and an identity that look
    governed and are not.

    An empty map is no zones, not all of them. A zone declared here and given
    no path can reach nothing and be reached by nothing; that is the intended
    starting state, and every route out of it is a separate declaration
    somebody had to write down.

      * `region` is where the subnet lives. Zones that talk to each other
        should share one, or the paths below cross a region boundary and pay
        for it on every hop.
      * `subnet_cidr` is the zone's own range, and it is the range the path
        rules name — so two zones sharing one would be two zones with one
        boundary drawn between them on paper. A Cloud Run direct-VPC-egress
        interface needs a /26 or larger, and Google reserves addresses in it
        as instances scale; a /24 per zone is the working size.

    A zone's identities are not declared here. They are the Cloud Run
    workloads the catalogue places in the zone, and the root passes them in
    through `zone_identities`, so a zone with no workload has no identity and
    no grant.
  EOT

  type = map(object({
    region      = string
    subnet_cidr = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for name in keys(var.zones) : contains([
        "public-edge",
        "application-identity",
        "ingestion-discovery",
        "cognition",
        "valuation",
        "intelligence",
        "optimisation",
        "control-fabric",
        "execution",
        "ledger",
        "wallet-read",
        "treasury-write",
        "management",
      ], name)
    ])
    error_message = "A zone name outside the thirteen of blueprint §46.1 is refused, not defaulted. The names are public-edge, application-identity, ingestion-discovery, cognition, valuation, intelligence, optimisation, control-fabric, execution, ledger, wallet-read, treasury-write, management. A fourteenth zone is an edit to local.zone_names and local.sanctioned_paths in this module, which is the review it deserves."
  }

  validation {
    condition = alltrue([
      for zone in values(var.zones) :
      can(cidrhost(zone.subnet_cidr, 0)) && zone.subnet_cidr != "0.0.0.0/0"
    ])
    error_message = "Every zone needs a real IPv4 range of its own, and the whole internet is not one."
  }

  validation {
    condition     = length(distinct([for zone in values(var.zones) : zone.subnet_cidr])) == length(var.zones)
    error_message = "Two zones share a subnet range. The path rules name ranges, so zones sharing one can reach each other whatever this module declares — the boundary would exist only in the diagram."
  }

}

variable "zone_identities" {
  description = <<-EOT
    The service accounts placed in each zone, keyed by zone name: the
    identities `modules/cloudrun` created for the workloads the catalogue
    puts there. The ledger and control-fabric grants below are made to these
    accounts and to nothing else.

    Empty by default, which grants nothing. A zone named here that is not in
    `zones` is refused, because an identity in a zone that has no subnet and
    no rules is an identity outside every boundary.

    Whether one account appears under two zones cannot be checked at plan
    time — the emails are not known until the accounts exist — so the
    structural guarantee is the catalogue's: every workload names exactly one
    zone, and `modules/cloudrun` creates exactly one account per workload.
  EOT

  type    = map(list(string))
  default = {}

  validation {
    condition = alltrue([
      for zone in keys(var.zone_identities) : contains([
        "public-edge",
        "application-identity",
        "ingestion-discovery",
        "cognition",
        "valuation",
        "intelligence",
        "optimisation",
        "control-fabric",
        "execution",
        "ledger",
        "wallet-read",
        "treasury-write",
        "management",
      ], zone)
    ])
    error_message = "zone_identities names a zone outside the thirteen of blueprint §46.1. An identity in an undeclared zone is an identity outside every boundary."
  }
}

variable "google_apis_range" {
  description = <<-EOT
    The range every zone may reach Google APIs on, for the one egress rule
    that permits it. The restricted VIP by default, which `modules/network`'s
    private zone resolves every `*.googleapis.com` to.
  EOT
  type        = string
  default     = "199.36.153.8/30"

  validation {
    condition     = var.google_apis_range != "0.0.0.0/0"
    error_message = "Google's APIs are not the whole internet."
  }
}

variable "permitted_paths" {
  description = <<-EOT
    The zone-to-zone paths that exist, keyed by a short name describing the
    path — `application-reads-ledger`, `fabric-publishes-to-execution`.

    Empty by default, and an empty map means no zone can reach any other. That
    is the fail-closed reading and the only safe one: a pair with no declared
    path is denied, so forgetting a path breaks a deployment loudly rather than
    leaving an opening quietly.

    A declaration is not sufficient on its own. The pair and the mode must both
    appear in `local.sanctioned_paths`, which is blueprint §46.1 transcribed —
    so a caller cannot invent a route between the wallet read path and the
    treasury write path, or from a client-facing zone to the ledger, however
    the tfvars are written. The plan refuses it and names the pair.

      * `mode` is one of `request`, `read`, `append`, `publish`, `intent`,
        `audited`, and it decides the ledger and control-fabric grants as well
        as documenting the path. It is not a label.
      * `ports` are the destination ports. TCP only; nothing in this model
        talks UDP across a zone boundary.
      * `note` says why the path exists. Required and non-empty: the reviewer
        of the tfvars is the last person who can refuse a route, and they need
        the argument, not the tuple.
  EOT

  type = map(object({
    from  = string
    to    = string
    mode  = string
    ports = list(number)
    note  = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for name in keys(var.permitted_paths) :
      can(regex("^[a-z][a-z0-9-]{1,23}[a-z0-9]$", name))
    ])
    error_message = "A path name is lower case, starts with a letter, contains only letters, digits and hyphens, and is at most 25 characters — it becomes part of a firewall rule name, and Google allows 63 for the whole thing."
  }

  validation {
    condition = alltrue([
      for path in values(var.permitted_paths) :
      contains(["request", "read", "append", "publish", "intent", "audited"], path.mode)
    ])
    error_message = "A path mode is request, read, append, publish, intent or audited. An unrecognised mode is refused rather than treated as read-write."
  }

  validation {
    condition = alltrue([
      for path in values(var.permitted_paths) : path.from != path.to
    ])
    error_message = "A path from a zone to itself is not a boundary crossing. Traffic inside a zone needs no declaration here."
  }

  validation {
    condition = alltrue([
      for path in values(var.permitted_paths) :
      length(path.ports) > 0 && alltrue([for port in path.ports : port > 0 && port <= 65535])
    ])
    error_message = "A path names at least one destination port, and a port is a port. A path with no ports is a rule that permits nothing and reads as a permitted route."
  }

  validation {
    condition = alltrue([
      for path in values(var.permitted_paths) : trimspace(path.note) != ""
    ])
    error_message = "Every path carries a note saying why it exists. A route nobody could argue for in one sentence is a route to delete rather than to document later."
  }
}

variable "external_egress" {
  description = <<-EOT
    The allowlist: every destination outside the VPC any zone may reach, one
    entry per destination, keyed by a short name — `ibm-quantum-runtime`,
    `venue-primary-fix`.

    Empty by default, which is a platform that reaches nothing external. Each
    entry is a host and a port, and the `purpose` must be one the zone is
    permitted to hold:

      * `ingestion-discovery` — `information-source`. The widest external
        surface on the platform, and it can reach nothing that moves money.
      * `execution` — `venue`.
      * `wallet-read` — `venue-read`, `chain`, `custodian-read`.
      * `treasury-write` — `custodian`, `withdrawal-api`.
      * `optimisation` — `ibm-quantum`. This is the only place in the module
        where an IBM destination can be declared, so no other zone can reach
        IBM by any spelling of any tfvars file.
      * every other zone — nothing. An entry naming one is refused.

    Ranges are not guessed here. They come from the counterparty's own
    connectivity documentation, and a deployment with this map empty reaches
    no external host at all — the correct state for connectivity nobody has
    confirmed.
  EOT

  type = map(object({
    zone    = string
    cidr    = string
    port    = number
    purpose = string
    note    = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for name in keys(var.external_egress) :
      can(regex("^[a-z][a-z0-9-]{1,23}[a-z0-9]$", name))
    ])
    error_message = "An egress entry name is lower case, starts with a letter, contains only letters, digits and hyphens, and is at most 25 characters; it becomes part of a firewall rule name."
  }

  validation {
    condition = alltrue([
      for entry in values(var.external_egress) :
      contains([
        "information-source",
        "venue",
        "venue-read",
        "chain",
        "custodian",
        "custodian-read",
        "withdrawal-api",
        "ibm-quantum",
      ], entry.purpose)
    ])
    error_message = "An egress purpose is one of information-source, venue, venue-read, chain, custodian, custodian-read, withdrawal-api, ibm-quantum. The purpose decides which zone may hold the entry, so an unrecognised one cannot be checked against anything."
  }

  validation {
    condition = alltrue([
      for entry in values(var.external_egress) :
      can(regex("^([0-9]{1,3}\\.){3}[0-9]{1,3}/[0-9]{1,2}$", entry.cidr))
    ])
    error_message = "An allowlist destination is an IPv4 CIDR — a host or the smallest range the counterparty publishes."
  }

  validation {
    condition = alltrue([
      for entry in values(var.external_egress) :
      entry.cidr != "0.0.0.0/0" && tonumber(split("/", entry.cidr)[1]) >= 24
    ])
    error_message = "An allowlist entry may be no broader than a /24, and 0.0.0.0/0 is not an allowlist entry at all. Name the addresses the counterparty publishes; a range chosen for convenience is the range that turns out to contain something else."
  }

  validation {
    condition = alltrue([
      for entry in values(var.external_egress) : entry.port > 0 && entry.port <= 65535
    ])
    error_message = "An egress port must be a port."
  }

  validation {
    condition = alltrue([
      for entry in values(var.external_egress) : trimspace(entry.note) != ""
    ])
    error_message = "Every allowlist entry carries a note naming the counterparty and the document the range came from. An entry whose provenance nobody recorded cannot be reviewed, only inherited."
  }
}

variable "public_ingress" {
  description = <<-EOT
    Where a client may arrive, keyed by a short name.

    Empty by default. An entry permits Google's load-balancing and health-check
    ranges to reach one zone on one port, and it is refused for any zone but
    `public-edge` and `application-identity` — the public edge, the static
    shell it serves, and the authenticated application APIs. A client never
    reaches Spanner, Pub/Sub, an execution node, a venue, IBM, custody or
    signing material, and the refusal is at plan time rather than in a review
    comment.

    This is also where customer traffic and trading traffic are kept apart. No
    trading zone can be given a load balancer here at all, so the two cannot
    share one, and since each zone holds its own identity and its own routes
    they share no credential and no path either.
  EOT

  type = map(object({
    zone = string
    port = number
    note = string
  }))

  default = {}

  validation {
    condition = alltrue([
      for name in keys(var.public_ingress) :
      can(regex("^[a-z][a-z0-9-]{1,23}[a-z0-9]$", name))
    ])
    error_message = "A public ingress name is lower case, starts with a letter, contains only letters, digits and hyphens, and is at most 25 characters; it becomes part of a firewall rule name."
  }

  validation {
    condition = alltrue([
      for entry in values(var.public_ingress) : entry.port > 0 && entry.port <= 65535
    ])
    error_message = "A public ingress port must be a port."
  }

  validation {
    condition = alltrue([
      for entry in values(var.public_ingress) : trimspace(entry.note) != ""
    ])
    error_message = "Every public ingress entry says what it serves. The one door on the platform is worth a sentence."
  }
}

variable "ledger_database" {
  description = <<-EOT
    The Spanner database the ledger zone holds, if this deployment has one.

    Null by default, and null means no ledger grant is made to any zone — a
    deployment whose ledger has not been provisioned gets no bindings rather
    than bindings against a database that does not exist.

    When it is set, the grants follow the declared path modes: a zone with a
    `read` path to the ledger gets `roles/spanner.databaseReader`, and one with
    an `append` path gets `roles/spanner.databaseUser`. The second is wider
    than the word `append` suggests and the module says so where it makes the
    binding; see NOT-ENFORCED-HERE.md.
  EOT

  type = object({
    instance = string
    database = string
  })

  default = null
}

variable "control_fabric_topic" {
  description = <<-EOT
    The Pub/Sub topic the control fabric ships payloads on, if it exists.

    Null by default, for the same reason as the ledger: no topic, no bindings.
    When set, a zone with a declared `publish` path to the control fabric gets
    `roles/pubsub.publisher` and nothing else, and the zone the fabric
    publishes to gets the attach-side grant only.
  EOT

  type    = string
  default = null
}
