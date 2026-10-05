# The partner interconnect's shape, planned (GCP-031).
#
# Written because the module had no plan at all: its refusals live in
# `validation` blocks, which `terraform validate` does not evaluate. Mocked
# provider, `command = plan`: nothing is created and no circuit is ordered.
# Each refusal is paired with the admission of the same shape, so a gate that
# refuses everything cannot pass for a strict one.

mock_provider "google" {}

variables {
  project_id  = "connectivity-plan-harness"
  environment = "dev"
  labels      = {}
  network_id  = "projects/connectivity-plan-harness/global/networks/qip"
}

run "a_flag_left_off_plans_no_router_and_no_attachment_whatever_the_map_says" {
  command = plan

  variables {
    enable_partner_interconnect = false
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
  }

  assert {
    condition     = length(google_compute_router.interconnect) == 0 && length(google_compute_interconnect_attachment.partner) == 0
    error_message = "an attachment was planned while enable_partner_interconnect was false; it would bill and read as a working private path"
  }
}

run "two_attachments_in_one_region_share_one_router_with_bgp_and_stay_administratively_down" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
      chicago-b = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_2" }
    }
  }

  assert {
    condition     = length(google_compute_interconnect_attachment.partner) == 2 && length(google_compute_router.interconnect) == 1
    error_message = "two attachments in one region must plan two attachments on exactly one Cloud Router"
  }

  assert {
    condition     = google_compute_router.interconnect["us-central1"].bgp[0].asn == 64514 && google_compute_router.interconnect["us-central1"].bgp[0].advertise_mode == "DEFAULT"
    error_message = "the router must run BGP with the private ASN and advertise only the VPC's own subnets"
  }

  assert {
    condition     = alltrue([for a in values(google_compute_interconnect_attachment.partner) : a.type == "PARTNER" && a.admin_enabled == false])
    error_message = "an attachment must be a PARTNER attachment and start administratively down"
  }
}

run "an_attachment_that_leaves_the_availability_domain_to_google_is_refused" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_ANY" }
    }
  }

  expect_failures = [var.partner_interconnects]
}
