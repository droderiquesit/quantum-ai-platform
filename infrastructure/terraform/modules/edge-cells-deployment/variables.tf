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
    region              = string
    zone                = string
    subnet_cidr         = string
    node_count          = number
    machine_type        = string
    shadow_mode         = bool
    health_port         = optional(number, 8080)
    watchdog_seconds    = optional(number, 0)
    venues              = map(object({ cidr = string, port = number }))
    region_allocation   = string
    isolated_cpus       = optional(string, "2-21")
    create_egress_nat   = optional(bool, false)
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
    from_region              = string
    to_region                = string
    rtt_ms                   = number
    inventory_band_pct       = number
    dislocation_threshold_pct = number
  }))

  default = []
}

variable "boot_image" {
  description = "The self-link of the GCP image all nodes boot from, pinned to one image (not a family)."
  type        = string
}

variable "capital_envelope_secret_id" {
  description = "Secret Manager secret id holding the key nodes verify capital envelopes against."
  type        = string
}

variable "venue_credential_secret_id" {
  description = "Secret Manager secret id holding the venue credential, or null if not applicable."
  type        = string
  default     = null
}

variable "venue_credential_readable" {
  description = <<-EOT
    Whether the venue credential is readable in this environment.

    True only in environments whose autonomy ceiling could use it (the three
    live rungs). For observation and advisory, this is false and nodes cannot
    reach live venues even if shadow_mode is turned off.
  EOT

  type    = bool
  default = false
}

variable "evidence_bucket" {
  description = "GCS bucket for writing evidence, or null if not configured."
  type        = string
  default     = null
}

variable "telemetry_endpoint" {
  description = "The endpoint where telemetry is scraped (for health check documentation)."
  type        = string
  default     = "/metrics"
}

variable "egress_bootstrap" {
  description = "The Envoy bootstrap configuration for the node's proxy sidecar."
  type        = string
}

variable "egress_endpoints" {
  description = "Egress proxy listener endpoints, keyed by listener name (e.g., { gcp = \"http://127.0.0.1:9101\" })."
  type        = map(string)
}

variable "google_apis_range" {
  description = "The CIDR range for Google APIs (default: the restricted VIP)."
  type        = string
  default     = "199.36.153.8/30"
}

variable "default_pricing" {
  description = "Default pricing mode for all strategies deployed on this node (e.g., 'marketable' or 'rest-at-mid:30')."
  type        = string
  default     = ""
}

variable "strategy_plan_path" {
  description = "Path to the compiled strategy plan the node reads at boot (or empty if none)."
  type        = string
  default     = ""
}

variable "cross_region_mirror_path" {
  description = "Path to the cross-region mirror configuration file (or empty if none)."
  type        = string
  default     = ""
}

variable "required_hugepages_gb" {
  description = "Gigabytes of huge pages the boot image must have preallocated."
  type        = number
  default     = 1
}

variable "labels" {
  description = "Labels to apply to all resources."
  type        = map(string)
  default     = {}
}
