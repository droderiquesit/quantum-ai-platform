# The public edge: Cloud Armor, the global HTTPS load balancer, Cloud CDN.
#
# Blueprint §40.5 and §40.14. One security edge in front of the three
# customer-facing surfaces, and in front of nothing else. §45.1 puts it plainly
# and the sentence is the whole design constraint: **web and mobile only,
# never in front of venue connectivity**.
#
# What that means here, concretely, and why each is structural rather than
# documented:
#
#   * The module creates nothing unless `hostnames` is non-empty. Every
#     environment leaves it empty, so today this module plans to zero
#     resources everywhere. An edge that exists because a module was
#     instantiated is a public address nobody decided to open, and this
#     platform has no customer surface deployed to put behind one.
#   * A backend may front only a zone §46.1 marks client-reachable. The
#     precondition below refuses `execution`, `ledger`, `treasury-write`,
#     `optimisation` and the rest by name, at plan time. Trading traffic
#     leaves a regional node through Cloud NAT to a venue and never passes
#     here; the two kinds of traffic therefore share no load balancer, and
#     since each zone holds its own identity they share no credential either.
#   * There is no HTTP listener. A global forwarding rule on 80 that redirects
#     is still an unencrypted endpoint answering on the public internet, and a
#     redirect is advice a client may ignore. Port 443 or nothing.
#   * The static shell is a bucket behind Cloud CDN, not a server. There is no
#     origin to compromise and nothing in the bucket that is not in the
#     commit.
#
# Nothing here reads a secret, and nothing here can. §40.14's rule is that no
# credential, key or signing share is ever delivered to a client; the surface
# this module serves is a static bundle and a proxy to an authenticated API,
# and neither is given a Secret Manager volume by this file or any other.

locals {
  # Whether this environment has a customer surface at all. One expression,
  # read by every `count` below, so that "the edge exists" cannot be true of
  # some resources and false of others — a half-created edge is an address
  # with no policy on it.
  enabled = length(var.hostnames) > 0 ? 1 : 0

  # The zones a client may terminate in, from blueprint §46.1's
  # `client_reachable_zones`. Repeated here rather than passed in, because a
  # list this module is *given* is a list the caller can widen, and the point
  # of the refusal below is that widening it is an edit to this file.
  client_reachable_zones = ["public-edge", "application-identity"]

  name = "qip-${var.environment}-edge"
}

# --- Cloud Armor -------------------------------------------------------------

# The policy every backend below is attached to.
#
# Rules are evaluated in priority order, lowest first, and the default rule at
# 2147483647 is the one Google requires. The order here is deliberate: the
# geographic refusal comes before the rate limit, so a client from outside the
# permitted regions is refused rather than throttled, and a throttled client
# is one that was allowed to be there.
resource "google_compute_security_policy" "edge" {
  count = local.enabled

  project     = var.project_id
  name        = "${local.name}-armor"
  description = "The public edge's policy: geographic refusal, then a per-address rate limit, then allow. Blueprint §40.14."

  # Adaptive protection is the layer-seven flood detector. On, because the
  # alternative is discovering a flood from the bill.
  adaptive_protection_config {
    layer_7_ddos_defense_config {
      enable = true
    }
  }

  dynamic "rule" {
    for_each = length(var.permitted_regions) > 0 ? [1] : []
    content {
      action   = "deny(403)"
      priority = 1000
      match {
        expr {
          expression = "!(origin.region_code in [${join(", ", [for code in var.permitted_regions : "'${code}'"])}])"
        }
      }
      description = "Refuse a client arriving from outside the countries the desk operates from."
    }
  }

  # The rate limit, per client address. `rate_based_ban` rather than
  # `throttle`: a throttle refuses the excess and lets the next minute start
  # clean, which an automated client does not notice; a ban makes the refusal
  # last long enough to be a signal.
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

# --- the static shell, behind Cloud CDN --------------------------------------

resource "google_storage_bucket" "static_shell" {
  count = local.enabled

  project  = var.project_id
  name     = "${local.name}-shell-${var.project_id}"
  location = var.region

  uniform_bucket_level_access = true

  # The one bucket on this platform a public reader may see, and it still does
  # not get `public_access_prevention = "inherited"` by accident: the CDN
  # reads it through the backend bucket's own identity, so the objects stay
  # private and Google's edge is the only reader.
  public_access_prevention = "enforced"

  versioning {
    enabled = true
  }

  lifecycle_rule {
    condition {
      days_since_noncurrent_time = var.static_shell_retention_days
    }
    action {
      type = "Delete"
    }
  }

  labels = var.labels
}

resource "google_compute_backend_bucket" "static_shell" {
  count = local.enabled

  project     = var.project_id
  name        = "${local.name}-shell"
  bucket_name = google_storage_bucket.static_shell[0].name
  description = "The installable shell §40.5 puts behind Cloud CDN. Static bytes from the commit; no origin to compromise."

  enable_cdn = true

  cdn_policy {
    cache_mode = "CACHE_ALL_STATIC"

    # A negative TTL on an error, so a bad deploy is not cached for an hour.
    negative_caching = true
    negative_caching_policy {
      code = 404
      ttl  = 60
    }

    default_ttl = 3600
    max_ttl     = 86400
    client_ttl  = 3600

    # Keep serving the last good object for a day if the origin stops
    # answering. The shell is the thing a client needs in order to be told
    # anything at all, including that the platform is down.
    serve_while_stale = 86400
  }

  # No `signed_url_cache_max_age_sec` and no signed-URL key, deliberately: a
  # signed URL is a credential in a link, and §40.14 says no client persists
  # anything beyond a session identifier. The shell is public bytes from the
  # commit; there is nothing here to sign access to.
}

# --- the application APIs ----------------------------------------------------

# A serverless network endpoint group is the only way a global load balancer
# reaches Cloud Run, and it is regional: it can name a service in its own
# region and in no other.
resource "google_compute_region_network_endpoint_group" "application" {
  count = local.enabled > 0 && var.application_backend != null ? 1 : 0

  project               = var.project_id
  name                  = "${local.name}-app"
  region                = var.region
  network_endpoint_type = "SERVERLESS"

  cloud_run {
    service = var.application_backend.service_name
  }
}

resource "google_compute_backend_service" "application" {
  count = local.enabled > 0 && var.application_backend != null ? 1 : 0

  project     = var.project_id
  name        = "${local.name}-app"
  description = "The authenticated application APIs, behind the one security edge. §40.5: a surface raises intents and reaches no strategy, order, venue, QPU or key."

  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = "HTTPS"

  security_policy = google_compute_security_policy.edge[0].id

  backend {
    group = google_compute_region_network_endpoint_group.application[0].id
  }

  # Cloud CDN is for the shell. An authenticated API behind a cache is an API
  # that can serve one session's response to another session.
  enable_cdn = false

  log_config {
    enable      = true
    sample_rate = 1.0
  }

  lifecycle {
    # The refusal §40.5 is built on. `execution`, `ledger`, `control-fabric`,
    # `wallet-read`, `treasury-write`, `optimisation` and the rest are not
    # scoped down here, they are refused: a client reaching any of them is the
    # failure this whole layer exists to make impossible, and a review comment
    # is not a network control.
    precondition {
      condition     = contains(local.client_reachable_zones, var.application_backend.trust_zone)
      error_message = "The public edge would front a backend in the ${var.application_backend.trust_zone} zone. A client reaches the public edge and the authenticated application APIs — never Spanner, Pub/Sub, an execution node, a venue, IBM, custody or signing material (§40.14). Customer traffic and trading traffic share no load balancer, so this is refused rather than narrowed. Move the surface into application-identity, or serve it from somewhere that is not the public internet."
    }
  }
}

# --- the front door ----------------------------------------------------------

resource "google_compute_url_map" "edge" {
  count = local.enabled

  project     = var.project_id
  name        = local.name
  description = "The static shell by default; the application APIs under /api when one is declared."

  default_service = google_compute_backend_bucket.static_shell[0].id

  dynamic "host_rule" {
    for_each = var.application_backend == null ? [] : [1]
    content {
      hosts        = ["*"]
      path_matcher = "application"
    }
  }

  dynamic "path_matcher" {
    for_each = var.application_backend == null ? [] : [1]
    content {
      name            = "application"
      default_service = google_compute_backend_bucket.static_shell[0].id

      path_rule {
        paths   = ["/api", "/api/*"]
        service = google_compute_backend_service.application[0].id
      }
    }
  }
}

resource "google_compute_managed_ssl_certificate" "edge" {
  count = local.enabled

  project = var.project_id
  name    = local.name

  managed {
    domains = var.hostnames
  }

  # Google will not reissue in place; a domain change replaces the
  # certificate, and the replacement has to exist before the proxy stops
  # naming the old one or the edge serves nothing for the minutes in between.
  lifecycle {
    create_before_destroy = true
  }
}

resource "google_compute_target_https_proxy" "edge" {
  count = local.enabled

  project = var.project_id
  name    = local.name
  url_map = google_compute_url_map.edge[0].id

  ssl_certificates = [google_compute_managed_ssl_certificate.edge[0].id]

  # TLS 1.2 and above, and no cipher Google classes as compatible-only. The
  # default profile admits TLS 1.0 for clients this platform does not have.
  ssl_policy = google_compute_ssl_policy.edge[0].id
}

resource "google_compute_ssl_policy" "edge" {
  count = local.enabled

  project         = var.project_id
  name            = local.name
  profile         = "RESTRICTED"
  min_tls_version = "TLS_1_2"
}

resource "google_compute_global_address" "edge" {
  count = local.enabled

  project      = var.project_id
  name         = local.name
  ip_version   = "IPV4"
  address_type = "EXTERNAL"
}

# 443 and nothing else. There is deliberately no forwarding rule on 80: a
# redirect from an unencrypted listener is still an unencrypted listener, and
# the first request to it has already travelled in the clear.
resource "google_compute_global_forwarding_rule" "https" {
  count = local.enabled

  project    = var.project_id
  name       = "${local.name}-https"
  target     = google_compute_target_https_proxy.edge[0].id
  ip_address = google_compute_global_address.edge[0].id
  port_range = "443"

  load_balancing_scheme = "EXTERNAL_MANAGED"

  labels = var.labels
}
