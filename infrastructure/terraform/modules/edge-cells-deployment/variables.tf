variable "execution_nodes" {
  description = <<-EOT
    The execution nodes to deploy, keyed by node id.

    Each node configuration includes:
      region: The region the node runs in (e.g., "us-east4")
      zone: The zone within that region (must start with the region)
      subnet_cidr: The /24 subnet for this node (must not overlap other nodes or zones)
      node_count: 0 (provisioned, not running) or 1 (running with blue-green reserve)
      machine_type: c3-highcpu-8, c3-highcpu-22, c3d-highcpu-8, c3d-highcpu-16
      shadow_mode: true (no venue connectivity) or false (venue rules created)
      health_port: Port the health endpoint listens on (default 8080)
      watchdog_seconds: Systemd watchdog interval (0 = off, until binary supports it)
      venues: Map of venue configs (cidr, port)
      region_allocation: Capital ceiling for this region in decimal form (e.g., "250000")
      isolated_cpus: The range of cores to isolate (derived from machine_type by default)
      create_egress_nat: Whether to create a Cloud NAT for this node's egress

    Example:
      {
        "cell-us-east4" = {
          region         = "us-east4"
          zone           = "us-east4-a"
          subnet_cidr    = "10.240.0.0/24"
          node_count     = 1
          machine_type   = "c3-highcpu-22"
          shadow_mode    = true
          health_port    = 8080
          watchdog_seconds = 0
          venues = {
            "sim" = { cidr = "10.0.0.0/8", port = 443 }
          }
          region_allocation = "500000"
          isolated_cpus = "2-21"
          create_egress_nat = false
        }
      }
  EOT

  type = map(object({
    region            = string
    zone              = string
    subnet_cidr       = string
    node_count        = number
    machine_type      = string
    shadow_mode       = bool
    health_port       = optional(number, 8080)
    watchdog_seconds  = optional(number, 0)
    venues            = map(object({ cidr = string, port = number }))
    region_allocation = string
    isolated_cpus     = optional(string, "2-21")
    create_egress_nat = optional(bool, false)
  }))

  validation {
    condition = alltrue([
      for node_id, config in var.execution_nodes : (
        config.node_count == 0 || config.node_count == 1 || config.node_count == 2
      )
    ])
    error_message = "node_count must be 0 (provisioned), 1 (running), or 2 (blue-green replacement in progress)."
  }

  validation {
    condition = alltrue([
      for node_id, config in var.execution_nodes : startswith(config.zone, config.region)
    ])
    error_message = "Each zone must be in its region (e.g., us-east4-a is in us-east4)."
  }

  validation {
    condition = alltrue([
      for node_id, config in var.execution_nodes : length(config.venues) > 0
    ])
    error_message = "Each node must be configured for at least one venue."
  }

  # A regional capital ceiling is a positive decimal string. `tonumber`
  # refuses a non-numeric string with an error no `error_message` can report,
  # so it is wrapped in `try`, which turns that refusal into this one.
  validation {
    condition = alltrue([
      for node_id, config in var.execution_nodes : try(tonumber(config.region_allocation) > 0, false)
    ])
    error_message = "Each region_allocation must be a positive decimal number, such as \"250000\"; zero, a negative, or a non-number is refused rather than read as no capital."
  }
}

variable "trust_zones" {
  description = <<-EOT
    The trust zones where the central plane's workloads run, keyed by zone name.

    Each zone has a subnet CIDR; cells use these ranges to permit central plane
    ingress to their health ports.
  EOT

  type = map(object({
    name        = string
    subnet_cidr = string
  }))
}

variable "cross_region_mirrors" {
  description = <<-EOT
    Cross-region venue mirroring configuration (ADR 0039, §31.1).

    When a cell trades a venue in another region, it sends orders to a mirror
    point in the destination region. The mirror point executes locally and sends
    fills back.

    Each entry specifies:
      from_region: The region with the original venue
      to_region: The region that mirrors it
      rtt_ms: Measured round-trip latency between regions
      inventory_band_pct: Inventory band as a percentage (e.g., 2 for 2%)
      dislocation_threshold_pct: Dislocation threshold as a percentage

    Example:
      [
        {
          from_region = "us-east4"
          to_region = "us-west1"
          rtt_ms = 42
          inventory_band_pct = 2
          dislocation_threshold_pct = 10
        }
      ]
  EOT

  type = list(object({
    from_region               = string
    to_region                 = string
    rtt_ms                    = number
    inventory_band_pct        = number
    dislocation_threshold_pct = number
  }))

  default = []
}

variable "google_apis_range" {
  description = "The CIDR range for Google APIs (default: the restricted VIP)."
  type        = string
  default     = "199.36.153.8/30"
}

