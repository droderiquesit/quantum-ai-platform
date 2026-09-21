# Test: one central plane, on Cloud Run, and no node.
#
# A test environment that could reach a real venue is a test environment that
# can send a real order, so no node and no external egress — the same shape
# as dev with a project of its own.

# Not provisioned. `unprovisioned` is a valid project-id *shape* that the
# root refuses by name at plan time, and deploy.yml and vendor.yml refuse it
# before they authenticate; a plausible-looking id pointing at a deleted
# project fails much later with an authentication error about an audience
# nobody can explain. Provisioning this environment means a project of its
# own — never one another environment already uses — its own state bucket,
# and the id and number recorded here. See environments/README.md.
project_id     = "unprovisioned"
project_number = 0

environment      = "test"
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

# No control plane (ADR 0036): gitops_enabled stays at its default of false,
# so no cluster, no controller identity and nothing reconciling the
# RunService manifests under infrastructure/gitops/envs/test/ into a project
# that does not exist yet. Turning it on needs a `management` entry in
# trust_zones above and a gitops_master_ipv4_cidr_block; see dev.

# OpenObserve (ADR 0028) is not deployed here either: vendored_openobserve_image_digest
# stays at its default of null, the same closed state as every other environment,
# and see dev/terraform.tfvars for what setting it requires — a digest and a
# `management` entry in trust_zones above, neither declared here.

# The six optional files the central roots can be given stay unset here, at
# their default of null: venue_registrations_file, because nobody
# has registered with a venue from this environment and a record is a named
# person's act (docs/operations/registering-a-venue.md); wallet_statement_file,
# because this environment trades on the in-process simulated venue (ADR
# 0003), which issues no custodian statement to mount; and capital_fabric_file,
# because that same simulated venue gives a corridor nowhere to carry capital
# to, so declaring a destination would journal an act nobody performed. Unset
# renders no configuration file and therefore no variable, so the API's
# registry holds nobody, /wallet answers `assembled: false` and
# /transfer-gate answers `last_assessment: null` — see dev/terraform.tfvars
# for the whole argument, including why a committed statement is a same-day
# act and why a fabric declaration is appended to rather than edited.
#
# central_horizons_file and source_candidates_file — the deep brain's own
# pair — stay null for the same reason: no strategy run here has a stated
# §23.4 horizon to declare, and the committed candidate catalogue, though
# it names a source on a route the bootstrap does serve (under ADR 0060 a
# source is probed only where a reviewed egress route already exists), has
# not yet been observed making that call from a deployed process, so
# mounting it would put a claim in the banner no run has earned. Unset, the
# node assesses nothing and says so — which starves no control, unlike an
# absent universe. deepbrain_discover_every stays null
# alongside them, on purpose: a cadence with no candidate list to assess
# would run a pass that decides about nothing, so the pair is turned on
# together or not at all — see dev/terraform.tfvars for the whole argument.
# deepbrain_connector stays null too: the deep brain is the one brain that
# can reach a vendor, and no subject has two vendors among the shipped
# connectors, so polling one here would feed a rule-31 hold it cannot lift —
# dev/terraform.tfvars says what setting it would mean.
#
# risk_limits_file — the one optional file all three central roots share —
# stays null because no recalibration has been signed (ADR 0061): the file
# is the artefact two operators' signatures emit, and until one exists the
# shipped `conservative_default` is the only set any process has run under.
# risk_limits_file = "data/risk-limits/<the desk's signed set>.json" would
# mount it; dev/terraform.tfvars says what that means.
#
# region_dark_after stays null — ADR 0079's window, QIP_REGION_DARK_AFTER on
# the API — because this API serves no mesh, so a window would arm a
# derivation over a centre that can hear no cell, and because no measurement
# exists yet to pick the number from. Unset, the derivation is off and
# /api/v1/regions says so; dev/terraform.tfvars says what setting it means.

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

# No public edge. See dev/terraform.tfvars for why an absent Cloud Armor
# policy, load balancer and CDN is a decision here rather than an omission:
# there is no customer surface deployed to put behind one, and an edge in
# front of nothing is a public address nobody opened on purpose.

# No DNS zone. `dns_zone_domain` is deliberately unset here, and this is a
# property of domains rather than a preference: `algorik.ai` has exactly one
# authoritative zone, dev owns it, and a second zone declared here would apply
# cleanly and then serve a second set of records from a second set of
# nameservers. Only whichever set the registrar names would be the one anybody
# sees; this one would be a state file full of records nobody resolves.
#
# No plan can catch that — each environment has its own state and none can see
# another's — so the guard is this absence, the empty default on
# `dns_zone_domain`, the `count` on `module.dns_zone`, and
# `a_single_environment_owns_the_dns_zone_for_the_domain` in the infrastructure
# acceptance suite, which reads all four of these files and fails on a second
# declaration. If this environment ever genuinely needs a name, it takes a
# subdomain delegated from dev's zone, not a zone of its own for the same
# domain.
