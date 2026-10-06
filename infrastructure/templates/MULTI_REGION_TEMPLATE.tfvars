# Multi-region edge cell deployment template.
#
# This file shows how to configure seven regional edge cells, staged from
# shadow mode through observation to venue paths opened. Every cell is
# paper-only throughout (ADR 0003): leaving shadow mode opens paths to the
# simulated venue and provider sandboxes, never to a live order route. Every
# cell is independent until capital mirrors are configured.
#
# This is a template, not an environment. It lives outside
# infrastructure/environments/ because `infra.yml` parses every tfvars file
# there as one of the four environments.
#
# See infrastructure/EDGE_CELL_DEPLOYMENT_GUIDE.md for the deployment sequence.

# ============================================================================
# EXECUTION NODES: One per region
# ============================================================================
#
# The blueprint calls for seven regional cells. Each is configured here with:
# - region/zone: Where it runs
# - subnet_cidr: A /24 for its own network interface
# - machine_type: c3 or c3d high-CPU shape
# - shadow_mode: true until observed, then false
# - venues: Where the cell can trade (empty only if shadow_mode = true)
# - region_allocation: Capital ceiling for this region

execution_nodes = {
  # PRIMARY REGION: Deployed first, observed, exit shadow mode first
  #
  # us-east4 is the primary trading region. This cell is the reference for
  # all other cells' decisions and the reconciliation point for cross-region
  # mirrors. Deployed first, leaves shadow mode first once observed.
  "cell-us-east4" = {
    region           = "us-east4"
    zone             = "us-east4-a"
    subnet_cidr      = "10.240.0.0/24"
    node_count       = 1 # Start with 1; 0 = provisioned but not running
    machine_type     = "c3-highcpu-22"
    shadow_mode      = false # Exited shadow mode after observation
    health_port      = 8080
    watchdog_seconds = 0 # Until qip-edge-node implements sd_notify
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
      # Only simulated venues and provider sandboxes belong here; the
      # platform does not trade live (ADR 0003).
    }
    region_allocation = "500000" # $500k base allocation
    isolated_cpus     = "2-21"
    create_egress_nat = false
  }

  # SECONDARY REGIONS: Deployed in shadow mode, observed, exit when stable
  #
  # These regions trade the same instruments but are geographically distant.
  # Each is deployed in shadow mode, observed for 2-4 weeks against the
  # primary region's GKE pod, then exits shadow mode independently.

  "cell-us-west1" = {
    region           = "us-west1"
    zone             = "us-west1-a"
    subnet_cidr      = "10.241.0.0/24"
    node_count       = 1
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true # Shadow mode until observed
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "250000" # Smaller allocation in secondary region
    isolated_cpus     = "2-21"
    create_egress_nat = false
  }

  "cell-europe-west1" = {
    region           = "europe-west1"
    zone             = "europe-west1-b"
    subnet_cidr      = "10.242.0.0/24"
    node_count       = 1
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "250000"
    isolated_cpus     = "2-21"
    create_egress_nat = false
  }

  # COLOCATED REGIONS: Not in GCP; connected via partner interconnect
  #
  # These regions are not in Google Cloud (Chicago, New York, Dubai).
  # They run on Compute Engine in nearby GCP regions, connected back to
  # their venues via partner interconnect. See modules/connectivity for the
  # network topology.

  "cell-chicago" = {
    region           = "us-central1" # GCP region (nearest to Chicago)
    zone             = "us-central1-a"
    subnet_cidr      = "10.243.0.0/24"
    node_count       = 0 # Provisioned, not running, until venues are ready
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "200000"
    isolated_cpus     = "2-21"
    create_egress_nat = true # Colocated venues need external egress
  }

  "cell-newyork" = {
    region           = "us-east4" # Shares region with primary, own subnet
    zone             = "us-east4-b"
    subnet_cidr      = "10.244.0.0/24"
    node_count       = 0 # Provisioned only
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "200000"
    isolated_cpus     = "2-21"
    create_egress_nat = true
  }

  "cell-dubai" = {
    region           = "me-central1"
    zone             = "me-central1-a"
    subnet_cidr      = "10.245.0.0/24"
    node_count       = 0 # Provisioned only
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "150000"
    isolated_cpus     = "2-21"
    create_egress_nat = true
  }

  # ADDITIONAL REGIONS: Asia-Pacific (future expansion)
  #
  # Placeholder configurations for Asia-Pacific regions, deployed when venues
  # in those regions are registered.

  "cell-asia-southeast1" = {
    region           = "asia-southeast1"
    zone             = "asia-southeast1-a"
    subnet_cidr      = "10.246.0.0/24"
    node_count       = 0 # Not yet deployed
    machine_type     = "c3-highcpu-22"
    shadow_mode      = true
    health_port      = 8080
    watchdog_seconds = 0
    venues = {
      "sim" = { cidr = "10.0.0.0/8", port = 443 }
    }
    region_allocation = "200000"
    isolated_cpus     = "2-21"
    create_egress_nat = true
  }
}

# ============================================================================
# CROSS-REGION MIRRORS: Capital sharing and coordination (ADR 0039, §31.1)
# ============================================================================
#
# When a cell trades a venue in another region, it sends orders to the cell
# in that region (the "mirror point") and receives fills back. The mirror
# point coordinates capital allocation and confirms each fill.
#
# Each entry names:
# - from_region: The region with the original venue
# - to_region: The region that mirrors it
# - rtt_ms: Measured round-trip latency between regions
# - inventory_band_pct: Inventory band as a percentage (2% = 0.02)
# - dislocation_threshold_pct: Dislocation threshold (10% = 0.10)
#
# Populate these as venues are registered. A cell reads its full mirror config
# from the file at `cross_region_mirror_path` (see node.env below).

cross_region_mirrors = [
  # East Coast to West Coast
  {
    from_region               = "us-east4"
    to_region                 = "us-west1"
    rtt_ms                    = 42
    inventory_band_pct        = 2
    dislocation_threshold_pct = 10
  },
  # East Coast to Europe
  {
    from_region               = "us-east4"
    to_region                 = "europe-west1"
    rtt_ms                    = 90
    inventory_band_pct        = 3
    dislocation_threshold_pct = 15
  },
  # Primary to secondary regions
  {
    from_region               = "us-central1" # Chicago
    to_region                 = "us-east4"
    rtt_ms                    = 20
    inventory_band_pct        = 1
    dislocation_threshold_pct = 5
  },
  # Europe to Asia
  {
    from_region               = "europe-west1"
    to_region                 = "asia-southeast1"
    rtt_ms                    = 180
    inventory_band_pct        = 5
    dislocation_threshold_pct = 25
  }
]

# ============================================================================
# CONFIGURATION FILES: Paths each node reads at boot
# ============================================================================
#
# The startup script in modules/execution-node writes these paths into
# the node's systemd EnvironmentFile (node.env). Each path is absolute,
# pointing to a file in the boot image, a mounted disk, or a bucket.

# Strategy plan: Compiled strategy IR the node runs.
# Empty if no strategy is to be deployed.
strategy_plan_path = ""

# Cross-region mirror config: YAML describing mirror points and latencies.
# Path structure:
#   from_region: us-east4
#   mirrors:
#     - to_region: us-west1
#       rtt_ms: 42
#       inventory_band_pct: 0.02
#       dislocation_threshold_pct: 0.10
# Empty if no cross-region trading is configured.
cross_region_mirror_path = ""

# Default pricing mode for all strategies:
# - "": No default, each strategy is priced by its own config
# - "marketable": Price at the bid/ask spread
# - "rest-at-mid:30": Rest at mid price, withdraw after 30 seconds
default_pricing = ""

# ============================================================================
# BOOT IMAGE: Required — pinned to one image, never a family
# ============================================================================
#
# The self-link of the GCP image all nodes boot from. This is created by the
# image bake workflow (see .github/workflows/image.yml) and must be pinned
# to a specific image, never a family.
#
# To generate a new image:
#   gh workflow run image.yml -f region=us-east4
#
# The workflow outputs the image self-link. Paste it here:

boot_image = "projects/algorik-dev/global/images/qip-edge-20240101-120000"

# ============================================================================
# EVIDENCE AND OBSERVABILITY
# ============================================================================
#
# Write-once evidence bucket for operation logs and recovery data.
# Create this bucket manually with versioning enabled.
evidence_bucket = null # "gs://algorik-dev-evidence"
