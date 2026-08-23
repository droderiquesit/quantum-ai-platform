# Development.
#
# Paper trading, small, and reachable only from the office network.
# The project this environment lives in. An identifier, not a secret: it
# appears in every resource name and in the pipeline's own configuration, so
# keeping it out of version control would buy nothing and cost reproducibility.
project_id = "project-d3f96b6b-852b-4460-b6d"

environment      = "development"
autonomy_ceiling = "paper_trading"

node_count   = 1
machine_type = "n2-standard-2"

# Replace with the ranges that actually need control-plane access. The
# validation rule refuses 0.0.0.0/0, so leaving this empty is safer than
# guessing.
authorised_networks = []

# The repository permitted to push images and apply manifests here, through
# workload identity federation. No key exists; the pipeline exchanges a token
# GitHub mints for one job.
github_repository = "droderiquesit/quantum-ai-platform"

# One edge cell, next to the venue whose calendar the platform already models.
# The other six planned locations are a copy of this block with a different
# region, cell id and address allocation — see
# docs/operations/deploying-an-edge-cell.md, which carries the map and the
# allocation scheme.
#
# `venues` is empty on purpose. The address ranges a venue publishes are not
# guessable, and a cell with an empty venue map can reach no venue at all,
# which is the correct state for a cell whose connectivity nobody has
# confirmed. Filling it in is the last step of bringing a cell up, not the
# first.
edge_cells = {
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.16.0.0/20"
    pod_cidr     = "10.20.0.0/14"
    service_cidr = "10.24.0.0/20"
    venues       = {}
  }
}
