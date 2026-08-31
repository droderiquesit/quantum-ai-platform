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
# `roles/iap.httpsResourceAccessor` is the whole access decision: an identity
# not on this list is refused by Google before the request reaches the load
# balancer's backend, which is why the list is short and lives in a reviewed
# file. It is deliberately not `allAuthenticatedUsers` — that admits every
# Google account in the world, which is a different thing from admitting the
# desk.
#
# `_type_compute_` is load-bearing, and the first attempt used the wrong one.
# IAP's IAM resources form a hierarchy, and a binding is only read at or below
# the node it is attached to:
#
#     projects/{p}/iap_web                      google_iap_web_iam_member
#     projects/{p}/iap_web/compute              this resource
#     projects/{p}/iap_web/compute/services/{s} google_iap_web_backend_service_iam_member
#
# A GKE Ingress backend lives at the third line, and `google_iap_web_iam_member`
# attaches to the first — which does not cascade. The apply succeeded, the
# console_operators list looked granted, and the desk got
# "You don't have access" from IAP with `gcloud iap web get-iam-policy
# --resource-type=backend-services` returning an empty policy. That is the
# worst shape a permission bug takes: everything reports success and the
# grant simply is not where the check reads.
#
# The middle line is chosen over the third deliberately. GKE names a backend
# service after the cluster, namespace, service and port
# (`k8s1-92e95901-argocd-argocd-server-443-…`), so a per-service binding would
# have to predict a generated name and would silently stop applying when the
# Service is recreated under a new one. Binding at `compute` covers every
# backend service in the project, so a third console later adds no access
# decision — which is the arrangement to keep, because a list that must be
# remembered per console is one that will eventually be forgotten for one.
resource "google_iap_web_type_compute_iam_member" "console_operators" {
  for_each = var.enabled ? toset(var.operators) : toset([])
  project  = var.project_id
  role     = "roles/iap.httpsResourceAccessor"
  member   = each.value
}
