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

# The module's own inputs, all six. This block used to pass `network_id`,
# `subnet_id` and `egress_proxy_port`, which the module does not declare, and
# omit `labels` and `image_prefix`, which it requires, so no run could start.
variables {
  project_id   = "egress-proxy-plan-harness"
  environment  = "dev"
  region       = "us-east4"
  labels       = {}
  image_prefix = "us-east4-docker.pkg.dev/egress-proxy-plan-harness/qip"
}

# --- venue hosts are refused -----------------------------------------------

run "the_validation_refuses_allowed_upstreams_containing_venue" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.example-venue.com", # Contains "venue"
    ]
  }

  # The venue/broker/exchange validation on `allowed_upstreams` should fire.
  expect_failures = [var.allowed_upstreams]
}

run "the_validation_refuses_allowed_upstreams_containing_broker" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.broker-service.com", # Contains "broker"
    ]
  }

  expect_failures = [var.allowed_upstreams]
}

run "the_validation_refuses_allowed_upstreams_containing_exchange" {
  command = plan

  variables {
    allowed_upstreams = [
      "storage.googleapis.com",
      "api.crypto-exchange.com", # Contains "exchange"
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

  # This asserted `length(module.proxy.google_compute_firewall_rule) >= 0`:
  # the module has no `proxy` call and no firewall, and a length is never
  # negative, so it named nothing and could not fail. What admission means
  # here is that the plan reached the bootstrap object, whose precondition
  # holds the allowlist to the hosts the committed bootstrap dials, and
  # published those hosts.
  assert {
    condition     = output.dialled_upstreams == sort(var.allowed_upstreams)
    error_message = "the proxy module did not plan with valid vendor hosts"
  }
}
