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

# All seven planned cells, one per venue metro. Ranges follow the allocation
# scheme in docs/operations/deploying-an-edge-cell.md — cell n takes
# 10.(16n).0.0/20, 10.(16n+4).0.0/14 and 10.(16n+8).0.0/20, so no two overlap.
# Overlapping ranges route to whichever subnet was created first, silently,
# which is why the scheme is arithmetic rather than chosen per cell.
#
# Two of the seven have no Google Cloud region in the right metropolitan area.
# They are still listed, with the distance recorded against each, because a
# cell that is 400km from its venue is a real architectural cost and ADR 0008's
# own reversal condition turns on exactly that measurement.
#
# `venues` is empty on purpose. The address ranges a venue publishes are not
# guessable, and a cell with an empty venue map can reach no venue at all,
# which is the correct state for a cell whose connectivity nobody has
# confirmed. Filling it in is the last step of bringing a cell up, not the
# first.
edge_cells = {
  # Dallas — in the metro
  "dallas-1" = {
    region       = "us-south1"
    subnet_cidr  = "10.16.0.0/20"
    pod_cidr     = "10.20.0.0/14"
    service_cidr = "10.24.0.0/20"
    venues       = {}
  }

  # Chicago — Council Bluffs, ~400km — see the runbook
  "chicago-1" = {
    region       = "us-central1"
    subnet_cidr  = "10.32.0.0/20"
    pod_cidr     = "10.36.0.0/14"
    service_cidr = "10.40.0.0/20"
    venues       = {}
  }

  # NY/NJ — Ashburn, ~330km — see the runbook
  "newyork-1" = {
    region       = "us-east4"
    subnet_cidr  = "10.48.0.0/20"
    pod_cidr     = "10.52.0.0/14"
    service_cidr = "10.56.0.0/20"
    venues       = {}
  }

  # London — in the metro
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.64.0.0/20"
    pod_cidr     = "10.68.0.0/14"
    service_cidr = "10.72.0.0/20"
    venues       = {}
  }

  # Frankfurt — in the metro
  "frankfurt-1" = {
    region       = "europe-west3"
    subnet_cidr  = "10.80.0.0/20"
    pod_cidr     = "10.84.0.0/14"
    service_cidr = "10.88.0.0/20"
    venues       = {}
  }

  # Singapore — in the metro
  "singapore-1" = {
    region       = "asia-southeast1"
    subnet_cidr  = "10.96.0.0/20"
    pod_cidr     = "10.100.0.0/14"
    service_cidr = "10.104.0.0/20"
    venues       = {}
  }

  # Tokyo — in the metro
  "tokyo-1" = {
    region       = "asia-northeast1"
    subnet_cidr  = "10.112.0.0/20"
    pod_cidr     = "10.116.0.0/14"
    service_cidr = "10.120.0.0/20"
    venues       = {}
  }
}
