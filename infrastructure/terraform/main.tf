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
  service_accounts = {
    api       = "qip-api"
    fastbrain = "qip-fastbrain"
    deepbrain = "qip-deepbrain"
  }
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

  node_count      = var.node_count
  machine_type    = var.machine_type
  kms_key_id      = module.secrets.node_encryption_key_id
  service_account = module.secrets.service_account_emails[local.service_accounts.deepbrain]
}

module "secrets" {
  source = "./modules/secrets"

  project_id       = var.project_id
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
