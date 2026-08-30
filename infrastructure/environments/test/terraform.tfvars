# Test: the central plane and one cell.
#
# One cell rather than none, because the failures worth catching before
# production are the ones that need two processes and a network between them —
# an envelope that does not verify, a delta that arrives twice, a peer that
# stops answering. One cell exercises every one of those. Nine would exercise
# the same code nine times.
#
# `london-1` because it is one of the six that is genuinely in the right
# metropolitan area, so a latency number measured here means something.

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
project_id = "project-d3f96b6b-852b-4460-b6d"

# The project's numeric id, recorded so nothing has to ask for it. It is an
# identifier like the id above, Google's service agents are named by it, and
# the workload-identity audience contains it — a workflow that can read this
# file can construct its own authentication with no repository variable
# involved, which matters because a variable set from a broken shell once
# carried an install advisory where this number should have been.
project_number = 601575011290

environment      = "test"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

node_count   = 1
machine_type = "e2-standard-4"

# One node per zone, up to three. Same reasoning as development: nothing here
# is warm-critical, and a low ceiling makes a runaway workload visible as an
# unschedulable pod instead of as a bill nobody reads until month end.
min_node_count = 1
max_node_count = 3

authorised_networks = []

edge_cells = {
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.68.0.0/20"
    pod_cidr     = "10.68.16.0/20"
    service_cidr = "10.68.32.0/20"
    # No venues. A test cell that could reach a real venue is a test cell that
    # can send a real order.
    venues = {}
  }
}

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

# Off, so infra.yml's `down` can actually destroy this cluster between test
# sessions — see the same line in dev/terraform.tfvars.
cluster_deletion_protection = false
