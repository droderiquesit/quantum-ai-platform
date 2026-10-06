# The multi-region edge cell topology, as a contract and nothing else.
#
# This module creates no resource and calls no module. It validates a set of
# regional cells — every zone inside its region, every cell configured for a
# venue, every regional capital ceiling a positive number — and derives the
# values the root hands to `modules/execution-node` and `modules/edge-mesh`:
# the central plane's ingress ranges and one PSC address per region.
#
# It used to call both modules itself. That was wrong in two ways at once.
# The root already composes `execution-node` (`module "execution_node"` in
# `infrastructure/terraform/main.tf`), so a second composition here was a
# second set of nodes keyed by the same ids the moment anything called it.
# And a nested call is invisible to `terraform_contract.rs`, whose
# correspondence scan reads only the root's calls against each callee's
# `variables.tf` — which is how this module came to pass `isolated_cpus` and
# `telemetry_endpoint` to an `execution-node` that declares neither, a
# configuration `terraform validate` refuses and nothing in the tree ever
# ran. Composition belongs to the root, one level deep, where the scan sees
# every argument.
#
# ADR 0008 decides that cells are independent and only share capital through
# grants, not through coordination. ADR 0039 then asks: how does a cell know
# its share when a capital envelope spans multiple regions? Answer: the cell
# is told at boot, reads it from `node.env`, and starts with that share. The
# centre updates via capital envelope rotations; a cell that has lost the
# centre keeps trading inside the share it was given — on the simulated venue,
# because every cell is paper-only (ADR 0003).

locals {
  # The ranges the central plane's workloads run in, plus the Google APIs
  # range. `edge-mesh` admits exactly these (and the other cells' subnets) to
  # a cell's health port.
  central_plane_ranges = tolist(concat(
    sort(distinct([for zone in values(var.trust_zones) : zone.subnet_cidr])),
    [var.google_apis_range]
  ))

  regions = sort(distinct([for config in values(var.execution_nodes) : config.region]))

  # PSC endpoint addresses, one per region where a node is deployed. Addresses
  # are in the 10.255.0.0/24 range, never overlapping with any zone or node
  # subnet, and assigned deterministically so the far end can compute them.
  psc_addresses = {
    for idx, region in local.regions :
    region => "10.255.0.${idx + 1}"
  }
}
