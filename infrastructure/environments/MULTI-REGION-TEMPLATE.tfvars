# Multi-region deployment template
#
# This file demonstrates how to configure execution nodes across multiple regions.
# Copy this to environments/<env>/terraform.tfvars and customize for your deployment.
#
# IMPORTANT: Each region is a separate ADR decision. Deploy one region first,
# observe for 7+ days, then propose each additional region in a new ADR with evidence.
#
# Current state: Only dev:us-east4 is authorised (ADR 0035).

# --- The project ---

project_id     = "algorik-platform-prod"
project_number = 123456789012
environment    = "prod"
region         = "us-east4"  # Default region for resources without an explicit region

autonomy_ceiling = "paper_trading"

# --- Trust zones (blueprint §46.1) ---
#
# Each region adds trust zone entries. These are shared across all regions for
# a given environment (they live in the same VPC). A single environment cannot
# have trust zones with the same name in different regions; Google enforces
# regional subnets.

trust_zones = {
  # us-east4 trust zones
  "application-identity" = {
    region      = "us-east4"
    subnet_cidr = "10.0.32.0/24"
  }
  "cognition" = {
    region      = "us-east4"
    subnet_cidr = "10.0.33.0/24"
  }
  "intelligence" = {
    region      = "us-east4"
    subnet_cidr = "10.0.34.0/24"
  }
  "management" = {
    region      = "us-east4"
    subnet_cidr = "10.0.35.0/24"
  }

  # europe-west2 trust zones (London)
  "application-identity-london" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.48.0/24"
  }
  "cognition-london" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.49.0/24"
  }
  "intelligence-london" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.50.0/24"
  }
}

# --- Paths between zones ---
#
# If you have strategies deployed in multiple regions that need to coordinate,
# or if cells need to replicate state, declare the paths here.
#
# Current state: No inter-region communication. Each region's centre and cell
# are independent.

permitted_paths = {}

# Example: Allow cell-to-cell state replication
# permitted_paths = {
#   "newyork-to-london-mesh" = {
#     from  = "execution-nodes"     # Tag on newyork-1
#     to    = "execution-nodes"     # Tag on london-1
#     mode  = "allow"
#     ports = [9100]                # Custom mesh port
#     note  = "Cross-region peer state replication"
#   }
# }

# --- External egress ---
#
# Declare every destination outside the VPC. The egress proxy checks these
# against its bootstrap configuration and the acceptance suite enforces they match.
#
# Current state: No external destinations (all simulation). Add entries when
# a real market data vendor or venue connectivity is decided.

external_egress = {}

# Example: Market data feed
# external_egress = {
#   "ecb-rates" = {
#     zone    = "management"
#     cidr    = "185.215.0.0/16"        # ECB IP ranges (example)
#     port    = 443
#     purpose = "market-data"
#     note    = "ECB reference rate feed for FX reference ledger"
#   }
# }

# --- Public ingress ---
#
# Where customers or operators reach the platform. The console (portal) uses
# Identity-Aware Proxy on Cloud Run; this is for publicly-facing APIs only.
#
# Current state: None. No API is published to the internet.

public_ingress = {}

# --- GitOps control plane (ADR 0036) ---
#
# Shared across all regions in this environment. Only one GKE cluster.

gitops_enabled                = true
gitops_master_ipv4_cidr_block = "10.0.36.0/28"

# --- Execution nodes (the edge plane) ---
#
# One per region. Each represents a distinct regional cell.
#
# FIELDS:
#
# - `region`, `zone`: Chosen based on venue proximity and availability
# - `subnet_cidr`: From the execution node ladder (environments/README.md)
#                 us-east4:     10.64.0.0/16 upward
#                 europe-west2: 10.68.0.0/16 upward
# - `machine_type`: c3-highcpu-8, -16, -22 or c3d-highcpu-8, -16 per venue count
# - `boot_image`: Self-link of the baked image (no image families)
# - `region_allocation`: Notional capital envelope (base currency)
# - `venues`: Map of venue_id → {cidr, port}
# - `shadow_mode`: Always true on first deployment (ADR 0035)
# - `create_egress_nat`: true for venues on public internet; false for simulated
# - `default_pricing`: Strategy id to use for order pricing (empty = no pricing)
# - `strategy_plan_path`: Path to compiled strategy plan (empty = no plan)
# - `cross_region_mirror_path`: Path to cross-region mirror rules (empty = no mirroring)

execution_nodes = {
  # us-east4 (Ashburn) — closest to NY/NJ venues
  "newyork-1" = {
    region           = "us-east4"
    zone             = "us-east4-b"
    subnet_cidr      = "10.64.0.0/24"
    machine_type     = "c3-highcpu-16"
    boot_image       = "projects/algorik-platform-prod/global/images/qip-edge-node-us-east4-20261006-abc123"
    region_allocation = "500000.00"

    venues = {
      # Simulated venue (no internet route needed)
      "simulated" = { cidr = "127.0.0.1/32", port = 9001 }
    }

    shadow_mode              = true
    create_egress_nat        = false    # Simulated venue; no internet
    default_pricing          = ""
    strategy_plan_path       = ""
    cross_region_mirror_path = ""
  }

  # europe-west2 (London) — closest to European venues
  # Uncomment and configure after newyork-1 has been observed for 7+ days.
  #
  # "london-1" = {
  #   region           = "europe-west2"
  #   zone             = "europe-west2-a"
  #   subnet_cidr      = "10.68.0.0/24"
  #   machine_type     = "c3-highcpu-16"
  #   boot_image       = "projects/algorik-platform-prod/global/images/qip-edge-node-europe-west2-20261006-xyz789"
  #   region_allocation = "500000.00"
  #
  #   venues = {
  #     "simulated" = { cidr = "127.0.0.1/32", port = 9001 }
  #   }
  #
  #   shadow_mode              = true
  #   create_egress_nat        = false
  #   default_pricing          = ""
  #   strategy_plan_path       = ""
  #   cross_region_mirror_path = ""
  # }

  # us-west1 (Oregon) — closest to US West Coast venues
  # Uncomment and configure after london-1 has been observed for 7+ days.
  #
  # "oakland-1" = {
  #   region           = "us-west1"
  #   zone             = "us-west1-b"
  #   subnet_cidr      = "10.69.0.0/24"
  #   machine_type     = "c3-highcpu-16"
  #   boot_image       = "projects/algorik-platform-prod/global/images/qip-edge-node-us-west1-20261006-vwx456"
  #   region_allocation = "250000.00"
  #
  #   venues = {
  #     "simulated" = { cidr = "127.0.0.1/32", port = 9001 }
  #   }
  #
  #   shadow_mode              = true
  #   create_egress_nat        = false
  #   default_pricing          = ""
  #   strategy_plan_path       = ""
  #   cross_region_mirror_path = ""
  # }
}

# --- Image baking ---
#
# Subnet for the temporary builder VM (instance.yml workflow).
# Commented out by default; uncomment when baking an image.
#
# When enabled, the subnet must:
# - Not overlap any trust zone or execution node subnet
# - Be a /28 (28–30 addresses; instance.yml uses one)
# - Be in the same region as the default `region` above

# image_bake_subnet_cidr = "10.1.0.0/28"

# --- Observability ---

workload_metrics_exist = false  # Flip to true when ingestion is proven

metrics_collector_image_digest = null  # null = no sidecar on Cloud Run

vendored_openobserve_image_digest = null  # null = OpenObserve not deployed

# --- Other settings ---

storage_target           = "memory"
cycle_interval_seconds   = "300"
market_data_connector    = null
deepbrain_connector      = null
venue_registrations_file = null
wallet_statement_file    = null
capital_fabric_file      = null
central_horizons_file    = null
risk_limits_file         = null
source_candidates_file   = null

enable_bigquery              = false
enable_cloud_storage         = false
enable_alloydb               = false
enable_bigtable              = false
enable_memorystore           = false
enable_spanner               = false
enable_vertex_ai             = false
enable_partner_interconnect  = false
enable_private_service_connect = false

snapshot_start_time  = "05:00"
snapshot_retain_days = 90

billing_budget_enabled = false

github_repository = "droderiquesit/quantum-ai-platform"

public_edge = {}
