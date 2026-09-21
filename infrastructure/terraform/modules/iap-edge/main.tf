# One Identity-Aware Proxy front door, in front of one Cloud Run service.
#
# ADR 0094. This module exists because the other two front doors in this tree
# cannot do this job, and the reasons are facts rather than preferences:
#
#   * `modules/gitops-gateway` stands up the address and the certificate for a
#     **GKE Gateway**, and a Gateway routes to in-cluster Services. An
#     `HTTPRoute`'s `backendRef` names a `Service` or a `ServiceImport`; a
#     `GCPBackendPolicy`'s `targetRef` is `kind: Service`. There is no backend
#     kind in the Gateway API, nor in GKE's `networking.gke.io` extensions,
#     that names a serverless network endpoint group — and the URL map behind
#     that Gateway is the controller's, so a serverless backend attached to it
#     from outside is drift the controller reverts.
#   * `modules/public-edge` owns the only serverless NEG in the tree, and it
#     is the **anonymous customer edge**: a Cloud CDN bucket as its default
#     backend, a static shell, and `hostnames = []` in all four environments
#     on purpose. Putting an operator console behind it means lighting up five
#     resources nobody asked for so that one can be used, and hollowing out
#     the argument its own README makes.
#
# So this is the serverless-NEG twin of `modules/gitops-gateway`, shaped the
# same way and refusing the same things.
#
# ## The order of the gates, and why that order
#
# A request reaches a Google global external Application Load Balancer, which
# terminates TLS on a Google-managed certificate, and is handed to
# Identity-Aware Proxy. IAP checks the caller against
# `roles/iap.httpsResourceAccessor` **before** the request is forwarded at
# all. An unauthenticated request — or one from an identity nobody granted —
# is refused by Google's own front end and never reaches Cloud Run.
#
# ## Where the access list is, and why it is not here
#
# There is deliberately no `iap_members` input.
#
# `modules/gitops-gateway` grants `roles/iap.httpsResourceAccessor` at the
# **project** level, because the backend service a GKE Gateway creates is
# named by its controller at reconcile time and has no Terraform address to
# bind to. Project-level IAM is inherited by every IAP-protected resource in
# the project, this backend service included. A per-resource members list here
# could therefore only ever *widen* that set and never narrow it: it would
# read in the console as this door's own access list while being unable to
# exclude anybody. A control that cannot fire is not a control, so there is
# none — one list, named in the tfvars, for both doors. ADR 0094 decision 3.
#
# ## Why the Cloud Run service still requires authentication
#
# IAP forwards as its own service agent. The service therefore stays
# `INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER` with no `allUsers` invoker, and the
# agent is granted `roles/run.invoker` on that one service — as an
# `IAMPolicyMember` beside the manifest, because since ADR 0036 the service is
# Config Connector's. The alternative shown in most documentation is
# `INGRESS_TRAFFIC_ALL` plus `allUsers`, which leaves the service's own
# `run.app` URL answering the internet anonymously: IAP guarding a front door
# with an open side entrance.
#
# ## The address is reserved here and the DNS record is not
#
# The same split `modules/gitops-gateway` makes, for the same reason:
# `algorik.ai` answers from nameservers outside this project, so the A record
# is an operator's step and the address is an output. A Google-managed
# certificate stays in PROVISIONING until that record resolves, which is the
# feedback that the step was done.

locals {
  name = "${var.service_name}-iap"

  # The zones a client may terminate in, from blueprint §46.1's
  # `client_reachable_zones`. Repeated here rather than passed in, exactly as
  # `modules/public-edge` repeats it and for the same reason: a list this
  # module is *given* is a list the caller can widen, and the point of the
  # refusal below is that widening it is an edit to this file that a reviewer
  # sees.
  client_reachable_zones = ["public-edge", "application-identity"]
}

# --- the address --------------------------------------------------------------

# Global, because a global external Application Load Balancer cannot serve a
# regional address. Reserved rather than ephemeral so the A record an operator
# creates at the registrar keeps pointing at something after the load balancer
# is destroyed and recreated — an ephemeral address goes back to Google and
# the record then points at whatever Google hands the next customer.
resource "google_compute_global_address" "edge" {
  project     = var.project_id
  name        = local.name
  description = "The IAP front door for ${var.service_name}. The A record for ${var.hostname} points here, and is created at the registrar by hand."
  ip_version  = "IPV4"
}

# --- the certificate ----------------------------------------------------------

# Google-managed, so no private key is in this repository, in state, or on any
# disk an operator touches. It provisions once the A record resolves to the
# address above.
resource "google_compute_managed_ssl_certificate" "edge" {
  project = var.project_id
  name    = local.name

  managed {
    domains = [var.hostname]
  }

  # A certificate cannot change the names it covers, so a hostname change
  # replaces it. Create the replacement before destroying the one serving
  # traffic, or every request fails TLS for as long as the new one takes to
  # provision — minutes, not seconds.
  lifecycle {
    create_before_destroy = true
  }
}

# --- Cloud Armor ---------------------------------------------------------------

# A per-address rate limit in front of a door IAP already guards.
#
# What this is and is not, said plainly because the ordering is easy to get
# backwards: Cloud Armor is attached to the *backend service*, so IAP has
# already admitted the caller by the time a rule here is evaluated. An
# anonymous flood is absorbed by Google's front end, which is where it should
# be. What this limits is an admitted session that starts behaving like a
# script — which is the one abuse IAP cannot see, because it authenticated the
# caller correctly.
#
# `rate_based_ban` rather than `throttle`, for `modules/public-edge`'s reason:
# a throttle refuses the excess and lets the next minute start clean, which an
# automated client does not notice; a ban makes the refusal last long enough
# to be a signal.
resource "google_compute_security_policy" "edge" {
  project     = var.project_id
  name        = local.name
  description = "A per-address rate limit behind IAP on ${var.hostname}. IAP decides who may pass; this decides how fast an admitted caller may go."

  adaptive_protection_config {
    layer_7_ddos_defense_config {
      enable = true
    }
  }

  rule {
    action   = "rate_based_ban"
    priority = 2000

    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }

    rate_limit_options {
      conform_action = "allow"
      exceed_action  = "deny(429)"

      enforce_on_key = "IP"

      rate_limit_threshold {
        count        = var.rate_limit_requests_per_minute
        interval_sec = 60
      }

      ban_duration_sec = 600
    }

    description = "Throttle and then ban a single address exceeding the reviewed request rate."
  }

  rule {
    action   = "allow"
    priority = 2147483647

    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }

    description = "The default rule Google requires. Everything not refused above is served."
  }
}

# --- the backend ----------------------------------------------------------------

# A serverless network endpoint group is the only way a global load balancer
# reaches Cloud Run, and it is regional: it can name a service in its own
# region and in no other.
resource "google_compute_region_network_endpoint_group" "service" {
  project               = var.project_id
  name                  = local.name
  region                = var.region
  network_endpoint_type = "SERVERLESS"

  cloud_run {
    service = var.service_name
  }
}

# The gate.
#
# `iap { enabled = true }` with no OAuth client named: the provider's own
# schema says "If OAuth client is not set, the Google-managed OAuth client is
# used", which is the same choice `modules/gitops-gateway` reached — no client
# secret to mint, store, rotate or leak, and no project-level singleton that
# cannot be deleted once created.
resource "google_compute_backend_service" "service" {
  project     = var.project_id
  name        = local.name
  description = "${var.service_name} behind Identity-Aware Proxy on ${var.hostname}. IAP authenticates before the request is forwarded; nothing here is reachable without passing it."

  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = "HTTPS"

  security_policy = google_compute_security_policy.edge.id

  backend {
    group = google_compute_region_network_endpoint_group.service.id
  }

  iap {
    enabled = true
  }

  # No CDN, and not as an oversight. An authenticated surface behind a cache
  # is a surface that can serve one session's response to another session,
  # and IAP's own session cookie is exactly the header a cache must never key
  # on.
  enable_cdn = false

  log_config {
    enable      = true
    sample_rate = 1.0
  }

  lifecycle {
    # The refusal §40.5 is built on, copied from `modules/public-edge`
    # deliberately rather than referenced: the two modules can each put a
    # backend in front of a Cloud Run service, so each has to refuse the same
    # zones or the rule is only true of whichever one a reader happened to
    # open. `execution`, `ledger`, `control-fabric`, `wallet-read`,
    # `treasury-write` and `optimisation` are refused by name, not narrowed.
    precondition {
      condition     = contains(local.client_reachable_zones, var.trust_zone)
      error_message = "The IAP edge would front a backend in the ${var.trust_zone} zone. A client reaches the public edge and the authenticated application surfaces — never Spanner, Pub/Sub, an execution node, a venue, IBM, custody or signing material (§40.14). An identity check in front of a trading zone is still a route into it, so this is refused rather than narrowed. Move the surface into application-identity, or serve it from somewhere that is not the public internet."
    }
  }
}

# --- the front door --------------------------------------------------------------

# One hostname, one backend, no path matcher. The portal is a standalone
# Next.js server that serves its own routes, its own static assets and its own
# `/api`; a path rule splitting it would break it, and a default service that
# is anything else would be a second thing behind this certificate that nobody
# named.
resource "google_compute_url_map" "edge" {
  project         = var.project_id
  name            = local.name
  description     = "Everything on ${var.hostname} reaches ${var.service_name} and nothing else."
  default_service = google_compute_backend_service.service.id
}

resource "google_compute_ssl_policy" "edge" {
  project         = var.project_id
  name            = local.name
  profile         = "RESTRICTED"
  min_tls_version = "TLS_1_2"
}

resource "google_compute_target_https_proxy" "edge" {
  project = var.project_id
  name    = local.name
  url_map = google_compute_url_map.edge.id

  ssl_certificates = [google_compute_managed_ssl_certificate.edge.id]
  ssl_policy       = google_compute_ssl_policy.edge.id
}

# 443 and nothing else. There is deliberately no forwarding rule on 80: a
# redirect from an unencrypted listener is still an unencrypted listener, and
# the first request to it has already travelled in the clear — carrying,
# here, whatever cookie the browser held for this name.
resource "google_compute_global_forwarding_rule" "https" {
  project    = var.project_id
  name       = "${local.name}-https"
  target     = google_compute_target_https_proxy.edge.id
  ip_address = google_compute_global_address.edge.id
  port_range = "443"

  load_balancing_scheme = "EXTERNAL_MANAGED"

  labels = var.labels
}
