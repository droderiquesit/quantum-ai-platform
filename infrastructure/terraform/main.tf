# The platform's infrastructure.
#
# Every default here is the restrictive one. A cluster that is private, has no
# public node addresses, encrypts its secrets with a customer-managed key and
# denies all egress except what is named is harder to stand up than one that
# is not — and the difficulty is the point, because the alternative is a
# platform that trades real money on a network anyone can reach.
#
# Nothing in this configuration enables live trading. The autonomy ceiling is
# an application setting, deliberately not an infrastructure one, so that
# changing it is a decision someone makes rather than a side effect of a
# deployment.

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

  # Every resource carries these, so an unlabelled resource is visibly an
  # unmanaged one.
  labels = {
    platform    = "qip"
    environment = var.environment
    managed_by  = "terraform"
    # Whether this environment is permitted to reach a real venue. Labelled so
    # a query can answer "which of our clusters can trade" without reading
    # application configuration.
    live_capable = var.autonomy_ceiling == "paper_trading" ? "false" : "true"
  }

  # The service accounts, one per deployable, so a compromised component has
  # only its own permissions.
  #
  # The key is the deployable's short name and the value is both the Google
  # account's prefix and the Kubernetes service account the workload identity
  # binding names, so the three cannot drift apart.
  #
  # There is deliberately no entry for `qip-web`. It is a library the API links
  # and renders from — `backend/crates/apps/qip-web` declares no `[[bin]]` and has no
  # `main.rs` — so an account for it would be an identity with nothing
  # attached, which is the state this file is being changed to remove.
  #
  # Edge cells are not here either. Each gets its own account inside its own
  # module, because a cell is created and destroyed as a unit and an account
  # left behind by a removed cell is a credential nobody owns.
  service_accounts = {
    api       = "qip-api"
    fastbrain = "qip-fastbrain"
    deepbrain = "qip-deepbrain"
  }

  # Where the central plane is, from a cell's point of view: the primary
  # subnet, the pods in it, and the private Google API endpoint. Named as a
  # local so every cell gets the same answer.
  central_plane_ranges = [
    var.subnet_cidr,
    var.pod_cidr,
    local.private_google_apis,
  ]

  # Private Google access. The one range through which a workload reaches
  # Google APIs without leaving the VPC and without a route to anywhere else.
  private_google_apis = "199.36.153.8/30"

  # Where journal backups are stored. The cluster's own region unless a
  # deployment names another one, which is the difference between surviving a
  # failed disk and surviving a lost region. See modules/backup/NOT-COVERED.md.
  backup_location = var.backup_location != "" ? var.backup_location : var.region
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
  enable_console_ingress         = var.enable_console_ingress

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

  # The pod and service ranges are secondary ranges on the subnet, so pods get
  # real addresses inside the VPC and network policy applies to them.
  subnet_cidr  = var.subnet_cidr
  pod_cidr     = var.pod_cidr
  service_cidr = var.service_cidr

  # The console's route to the platform (ADR 0018). Both null in an
  # environment that has not opted in, which creates neither the subnet nor
  # the firewall rule that goes with it.
  console_egress_cidr  = var.console_egress_cidr
  api_internal_address = var.api_internal_address
}

module "cluster" {
  source = "./modules/cluster"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  network_id    = module.network.network_id
  subnet_id     = module.network.subnet_id
  pod_range     = module.network.pod_range_name
  service_range = module.network.service_range_name

  node_disk_type    = var.node_disk_type
  node_disk_size_gb = var.node_disk_size_gb

  cluster_deletion_protection = var.cluster_deletion_protection

  # Where an operator may reach the control plane from. Never the whole
  # internet: a private cluster with a public control plane is a private
  # cluster in name only.
  authorised_networks = var.authorised_networks

  # The size at creation. After that the autoscaler owns it — the node pool
  # ignores changes to this deliberately, because editing it would otherwise
  # destroy and recreate the pool.
  node_count   = var.node_count
  machine_type = var.machine_type
  kms_key_id   = module.secrets.node_encryption_key_id

  # What the pool may grow and shrink to, per zone. Until this existed the pool
  # was a fixed size and `qip-api`'s HorizontalPodAutoscaler could not reach its
  # own `maxReplicas: 6` — nothing could add a node, so the ceiling on that
  # workload was capacity rather than policy, and the autoscaler's answer to
  # load past the committed nodes was a pod stuck in `Pending`.
  min_node_count = var.min_node_count
  max_node_count = var.max_node_count

  # Dated change freezes. Empty by default: a GKE maintenance exclusion is a
  # fixed pair of timestamps rather than a recurring rule, so "not during
  # market hours" is covered by the weekly Sunday window in the module and this
  # is for the specific frozen weekend somebody has in a calendar.
  maintenance_exclusions = var.maintenance_exclusions

  # Off. Confidential VMs are real hardening and this is a decision rather than
  # a default — see the variable and modules/data/NOT-PROVISIONED.md. The short
  # version: `backend/crates/libs/qip-confidential` is statistical disclosure control
  # with no enclave and no attestation, and enabling this next to a crate with
  # that name lets the two together imply a guarantee neither provides.
  enable_confidential_nodes = var.enable_confidential_nodes

  # The nodes' own account. Previously this was the deep brain's workload
  # account, which meant a node compromise yielded that workload's permissions
  # and made the one-account-per-deployable rule untrue for the deployable that
  # holds the most. It also indexed the account map by the account *name*
  # rather than by the deployable's key, so the lookup found nothing.
  service_account = module.secrets.node_service_account_email
}

module "secrets" {
  source = "./modules/secrets"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id       = var.project_id
  project_number   = local.project_number
  region           = var.region
  environment      = var.environment
  labels           = local.labels
  service_accounts = local.service_accounts

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
    # The key an edge cell verifies a capital envelope's signature against.
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
  # This was written `!= "paper_trading"`, which is the same sentence read
  # backwards. The ladder has six rungs and `variables.tf` refuses the three
  # live ones at plan time, so the only values that reach here are
  # `observation`, `advisory` and `paper_trading` — and `!= "paper_trading"` is
  # true for exactly the two rungs *below* the paper one. The grant it guards
  # could therefore never appear for the case it was written for, and always
  # appeared for the two cases with the least use for a venue credential. The
  # concrete failure: an operator hardening dev to `observation` — the move
  # `variables.tf` explicitly invites — got a plan that *added*
  # `roles/secretmanager.secretAccessor` on the venue credential to the fast
  # brain. Lowering autonomy handed out the credential.
  #
  # Naming the live rungs makes the predicate say the property rather than its
  # complement, and it is false for every configuration that can pass
  # validation, so no plan this repository can produce creates the grant at
  # all. That is the intended state, not an accident of the list: the platform
  # is paper-trading only and nothing here should hold a venue credential.
  #
  # Written as the membership test rather than as a bare `false` because
  # `false` records the current answer and loses the question — the next reader
  # deletes the variable and with it the reason the resource exists. The cost
  # of saying it this way is stated rather than discovered: anyone who ever
  # deletes the plan-time refusal in `variables.tf` re-enables this grant as a
  # side effect, so that change must revisit this line. Two acceptance tests
  # stand in the way of it happening quietly —
  # `no_environment_can_be_applied_at_a_ceiling_that_reaches_a_real_venue` and
  # `the_venue_credential_is_unreadable_where_live_trading_is_impossible`,
  # which evaluates this predicate for every rung a plan can carry.
  venue_credential_readable = contains(["supervised_live", "limited_autonomous_live", "autonomous_live"], var.autonomy_ceiling)

  # The console's identity and its one grant, created only where the console
  # has a route to the platform at all (ADR 0018).
  console_enabled = var.console_egress_cidr != null
}

# Pods authenticate as their Google service accounts through the cluster's
# workload identity pool. These bindings live here, not in modules/secrets,
# because the pool they name — `<project_id>.svc.id.goog` — exists only once
# the cluster does, and the cluster consumes the secrets module's
# node-encryption key: a binding inside that module would need the cluster to
# precede its own dependency. The first real apply hit exactly that —
# "Identity Pool does not exist" — three times.
resource "google_service_account_iam_member" "workload_identity" {
  for_each = local.service_accounts

  service_account_id = module.secrets.service_account_names[each.key]
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[qip/${each.value}]"

  depends_on = [module.cluster]
}

module "observability" {
  source = "./modules/observability"

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  # Whether the four workload alerts can exist yet — see the variable.
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

  # The nodes pull, because the kubelet does. The workloads are listed so a
  # component can read the digest of the image it is running, which is what
  # makes a provenance claim checkable from inside the cluster.
  pull_service_accounts = concat(
    [module.secrets.node_service_account_email],
    values(module.secrets.service_account_emails),
  )
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
  writer_service_accounts = [module.secrets.service_account_emails["deepbrain"]]
  reader_service_accounts = [module.secrets.service_account_emails["api"]]
}

# The edge cells.
#
# One module, instantiated once per entry in `edge_cells`. The architecture
# calls for seven; this ships with one, and the other six are entries in a
# variable rather than six more directories. See the variable for the map, and
# docs/operations/deploying-an-edge-cell.md for what else each one needs.
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
    module.secrets.service_account_emails["deepbrain"],
  ]
  reader_service_accounts = [
    module.secrets.service_account_emails["api"],
    module.secrets.service_account_emails["deepbrain"],
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
  training_service_account = module.secrets.service_account_emails["deepbrain"]
}

module "edge_cell" {
  source   = "./modules/edge-cell"
  for_each = var.edge_cells

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment

  cell_id = each.key
  region  = each.value.region

  network_id   = module.network.network_id
  subnet_cidr  = each.value.subnet_cidr
  pod_cidr     = each.value.pod_cidr
  service_cidr = each.value.service_cidr

  # What the cell may reach, and nothing else. An empty venue map is a cell
  # that can reach no venue, which is the correct state for a cell whose
  # connectivity has not been confirmed.
  venues               = each.value.venues
  central_plane_ranges = local.central_plane_ranges

  capital_envelope_secret_id = module.secrets.secret_ids["qip-capital-envelope-key"]
  evidence_bucket            = module.evidence.bucket_name
  registry_location          = var.region
  registry_repository        = module.registry.repository_name
}

# Only an image this pipeline signed may run.
#
# The cluster above already sets `binary_authorization =
# PROJECT_SINGLETON_POLICY_ENFORCE`. Until this module existed there was no
# policy for it to enforce, so Google evaluated the implicit one — whose
# default rule is `ALWAYS_ALLOW` — and the cluster refused nothing while
# reading, in the configuration and in the console, as though it did.
#
# Not optional and deliberately not behind a flag: with enforcement already on,
# the only alternative to a deny-by-default policy is the implicit policy that
# admits everything, so an off switch here would be a switch whose off position
# is the gap. `exempt_image_patterns` is likewise not surfaced as a root
# variable — an exemption is an image that runs unsigned, and it should be a
# deliberate edit to the module rather than a line in a tfvars file.
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

  # `<location>.<name>`, which is the only form Binary Authorization matches a
  # cluster rule on. Taken from the cluster module's output so the policy
  # cannot end up naming a cluster that does not exist — a rule that matches
  # nothing is never evaluated and reports nothing.
  cluster_id = "${var.region}.${module.cluster.name}"

  # The pipeline signs. That is the honest shape of this control and its main
  # limitation: whoever can run a step in that pipeline can sign an image.
  # modules/binaryauthorization/OUT-OF-BAND.md says what a stronger
  # arrangement would be and why this repository cannot hold it.
  ci_service_account = module.cicd.service_account_email
}

# The delivery consoles on a real URL, behind an identity check.
#
# Off unless an environment turns it on: this is the only route into a cluster
# deliberately built to have none, so it is a decision recorded per
# environment rather than a default. See the module for why the identity check
# is Identity-Aware Proxy rather than the consoles' own passwords.
module "console_ingress" {
  source = "./modules/console-ingress"

  depends_on = [module.services]

  enabled     = var.enable_console_ingress
  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels
  operators   = var.console_operators
}

# Private links and direct peering.
#
# Off in every environment unless a deployment says otherwise, and the reason
# is stronger than the one for a managed database: an interconnect attachment
# is a resource waiting for a partner circuit that nobody has ordered. It
# cannot be made to work from here — a partner, a cross-connect, a pairing key
# handed over and a VLAN attachment on their side are all out of band, and
# modules/connectivity/NOT-ORDERED.md lists them in the order they happen.
#
# What it is for is written down rather than implied. The `prod` tfvars records
# that `chicago-1`, `newyork-1` and `dubai-1` run 400, 300 and 380km from the
# venues they trade, because Google Cloud has no region in those metros;
# colocation with a partner interconnect back to this VPC is the first of the
# three honest answers `docs/operations/deploying-an-edge-cell.md` names, and
# this is the VPC half of it.
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

# Backups for the state that cannot be rebuilt.
#
# `docs/operations/disaster-recovery.md` named the gap precisely: the edge cell
# journal claims are `Retain`, and retained is not backed up. `Retain` stops
# Kubernetes deleting a disk when the claim goes away and does nothing about a
# disk that fails, a project that is deleted or a region that is lost. The
# journal is what a cell decided and why, and it is the one record that cannot
# be recomputed from the feed or from the venue.
#
# Not behind a flag, and for the same reason as Binary Authorization: a switch
# whose off position is the gap the runbook already documents would leave that
# gap in place and add a line to the configuration implying otherwise.
# `backup_paused` is the honest form of "not right now" — it keeps the plan, the
# key and the retention and suspends the schedule.
#
# Two mechanisms, which is deliberate rather than belt and braces. Backup for
# GKE selects by namespace and needs nobody to remember anything, and it stops
# covering a journal the moment its claim is deleted. A Compute Engine snapshot
# schedule keeps covering the disk after that — the `Retain` reclaim policy in
# `journal-storage.yaml` exists so that disk survives the claim — and protects
# nothing until somebody attaches it to a disk, which Terraform cannot do
# because the disks are named `pvc-<uuid>` and created long after any apply.
# `journal_snapshot_attachment_command` is that step. See the module.
module "backup" {
  source = "./modules/backup"

  project_id     = var.project_id
  project_number = local.project_number
  environment    = var.environment
  labels         = local.labels

  # In the platform's existing key ring, like the evidence and model keys.
  key_ring_id = module.secrets.key_ring_id

  # From the cluster's output rather than assembled here: a plan naming a
  # cluster that does not exist is accepted by the API and protects nothing.
  cluster_id     = module.cluster.id
  cluster_region = var.region

  # The cluster's own region by default. That covers a failed disk, a deleted
  # PersistentVolume and an operator error, and does not cover losing the
  # region — the `journal_backup` output reports which of the two this is.
  backup_location = local.backup_location

  backup_schedule  = var.backup_schedule
  backup_paused    = var.backup_paused
  retain_days      = var.backup_retain_days
  delete_lock_days = var.backup_delete_lock_days

  # The disk-level half. Offset in time from the plan above so two mechanisms
  # do not read the same volume in the same minute, and retained longer,
  # because these are what still covers a journal whose claim has been deleted
  # — a decommissioned cell whose record somebody may still have to answer for.
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
# Turning it on where the organisation has not activated SCC creates two
# detectors that are stored, never run, and read in the console as a project
# being watched. That is worse than the gap it replaces: an absent control is
# visibly absent, and a control that never fires looks like a clean result.
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
