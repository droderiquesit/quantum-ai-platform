# Development: the central plane on its own.
#
# No cells. A cell exists to be next to a venue, and there is no venue here —
# a development cell would be the same binary on the same machine as the
# central plane, testing the deployment topology and nothing about latency.
# `edge_cells = {}` is a working configuration, not an incomplete one.

# The project this environment lives in. An identifier, not a secret: it
# appears in every resource name and in the pipeline's own configuration, so
# keeping it out of version control would buy nothing and cost reproducibility.
#
# All four environments name the same project today. Separate projects would
# be better — a blast radius that stops at a project boundary is the only one
# that reliably stops — and the change is this one line per file plus a state
# bucket each. What makes one project survivable meanwhile is that every
# resource carries the `environment` prefix below, so `dev` and `prod` cannot
# collide on a name.
project_id = "algorik-dev"

# The project's numeric id, recorded so nothing has to ask for it. It is an
# identifier like the id above, Google's service agents are named by it, and
# the workload-identity audience contains it — a workflow that can read this
# file can construct its own authentication with no repository variable
# involved, which matters because a variable set from a broken shell once
# carried an install advisory where this number should have been.
project_number = 95200532413

environment      = "dev"
region           = "us-east4"
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

# The first two remote cells of the v4 footprint (docs/operations/
# deploying-an-edge-cell.md): London in the metro, Tokyo in the metro.
# CIDRs follow the runbook's ladder — cell 1 and cell 2. `venues` is empty
# deliberately: a fresh cell can reach nothing until the venue ranges are
# added, which is step 9 of the runbook and never the default.
edge_cells = {
  london-1 = {
    region       = "europe-west2"
    subnet_cidr  = "10.16.0.0/20"
    pod_cidr     = "10.20.0.0/14"
    service_cidr = "10.24.0.0/20"
    venues       = {}
  }
  tokyo-1 = {
    region       = "asia-northeast1"
    subnet_cidr  = "10.32.0.0/20"
    pod_cidr     = "10.36.0.0/14"
    service_cidr = "10.40.0.0/20"
    venues       = {}
  }
}

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
# `backend/crates/libs/qip-confidential` is statistical disclosure control with no
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

# pd-standard, 50GB: a fresh project's SSD_TOTAL_GB quota is 250 in the
# region, and a regional cluster's three pd-ssd boot disks at the default
# 100GB are already 300 — the first apply died on exactly that. Standard
# disks draw on a separate, far larger quota, and development does not trade
# on its disk latency.
node_disk_type    = "pd-standard"
node_disk_size_gb = 50

# Off, so infra.yml's `down` can actually destroy this cluster between test
# sessions and, separately, so a cluster left `tainted` by a failed create
# (quota, a bad config) can be destroyed and recreated rather than stuck
# forever. The provider default is true and refuses both with the same
# message a deliberate teardown gets. Nothing here trades, so there is no
# live book this protects.
cluster_deletion_protection = false

# Flip to true and re-apply after the first deployment is running: the four
# workload alert policies name Prometheus metrics, and Cloud Monitoring
# refuses a policy for a metric it has never ingested. While this is false
# the alerts do not exist, which is the honest description of an environment
# whose workloads have never emitted a metric.
# workload_metrics_exist = true

# --- Customer identity ------------------------------------------------------
# Identity Platform for customer sign-in, activated once real hostnames
# existed to authorize. The domains are the Cloud Run URLs a deploy actually
# printed (never invented, per the variable's contract) plus localhost for
# development against the real project.
enable_identity_platform = true
identity_authorized_domains = [
  "localhost",
  "algorik-portal-rgxpsss2lq-uk.a.run.app",
  "algorik-landing-rgxpsss2lq-uk.a.run.app",
]
