# Multi-region edge cell deployment orchestration.
#
# This module composes execution nodes and the edge mesh into a coordinated
# system. A cell is one execution node, one subnetwork, and one set of firewall
# rules. Seven cells means seven nodes, seven subnets, and rules that isolate
# them until venues are configured.
#
# ADR 0008 decides that cells are independent and only share capital through
# grants, not through coordination. ADR 0039 then asks: how does a cell know
# its share when a capital envelope spans multiple regions?
# Answer: the cell is told at boot, reads it from `node.env`, and starts with
# that share. The centre updates via capital envelope rotations; a cell that
# has lost the centre keeps trading inside the share it was given.

locals {
  prefix = "qip-${var.environment}"

  # Derived configuration for the edge mesh. This module gathers data from
  # the execution nodes and passes them through to edge-mesh so both use the
  # same source of truth.
  mesh_execution_nodes = var.execution_nodes

  mesh_central_plane_ranges = concat(
    [
      for zone_name in distinct([for workload in var.trust_zones : workload.name]) :
      var.trust_zones[zone_name].subnet_cidr
      if contains(keys(var.trust_zones), zone_name)
    ],
    [var.google_apis_range]
  )

  # PSC endpoint addresses, one per region where a node is deployed. Addresses
  # are in the 10.255.0.0/24 range, never overlapping with any zone or node
  # subnet, and assigned deterministically so the far end can compute them.
  psc_addresses = {
    for idx, region in sort(distinct([for config in var.execution_nodes : config.region])) :
    region => "10.255.0.${idx + 1}"
  }
}

# --- Execution nodes --------------------------------------------------------
#
# One per region. Each node:
# - Runs the qip-edge-node binary in shadow mode (default) or live (after observation)
# - Has its own subnet, tagged firewall rules, and service account
# - Verifies capital envelopes before accepting them
# - Reports health, telemetry and evidence to the platform

module "execution_node" {
  for_each = var.execution_nodes

  source = "../execution-node"

  project_id  = var.project_id
  environment = var.environment
  node_id     = each.key

  # Location
  region = each.value.region
  zone   = each.value.zone

  # Networking
  network_id           = var.network_id
  subnet_cidr          = each.value.subnet_cidr
  central_plane_ranges = local.mesh_central_plane_ranges
  google_apis_range    = var.google_apis_range

  # Compute shape
  machine_type = each.value.machine_type
  node_count   = each.value.node_count

  # Image contract
  boot_image              = var.boot_image
  required_hugepages_gb   = var.required_hugepages_gb
  isolated_cpus           = each.value.isolated_cpus
  health_port             = each.value.health_port
  watchdog_seconds        = each.value.watchdog_seconds

  # Venues and trading
  venues                  = each.value.venues
  shadow_mode             = each.value.shadow_mode
  venue_credential_secret_id = var.venue_credential_secret_id
  venue_credential_readable  = var.venue_credential_readable

  # Configuration
  default_pricing         = var.default_pricing
  strategy_plan_path      = var.strategy_plan_path
  cross_region_mirror_path = var.cross_region_mirror_path
  region_allocation       = each.value.region_allocation

  # Capital envelope verification
  capital_envelope_secret_id = var.capital_envelope_secret_id

  # Observability and evidence
  health_port           = each.value.health_port
  evidence_bucket       = var.evidence_bucket
  telemetry_endpoint    = var.telemetry_endpoint

  # Egress proxy
  egress_bootstrap = var.egress_bootstrap
  egress_endpoints = var.egress_endpoints

  # Networking options
  create_egress_nat = each.value.create_egress_nat

  labels = var.labels
}

# --- Edge mesh connectivity ------------------------------------------------
#
# Creates the paths between cells and from cells to the central plane.
# In shadow mode, cells are isolated from each other. Turning off shadow mode
# creates ingress rules allowing the central plane and other cells to reach it.

module "edge_mesh" {
  source = "../edge-mesh"

  project_id  = var.project_id
  environment = var.environment
  network_id  = var.network_id

  execution_nodes          = local.mesh_execution_nodes
  central_plane_ranges     = local.mesh_central_plane_ranges
  cross_region_mirrors     = var.cross_region_mirrors
  psc_endpoint_addresses   = local.psc_addresses

  labels = var.labels

  # Depend on the execution nodes being created first, so the firewall rules
  # find the network and tags they target.
  depends_on = [module.execution_node]
}

# --- Order routing configuration (documentation) ----------------------------
#
# Order routing is implicit in the firewall rules above. Here is how it works:
#
# 1. A cell receives a market signal and computes an order
# 2. The cell's `qip-edge-node` checks feasibility:
#    - Is there capital allocated for this venue?
#    - Is this venue withdrawn?
#    - Does the order stay inside the per-region ceiling?
#    - Does it stay inside the global concentration limit?
# 3. If feasible, the cell routes the order to the venue:
#    - Direct: if the venue is in the same region (firewall rule permits it)
#    - Mirrored: if the venue is in another region (cross-region mirror table)
# 4. The venue fills the order or rejects it
# 5. The fill is recorded in the cell's journal and sent to the central plane
# 6. The central plane's risk service updates the global exposure and notifies
#    other cells of new capital constraints
#
# Cross-region venues are reached by **mirroring**, not by re-routing. A cell
# that wants to trade a venue in another region includes it in the mirror
# configuration (§31.1), learns its capital allocation from the centre's
# updates, and sends orders to its own local proxy (the mirror point in the
# same region). The proxy forwards to the distant venue and relays fills back.
#
# This topology has two consequences:
# - A cell never reaches another region's venue directly; it goes through
#   a cell in the mirror's destination region
# - Fills from a mirrored venue are confirmed by two cells: the mirror point
#   (which was sent the order) and the originating cell (which receives the
#   fill report). Breaks between them are reconciliation breaks.
#
# See ADR 0039 for the capital sharing protocol and qip-edge-node/docs/31-mirroring.md
# for the implementation.
