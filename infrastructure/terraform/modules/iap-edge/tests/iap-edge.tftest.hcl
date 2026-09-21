# The IAP edge, planned.
#
# Written with the module, because a module that has never been planned is a
# module whose preconditions are assertions about assertions — the state
# `modules/trust-zones` was in until the day its harness was added, and the
# state in which two of its controls turned out not to fire.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created. Every refusal is paired with the admission of the same
# shape, because the half that matters is the second one. A hostname
# validation that refuses everything passes a one-sided test and produces a
# door nobody can open, and `qip-acceptance`'s `terraform_plan` suite fails a
# harness that proves only one of the two halves.

mock_provider "google" {}

variables {
  project_id   = "iap-edge-plan-harness"
  environment  = "dev"
  region       = "us-east4"
  labels       = {}
  service_name = "qip-dev-portal"
  trust_zone   = "application-identity"
}

# --- the hostname ------------------------------------------------------------

run "a_reviewed_hostname_is_admitted_and_brings_up_the_whole_door" {
  command = plan

  variables {
    hostname = "portal.algorik.ai"
  }

  # The admitting half. A validation proven only to refuse is a validation
  # that may refuse every value, and this is the run that tells the two
  # apart: the same expression that rejects the five shapes below lets the
  # name the dev tfvars actually carries through.
  assert {
    condition = length(one(google_compute_managed_ssl_certificate.edge.managed).domains) == 1 && contains(
      one(google_compute_managed_ssl_certificate.edge.managed).domains, "portal.algorik.ai"
    )
    error_message = "the certificate does not cover the hostname it was given; a door whose certificate names something else serves nothing on 443"
  }

  assert {
    condition     = google_compute_global_forwarding_rule.https.port_range == "443"
    error_message = "the IAP edge listens on a port other than 443"
  }

  # The gate itself. `enabled = false` here would be a load balancer publishing
  # the console to the internet with the module's whole argument still written
  # above it.
  assert {
    condition     = google_compute_backend_service.service.iap[0].enabled == true
    error_message = "the backend service does not enable IAP; the door is open"
  }

  # Google-managed OAuth: no client, no secret, nothing to mint or rotate.
  assert {
    condition     = google_compute_backend_service.service.iap[0].oauth2_client_id == null && google_compute_backend_service.service.iap[0].oauth2_client_secret == null
    error_message = "an OAuth client is named on the IAP block; the Google-managed client is used when none is, and a named one is a secret this repository would then have to hold"
  }

  # A serverless NEG is the only way a global load balancer reaches Cloud Run,
  # and it must name the service this door is for.
  assert {
    condition     = google_compute_region_network_endpoint_group.service.network_endpoint_type == "SERVERLESS" && google_compute_region_network_endpoint_group.service.cloud_run[0].service == "qip-dev-portal"
    error_message = "the network endpoint group is not a serverless group naming the fronted Cloud Run service"
  }

  # An authenticated surface behind a cache can serve one session's response
  # to another session, and IAP's session cookie is exactly the header a cache
  # must never key on.
  assert {
    condition     = google_compute_backend_service.service.enable_cdn == false
    error_message = "the IAP-protected backend is served through Cloud CDN"
  }

  # TLS 1.2 and the RESTRICTED profile. The default profile admits TLS 1.0 for
  # clients this platform does not have.
  assert {
    condition     = google_compute_ssl_policy.edge.min_tls_version == "TLS_1_2" && google_compute_ssl_policy.edge.profile == "RESTRICTED"
    error_message = "the TLS policy is not RESTRICTED at 1.2 or above"
  }
}

run "a_hostname_carrying_a_scheme_is_refused" {
  command = plan

  variables {
    # The shape somebody pastes out of a browser. It would reach the
    # certificate's SAN list as a name Google cannot issue for, and the
    # failure would arrive at apply, after the address had been reserved.
    hostname = "https://portal.algorik.ai"
  }

  expect_failures = [var.hostname]
}

run "a_hostname_carrying_a_path_is_refused" {
  command = plan

  variables {
    hostname = "portal.algorik.ai/console"
  }

  expect_failures = [var.hostname]
}

run "a_hostname_carrying_a_port_is_refused" {
  command = plan

  variables {
    hostname = "portal.algorik.ai:443"
  }

  expect_failures = [var.hostname]
}

run "an_upper_case_hostname_is_refused" {
  command = plan

  variables {
    # DNS is case-insensitive and a managed certificate's domain list is not.
    hostname = "Portal.algorik.ai"
  }

  expect_failures = [var.hostname]
}

run "a_wildcard_hostname_is_refused" {
  command = plan

  variables {
    # Google's managed certificates do not issue for a wildcard, and a
    # wildcard here would name hosts nobody enumerated.
    hostname = "*.algorik.ai"
  }

  expect_failures = [var.hostname]
}

run "a_bare_name_with_no_dot_is_refused" {
  command = plan

  variables {
    # `portal` resolves inside somebody's search domain and nowhere else. It
    # is the value a hurried edit leaves behind, and a certificate ordered for
    # it never issues.
    hostname = "portal"
  }

  expect_failures = [var.hostname]
}

# --- what may be put behind this door ----------------------------------------

run "the_execution_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostname = "portal.algorik.ai"
    # The regional node. §40.14 lists it under what a client may never reach,
    # and an identity check in front of it does not change that: IAP decides
    # who may pass, never what is on the other side.
    service_name = "qip-dev-edge-node"
    trust_zone   = "execution"
  }

  expect_failures = [google_compute_backend_service.service]
}

run "the_ledger_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostname     = "portal.algorik.ai"
    service_name = "qip-dev-ledger"
    trust_zone   = "ledger"
  }

  expect_failures = [google_compute_backend_service.service]
}

run "the_treasury_write_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostname     = "portal.algorik.ai"
    service_name = "qip-dev-treasury"
    trust_zone   = "treasury-write"
  }

  expect_failures = [google_compute_backend_service.service]
}

run "the_public_edge_zone_may_be_fronted_too" {
  command = plan

  variables {
    hostname     = "portal.algorik.ai"
    service_name = "qip-dev-portal"
    trust_zone   = "public-edge"
  }

  # The second of the two zones §46.1 marks client-reachable, admitted as well
  # as the first. Without this the refusal above could be true of everything
  # but one value and read as a rule about zones rather than an allowlist of
  # two.
  assert {
    condition     = google_compute_backend_service.service.name == "qip-dev-portal-iap"
    error_message = "a backend in the public-edge zone was refused; the allowlist has collapsed to one value"
  }
}

# --- the rate limit ----------------------------------------------------------

run "a_rate_limit_of_zero_is_refused" {
  command = plan

  variables {
    hostname                       = "portal.algorik.ai"
    rate_limit_requests_per_minute = 0
  }

  expect_failures = [var.rate_limit_requests_per_minute]
}

run "a_reviewed_rate_limit_is_admitted_and_bans_rather_than_only_throttling" {
  command = plan

  variables {
    hostname                       = "portal.algorik.ai"
    rate_limit_requests_per_minute = 300
  }

  assert {
    # The inner comprehension first, so the outer one never touches the
    # default allow rule — which carries no rate_limit_options at all, and
    # reading through it is an error rather than a false.
    condition = length([
      for rule in [
        for candidate in google_compute_security_policy.edge.rule : candidate
        if candidate.priority == 2000
      ] : rule
      if one(one(rule.rate_limit_options).rate_limit_threshold).count == 300
    ]) == 1
    error_message = "the reviewed rate limit did not reach the policy; the number in the tfvars would be a number nothing enforces"
  }

  assert {
    condition = length([
      for rule in google_compute_security_policy.edge.rule : rule
      if rule.priority == 2000 && rule.action == "rate_based_ban"
    ]) == 1
    error_message = "the policy throttles rather than bans; a throttle lets the next minute start clean, which an automated client does not notice"
  }
}

# --- the derived names -------------------------------------------------------

run "a_service_name_too_long_to_derive_a_resource_name_from_is_refused" {
  command = plan

  variables {
    hostname = "portal.algorik.ai"
    # 60 characters. `-iap-https` takes it past the 63 Compute allows, and
    # Google refuses it at apply — after the certificate has been ordered.
    service_name = "qip-dev-a-service-name-nobody-would-choose-but-somebody-might"
  }

  expect_failures = [var.service_name]
}
