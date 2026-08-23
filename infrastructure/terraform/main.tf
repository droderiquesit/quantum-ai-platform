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

locals {
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
  # and renders from — `crates/apps/qip-web` declares no `[[bin]]` and has no
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
}

module "network" {
  source = "./modules/network"

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  # The pod and service ranges are secondary ranges on the subnet, so pods get
  # real addresses inside the VPC and network policy applies to them.
  subnet_cidr  = var.subnet_cidr
  pod_cidr     = var.pod_cidr
  service_cidr = var.service_cidr
}

module "cluster" {
  source = "./modules/cluster"

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  network_id    = module.network.network_id
  subnet_id     = module.network.subnet_id
  pod_range     = module.network.pod_range_name
  service_range = module.network.service_range_name

  # Where an operator may reach the control plane from. Never the whole
  # internet: a private cluster with a public control plane is a private
  # cluster in name only.
  authorised_networks = var.authorised_networks

  node_count   = var.node_count
  machine_type = var.machine_type
  kms_key_id   = module.secrets.node_encryption_key_id

  # The nodes' own account. Previously this was the deep brain's workload
  # account, which meant a node compromise yielded that workload's permissions
  # and made the one-account-per-deployable rule untrue for the deployable that
  # holds the most. It also indexed the account map by the account *name*
  # rather than by the deployable's key, so the lookup found nothing.
  service_account = module.secrets.node_service_account_email
}

module "secrets" {
  source = "./modules/secrets"

  project_id       = var.project_id
  project_number   = var.project_number
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

  # The venue credential is readable only by an environment that could use it.
  # An environment that cannot trade live has no business holding one.
  venue_credential_readable = var.autonomy_ceiling != "paper_trading"
}

module "observability" {
  source = "./modules/observability"

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  # Alerting thresholds. The kill-switch alert has no threshold: any trip is
  # worth waking someone for.
  notification_channels = var.notification_channels
}

module "cicd" {
  source = "./modules/cicd"

  project_id  = var.project_id
  environment = var.environment

  # Which repository may impersonate the pipeline account. No default: see the
  # variable.
  github_repository = var.github_repository
}

module "registry" {
  source = "./modules/registry"

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
