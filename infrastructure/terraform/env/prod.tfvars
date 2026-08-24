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

# Three per zone committed, never fewer, up to eight.
#
# Both numbers are **per zone** and this is a regional cluster, so the real
# range is nine to twenty-four nodes.
#
# The floor is not lower than the committed size on purpose. Scaling down means
# draining, and the quiet period on this platform is a market that has closed
# followed by one that opens; giving back nodes overnight buys a few hours of a
# smaller bill and pays for it with cold starts and a wave of rescheduling at
# the open.
#
# The ceiling is what makes `qip-api`'s HorizontalPodAutoscaler a policy rather
# than a capacity limit — before there was any autoscaling, nothing in the
# system could add a node, so its `maxReplicas: 6` was a number the cluster
# could not honour. It is also a bound on the other direction: eight per zone
# is room for the cells to be rescheduled off a lost node and for an upgrade to
# surge, and it is not room for a wedged workload to buy nodes all day.
min_node_count = 3
max_node_count = 8

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

# --- Off, and each is a decision rather than an oversight --------------------

# Confidential VMs on the nodes. Real hardening, and off because
# `crates/libs/qip-confidential` is statistical disclosure control with no
# enclave and no attestation — turning this on next to a crate with that name
# lets the two together imply a guarantee neither provides. It is also never a
# one-line change: the machine type above is Intel and this needs n2d, c2d or
# c3d, which the cluster module refuses at plan time.
enable_confidential_nodes = false

# Security Command Center's project-scoped resources: two custom detectors that
# watch for a cluster with Binary Authorization enforcement turned off or a
# public control plane. Off because they only ever evaluate if SCC is activated
# at the organisation, which is not a project-level act and which nothing here
# can check. Detectors that are stored and never run read in the console as a
# project being watched, which is worse than the gap they replace.
enable_security_command_center = false

# The only repository whose pipeline may deploy into this project. No default
# exists for this on purpose: a default would name a repository somebody else
# could be running, and the consequence of getting it wrong is that their
# pipeline pushes images and applies manifests here.
github_repository = "droderiquesit/quantum-ai-platform"
