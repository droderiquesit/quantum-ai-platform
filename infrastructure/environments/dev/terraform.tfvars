# Development: the central plane on its own, on Cloud Run.
#
# No execution node. A node exists to be next to a venue, and there is no
# venue here — and a node must be configured for at least one venue before
# the plan admits it. `execution_nodes = {}` is a working configuration, not
# an incomplete one.

# The project this environment lives in. An identifier, not a secret: it
# appears in every resource name and in the pipeline's own configuration, so
# keeping it out of version control would buy nothing and cost reproducibility.
#
# Each environment names a project of its own, or says it has none. Two
# environments in one project share one IAM boundary, one KMS key ring and one
# Binary Authorization attestor, whatever their name prefixes say.
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

# --- The trust zones (blueprint §46.1) ---------------------------------------
#
# The three zones the catalogue places a workload in, and no others: a zone
# with nothing in it constrains nothing, and a range chosen for a zone nobody
# uses is the range that collides later. Each is a /24 — a Cloud Run direct
# VPC egress interface needs a /26 or larger and Google reserves addresses
# in it as instances scale.
#
# The ranges sit above the console's 10.0.16.0/26 and below the ladder the
# execution nodes draw from (10.<64+n>.0.0/16, per environments/README.md),
# so nothing here overlaps a node's range whichever node is added first.
trust_zones = {
  "application-identity" = {
    region      = "us-east4"
    subnet_cidr = "10.0.32.0/24"
  }
  "cognition" = {
    region      = "us-east4"
    subnet_cidr = "10.0.33.0/24"
  }
  "intelligence" = {
    region      = "us-east4"
    subnet_cidr = "10.0.34.0/24"
  }
}

# No path between zones, no external egress, no public ingress. The API is
# reached by the console as a named Cloud Run invoker, which needs no
# firewall path; nothing here reaches a vendor, because no vendor's ranges
# have been recorded and the proxy's IBM listeners are reachable from no
# zone declared here. Each of these is a declaration somebody has to write
# down, with a note saying why.
permitted_paths = {}
external_egress = {}
public_ingress  = {}

# No execution node. See the header, and modules/execution-node/README.md
# for the entry a node needs when a venue's published ranges exist.
execution_nodes = {}

# Every managed service off. Development runs on memory, which is what the
# implemented storage targets are for on an instance with no volume.
enable_bigquery      = false
enable_cloud_storage = false
enable_alloydb       = false
enable_bigtable      = false
enable_memorystore   = false
enable_spanner       = false
enable_vertex_ai     = false

# --- Off, and each is a decision rather than an oversight --------------------

# Security Command Center's project-scoped resources. Off because they only
# ever evaluate if SCC is activated at the organisation, which is not a
# project-level act and which nothing here can check. Detectors that are
# stored and never run read in the console as a project being watched, which
# is worse than the gap they replace.
enable_security_command_center = false

# The only repository whose pipeline may deploy into this project. No default
# exists for this on purpose: a default would name a repository somebody else
# could be running, and the consequence of getting it wrong is that their
# pipeline pushes images and moves services here.
github_repository = "droderiquesit/quantum-ai-platform"

# Flip to true and re-apply only once a scrape has been observed — a
# `prometheus.googleapis.com/qip_*` descriptor visible in this project. The
# alert policies name Prometheus metrics, and Cloud Monitoring refuses a
# policy for a metric it has never ingested. While this is false the alerts
# do not exist, which is the honest description of an environment nothing
# has scraped: modules/observability/NOT-SCRAPED.md says what does not yet.
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

# --- The console's route to the platform (ADR 0018) --------------------------
#
# The portal reaches `qip-api` at the API's own Cloud Run URL, as the one
# invoker the catalogue names. This subnet is the interface the portal's
# direct VPC egress attaches to; 10.0.16.0/26 is a /26 because that is the
# smallest direct VPC egress accepts, and the console needs a handful of
# addresses, not a network.
#
# There is no `api_internal_address` any more. The GKE runtime reserved an
# address for an internal load balancer the cluster created from a Service;
# with no cluster there is no load balancer and nothing to reserve. The URL
# the console calls is the `api_internal_base_url` output.
console_egress_cidr = "10.0.16.0/26"
