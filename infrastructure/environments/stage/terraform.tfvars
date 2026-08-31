# Stage: production's shape at a third of its size.
#
# Three cells rather than one or nine. One cell cannot show a failure that only
# appears when several cells publish to the centre at once — a queue that
# refuses under combined load, a spool that fills, a circuit breaker that opens
# for one peer while the others keep working. Nine would cost production money
# to learn nothing more.
#
# The three are chosen to span the differences that matter rather than to be
# nearby: three continents, three round-trip times to the centre, and one of
# them (`newyork-1`) is a cell the documentation is explicit is *not* in its
# metropolitan area. Staging is where that cost should be measured, not
# production.

# This environment is NOT PROVISIONED, and the marker below is what says so.
#
# It used to carry a real project id — the one all four environments shared,
# on the reasoning that the `environment` prefix kept their resource names
# apart. Two things retired that arrangement. `dev` moved to its own project
# (`algorik-dev`), so the premise that all four name one project stopped
# being true; and the project the other three still named was deleted, so the
# recorded id pointed at nothing while still looking entirely plausible in a
# file review.
#
# A dead id that looks real is worse than an obvious hole: a plan or a deploy
# aimed here would fail at authentication with a message about an audience
# nobody could explain. `unprovisioned` fails immediately instead, in
# variables.tf at plan time and in deploy.yml before it authenticates, and
# both refusals name what to do. Provisioning this environment means a real
# project of its own with its own state bucket — never a project another
# environment already uses, which `every_environment_names_a_project_of_its_own`
# in the acceptance suite enforces.
project_id = "unprovisioned"

# Zero, because there is no project to number. The real value is read from
# the project at provisioning time and recorded here then.
project_number = 0

environment      = "stage"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

node_count   = 2
machine_type = "e2-standard-8"

# Two per zone, up to five. Staging is where a production-shaped load is run
# against a production-shaped cluster, so the floor matches production's
# reasoning — capacity that is never reclaimed under a quiet period — and the
# ceiling is lower because there is no real book behind it.
min_node_count = 2
max_node_count = 5

authorised_networks = []

edge_cells = {
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.68.0.0/20"
    pod_cidr     = "10.68.16.0/20"
    service_cidr = "10.68.32.0/20"
    venues       = {}
  }
  "newyork-1" = {
    region       = "us-east4"
    subnet_cidr  = "10.67.0.0/20"
    pod_cidr     = "10.67.16.0/20"
    service_cidr = "10.67.32.0/20"
    venues       = {}
  }
  "tokyo-1" = {
    region       = "asia-northeast1"
    subnet_cidr  = "10.71.0.0/20"
    pod_cidr     = "10.71.16.0/20"
    service_cidr = "10.71.32.0/20"
    venues       = {}
  }
}

# Still off. Staging proves the deployment, and turning on a store no adapter
# can open would prove nothing except that Terraform can create it.
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
