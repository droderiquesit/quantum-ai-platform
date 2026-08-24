# Development: the central plane on its own.
#
# No cells. A cell exists to be next to a venue, and there is no venue here —
# a development cell would be the same binary on the same machine as the
# central plane, testing the deployment topology and nothing about latency.
# `edge_cells = {}` is a working configuration, not an incomplete one.

environment      = "dev"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

node_count   = 1
machine_type = "e2-standard-4"

# The pool may shrink to one node per zone and grow to three.
#
# A floor of one is correct here and nowhere else: development has no market
# open to be cold for, so the argument for keeping capacity warm overnight does
# not apply. The ceiling is low on purpose — this environment has no edge cells
# and nothing that should ever need nine nodes, so a low maximum turns a runaway
# workload into a `Pending` pod rather than into a bill.
min_node_count = 1
max_node_count = 3

# Reachable from the office range only. Never the whole internet: a private
# cluster with a public control plane is a private cluster in name only.
authorised_networks = []

edge_cells = {}

# Every managed service off. Development runs on memory and local files, which
# is what the three implemented storage targets are for.
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
