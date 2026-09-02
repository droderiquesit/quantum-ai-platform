variable "project_id" {
  type = string
}

variable "environment" {
  type = string
}

variable "node_id" {
  description = <<-EOT
    The node's identifier, and the cell id the binary is configured with.

    An execution node stands in for exactly one cell: it reads `QIP_CELL_ID`
    and refuses a capital envelope addressed elsewhere, so a node whose id does
    not match the cell it is shadowing verifies nothing and rejects every grant
    it is sent. The two names are the same name.
  EOT

  type = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,26}[a-z0-9]$", var.node_id))
    error_message = "A node id is lower case, starts with a letter and contains only letters, digits and hyphens."
  }
}

variable "region" {
  description = "The region the node runs in. Chosen for its distance to the venues, not for convenience."
  type        = string
}

variable "zone" {
  description = <<-EOT
    The zone the node's managed instance group and its placement policy live in.

    Zonal rather than regional, and that is the whole point of the machine: a
    compact placement policy is a statement about physical proximity inside one
    zone, and a regional group would spread the replacements of a single node
    across failure domains whose inter-zone latency is the number the node
    exists to avoid.
  EOT

  type = string

  validation {
    condition     = startswith(var.zone, "${var.region}-")
    error_message = "The zone must be in the region: a node in ${var.region} with a zone elsewhere is a node whose subnet and instance group are in different places, and the apply fails after the subnet exists."
  }
}

variable "network_id" {
  type = string
}

variable "subnet_cidr" {
  description = <<-EOT
    The node's own range. Must not overlap a trust zone's, another node's or
    the primary subnet's.

    A range of its own rather than a share of the primary, for the reason
    every trust-zone subnet has one (`modules/trust-zones`): the firewall
    rules that decide what may reach this machine target a subnet and a tag,
    and a node drawn from a range something else also allocates from is a
    node those rules describe only by accident. The cluster this range once
    had to avoid colliding with is gone (ADR 0024); the reason to keep the
    range separate is not.
  EOT

  type = string
}

variable "machine_type" {
  description = <<-EOT
    The machine shape, chosen by venue count. No default, deliberately.

    §41.4 says "c3-highcpu-8 to -22 by venue count", which is a choice somebody
    makes with the venue list in front of them. A default here would be that
    choice made by whoever wrote this file, for a deployment they have not seen.

    The permitted set is the C3 and C3D high-CPU shapes inside that range and
    nothing else. `c3-highcpu-4` is below it, `c3-highcpu-44` is above it, and
    every other family — N2, C2, E2 — either has no Titanium offload or no
    TIER_1 networking, both of which the hot path depends on.

    Note what the small shapes cost you: §41.3 assigns threads to cores 2–15,
    which needs sixteen. On an eight-vCPU shape this module isolates cores 2–7
    and the assignment does not fit; see `isolated_cpus` in the outputs and the
    "What this module cannot enforce" section of the README.
  EOT

  type = string

  validation {
    condition = contains([
      "c3-highcpu-8",
      "c3-highcpu-22",
      "c3d-highcpu-8",
      "c3d-highcpu-16",
    ], var.machine_type)
    error_message = "The machine type must be one of c3-highcpu-8, c3-highcpu-22, c3d-highcpu-8, c3d-highcpu-16 — the C3 and C3D high-CPU shapes between 8 and 22 vCPU that §41.4 permits."
  }
}

variable "boot_image" {
  description = <<-EOT
    The self-link of the image the node boots, pinned to one image.

    Named exactly, never through a family. A family is a moving pointer, and a
    deployment that trusts one trusts whoever last pushed to it — the same
    argument the registry makes for `immutable_tags` and Binary Authorization
    makes for digests. Here it is stronger, because there is no container
    runtime and therefore no admission controller between this value and a
    process on the hot path.

    The image is the other half of this module. Everything in §41.4 that lives
    below the kernel command line — isolcpus, huge pages, no swap, no container
    runtime — is built into it, and the startup script verifies rather than
    creates them. See README.md.
  EOT

  type = string

  validation {
    condition     = !can(regex("/family/", var.boot_image))
    error_message = "An image family is a moving pointer, not an immutable image. Name the image itself."
  }
}

variable "node_count" {
  description = <<-EOT
    How many instances the group holds. One, per §41.4 — a single dedicated
    machine per region.

    Blue-green replacement does not need a second permanent instance: the
    update policy below surges one, proves it, and retires the old one. A
    standing pair is two machines holding venue sessions for the same cell,
    which is a duplicate-order problem rather than redundancy.
  EOT

  type    = number
  default = 1

  validation {
    condition     = var.node_count >= 1 && var.node_count <= 2
    error_message = "An execution node group holds one instance, or two only while a replacement is being observed."
  }
}

variable "shadow_mode" {
  description = <<-EOT
    Whether the node runs in shadow mode. True by default, and the default is
    the safe one.

    ADR 0020 step 3 requires a node to be observed before it takes sessions:
    "a node that takes venue sessions before it has been observed is the failure
    this whole ordering exists to avoid."

    Shadow mode here is not a flag the binary reads and could ignore. With it
    on, this module creates no venue egress rule and no venue-credential
    binding, so the node cannot open a venue session at all — it can reach the
    central plane and Google APIs and nothing else. Turning it off is what
    creates the path, which makes taking venue sessions a reviewed diff rather
    than a value in a config map.
  EOT

  type    = bool
  default = true
}

variable "venues" {
  description = <<-EOT
    The venues this node is configured for, keyed by venue identifier.

    Empty is not permitted and there is no default: `qip-edge-node` refuses to
    start with an empty `QIP_VENUES`, so a node deployed without this would
    boot, fail its preflight and restart for ever. The precondition in main.tf
    moves that to plan time.

    In shadow mode the node is configured for these venues and can reach none of
    them. That is the intended asymmetry: the deployment states what it would
    connect to, and the firewall still says no.

    The address ranges are not guessed here. They come from the venue's own
    connectivity documentation or from the extranet provider.
  EOT

  type = map(object({
    cidr = string
    port = number
  }))

  validation {
    condition = alltrue([
      for venue in var.venues : venue.cidr != "0.0.0.0/0" && venue.cidr != "::/0"
    ])
    error_message = "A venue range of the whole internet is not a venue range. Name the ranges the venue publishes."
  }

  validation {
    condition = alltrue([
      for venue in var.venues : venue.port > 0 && venue.port <= 65535
    ])
    error_message = "A venue port must be a port."
  }
}

variable "central_plane_ranges" {
  description = <<-EOT
    Where the central plane is.

    Empty by default: a node that cannot reach the centre keeps working inside
    the envelope it already holds (ADR 0008), so a missing value here degrades
    the deployment rather than opening it.
  EOT

  type    = list(string)
  default = []

  validation {
    condition = alltrue([
      for range in var.central_plane_ranges : range != "0.0.0.0/0"
    ])
    error_message = "The central plane is not the whole internet."
  }
}

variable "google_apis_range" {
  description = <<-EOT
    The range the node reaches Google APIs on, for the one egress rule that
    permits it.

    The restricted VIP by default — the same `199.36.153.8/30` the trust
    zones' Google-API egress rules name (`modules/trust-zones`), reached
    through the subnet's private Google access; the NetworkPolicies that once
    named it left with the chart (ADR 0024). Where a Private Service Connect endpoint
    exists instead (`modules/connectivity`), this is that endpoint's address as
    a /32, and the far end resolves Google API hostnames to it.
  EOT

  type    = string
  default = "199.36.153.8/30"

  validation {
    condition     = var.google_apis_range != "0.0.0.0/0"
    error_message = "Google's APIs are not the whole internet."
  }
}

variable "egress_bootstrap" {
  description = <<-EOT
    The Envoy bootstrap the node's proxy unit runs, as text: `modules/egress-proxy`
    publishes the one committed file for the Cloud Run sidecars and the root
    passes the same content here, so the node and the services cannot carry
    different allowlists.

    The dependency policy permits `serde` and `serde_json`, so
    `qip_transport::http` has no TLS stack and refuses the `https` scheme by
    name. Every outbound adapter in the binary therefore speaks to
    `http://127.0.0.1:910x`, and this bootstrap is what answers there. It
    must be a *reverse* proxy: `HttpRequest::encode` emits an origin-form
    request line and never `CONNECT`, so the destination is chosen by which
    listener the client connects to and cannot be named in the request.

    Required, with no default. A node with no proxy has no outbound HTTPS at
    all, and this module does not and must not solve that by adding TLS —
    that is a crypto dependency and an ADR, neither of which belongs here.
  EOT

  type = string

  validation {
    condition     = length(var.egress_bootstrap) > 1000 && strcontains(var.egress_bootstrap, "static_resources:")
    error_message = "The egress bootstrap is not an Envoy configuration. Pass `file(\"../egress/envoy.yaml\")` — the one committed bootstrap — not a path to it."
  }

  validation {
    condition     = !strcontains(var.egress_bootstrap, "address: 0.0.0.0")
    error_message = "The egress bootstrap binds a listener to 0.0.0.0. On the node every listener is loopback: a proxy reachable from the network is a proxy every neighbour can reach."
  }
}

variable "egress_endpoints" {
  description = <<-EOT
    The addresses the binary's outbound adapters are configured with, keyed
    by listener name, from `modules/egress-proxy`. Every value is
    `http://127.0.0.1:<port>`, and the validation refuses anything else: an
    `https` endpoint is an adapter that refuses at construction, and an
    address off the machine is a proxy this node was never given a rule to
    reach.
  EOT

  type = map(string)

  validation {
    condition     = alltrue([for endpoint in values(var.egress_endpoints) : startswith(endpoint, "http://127.0.0.1:")])
    error_message = "Every egress endpoint is http://127.0.0.1:<port>. `qip_transport::http` refuses https by name, and the proxy lives on this machine."
  }

  validation {
    condition     = contains(keys(var.egress_endpoints), "gcp")
    error_message = "The egress endpoints name no `gcp` listener, which is the one QIP_GCP_ENDPOINT is configured from."
  }
}

variable "capital_envelope_secret_id" {
  description = <<-EOT
    The Secret Manager secret holding the key the node verifies capital
    envelopes against.

    Required, because `qip-edge-node` will not start without it. Held as a
    secret for its integrity rather than its confidentiality: somebody who can
    replace it can mint envelopes.
  EOT

  type = string
}

variable "venue_credential_secret_id" {
  description = <<-EOT
    The Secret Manager secret holding the venue credential, when the deployment
    has one at all.

    Null by default, which is a node that is granted nothing. Naming it is not
    enough to make it readable — see `venue_credential_readable`.
  EOT

  type    = string
  default = null
}

variable "venue_credential_readable" {
  description = <<-EOT
    Whether the venue credential may be read at all in this environment.

    True only where the environment's ceiling could actually use the
    credential. False by default, and the root module computes it as a
    membership test over the three live rungs — never as a negation of
    `paper_trading`, which is true for `observation` and `advisory` too and so
    grants the credential to precisely the ceilings that are furthest from
    needing it. The root passes the same value to `modules/secrets`.

    This module deliberately does not accept the ceiling itself: an
    execution node that could name its own autonomy level would be a fourth
    place the paper-trading boundary is decided, and the boundary is worth more
    with three places than with four.

    The binding this gates additionally requires `shadow_mode = false`. A node
    nobody has observed yet has no business holding a credential it could
    authenticate with, whatever the environment's ceiling says.
  EOT

  type    = bool
  default = false
}

variable "evidence_bucket" {
  description = <<-EOT
    The write-once evidence bucket, when this node writes to it. Null by
    default, which grants nothing.

    Where it is set the node is granted object creation and nothing else — the
    same narrow role every other writer holds, for the reason `modules/evidence`
    gives: an append-only store whose writer can delete is a store nobody has
    deleted from yet.
  EOT

  type    = string
  default = null
}

variable "health_port" {
  description = "The port `qip-edge-node` serves its health surface on, and the only port anything may open to the node."
  type        = number
  default     = 8080

  validation {
    condition     = var.health_port > 0 && var.health_port <= 65535
    error_message = "A health port must be a port."
  }
}

variable "watchdog_seconds" {
  description = <<-EOT
    The systemd watchdog interval, or zero for no watchdog. Zero by default,
    and the default is an honest one rather than a cautious one.

    §41.4 asks for a watchdog. A watchdog is a contract with the supervised
    process: systemd expects `sd_notify(WATCHDOG=1)` within the interval and
    kills the process when it does not arrive. `qip-edge-node` sends no such
    notification today, so setting this against the current binary produces a
    kill and a restart every interval for ever — a supervision setting that
    turns a healthy process into a crash loop, which is worse than no watchdog
    because the console shows a unit systemd is diligently restarting.

    Set it when the image ships a binary that pings. Until then this module
    provides `Restart=always`, which is the half of §41.4's supervision line the
    current binary can actually honour.
  EOT

  type    = number
  default = 0

  validation {
    condition     = var.watchdog_seconds >= 0 && var.watchdog_seconds <= 300
    error_message = "A watchdog interval is between zero (off) and 300 seconds."
  }
}

variable "required_hugepages_gb" {
  description = <<-EOT
    How many gigabytes of huge pages the image must have preallocated before
    the node is allowed to serve.

    §41.4 requires preallocated huge pages and `mlockall`. Terraform cannot
    allocate a huge page; the kernel command line in the image does, and the
    startup script here refuses to start the unit when the machine came up
    without them. One gigabyte by default because that is the smallest amount
    that proves the image was built for this rather than picked up from a
    catalogue.
  EOT

  type    = number
  default = 1

  validation {
    condition     = var.required_hugepages_gb >= 1
    error_message = "Requiring zero huge pages is not a requirement. If the image genuinely does not preallocate them, that is an image to fix, not a check to disable."
  }
}

variable "create_egress_nat" {
  description = <<-EOT
    Whether this module creates the Cloud Router and NAT the node's egress
    leaves through. False by default.

    §41.4 says Cloud NAT for egress, and `modules/network` already provisions
    one — but a NAT is regional and that one covers all ranges in the primary
    region only. So: leave this false when the node runs in the primary region,
    because a second NAT covering the same subnetworks in the same region is a
    conflicting configuration the API rejects at apply; set it true when the
    node runs in a region that has no NAT of its own.

    False is also the closed default. With no NAT and no external address, the
    node's only routes off the machine are the ones the firewall rules below
    name explicitly, and everything else fails to connect rather than leaving.
  EOT

  type    = bool
  default = false
}

variable "labels" {
  type    = map(string)
  default = {}
}
