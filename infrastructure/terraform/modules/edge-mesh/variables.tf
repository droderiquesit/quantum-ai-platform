variable "project_id" {
  description = "The project every resource here is created in."
  type        = string
}

variable "environment" {
  description = "The deployment environment — dev, test, stage, or prod."
  type        = string
}

variable "network_id" {
  description = "The VPC network all cells attach to."
  type        = string
}

variable "execution_nodes" {
  description = <<-EOT
    The execution nodes deployed in this environment, keyed by node id.

    Each node config includes:
      region: The region it runs in
      zone: The zone within that region
      subnet_cidr: The /24 subnet the node attaches to
      health_port: The port its health endpoint uses
      shadow_mode: Whether the node runs in shadow mode (true = cannot reach venues)
      venues: The venues it is configured for (keyed by venue id)

    When a node is not in shadow mode, mesh connectivity rules are created.
  EOT

  type = map(object({
    region      = string
    zone        = string
    subnet_cidr = string
    health_port = number
    shadow_mode = bool
    venues      = map(object({ cidr = string, port = number }))
  }))

  validation {
    condition = alltrue([
      for node_id, config in var.execution_nodes : length(config.venues) > 0
    ])
    error_message = "Every node must be configured for at least one venue."
  }
}

variable "central_plane_ranges" {
  description = <<-EOT
    The CIDR ranges where the central plane workloads run.

    These are the subnets from the trust zones plus the private Google APIs
    endpoint. Cells that are not in shadow mode create ingress rules allowing
    the central plane to reach their health port.
  EOT

  type = list(string)

  validation {
    condition = alltrue([
      for range in var.central_plane_ranges : range != "0.0.0.0/0"
    ])
    error_message = "The central plane ranges must be specific; the whole internet is not permitted."
  }
}

variable "cross_region_mirrors" {
  description = <<-EOT
    Cross-region mirroring configuration (ADR 0039, §31.1).

    Each entry names a region whose capital is mirrored into another region,
    including the measured round-trip latency and the inventory parameters
    (band and dislocation threshold).

    The format is:
      {
        from_region = "us-east4"
        to_region   = "us-west1"
        rtt_ms      = 42
        inventory_band_pct = 2
        dislocation_threshold_pct = 10
      }

    When this is non-empty, cells create cross-region communication paths.
    The presence of an entry does not create routes; it informs the cell what
    to expect when it starts.
  EOT

  type = list(object({
    from_region               = string
    to_region                 = string
    rtt_ms                    = number
    inventory_band_pct        = number
    dislocation_threshold_pct = number
  }))

  default = []

  validation {
    condition = alltrue([
      for mirror in var.cross_region_mirrors :
      mirror.from_region != mirror.to_region
      && mirror.rtt_ms > 0
      && mirror.inventory_band_pct > 0
      && mirror.dislocation_threshold_pct > 0
    ])
    error_message = "Each cross-region mirror must connect different regions with positive latency and parameters."
  }
}

variable "psc_endpoint_addresses" {
  description = <<-EOT
    The private internal addresses for Private Service Connect endpoints,
    keyed by region.

    A cell in one region reaches a cell in another through this endpoint's
    address when the regions are distant. Addresses must not collide with
    any subnet CIDR in the VPC.

    Example: { "us-east4" = "10.255.0.1", "us-west1" = "10.255.0.2" }
  EOT

  type = map(string)

  validation {
    condition = alltrue([
      for addr in values(var.psc_endpoint_addresses) :
      can(regex("^([0-9]{1,3}\\.){3}[0-9]{1,3}$", addr))
    ])
    error_message = "Each PSC endpoint address must be a valid IPv4 address."
  }
}

variable "labels" {
  description = "Labels to apply to all resources."
  type        = map(string)
  default     = {}
}
