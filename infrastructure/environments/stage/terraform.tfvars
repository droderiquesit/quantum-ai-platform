# Stage: the shape production will have, on a project of its own.
#
# The three zones the catalogue needs; no node until a venue's published
# ranges exist for one, and the first node here is the shadow-mode one ADR
# 0020 step 3 asks to observe before production gets any.

# Not provisioned. `unprovisioned` is a valid project-id *shape* that the
# root refuses by name at plan time, and deploy.yml and vendor.yml refuse it
# before they authenticate; a plausible-looking id pointing at a deleted
# project fails much later with an authentication error about an audience
# nobody can explain. Provisioning this environment means a project of its
# own — never one another environment already uses — its own state bucket,
# and the id and number recorded here. See environments/README.md.
project_id     = "unprovisioned"
project_number = 0

environment      = "stage"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

# --- The trust zones (blueprint §46.1) ---------------------------------------
#
# The three zones the catalogue places a workload in. Same ranges as dev:
# each environment is its own project and its own VPC, so the ranges do not
# collide across environments, and one address plan is one fewer thing to
# get wrong.
trust_zones = {
  "application-identity" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.32.0/24"
  }
  "cognition" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.33.0/24"
  }
  "intelligence" = {
    region      = "europe-west2"
    subnet_cidr = "10.0.34.0/24"
  }
}

permitted_paths = {}
external_egress = {}
public_ingress  = {}

# No execution node. A node must be configured for at least one venue, and
# no venue's published ranges are recorded anywhere; see
# modules/execution-node/README.md for the entry when they are.
execution_nodes = {}

# No image has ever been built for this environment, so there is no digest
# to create a service at. deploy.yml writes images.tfvars beside this file
# on its first run against a provisioned project.
image_digests = {}

enable_bigquery      = false
enable_cloud_storage = false
enable_alloydb       = false
enable_bigtable      = false
enable_memorystore   = false
enable_spanner       = false
enable_vertex_ai     = false

# Off; see dev/terraform.tfvars for why each is a decision rather than an
# oversight.
enable_security_command_center = false

# The only repository whose pipeline may deploy into this project.
github_repository = "droderiquesit/quantum-ai-platform"
