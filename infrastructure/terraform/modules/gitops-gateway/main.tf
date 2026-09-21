# The public front door to the GitOps control plane, and the thing that
# stands in front of it.
#
# ADR 0036 built the cluster with a private endpoint and no public address,
# and this module is the deliberate exception to that: an operator has to be
# able to see what Argo CD is reconciling and what Kargo is promoting, and
# `kubectl port-forward` through the Connect gateway is not an interface a
# person uses daily. What this module does *not* do is make the control plane
# publicly reachable. It makes it reachable to a named set of Google
# identities and to nobody else.
#
# ## The order of the gates, and why that order
#
# A request arrives at a Google load balancer, which terminates TLS on a
# Google-managed certificate, and is then handed to **Identity-Aware Proxy**.
# IAP checks the caller against `roles/iap.httpsResourceAccessor` on this
# backend *before* the request reaches the cluster at all. An unauthenticated
# request — or one from an identity nobody granted — is refused by Google's
# own front end and never becomes a packet on the VPC.
#
# That ordering is the whole design. A reverse proxy that forwards first and
# authenticates second has already given an attacker the thing being
# protected; IAP authenticates first, so Argo CD's login page is not an
# internet-facing surface at all. The cluster keeps its private endpoint and
# private nodes; nothing here opens the API server.
#
# ## Why there is still a password behind it
#
# Argo CD ships with `admin.enabled: "false"` and Dex deleted here, which
# means it has no identity to authenticate *as*. IAP alone would therefore
# put an operator through a Google login and land them on a page with no
# account to use. So the bootstrap seeds an admin credential from Secret
# Manager, and the two gates are independent: IAP decides who may reach the
# service, and Argo CD decides who may act on it. A misconfigured IAP policy
# still meets a password, and a leaked password still meets IAP. Either one
# alone would be a single point of failure guarding a controller that can
# reconcile arbitrary manifests into the cluster.
#
# ## The address is reserved here and the DNS record is not
#
# `google_compute_global_address` reserves a stable anycast IPv4 address that
# survives the load balancer being destroyed and recreated. The A record
# pointing at it lives at the registrar, outside this repository and outside
# this project — `algorik.ai` answers from `dns1.registrar-servers.com` — so
# the address is an **output** and the record is a manual step, named in the
# module's README rather than pretended away. A Google-managed certificate
# does not provision until that record resolves, which is the feedback that
# the step was done.

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    # `google_project_service_identity` is beta-only. A module that uses a
    # provider it does not declare inherits nothing and fails at *plan*,
    # after `validate` has already passed — which is why
    # `scripts/check-terraform-providers.sh` exists and why it caught this
    # rather than apply day catching it.
    google-beta = {
      source = "hashicorp/google-beta"
    }
  }
}

locals {
  prefix = "qip-${var.environment}"
  # Every hostname this front door answers for, in one list, because the
  # certificate and the IAP brand both need exactly the same set and two
  # lists that must agree will not.
  hostnames = [var.argocd_hostname, var.kargo_hostname]
}

# --- the address --------------------------------------------------------------

# Global, because the load balancer in front of the cluster is a global
# external Application Load Balancer and a regional address cannot serve one.
# Reserved rather than ephemeral so the DNS record the operator creates at the
# registrar keeps pointing at something after a `terraform destroy` of the
# Gateway — an ephemeral address would be returned to Google and the record
# would then point at whatever Google handed the next customer.
resource "google_compute_global_address" "gitops" {
  project     = var.project_id
  name        = "${local.prefix}-gitops"
  description = "The GitOps control plane's front door. The A records for ${join(" and ", local.hostnames)} point here, and are created at the registrar by hand."
  ip_version  = "IPV4"
}

# --- the certificate ----------------------------------------------------------

# Google-managed, so there is no private key in this repository, in state, or
# on any disk an operator touches. It provisions only once the A record
# resolves to the address above: Google proves control of the name by
# answering an HTTP challenge on it. A certificate stuck in PROVISIONING for
# more than about fifteen minutes means the DNS record is missing or points
# somewhere else, and that is the first thing to check.
resource "google_compute_managed_ssl_certificate" "gitops" {
  project = var.project_id
  name    = "${local.prefix}-gitops"

  managed {
    domains = local.hostnames
  }

  # A certificate cannot change the names it covers, so adding a hostname
  # replaces it. Create the replacement before destroying the one serving
  # traffic, or every request fails TLS for as long as the new one takes to
  # provision — which is minutes, not seconds.
  lifecycle {
    create_before_destroy = true
  }
}

# --- Identity-Aware Proxy -----------------------------------------------------

# There is deliberately no OAuth brand or client here.
#
# The first draft created a `google_iap_brand` and a `google_iap_client`,
# which is how IAP was configured for years. `terraform validate` refused it
# as deprecated — the resource stops working after July 2025 — and the
# replacement is better rather than merely newer: GKE's Gateway controller
# can enable IAP with **Google-managed OAuth**, so there is no client secret
# to mint, store, rotate or leak, and no project-level singleton that cannot
# be deleted once created.
#
# What remains is the only thing that ever mattered: the access list below,
# and `iap.enabled` on the backend policy the Gateway applies. Those two
# together are the gate.

# --- who may pass IAP ---------------------------------------------------------

# The whole access list, and the only thing standing between the internet and
# a controller that can reconcile arbitrary manifests into this cluster.
#
# `roles/iap.httpsResourceAccessor` is granted at the *project* level here
# rather than per-backend, because the backend service a GKE Gateway creates
# is named by the controller at reconcile time and does not exist as a
# Terraform address. The cost of that is stated rather than hidden: this
# grant covers every IAP-protected backend in the project, and today the
# GitOps Gateway is the only one. A second IAP-protected service would need
# this narrowed to per-resource bindings first.
resource "google_project_iam_member" "iap_accessors" {
  for_each = toset(var.iap_members)

  project = var.project_id
  role    = "roles/iap.httpsResourceAccessor"
  member  = each.value
}

# IAP's own service agent must be able to invoke the backend. Without this the
# load balancer authenticates the caller correctly and then returns 403 from
# the backend, which reads as an access-list problem and is not one.
resource "google_project_service_identity" "iap" {
  provider = google-beta
  project  = var.project_id
  service  = "iap.googleapis.com"
}
