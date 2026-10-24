# The hub's gate, planned. Mocked provider, command = plan: nothing is created.
# Each refusal is paired with the admission of the same shape.

mock_provider "google" {}

variables {
  project_id  = "ncc-plan-harness"
  environment = "dev"
  labels      = {}
  vpc_spokes = {
    service = "projects/ncc-plan-harness/global/networks/service"
    reflex  = "projects/ncc-plan-harness/global/networks/reflex"
  }
}

run "left_off_plans_no_hub_and_no_spoke_whatever_the_maps_say" {
  command = plan

  variables {
    enabled = false
  }

  assert {
    condition     = length(google_network_connectivity_hub.this) == 0 && length(google_network_connectivity_spoke.vpc) == 0
    error_message = "a hub or spoke was planned while enabled was false; it would bill and join planes the trust zones keep apart"
  }
}

run "switched_on_plans_one_hub_and_one_spoke_per_vpc" {
  command = plan

  variables {
    enabled = true
  }

  assert {
    condition     = length(google_network_connectivity_hub.this) == 1 && length(google_network_connectivity_spoke.vpc) == 2
    error_message = "enabled = true must plan one hub and a spoke for each named VPC"
  }
}

run "a_bare_network_name_is_refused" {
  command = plan

  variables {
    enabled    = true
    vpc_spokes = { service = "service" }
  }

  expect_failures = [var.vpc_spokes]
}

run "an_interconnect_spoke_with_no_attachment_is_refused" {
  command = plan

  variables {
    enabled             = true
    interconnect_spokes = { chicago = { location = "us-central1", uris = [] } }
  }

  expect_failures = [var.interconnect_spokes]
}

run "an_interconnect_spoke_with_an_attachment_is_admitted" {
  command = plan

  variables {
    enabled = true
    interconnect_spokes = {
      chicago = { location = "us-central1", uris = ["projects/ncc-plan-harness/regions/us-central1/interconnectAttachments/chicago-a"] }
    }
  }

  assert {
    condition     = length(google_network_connectivity_spoke.interconnect) == 1
    error_message = "a valid interconnect spoke was not planned"
  }
}
