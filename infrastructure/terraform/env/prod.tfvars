# Production: all nine cells.
#
# ADR 0008 calls for cells next to the venues they trade. Nine are listed in
# `docs/operations/deploying-an-edge-cell.md`, and three of them —
# `chicago-1`, `newyork-1` and `dubai-1` — are **not in their metropolitan
# area**. Google Cloud has no region in Chicago, none in NY/NJ and none in
# Dubai; the nearest are roughly 400, 300 and 380 kilometres away, which is
# several milliseconds a cell whose whole argument is source-adjacency cannot
# spend.
#
# They are here anyway, deliberately. Leaving them out would make this file
# describe a seven-cell platform and quietly drop two American equity and
# futures venues and the Gulf. Including them makes the gap visible in the
# thing an operator applies, where the honest options are the ones
# `deploying-an-edge-cell.md` names: colocation with a partner interconnect,
# running those cells outside Google Cloud, or accepting that they are not
# latency-competitive and saying so. What must not happen is that they are
# deployed to Council Bluffs, Ashburn and Doha and reported as being in
# Chicago, New York and Dubai.

environment = "prod"
region      = "europe-west2"

# Paper trading, in production, on purpose. Live trading is enabled by two
# authenticated approvals at run time, so a tfvars file that could turn it on
# would turn an infrastructure change into a trading decision.
autonomy_ceiling = "paper_trading"

node_count   = 3
machine_type = "e2-standard-16"

# Filled in with the operator ranges that may reach the control plane. Empty
# means nobody, which fails safe: an unreachable control plane is recoverable
# and an open one is not.
authorised_networks = []

edge_cells = {
  "dallas-1" = {
    region       = "us-south1"
    subnet_cidr  = "10.65.0.0/20"
    pod_cidr     = "10.65.16.0/20"
    service_cidr = "10.65.32.0/20"
    venues       = {}
  }
  # Council Bluffs, Iowa — about 400km from the Chicago venues.
  "chicago-1" = {
    region       = "us-central1"
    subnet_cidr  = "10.66.0.0/20"
    pod_cidr     = "10.66.16.0/20"
    service_cidr = "10.66.32.0/20"
    venues       = {}
  }
  # Ashburn, Virginia — about 300km from the NY/NJ venues.
  "newyork-1" = {
    region       = "us-east4"
    subnet_cidr  = "10.67.0.0/20"
    pod_cidr     = "10.67.16.0/20"
    service_cidr = "10.67.32.0/20"
    venues       = {}
  }
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.68.0.0/20"
    pod_cidr     = "10.68.16.0/20"
    service_cidr = "10.68.32.0/20"
    venues       = {}
  }
  "frankfurt-1" = {
    region       = "europe-west3"
    subnet_cidr  = "10.69.0.0/20"
    pod_cidr     = "10.69.16.0/20"
    service_cidr = "10.69.32.0/20"
    venues       = {}
  }
  "singapore-1" = {
    region       = "asia-southeast1"
    subnet_cidr  = "10.70.0.0/20"
    pod_cidr     = "10.70.16.0/20"
    service_cidr = "10.70.32.0/20"
    venues       = {}
  }
  "tokyo-1" = {
    region       = "asia-northeast1"
    subnet_cidr  = "10.71.0.0/20"
    pod_cidr     = "10.71.16.0/20"
    service_cidr = "10.71.32.0/20"
    venues       = {}
  }
  "saopaulo-1" = {
    region       = "southamerica-east1"
    subnet_cidr  = "10.72.0.0/20"
    pod_cidr     = "10.72.16.0/20"
    service_cidr = "10.72.32.0/20"
    venues       = {}
  }
  # Doha, Qatar — about 380km from Dubai.
  "dubai-1" = {
    region       = "me-central1"
    subnet_cidr  = "10.73.0.0/20"
    pod_cidr     = "10.73.16.0/20"
    service_cidr = "10.73.32.0/20"
    venues       = {}
  }
}

# Off, in production, and this is the line most likely to be changed by
# somebody who should not.
#
# The platform implements three storage targets — memory, local files and the
# in-tree engine — and refuses these six by name, each naming what it still
# needs. Turning one on here provisions a healthy, empty, billable instance
# that no code in this build can open, and the architecture diagram then reads
# as though the capability exists.
#
# Turn one on when its adapter exists and is wired, and confirm with the
# `enabled_without_an_adapter` output before applying.
enable_bigquery      = false
enable_cloud_storage = false
enable_alloydb       = false
enable_bigtable      = false
enable_memorystore   = false
enable_spanner       = false
enable_vertex_ai     = false

# The only repository whose pipeline may deploy into this project. No default
# exists for this on purpose: a default would name a repository somebody else
# could be running, and the consequence of getting it wrong is that their
# pipeline pushes images and applies manifests here.
github_repository = "droderiquesit/quantum-ai-platform"
