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

# The only repository whose pipeline may deploy into this project. No default
# exists for this on purpose: a default would name a repository somebody else
# could be running, and the consequence of getting it wrong is that their
# pipeline pushes images and applies manifests here.
github_repository = "droderiquesit/quantum-ai-platform"
