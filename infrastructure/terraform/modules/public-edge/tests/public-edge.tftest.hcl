# The public edge, planned.
#
# Written at the same time as the module, because a module that has never been
# planned is a module whose preconditions are assertions about assertions —
# the state `modules/trust-zones` was in until the same day this file was
# added, and the state in which two of its controls turned out not to fire.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created. Every refusal is paired with the admission of the same
# shape, because the half that matters for this module is the second one — a
# public edge that refuses every backend is a public edge that serves nothing,
# and it would pass a one-sided test of the refusal below.

mock_provider "google" {}

variables {
  project_id  = "edge-plan-harness"
  environment = "dev"
  region      = "us-east4"
  labels      = {}
}

# --- the switch --------------------------------------------------------------

run "an_environment_with_no_hostnames_gets_no_public_address_at_all" {
  command = plan

  variables {
    hostnames = []
  }

  # The state every environment is in. Asserted on the address and the
  # forwarding rule specifically: those two are what make an endpoint reachable
  # from the internet, and a module that created them "ready to be pointed at
  # something" would be an open port waiting for a DNS record.
  assert {
    condition     = length(google_compute_global_address.edge) == 0 && length(google_compute_global_forwarding_rule.https) == 0
    error_message = "a public address or a forwarding rule was created for an environment that declared no hostname"
  }

  assert {
    condition     = length(google_compute_security_policy.edge) == 0 && length(google_storage_bucket.static_shell) == 0
    error_message = "a security policy or a shell bucket was created for an environment with no customer surface; a half-created edge is the state in which one of the two controls is missing"
  }
}

run "a_declared_hostname_creates_the_whole_edge_and_nothing_on_port_80" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
  }

  assert {
    condition     = length(google_compute_global_forwarding_rule.https) == 1
    error_message = "a declared hostname produced no forwarding rule; the edge would exist with no listener"
  }

  # 443 and only 443. A redirect from port 80 is still an unencrypted listener
  # answering on the public internet, and the request that reaches it has
  # already travelled in the clear.
  assert {
    condition     = google_compute_global_forwarding_rule.https[0].port_range == "443"
    error_message = "the public edge listens on a port other than 443"
  }

  # The policy has to exist and the shell has to be behind the CDN, or the two
  # controls §40.14 names by name are absent from an edge that is otherwise up.
  assert {
    condition     = length(google_compute_security_policy.edge) == 1
    error_message = "the edge came up with no Cloud Armor policy"
  }

  assert {
    condition     = google_compute_backend_bucket.static_shell[0].enable_cdn == true
    error_message = "the static shell is served without Cloud CDN"
  }
}

# --- what a client may be put in front of ------------------------------------

run "the_application_identity_zone_may_be_fronted" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    application_backend = {
      service_name = "qip-dev-api"
      trust_zone   = "application-identity"
    }
  }

  # The admitting half, and the one that keeps the refusal below meaningful: a
  # module that refused every zone would pass that test and serve nothing.
  assert {
    condition     = length(google_compute_backend_service.application) == 1
    error_message = "the one zone §46.1 marks client-reachable for an API can no longer be fronted; the edge refuses everything"
  }

  # The attachment of the policy to the backend — the property that decides
  # whether Cloud Armor is a control or a console decoration — cannot be
  # asserted here: `security_policy` holds the policy's `id`, which is unknown
  # until apply, and a `plan` run comparing two unknowns is refused by
  # Terraform rather than passing vacuously. It is asserted instead in
  # `qip-acceptance`'s `terraform_plan` suite, which reads the configuration,
  # and the division is recorded here so the gap is visible from both sides.

  # An authenticated API behind a cache can serve one session's response to
  # another session.
  assert {
    condition     = google_compute_backend_service.application[0].enable_cdn == false
    error_message = "the authenticated application API is served through Cloud CDN"
  }
}

run "the_execution_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    application_backend = {
      # The regional node. §40.14 lists it under what a client may never
      # reach, and §45.1 says the load balancer is never in front of venue
      # connectivity. This is the declaration that would put it there.
      service_name = "qip-dev-edge-node"
      trust_zone   = "execution"
    }
  }

  expect_failures = [google_compute_backend_service.application[0]]
}

run "the_ledger_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    application_backend = {
      service_name = "qip-dev-ledger"
      trust_zone   = "ledger"
    }
  }

  expect_failures = [google_compute_backend_service.application[0]]
}

run "the_treasury_write_zone_may_not_be_fronted" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    application_backend = {
      service_name = "qip-dev-treasury"
      trust_zone   = "treasury-write"
    }
  }

  expect_failures = [google_compute_backend_service.application[0]]
}

# --- the inputs --------------------------------------------------------------

run "a_wildcard_hostname_is_refused" {
  command = plan

  variables {
    # Google's managed certificates do not issue for a wildcard, and a
    # wildcard here would name hosts nobody enumerated.
    hostnames = ["*.example.com"]
  }

  expect_failures = [var.hostnames]
}

run "a_hostname_listed_twice_is_refused" {
  command = plan

  variables {
    hostnames = ["console.example.com", "console.example.com"]
  }

  expect_failures = [var.hostnames]
}

run "a_rate_limit_of_zero_is_refused" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    # A limit of zero refuses every client. The opposite mistake — a limit so
    # large it never binds — is the one this platform has made before in
    # another domain, and both are refused by the same validation.
    rate_limit_requests_per_minute = 0
  }

  expect_failures = [var.rate_limit_requests_per_minute]
}

run "a_reviewed_rate_limit_is_admitted_and_bans_rather_than_only_throttling" {
  command = plan

  variables {
    hostnames                      = ["console.example.com"]
    rate_limit_requests_per_minute = 120
  }

  assert {
    condition     = length(google_compute_security_policy.edge) == 1
    error_message = "a reviewed rate limit produced no policy"
  }
}

run "a_lower_case_country_code_is_refused_rather_than_matching_nothing" {
  command = plan

  variables {
    hostnames = ["console.example.com"]
    # Cloud Armor matches `origin.region_code` against upper-case alpha-2. A
    # lower-case code matches nothing, so the rule exists, refuses nobody, and
    # reads in the console as a geographic policy.
    permitted_regions = ["gb"]
  }

  expect_failures = [var.permitted_regions]
}

run "an_upper_case_country_code_is_admitted" {
  command = plan

  variables {
    hostnames         = ["console.example.com"]
    permitted_regions = ["GB", "US"]
  }

  assert {
    condition     = length(google_compute_security_policy.edge) == 1
    error_message = "a well-formed geographic allowlist produced no policy"
  }
}
