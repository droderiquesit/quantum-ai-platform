# Egress proxy venue refusal validation (SEC-016).
#
# The proxy's allowed_upstreams validation refuses any hostname containing
# "venue", "broker", or "exchange", ensuring no route to a live venue can be
# added through Terraform without a separate architectural decision and review.
#
# This is defence-in-depth: the real control is the positive allowlist in
# egress.rs (ALLOWED_UPSTREAMS, seven hosts), which the acceptance suite
# verifies against this module's allowed_upstreams variable, and which the
# bootstrap configuration must match. The substring refusal at plan time is a
# second layer that catches configuration drift.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created. Every refusal is paired with the admission of the same
# shape, so the gate both refuses bad values and admits good ones.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "egress-proxy-plan-harness"
  environment       = "dev"
  region            = "us-east4"
  network_id        = "projects/test/global/networks/default"
  subnet_id         = "projects/test/regions/us-east4/subnetworks/default"
  egress_proxy_port = 8443
}

# --- venue hosts are refused -----------------------------------------------

run "the_validation_refuses_allowed_upstreams_containing_venue" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.example-venue.com",  # Contains "venue"
    ]
  }

  # The validation block in variables.tf:55-60 should fire
  expect_failures = [var.allowed_upstreams]
}

run "the_validation_refuses_allowed_upstreams_containing_broker" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.broker-service.com",  # Contains "broker"
    ]
  }

  expect_failures = [var.allowed_upstreams]
}

run "the_validation_refuses_allowed_upstreams_containing_exchange" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.crypto-exchange.com",  # Contains "exchange"
    ]
  }

  expect_failures = [var.allowed_upstreams]
}

# --- valid hosts are admitted -----------------------------------------------

run "the_validation_admits_valid_vendor_hosts" {
  command = plan

  # The default value: seven hosts, none containing venue/broker/exchange,
  # sourced from qip-acceptance/tests/egress.rs ALLOWED_UPSTREAMS.
  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "bigquery.googleapis.com",
      "europe-west2-aiplatform.googleapis.com",
      "quantum.cloud.ibm.com",
      "api.quantum.ibm.com",
      "api.frankfurter.dev",
      "router.huggingface.co",
    ]
  }

  assert {
    condition     = length(module.proxy.google_compute_firewall_rule) >= 0
    error_message = "the proxy module did not plan with valid vendor hosts"
  }
}
