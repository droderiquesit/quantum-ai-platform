# Production.
#
# The ceiling here is still paper trading. Raising it is a separate, reviewed
# change to this file, and even then it only permits two authenticated
# operators to enable live trading — it does not enable it.
#
# This is the one line in the repository that decides whether the platform can
# ever reach a real venue, which is why it is a line rather than an inference
# from the environment name.
environment      = "production"
autonomy_ceiling = "paper_trading"

node_count   = 3
machine_type = "n2-standard-8"

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
