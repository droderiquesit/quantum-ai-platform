# The GitOps control plane (ADR 0036): one GKE Autopilot cluster per
# environment that runs three controllers and no trading binary.
#
# ADR 0024 retired the GKE runtime because the platform's binaries do not
# need a scheduler: every warm binary is a Cloud Run service and the
# execution node is a bare machine. ADR 0036 brings back a cluster for one
# reason only — Argo CD, Kargo and Config Connector are Kubernetes
# controllers and have nowhere else to run — and it is shaped so that the
# reason cannot quietly widen:
#
#   * private nodes and a private endpoint, reachable from the management
#     trust zone's range and nowhere else. There is no public address to
#     harden because there is none at all; a workflow reaches the API server
#     through the fleet's Connect gateway, which is a Google API call and not
#     a route into the VPC.
#   * Autopilot, so there is no node pool for a person to schedule a
#     `qip-*` image onto by hand, and every Pod is admitted by the same
#     Binary Authorization policy the Cloud Run services are — the project
#     policy, one rule, no exemptions. A controller image runs here only
#     after `vendor.yml` mirrored and attested it.
#   * Config Connector installed by the bootstrap, as a vendored operator
#     manifest under infrastructure/gitops/bootstrap/config-connector-operator,
#     rather than by a Helm or Kubernetes provider — so the provider set
#     stays `google`/`google-beta` and `scripts/check-terraform-providers.sh`
#     has nothing new to refuse. ADR 0036 wrote "installed as the GKE
#     addon"; the API refused that on the first apply of this module
#     (infra.yml runs 34 and 35, 2026-09-05: `addons {"config-connector"}
#     are not supported for Autopilot clusters`), and Autopilot is the
#     property that keeps a `qip-*` image off the cluster, so the addon is
#     what gave way. This header used to repeat the ADR's sentence; it was
#     false for as long as this module existed.
#   * etcd encrypted with a key in the environment's own ring, because the
#     cluster holds the two GitHub App private keys as Kubernetes Secrets
#     (decision 3), and a Secret at rest in an etcd Google encrypts with a
#     key Google holds is a secret one identity short of being ours.
#
# Nothing in this module ever runs a `qip-*` image, and the acceptance suite
# refuses a Pod spec that names one. The three identities below are the whole
# of what the cluster may do to Google Cloud, and each holds the narrowest
# grant that performs its one job.

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

locals {
  prefix = "qip-${var.environment}"
  name   = "${local.prefix}-control-plane"

  # The four addresses of the restricted VIP, the same ones
  # `modules/network`'s `googleapis.com` zone answers with. Repeated here
  # rather than exported from that module because two private zones with
  # different names carry them, and a zone's records belong beside the zone.
  restricted_vip = ["199.36.153.8", "199.36.153.9", "199.36.153.10", "199.36.153.11"]

  # The Kubernetes service accounts that act as each Google identity, as
  # Workload Identity members. Each is `namespace/name` of a controller the
  # bootstrap installs; a binding for a KSA that does not exist grants
  # nothing, so an unlisted controller cannot borrow an identity.
  workload_identity_members = {
    kcc = ["cnrm-system/cnrm-controller-manager"]
    # The repo-server is what clones with the App credential; the proving
    # hook (decision 7) reads the serving revision back as the reconciler.
    # The hook's service account lives in `qip-run` beside the RunService
    # manifests it proves — infrastructure/gitops/envs/<env>/kustomization.yaml
    # sets that namespace on everything in the directory — and a binding
    # written for the wrong namespace grants nothing, silently.
    argocd = ["argocd/argocd-repo-server", "qip-run/qip-prove-serving"]
    # The controller runs the Warehouse's discovery against the registry and
    # the promotion's git push.
    kargo = ["kargo/kargo-controller"]
  }

  wi_bindings = merge([
    for identity, members in local.workload_identity_members : {
      for member in members : "${identity}:${member}" => {
        identity = identity
        member   = "serviceAccount:${var.project_id}.svc.id.goog[${member}]"
      }
    }
  ]...)
}

# --- the etcd key -------------------------------------------------------------

# Symmetric, in the platform's ring, rotated. The GKE service agent encrypts
# and decrypts with it; nothing else is granted on it.
resource "google_kms_crypto_key" "etcd" {
  name     = "${local.name}-etcd"
  key_ring = var.key_ring_id
  purpose  = "ENCRYPT_DECRYPT"

  # Ninety days, like the secrets key. Every version stays valid for what it
  # encrypted, so a rotation is a new version and never an outage.
  rotation_period = "7776000s"

  # Destroying the key makes every Secret in etcd unreadable at once, and the
  # failure appears on the next control-plane restart rather than now.
  lifecycle {
    prevent_destroy = true
  }

  labels = var.labels
}

# The GKE service agent, by project number. Without this the cluster create
# fails after the network peering exists, naming a key the agent cannot use.
resource "google_kms_crypto_key_iam_member" "gke_uses_etcd_key" {
  crypto_key_id = google_kms_crypto_key.etcd.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = "serviceAccount:service-${var.project_number}@container-engine-robot.iam.gserviceaccount.com"
}

# --- the cluster --------------------------------------------------------------

resource "google_container_cluster" "control_plane" {
  # The key grant before the cluster that needs it: Terraform infers no
  # dependency from a key *name* in `database_encryption`. The fleet grant
  # for the same reason: the `fleet` block below registers a membership as
  # the caller, and Terraform infers nothing from that either. Both are
  # consumed by the one create call, so both must exist before it — and
  # IAM propagation being what it is, a binding created seconds before the
  # call can still be refused; a second `up` then finds it held.
  depends_on = [
    google_kms_crypto_key_iam_member.gke_uses_etcd_key,
    google_project_iam_member.infra_registers_fleet,
  ]

  project  = var.project_id
  name     = local.name
  location = var.region

  # Autopilot: no node pool to schedule onto by hand, and every Pod admitted
  # by the project's Binary Authorization policy.
  enable_autopilot = true

  network    = var.network_id
  subnetwork = var.management_subnet_id

  # VPC-native, with the Pod and Service ranges chosen by GKE inside the
  # management subnet as GKE-managed secondary ranges. Named ranges would be
  # two more CIDRs in the tfvars for a cluster nothing routes to by Pod
  # address; the primary range is the one every firewall rule names, and
  # egress to the internet is masqueraded to it by the node.
  ip_allocation_policy {}

  # A cluster deleted by a plan nobody read takes every controller and both
  # App keys with it. Removing one is a deliberate two-step by a person.
  deletion_protection = true

  # Private in both directions. The endpoint has no public address at all,
  # rather than one behind an allowlist, and the only range that may reach
  # the private one is the management zone's — which is where an operator's
  # bastion would be, and where nothing is today.
  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = true
    master_ipv4_cidr_block  = var.master_ipv4_cidr_block

    master_global_access_config {
      enabled = false
    }
  }

  master_authorized_networks_config {
    gcp_public_cidrs_access_enabled = false

    cidr_blocks {
      cidr_block   = var.management_subnet_cidr
      display_name = "management trust zone"
    }
  }

  # Workload Identity, so a controller's Kubernetes service account acts as
  # exactly one Google identity below and holds no key of its own.
  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  release_channel {
    channel = "REGULAR"
  }

  # The same project policy every Cloud Run revision is evaluated against
  # (modules/binaryauthorization): deny by default, admit what the attestor
  # signed. `PROJECT_SINGLETON_POLICY_ENFORCE` is the only value that
  # evaluates a policy at all; `DISABLED` is the implicit-allow ADR 0024's
  # GKE runtime discovered it had.
  binary_authorization {
    evaluation_mode = "PROJECT_SINGLETON_POLICY_ENFORCE"
  }

  # No `addons_config` for Config Connector. There was one — `enabled = true`
  # under `config_connector_config` — and the API refused the whole create
  # with it: `addons {"config-connector"} are not supported for Autopilot
  # clusters` (infra.yml runs 34 and 35, 2026-09-05). The operator is a
  # vendored manifest the bootstrap applies (infrastructure/gitops/bootstrap/
  # config-connector-operator), then the one `ConfigConnector` object naming
  # the identity, and every `RunService` under infrastructure/gitops/envs/ is
  # reconciled by it. Nothing about the provider set changed: the operator
  # is `kubectl apply` of reviewed bytes, like the three controllers beside
  # it, and not a `kubernetes` or `helm` provider.

  # etcd under the platform's key. See the header.
  database_encryption {
    state    = "ENCRYPTED"
    key_name = google_kms_crypto_key.etcd.id
  }

  # Registered to the project's fleet, which is what the Connect gateway
  # routes through. A private endpoint is unreachable from a GitHub runner
  # by construction; the gateway is the one path in that is an
  # authenticated Google API call rather than a route into the VPC.
  fleet {
    project = var.project_id
  }

  # Every node carries the management zone's tag, so the zone's default deny
  # and its one egress allowlist are the rules that bind the controllers'
  # traffic. A node without the tag is a node outside every zone.
  node_pool_auto_config {
    network_tags {
      tags = [var.management_network_tag]
    }
  }

  resource_labels = var.labels

  lifecycle {
    precondition {
      condition     = var.master_ipv4_cidr_block != null
      error_message = "gitops_enabled is true and no master_ipv4_cidr_block was given. The control plane's endpoint needs a /28 of its own that overlaps no subnet; set gitops_master_ipv4_cidr_block in the environment's tfvars."
    }
  }
}

# --- the registries, without a route to the internet -------------------------
#
# A Cloud Run service pulls through Google's own path; a GKE node pulls like
# a VM, by name, and `*.pkg.dev` and `gke.gcr.io` are not under the
# `googleapis.com` zone `modules/network` sends to the restricted VIP. With
# only that zone, every image pull from a private node resolved a public
# address, the management zone's egress deny dropped it, and the cluster
# read as a scheduler that could not start a Pod. These two zones send both
# registry names to the same four addresses, on the same rule the zone
# already has for Google APIs.
resource "google_dns_managed_zone" "registry" {
  for_each = {
    "pkg-dev" = "pkg.dev."
    "gcr-io"  = "gcr.io."
  }

  project     = var.project_id
  name        = "${local.prefix}-${each.key}"
  dns_name    = each.value
  description = "Sends ${each.value} to the restricted VIP so the control plane pulls its controller images over Private Google Access."
  visibility  = "private"

  private_visibility_config {
    networks {
      network_url = var.network_id
    }
  }

  labels = var.labels
}

resource "google_dns_record_set" "registry_apex" {
  for_each = google_dns_managed_zone.registry

  project      = var.project_id
  managed_zone = each.value.name
  name         = each.value.dns_name
  type         = "A"
  ttl          = 300
  rrdatas      = local.restricted_vip
}

resource "google_dns_record_set" "registry_wildcard" {
  for_each = google_dns_managed_zone.registry

  project      = var.project_id
  managed_zone = each.value.name
  name         = "*.${each.value.dns_name}"
  type         = "A"
  ttl          = 300
  rrdatas      = local.restricted_vip
}

# --- the three identities -----------------------------------------------------

resource "google_service_account" "kcc" {
  project      = var.project_id
  account_id   = "${local.prefix}-kcc"
  display_name = "qip Config Connector (${var.environment})"
  description  = "Applies the RunService manifests under infrastructure/gitops/envs/ to Cloud Run. Acts through Workload Identity; it has no key."
}

resource "google_service_account" "argocd" {
  project      = var.project_id
  account_id   = "${local.prefix}-argocd"
  display_name = "qip Argo CD (${var.environment})"
  description  = "Reads the repository with the read-only GitHub App and reads the serving revision back to prove a sync. Acts through Workload Identity; it has no key."
}

resource "google_service_account" "kargo" {
  project      = var.project_id
  account_id   = "${local.prefix}-kargo"
  display_name = "qip Kargo (${var.environment})"
  description  = "Discovers attested digests in the registry and commits promotions with the write-scoped GitHub App. Acts through Workload Identity; it has no key."
}

locals {
  identities = {
    kcc    = google_service_account.kcc
    argocd = google_service_account.argocd
    kargo  = google_service_account.kargo
  }
}

# Which Kubernetes service accounts may act as each identity. One binding
# per controller KSA, and nothing project-wide: the `[namespace/name]`
# member is the whole of what may impersonate.
resource "google_service_account_iam_member" "workload_identity" {
  for_each = local.wi_bindings

  service_account_id = local.identities[each.value.identity].name
  role               = "roles/iam.workloadIdentityUser"
  member             = each.value.member
}

# Config Connector: create and update Cloud Run services and their invoker
# bindings. `run.admin` rather than `run.developer` because the invoker
# bindings — the console on the API, `allUsers` on OpenObserve (ADR 0030) —
# left `modules/cloudrun` with the service and are now `IAMPolicyMember`
# manifests it applies, and `developer` cannot set a service's IAM policy.
# `iam.serviceAccountUser` on each workload's own account is granted by
# `modules/cloudrun` per workload, never project-wide.
resource "google_project_iam_member" "kcc_runs" {
  project = var.project_id
  role    = "roles/run.admin"
  member  = "serviceAccount:${google_service_account.kcc.email}"
}

# Read a secret's metadata — that it exists, its versions — and never a
# payload. A `RunService` names secrets by id, and the reconciler resolves
# the reference before it asks Cloud Run to mount it; the value is read by
# the workload's own identity at container start, as it always was.
resource "google_project_iam_member" "kcc_sees_secrets" {
  project = var.project_id
  role    = "roles/secretmanager.viewer"
  member  = "serviceAccount:${google_service_account.kcc.email}"
}

# Argo CD's proving hook (ADR 0036 decision 7) reads the serving revision
# through the Cloud Run API and compares its digest to the manifest's. Read
# only: the reconciler proves what it applied and changes nothing itself.
resource "google_project_iam_member" "argocd_reads_run" {
  project = var.project_id
  role    = "roles/run.viewer"
  member  = "serviceAccount:${google_service_account.argocd.email}"
}

# Each controller reads exactly the App credential that is its own, and no
# other secret in the project. Scoped to the secret, like every accessor
# grant in `modules/cloudrun`.
resource "google_secret_manager_secret_iam_member" "argocd_reads_its_app" {
  project   = var.project_id
  secret_id = var.argocd_app_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.argocd.email}"
}

resource "google_secret_manager_secret_iam_member" "kargo_reads_its_app" {
  project   = var.project_id
  secret_id = var.kargo_app_secret_id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.kargo.email}"
}

# Kargo's Warehouse lists the tags and reads the manifests of the images the
# pipeline attested, in this environment's one repository. Repository-scoped
# rather than the project role, so the identity that decides what is
# promoted cannot read a registry it was not pointed at.
resource "google_artifact_registry_repository_iam_member" "kargo_reads_registry" {
  project    = var.project_id
  location   = var.region
  repository = var.registry_repository_name
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${google_service_account.kargo.email}"
}

# --- what the bootstrap may do ------------------------------------------------
#
# `infra.yml`'s bootstrap step applies the vendored controller manifests
# through the Connect gateway as the infrastructure account. That is two
# things: reaching the API server, and being authorised inside it. The first
# is `gkehub.gatewayEditor`, the predefined role that carries
# `gkehub.gateway.*` and nothing about clusters. The second is a custom role
# holding the `container.*` permissions `kubectl apply` of these manifests
# exercises and no more — deliberately not `roles/container.admin`, which the
# acceptance suite refuses in the workflow's self-grant loop because it adds
# every object in the cluster to an account that needs a fixed list of them.
#
# The list is what the vendored manifests contain: namespaces, CRDs, RBAC,
# service accounts, config maps, the two App-key secrets, deployments, a
# stateful set, services, network policies, webhooks, a cron job, and the
# custom resources of the three controllers (`thirdPartyObjects`). A kind
# added to a bootstrap manifest without its verbs here fails the apply
# naming the permission, which is the correct place to find out.
resource "google_project_iam_custom_role" "bootstrap" {
  project     = var.project_id
  role_id     = "qipGitopsBootstrap_${var.environment}"
  title       = "qip GitOps bootstrap (${var.environment})"
  description = "Apply the vendored controller manifests to the control-plane cluster. No delete, no pod exec, nothing outside the kinds the bootstrap carries."

  permissions = [
    "container.clusters.get",
    "container.clusters.getCredentials",
    "container.namespaces.create",
    "container.namespaces.get",
    "container.namespaces.list",
    "container.namespaces.update",
    "container.customResourceDefinitions.create",
    "container.customResourceDefinitions.get",
    "container.customResourceDefinitions.list",
    "container.customResourceDefinitions.update",
    "container.clusterRoles.create",
    "container.clusterRoles.get",
    "container.clusterRoles.list",
    "container.clusterRoles.update",
    "container.clusterRoles.bind",
    "container.clusterRoles.escalate",
    "container.clusterRoleBindings.create",
    "container.clusterRoleBindings.get",
    "container.clusterRoleBindings.list",
    "container.clusterRoleBindings.update",
    "container.roles.create",
    "container.roles.get",
    "container.roles.list",
    "container.roles.update",
    "container.roles.bind",
    "container.roles.escalate",
    "container.roleBindings.create",
    "container.roleBindings.get",
    "container.roleBindings.list",
    "container.roleBindings.update",
    "container.serviceAccounts.create",
    "container.serviceAccounts.get",
    "container.serviceAccounts.list",
    "container.serviceAccounts.update",
    "container.configMaps.create",
    "container.configMaps.get",
    "container.configMaps.list",
    "container.configMaps.update",
    "container.secrets.create",
    "container.secrets.get",
    "container.secrets.list",
    "container.secrets.update",
    "container.deployments.create",
    "container.deployments.get",
    "container.deployments.list",
    "container.deployments.update",
    "container.statefulSets.create",
    "container.statefulSets.get",
    "container.statefulSets.list",
    "container.statefulSets.update",
    "container.services.create",
    "container.services.get",
    "container.services.list",
    "container.services.update",
    "container.networkPolicies.create",
    "container.networkPolicies.get",
    "container.networkPolicies.list",
    "container.networkPolicies.update",
    "container.cronJobs.create",
    "container.cronJobs.get",
    "container.cronJobs.list",
    "container.cronJobs.update",
    "container.jobs.create",
    "container.jobs.get",
    "container.jobs.list",
    "container.jobs.update",
    "container.mutatingWebhookConfigurations.create",
    "container.mutatingWebhookConfigurations.get",
    "container.mutatingWebhookConfigurations.list",
    "container.mutatingWebhookConfigurations.update",
    "container.validatingWebhookConfigurations.create",
    "container.validatingWebhookConfigurations.get",
    "container.validatingWebhookConfigurations.list",
    "container.validatingWebhookConfigurations.update",
    "container.thirdPartyObjects.create",
    "container.thirdPartyObjects.get",
    "container.thirdPartyObjects.list",
    "container.thirdPartyObjects.update",
    "container.pods.get",
    "container.pods.list",
  ]
}

resource "google_project_iam_member" "infra_bootstraps" {
  project = var.project_id
  role    = google_project_iam_custom_role.bootstrap.id
  member  = "serviceAccount:${var.infra_service_account}"
}

resource "google_project_iam_member" "infra_reaches_gateway" {
  for_each = toset([
    "roles/gkehub.gatewayEditor",
    "roles/gkehub.viewer",
  ])

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${var.infra_service_account}"
}

# --- registering the cluster with the fleet ------------------------------------
#
# The cluster's `fleet` block registers a membership at create time, as the
# caller — the infrastructure account — and neither role above carries the
# permission that needs. infra.yml run 34 (2026-09-05, the first apply of
# this module) created 82 of its 83 resources and refused the cluster with
#
#   generic::permission_denied: Permission 'gkehub.memberships.create'
#   denied on 'projects/algorik-dev/locations/us-east4/memberships/
#   qip-dev-control-plane'
#
# and run 35, planning only the cluster, refused it the same way. The
# predefined roles that carry the permission are `gkehub.editor` and
# `gkehub.admin`, which also carry every feature, scope and binding in the
# fleet API; this custom role carries the one permission the create needs,
# the read that refreshes it, and the delete that the cluster's own removal
# — a deliberate two-step by a person, `deletion_protection` above — issues
# when GKE unregisters it. Nothing about fleet features, scopes or other
# clusters' memberships. Same discipline as the bootstrap role above and the
# workflow's self-grant loop: the one missing permission, named for the run
# that found it.
resource "google_project_iam_custom_role" "fleet_registrar" {
  project     = var.project_id
  role_id     = "qipGitopsFleetRegistrar_${var.environment}"
  title       = "qip GitOps fleet registrar (${var.environment})"
  description = "Register the control-plane cluster with the project's fleet and unregister it. No features, no scopes, nothing about another membership."

  permissions = [
    "gkehub.memberships.create",
    "gkehub.memberships.get",
    "gkehub.memberships.delete",
  ]
}

resource "google_project_iam_member" "infra_registers_fleet" {
  project = var.project_id
  role    = google_project_iam_custom_role.fleet_registrar.id
  member  = "serviceAccount:${var.infra_service_account}"
}
