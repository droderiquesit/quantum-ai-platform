# A real URL for the delivery consoles, with an identity check in front of it.
#
# Argo CD and Kargo are administrative control planes: whoever reaches them
# decides what runs in the cluster. Everything else in this configuration is
# built so that nothing routes in from the internet — private nodes, no public
# control-plane endpoint, every namespace denying ingress by default — and
# publishing a console is the one deliberate hole in that.
#
# So the hole is cut where the identity check is, not where the password is.
# Identity-Aware Proxy authenticates the request at Google's edge, before it
# reaches a backend at all: an unauthenticated request never touches Argo CD's
# login page, and the people who may pass are an IAM list in this file rather
# than whoever holds a shared password. Argo CD's own login still happens
# afterwards, so the console is behind two checks rather than one.
#
# What is deliberately NOT here: the OAuth client. IAP used to require a brand
# and client created through the IAP OAuth Admin APIs, which Google shut down
# in March 2026 — `gcloud iap oauth-brands` now refuses on any project that
# never had one. The replacement is IAP's Google-managed client, which the
# BackendConfig in the manifests turns on with `iap.enabled` and no secret at
# all. That is the better arrangement for the same reason the rest of this
# platform has no service-account keys: a client secret in Terraform is a
# client secret in every state backup.

# The address the certificate is issued for and DNS resolves to. Reserved
# rather than ephemeral because a managed certificate is bound to the name,
# the name is derived from the address, and an address that changed on
# recreation would silently invalidate the certificate that names it.
resource "google_compute_global_address" "console" {
  count   = var.enabled ? 1 : 0
  project = var.project_id
  name    = "qip-${var.environment}-console"

  # Global, because this fronts an external Application Load Balancer, which
  # is the only ingress class that speaks IAP.
  address_type = "EXTERNAL"
  ip_version   = "IPV4"

  labels = var.labels
}

# Kargo's own address.
#
# A second address rather than a second host on the first Ingress, because an
# Ingress may only name Services in its own namespace and Kargo's UI is in
# `kargo` while Argo CD's is in `argocd`. Two Ingresses therefore, and each
# external Application Load Balancer wants an address of its own.
resource "google_compute_global_address" "kargo" {
  count        = var.enabled ? 1 : 0
  project      = var.project_id
  name         = "qip-${var.environment}-kargo"
  address_type = "EXTERNAL"
  ip_version   = "IPV4"
  labels       = var.labels
}

# Who may pass the proxy.
#
# One grant covers both consoles: `google_iap_web_iam_member` binds at the
# project's IAP resource, which is the parent of every backend service in it.
# Adding a third console later therefore adds no access decision — which is
# the arrangement to keep, because an access list that has to be remembered
# separately per console is one that will eventually be forgotten for one.
#
# `roles/iap.httpsResourceAccessor` is the whole access decision: an identity
# not on this list is refused by Google before the request reaches the load
# balancer's backend, which is why the list is short and lives in a reviewed
# file. It is deliberately not `allAuthenticatedUsers` — that admits every
# Google account in the world, which is a different thing from admitting the
# desk.
resource "google_iap_web_iam_member" "console_operators" {
  for_each = var.enabled ? toset(var.operators) : toset([])
  project  = var.project_id
  role     = "roles/iap.httpsResourceAccessor"
  member   = each.value
}
