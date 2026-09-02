# The platform's infrastructure.
#
# Every default here is the restrictive one. A runtime with no external
# addresses, no route to the internet except what a trust zone names, secrets
# that reach a process only as files, and an admission policy that refuses an
# image nobody signed is harder to stand up than one without — and the
# difficulty is the point, because the alternative is a platform that trades
# real money on a network anyone can reach.
#
# The shape is the blueprint's (ADR 0022, ADR 0024): every warm binary on
# Cloud Run, scaling to zero, from `catalogue.tf`; one dedicated Compute Engine
# machine per region for the execution node, from `execution_nodes`; thirteen
# trust zones with default deny between them, from `trust_zones`. There is no
# Kubernetes here and nothing that reconciles one.
#
# Nothing in this configuration enables live trading. The autonomy ceiling is
# an application setting, deliberately not an infrastructure one, so that
# changing it is a decision someone makes rather than a side effect of a
# deployment — and `variables.tf` refuses the three live rungs at plan time.

terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    # One resource needs it: the Vertex AI metadata store, which records the
    # lineage of a managed training run and has no GA equivalent. Declared
    # rather than reached for implicitly, because `terraform validate` accepts
    # a module using an undeclared provider and only `plan` refuses it — so an
    # undeclared provider is a configuration that passes CI and fails on the
    # day somebody applies it.
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }

  # State holds secret material references and the full topology. It lives in
  # a bucket with versioning and customer-managed encryption, never on a
  # workstation.
  backend "gcs" {
    prefix = "qip/state"
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

provider "google-beta" {
  project = var.project_id
  region  = var.region
}

# The project's numeric id, asked for rather than typed in.
#
# Two IAM bindings name Google's own service agents, which are identified by
# number and not by id. Requiring the operator to supply it made the number a
# second source of truth that can disagree with `project_id` — and when it
# does, the bindings are granted to service agents in whatever project that
# number belongs to, which applies cleanly and grants nothing.
#
# `var.project_number` overrides this for the case the lookup cannot run; see
# the variable.
data "google_project" "this" {
  count      = var.project_number == null ? 1 : 0
  project_id = var.project_id
}

locals {
  # `tonumber` because the data source reports the number as a string and
  # every consumer of it declares a `number`.
  project_number = var.project_number != null ? var.project_number : tonumber(one(data.google_project.this[*].number))

  # Whether this environment's ceiling could reach a real venue at all.
  #
  # The one definition. Three places answered this question in three
  # spellings — the resource label below, the `live_capable` output, and the
  # `venue_credential_readable` predicate the secrets module is given — and two
  # of the three were the same sentence read backwards. The ladder has six
  # rungs; `variables.tf` refuses the three live ones at plan time, so the only
  # values that reach here are `observation`, `advisory` and `paper_trading`,
  # and `!= "paper_trading"` is true for exactly the two rungs *below* the
  # paper one. An operator hardening an environment to `observation` — the move
  # `variables.tf`'s own error message invites — was labelled live-capable and
  # got a `live_capable = true` output. The safest rung produced the loudest
  # alarm, and the console renders that alarm as "investigate before trusting
  # anything else on screen".
  #
  # Naming the live rungs states the property instead of its complement, and it
  # is false for every configuration that can pass validation. Having one
  # expression rather than three is the other half of the fix: the inversion
  # spread because a reader who corrected one spelling had no way to know about
  # the other two.
  ceiling_reaches_a_venue = contains(["supervised_live", "limited_autonomous_live", "autonomous_live"], var.autonomy_ceiling)

  # Every resource carries these, so an unlabelled resource is visibly an
  # unmanaged one.
  labels = {
    platform    = "qip"
    environment = var.environment
    managed_by  = "terraform"
    # Whether this environment is permitted to reach a real venue. Labelled so
    # a query can answer "which of our deployments can trade" without reading
    # application configuration.
    #
    # A label value is a string, and it stays one — a query filtering on
    # `live_capable=false` is the consumer, not a Terraform expression. But it
    # is `tostring` of the local rather than a ternary over two string
    # literals, because a ternary is exactly the shape the inversion hid in
    # once: swapping its arms is a one-character edit that reads as correct.
    live_capable = tostring(local.ceiling_reaches_a_venue)
  }

  # Private Google access. The one range through which a workload reaches
  # Google APIs without leaving the VPC and without a route to anywhere else.
  # `modules/network`'s private zone resolves every `*.googleapis.com` to it.
  private_google_apis = "199.36.153.8/30"

  # Where the central plane is, from a node's point of view: the ranges of
  # the trust zones the catalogue's workloads attach through, and the private
  # Google API endpoint. Derived from the catalogue and the declared zones
  # rather than typed, so a zone that moves moves the nodes' rule with it.
  central_plane_ranges = concat(
    [
      for zone in distinct([for workload in local.cloud_run_catalogue : workload.trust_zone]) :
      var.trust_zones[zone].subnet_cidr
      if contains(keys(var.trust_zones), zone)
    ],
    [local.private_google_apis],
  )
}

# The APIs everything else assumes.
#
# First, and everything below takes an explicit dependency on it. Without that
# ordering Terraform is free to create a subnet before the API that creates
# subnets is on, and the first apply into a fresh project stops partway with a
# `SERVICE_DISABLED` error — after the network exists, which is the state
# somebody then has to reason about.
#
# Not behind a flag, for the same reason as Binary Authorization below: the off
# position of that switch is the gap. An already-enabled API is adopted without
# a call, so this costs an existing project nothing.
#
# The conditional half mirrors the flags further down this file rather than
# reading them, so an API is enabled exactly when the resource that needs it is
# created. modules/services/BOOTSTRAP.md has the two things that must be true
# before even this can run.
module "services" {
  source = "./modules/services"

  project_id = var.project_id

  enable_bigquery                = var.enable_bigquery
  enable_alloydb                 = var.enable_alloydb
  enable_bigtable                = var.enable_bigtable
  enable_memorystore             = var.enable_memorystore
  enable_spanner                 = var.enable_spanner
  enable_vertex_ai               = var.enable_vertex_ai
  enable_security_command_center = var.enable_security_command_center

  # False. Disabling an API on destroy does not revoke access, it deletes the
  # resources under it — including ones this configuration never created. See
  # the variable.
  disable_services_on_destroy = var.disable_services_on_destroy
}

module "network" {
  source = "./modules/network"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  # The console's route to the platform (ADR 0018). Null in an environment
  # that has not opted in, which creates neither the subnet nor the console's
  # identity.
  console_egress_cidr = var.console_egress_cidr
}

module "secrets" {
  source = "./modules/secrets"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id     = var.project_id
  project_number = local.project_number
  region         = var.region
  environment    = var.environment
  labels         = local.labels

  # Which secrets exist. Their values are never in Terraform: the resource is
  # created empty and the value is written out of band, so a state file that
  # leaks does not leak credentials.
  secret_names = [
    "qip-token-operator",
    "qip-token-approver",
    "qip-token-analyst",
    "qip-token-viewer",
    "qip-token-monitor",
    # The venue credential. Present in every environment so the deployment is
    # uniform; readable only where the autonomy ceiling permits live trading.
    "qip-venue-credential",
    "qip-quantum-token",
    # The key a node verifies a capital envelope's signature against.
    # Held as a secret for its integrity rather than its confidentiality:
    # somebody who can replace it can mint envelopes, which is the one way a
    # cell's bound can be widened without the central plane agreeing.
    "qip-capital-envelope-key",
    # The market-data vendor's API key, read by
    # `qip_market_ingestion::rest::RestMarketDataAdapter`. It travels in a
    # request header and never in a URL, so it must not be assembled into one
    # here either: the adapter reads it from the environment at start-up and
    # redacts it from every Debug rendering.
    "qip-market-data-key",
  ]

  # The venue credential is readable only by an environment that could use it,
  # and the only ceilings that could use it are the three live ones.
  #
  # This was written `!= "paper_trading"` — see `ceiling_reaches_a_venue` above
  # for why that is the same sentence read backwards. The concrete failure here
  # was the worst of the three: an operator hardening dev to `observation` —
  # the move `variables.tf` explicitly invites — got a plan that *added*
  # `roles/secretmanager.secretAccessor` on the venue credential to the fast
  # brain. Lowering autonomy handed out the credential.
  #
  # The local is false for every configuration that can pass validation, so no
  # plan this repository can produce creates the grant at all. That is the
  # intended state, not an accident of the list: the platform is paper-trading
  # only and nothing here should hold a venue credential.
  #
  # A reference to the membership test rather than a bare `false`, because
  # `false` records the current answer and loses the question — the next reader
  # deletes the variable and with it the reason the resource exists. The cost
  # of saying it this way is stated rather than discovered: anyone who ever
  # deletes the plan-time refusal in `variables.tf` re-enables this grant as a
  # side effect, so that change must revisit the local. Three acceptance tests
  # stand in the way of it happening quietly —
  # `no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue`,
  # `the_venue_credential_is_unreadable_where_live_trading_is_impossible` and
  # `the_label_the_output_and_the_credential_predicate_agree_at_every_rung`,
  # the last two of which evaluate this predicate for every rung a plan can
  # carry.
  venue_credential_readable = local.ceiling_reaches_a_venue

  # The one identity that could ever hold it: the fast brain's Cloud Run
  # account, from the catalogue. Named here so the grant, when the ceiling
  # ever permits it, lands on the workload that would use it and on no other.
  venue_credential_reader = module.cloud_run["fastbrain"].service_account_email

  # The console's identity and its one grant, created only where the console
  # has a route to the platform at all (ADR 0018).
  console_enabled = var.console_egress_cidr != null
}

# The egress proxy: the one committed bootstrap, published for every rendering.
#
# Before this existed nothing on the platform could reach a vendor: the client
# refuses `https` by name and the proxy the chart described never ran. See
# modules/egress-proxy/README.md for why it is a sidecar and a systemd unit
# rather than a service of its own, and for the plan-time gate that refuses a
# widened allowlist.
module "egress_proxy" {
  source = "./modules/egress-proxy"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  image_prefix      = module.registry.image_prefix
  allowed_upstreams = var.egress_allowed_upstreams
}

# The trust zones (blueprint §46.1): a subnet, a tag and a default deny in
# both directions per zone; a path only where the tfvars name one and this
# module sanctions the pair; external egress only for the purposes a zone may
# hold, so IBM is reachable from `optimisation` and nowhere else. Every Cloud
# Run workload in the catalogue attaches to its zone's subnet and carries its
# tag. modules/trust-zones/NOT-ENFORCED-HERE.md is what a network still
# cannot hold.
module "trust_zones" {
  source = "./modules/trust-zones"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  region      = var.region
  network_id  = module.network.network_id

  zones           = var.trust_zones
  permitted_paths = var.permitted_paths
  external_egress = var.external_egress
  public_ingress  = var.public_ingress

  # The identities in each zone are the catalogue's workloads placed there,
  # computed in catalogue.tf.
  zone_identities = local.zone_identities
}

module "observability" {
  source = "./modules/observability"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  # Whether the workload alerts can exist yet — see the variable, and
  # modules/observability/NOT-SCRAPED.md for what scrapes what.
  workload_metrics_exist = var.workload_metrics_exist

  # Alerting thresholds. The kill-switch alert has no threshold: any trip is
  # worth waking someone for.
  notification_channels = var.notification_channels
}

module "cicd" {
  source = "./modules/cicd"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment

  # Which repository may impersonate the pipeline account. No default: see the
  # variable.
  github_repository = var.github_repository
}

module "registry" {
  source = "./modules/registry"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  ci_service_account = module.cicd.service_account_email

  # Cloud Run pulls as its own service agent, which the project grants
  # without a line here. The workloads are listed so a component can read the
  # digest of the image it is running, which is what makes a provenance claim
  # checkable from inside the process.
  pull_service_accounts = [for workload in module.cloud_run : workload.service_account_email]
}

module "evidence" {
  source = "./modules/evidence"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  # The evidence key lives in the platform's existing key ring rather than in a
  # second one nobody rotates.
  key_ring_id = module.secrets.key_ring_id

  # The deep brain produces the evidence for a decision; the API serves it to
  # whoever is asking. Deliberately two identities: the component that writes
  # the record and the component that shows it should not be the same one.
  #
  # Neither list may grow to include a role that can delete. See the module.
  writer_service_accounts = [module.cloud_run["deepbrain"].service_account_email]
  reader_service_accounts = [module.cloud_run["api"].service_account_email]
}

module "data" {
  source = "./modules/data"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  key_ring_id = module.secrets.key_ring_id
  network_id  = module.network.network_id

  # Every managed store is off unless a deployment says otherwise, because
  # this build implements three storage targets and refuses six. See the
  # module documentation: a provisioned database no adapter can open is a
  # bill, an attack surface, and a diagram that reads as a capability.
  enable_bigquery      = var.enable_bigquery
  enable_cloud_storage = var.enable_cloud_storage
  enable_alloydb       = var.enable_alloydb
  enable_bigtable      = var.enable_bigtable
  enable_memorystore   = var.enable_memorystore
  enable_spanner       = var.enable_spanner

  # The deep brain researches and the API reads; neither writes tick history.
  # Narrower than "every workload", because a component that can write the
  # archive can obscure what it did.
  writer_service_accounts = [
    module.cloud_run["deepbrain"].service_account_email,
  ]
  reader_service_accounts = [
    module.cloud_run["api"].service_account_email,
    module.cloud_run["deepbrain"].service_account_email,
  ]
}

module "ai" {
  source = "./modules/ai"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  providers = {
    google      = google
    google-beta = google-beta
  }

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  key_ring_id = module.secrets.key_ring_id
  network_id  = module.network.network_id

  enable_vertex_ai = var.enable_vertex_ai

  # Training is the deep brain's work. The fast path may not call a model at
  # all — `qip-fastbrain` refuses to start if any agent it hosts holds
  # `call_language_model` — so giving it a training identity would contradict
  # a guarantee the binary enforces at start-up.
  training_service_account = module.cloud_run["deepbrain"].service_account_email
}

# The execution nodes (blueprint §41.4).
#
# One module, instantiated once per entry in `execution_nodes`. The
# architecture calls for one dedicated machine per region; every environment
# ships with none, because a node must be configured for at least one venue
# and no venue's published ranges are recorded anywhere here. Adding one is
# an entry in a variable rather than a directory, and the plan that carries
# it is the evidence ADR 0020's step 3 asks for.
module "execution_node" {
  source   = "./modules/execution-node"
  for_each = var.execution_nodes

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  node_id = each.key
  region  = each.value.region
  zone    = each.value.zone

  network_id  = module.network.network_id
  subnet_cidr = each.value.subnet_cidr

  machine_type = each.value.machine_type
  boot_image   = each.value.boot_image

  create_egress_nat = each.value.create_egress_nat

  # Observed before it takes anything. ADR 0020 step 3. Not a tfvars value:
  # letting a node out of shadow mode is an edit here that a reviewer sees.
  shadow_mode = true

  venues               = each.value.venues
  central_plane_ranges = local.central_plane_ranges

  # Per node, because a plan and its pricing are a property of what a cell is
  # asked to run rather than of the environment. Both default to empty, which
  # is the node's own "deploy nothing" and is what every environment gets
  # until one names otherwise.
  default_pricing    = each.value.default_pricing
  strategy_plan_path = each.value.strategy_plan_path

  # The same bootstrap every Cloud Run sidecar mounts, and the loopback
  # addresses it answers on.
  egress_bootstrap = file("${path.module}/../egress/envoy.yaml")
  egress_endpoints = module.egress_proxy.endpoints

  capital_envelope_secret_id = module.secrets.secret_ids["qip-capital-envelope-key"]
  venue_credential_secret_id = module.secrets.secret_ids["qip-venue-credential"]

  # The root's own predicate, passed through unchanged. The module then
  # requires shadow mode to be off as well. A membership test over the three
  # live rungs and never `!= "paper_trading"`, which is true for exactly the
  # two rungs below paper trading and once granted the credential by lowering
  # the ceiling.
  venue_credential_readable = local.ceiling_reaches_a_venue

  evidence_bucket = module.evidence.bucket_name
}

# Only an image this pipeline signed may run.
#
# Every Cloud Run service in the catalogue evaluates the project's default
# policy on every revision. Until this module existed there was no policy for
# it to evaluate, so Google evaluated the implicit one — whose default rule is
# `ALWAYS_ALLOW` — and refused nothing while reading, in the configuration and
# in the console, as though it did.
#
# Not optional and deliberately not behind a flag: with evaluation already on,
# the only alternative to a deny-by-default policy is the implicit policy that
# admits everything, so an off switch here would be a switch whose off position
# is the gap. `exempt_image_patterns` is likewise not surfaced as a root
# variable — an exemption is an image that runs unsigned, and it should be a
# deliberate edit to the module rather than a line in a tfvars file. The one
# third-party image, the egress proxy, is mirrored and attested by vendor.yml
# rather than exempted.
module "binary_authorization" {
  source = "./modules/binaryauthorization"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  # The signing key lives in the platform's existing key ring, like the
  # evidence and data keys, rather than in a second ring nobody rotates.
  key_ring_id = module.secrets.key_ring_id

  # The pipeline signs. That is the honest shape of this control and its main
  # limitation: whoever can run a step in that pipeline can sign an image.
  # modules/binaryauthorization/OUT-OF-BAND.md says what a stronger
  # arrangement would be and why this repository cannot hold it.
  ci_service_account = module.cicd.service_account_email
}

# Private links and direct peering.
#
# Off in every environment unless a deployment says otherwise, and the reason
# is stronger than the one for a managed database: an interconnect attachment
# is a resource waiting for a partner circuit that nobody has ordered. It
# cannot be made to work from here — a partner, a cross-connect, a pairing key
# handed over and a VLAN attachment on their side are all out of band, and
# modules/connectivity/NOT-ORDERED.md lists them in the order they happen.
module "connectivity" {
  source = "./modules/connectivity"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  network_id = module.network.network_id

  enable_partner_interconnect = var.enable_partner_interconnect
  partner_interconnects       = var.partner_interconnects
  cloud_router_asn            = var.cloud_router_asn

  enable_private_service_connect  = var.enable_private_service_connect
  private_service_connect_address = var.private_service_connect_address
  private_service_connect_target  = var.private_service_connect_target
}

# Backups for the state that cannot be rebuilt: the execution node's journal.
#
# A disk snapshot schedule, attached to every disk the node's template labels
# `qip_journal=true`. Terraform cannot make the attachment — the disk is
# created by the instance group after any apply — so
# `journal_snapshot_attachment_command` is that step and the runbook carries
# it. Not behind a flag, for the same reason as Binary Authorization: a switch
# whose off position is the gap the runbook already documents would leave that
# gap in place and add a line to the configuration implying otherwise.
module "backup" {
  source = "./modules/backup"

  project_id  = var.project_id
  environment = var.environment
  region      = var.region
  labels      = local.labels

  # In the platform's existing key ring, like the evidence and model keys.
  key_ring_id = module.secrets.key_ring_id

  snapshot_start_time  = var.snapshot_start_time
  snapshot_retain_days = var.snapshot_retain_days

  depends_on = [module.services]
}

# Security Command Center, the project-scoped part of it.
#
# Off by default, and the reason is unusual enough to be worth stating here as
# well as in the variable: the resources are free and useful, and they only
# ever evaluate if SCC is activated at the **organisation** this project
# belongs to. This configuration is project-scoped by design — one project per
# environment, so a blast radius stops at a project boundary — and has no
# organisation id to check that with.
#
# modules/scc/ORGANISATION-SCOPED.md draws the line item by item, including why
# there is no notification config or BigQuery export here.
module "scc" {
  source = "./modules/scc"

  project_id = var.project_id

  enable_security_command_center = var.enable_security_command_center
  muted_findings                 = var.scc_muted_findings

  depends_on = [module.services]
}

module "identity" {
  source = "./modules/identity"

  # Customer identity, per environment. Off everywhere until an environment's
  # tfvars opts in — a plan for an environment that has not decided to run
  # customer sign-in must not create a customer directory as a side effect.
  enabled            = var.enable_identity_platform
  project_id         = var.project_id
  authorized_domains = var.identity_authorized_domains
  mfa_state          = var.identity_mfa_state
}
