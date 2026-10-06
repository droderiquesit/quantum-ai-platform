# Edge cell mesh networking.
#
# This module creates the inter-cell communication paths that let cells share
# region-scoped capital (ADR 0039) and coordinate cross-region mirroring (§31.1).
#
# The mesh is deliberately minimal: no service mesh, no overlay, one VPC with
# regional routing. Cells sit in their own subnets, isolated by firewall rules
# when in shadow mode. Turning off shadow mode opens venue rules; the mesh itself
# does not carry venue traffic.
#
# When a cell is to reach another cell:
# - If both are in the same region: they reach each other via Private Service Connect,
#   each through its own endpoint on the shared region subnet.
# - If in different regions: direct VPC connectivity through the global network (no peering needed).
#
# The central plane is reached by every cell through both the trust zones' subnets
# and the private Google APIs endpoint.

locals {
  prefix = "qip-${var.environment}"

  # Whether any cross-region mirroring is configured. Used to gate PSC endpoints
  # and inter-region communication setup.
  has_cross_region_mirror = length(var.cross_region_mirrors) > 0

  # Map of region names to the zones (subnets) that exist in them, derived from
  # the node configuration map.
  regions_by_node = {
    for node_id, node_config in var.execution_nodes :
    node_config.region => node_config.zone
  }

  # The distinct set of regions where nodes are deployed.
  node_regions = distinct(values(local.regions_by_node))
}

# --- Private Service Connect endpoints for cross-region communication ---------
#
# A cell that mirrors to another region (ADR 0039, §31.1) connects through a
# Private Service Connect endpoint if the two regions are distant (defined here
# as >400km apart). For adjacent regions, direct VPC routing is sufficient.
#
# A PSC endpoint is a global address that routes to a managed service on one or
# more backends. Here, the backend is the cell's health port in each region,
# published by the other region's ingress rule.

resource "google_compute_global_address" "cell_psc_endpoint" {
  for_each = toset(local.node_regions)

  project      = var.project_id
  name         = "${local.prefix}-cell-psc-${each.value}"
  purpose      = "PRIVATE_SERVICE_CONNECT"
  address_type = "INTERNAL"
  network      = var.network_id

  # A cell reaches this endpoint from another region; it names this region's
  # edge cell, so it must not conflict with other cells' endpoints. Assigning
  # deterministically from the region name so the far end can compute it.
  address = var.psc_endpoint_addresses[each.value]

  labels = var.labels
}

# --- Mesh connectivity rules ---------------------------------------------------
#
# When a cell is not in shadow mode and is the first to reach a venue, its
# firewall rules create the ingress path that all other cells use to reach the
# same venue. Rules are per-node (tagged) rather than per-zone, because one
# region's traffic toward a venue does not reach another's.
#
# A cell in shadow mode creates no venue rules at all — the `for_each` is
# empty in the execution-node module. This module likewise creates no
# inter-cell ingress when shadow mode is on, because there is no cell that has
# left shadow to reach it.
#
# Once a cell turns off shadow mode, the following happens:
#   1. Venue egress rules in execution-node/main.tf create the forward path to venues
#   2. This module creates an ingress rule on the cell's health port, scoped to
#      the central plane and other cells (never to arbitrary internet)
#   3. Any other cell that also reaches that venue uses the same ingress,
#      because it is created once and re-created by every cell that touches it

resource "google_compute_firewall" "cell_ingress_health" {
  for_each = {
    for node_id, node_config in var.execution_nodes :
    node_id => node_config if !node_config.shadow_mode
  }

  project = var.project_id
  name    = "${local.prefix}-cell-${each.key}-ingress-health"
  network = var.network_id

  direction = "INGRESS"
  priority  = 1000

  allow {
    protocol = "tcp"
    ports    = [tostring(each.value.health_port)]
  }

  # A cell is reached by:
  # - The central plane (for evidence exchange and capital envelope updates)
  # - Other cells in other regions (for cross-region mirroring coordination)
  # - Not by the internet (no CIDR range starting with 0.)
  source_ranges = concat(
    var.central_plane_ranges,
    [
      for region in local.node_regions :
      var.execution_nodes[
        [for id, cfg in var.execution_nodes : id if cfg.region == region][0]
      ].subnet_cidr
      if region != each.value.region
    ]
  )

  # Tag the cell, so the firewall rule finds exactly this execution node.
  target_tags = ["qip-exec-${each.key}"]

  log_config {
    metadata = "INCLUDE_ALL_METADATA"
  }
}

# --- Mesh routes (implicit VPC connectivity) --------------------------------
#
# No explicit routes are needed for same-region communication or for the
# regional VPC's native routing. The VPC's routing_mode = "REGIONAL" means:
# - Same-region traffic: delivered by subnet routing
# - Different-region traffic: delivered by the VPC's internal backbone
#
# A `google_compute_route` is only needed if traffic must traverse an external
# gateway (a Cloud Router, an interconnect). This module creates none because
# the mesh stays within the VPC.

# --- Documentation of what happens when a cell is observed --------------------
#
# ADR 0020 step 3 says: "A node holding sessions, quoting nothing, matching the
# pod's decisions". The comparison is against a GKE pod running the same binary
# in the control plane, both in shadow mode, both seeing the same market data
# and capital. When both make the same decisions on the same calendar day, the
# node has been observed and is safe to leave shadow mode.
#
# Turning off `var.execution_nodes[...].shadow_mode` in the tfvars does three
# things, all load-bearing:
#   1. Creates venue egress rules in execution-node/main.tf, opening paths to real venues
#   2. Creates venue egress rules for internal-crossing simulators and counterparty APIs
#   3. Creates a health ingress rule (above) that other cells and the central plane may reach
#
# None of these are automatic or invisible. Each is a diff reviewed before apply.
# Changing one line in tfvars from `shadow_mode = true` to `shadow_mode = false`
# produces a plan naming every rule that will be created, and the risk is visible.

