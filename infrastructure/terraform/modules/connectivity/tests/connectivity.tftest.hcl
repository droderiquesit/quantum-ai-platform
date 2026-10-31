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
    enable_ha_vpn = true
    ha_vpn_gateways = {
      us-central1 = { peer_asn = 65001, preshared_key = "test-key-chicago", peer_ip_addresses = ["203.0.113.10", "203.0.113.11"] }
    }
  }

  assert {
    condition     = length(google_compute_interconnect_attachment.partner) == 2 && length(google_compute_router.interconnect) == 1
    error_message = "two attachments in one region must plan two attachments on exactly one Cloud Router"
  }

  assert {
    condition     = google_compute_router.interconnect["us-central1"].bgp[0].asn == 16550 && google_compute_router.interconnect["us-central1"].bgp[0].advertise_mode == "DEFAULT"
    error_message = "the partner attachments' router must run BGP on Google's required ASN 16550 (R12-GCP-10010) and advertise only the VPC's own subnets"
  }

  assert {
    condition     = google_compute_router.vpn["us-central1"].bgp[0].asn == 64514 && google_compute_router.vpn["us-central1"].bgp[0].advertise_mode == "DEFAULT"
    error_message = "the HA VPN fallback's router must stay on the private cloud_router_asn, apart from the partner router"
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

# HA VPN fallback (GCP-032): every Interconnect attachment region gets a VPN gateway with tunnels.
#
# Since R12-GCP-10011 an attachment with the VPN flag off is refused at plan
# (the run after this one), so "off plans nothing" is proven with the
# interconnect off too: a gateway map alone bills nothing.
run "ha_vpn_flag_left_off_plans_no_gateway_or_tunnel" {
  command = plan

  variables {
    enable_partner_interconnect = false
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = false
    ha_vpn_gateways = {
      us-central1 = { peer_asn = 65001, preshared_key = "test-key", peer_ip_addresses = ["203.0.113.10"] }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.fallback) == 0 && length(google_compute_vpn_tunnel.fallback) == 0
    error_message = "a VPN gateway or tunnel was planned while enable_ha_vpn was false; they would bill for no fallback"
  }
}

# R12-GCP-10011: an attachment without its fallback fails the module test.
run "an_interconnect_attachment_with_no_ha_vpn_fallback_in_its_region_is_refused_at_plan" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = false
  }

  expect_failures = [google_compute_interconnect_attachment.partner]
}

# The fallback must be in the attachment's own region; one elsewhere is none.
run "an_attachment_whose_region_has_no_gateway_entry_is_refused_even_with_the_flag_on" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_gateways = {
      us-east1 = { peer_asn = 65002, preshared_key = "test-key-east", peer_ip_addresses = ["203.0.113.20"] }
    }
  }

  expect_failures = [google_compute_interconnect_attachment.partner]
}

# A fallback that names no peer address never comes up.
run "a_fallback_with_no_peer_address_is_refused" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_gateways = {
      us-central1 = { peer_asn = 65001, preshared_key = "test-key-chicago", peer_ip_addresses = [] }
    }
  }

  expect_failures = [var.ha_vpn_gateways]
}

run "each_interconnect_region_gets_a_vpn_gateway_with_two_tunnels_and_bgp_peers" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
      chicago-b = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_2" }
    }
    enable_ha_vpn = true
    ha_vpn_gateways = {
      us-central1 = { peer_asn = 65001, preshared_key = "test-key-chicago", peer_ip_addresses = ["203.0.113.10", "203.0.113.11"] }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.fallback) == 1
    error_message = "one region with attachments must plan exactly one HA VPN gateway"
  }

  assert {
    condition     = length(google_compute_router.vpn) == 1 && google_compute_router.vpn["us-central1"].bgp[0].asn == 64514 && google_compute_router.interconnect["us-central1"].bgp[0].asn == 16550
    error_message = "the fallback's tunnels need their own router on the private ASN, beside the partner router on 16550"
  }

  # The BGP sessions are what speak the ASN: every VPN interface and peer sits
  # on the VPN router, never the partner one. (A tunnel's router is the
  # router's computed id, unknown at plan; its interface and peer carry the
  # router's name, which is known.)
  assert {
    condition = alltrue(concat(
      [for i in values(google_compute_router_interface.vpn_fallback) : i.router == google_compute_router.vpn["us-central1"].name],
      [for p in values(google_compute_router_peer.vpn_fallback) : p.router == google_compute_router.vpn["us-central1"].name],
    ))
    error_message = "a VPN BGP session sits on the partner router, which speaks Google's 16550 rather than the VPC's private ASN"
  }

  assert {
    condition     = length(google_compute_vpn_tunnel.fallback) == 2
    error_message = "one HA VPN gateway must plan exactly two tunnels (tunnel-0, tunnel-1)"
  }

  assert {
    condition     = alltrue([for p in values(google_compute_router_peer.vpn_fallback) : p.advertised_route_priority == 200])
    error_message = "VPN peers must advertise with higher metric (200) than Interconnect (100), so traffic prefers the direct circuit"
  }
  assert {
    condition     = google_compute_external_vpn_gateway.peer["us-central1"].redundancy_type == "TWO_IPS_REDUNDANCY" && length(google_compute_external_vpn_gateway.peer["us-central1"].interface) == 2
    error_message = "the fallback's tunnels must terminate on an external peer gateway carrying both peer addresses"
  }

  assert {
    condition     = toset([for t in values(google_compute_vpn_tunnel.fallback) : t.peer_external_gateway_interface]) == toset([0, 1])
    error_message = "with two peer addresses each tunnel must land on its own peer interface"
  }
}

run "vpn_gateways_in_regions_with_no_interconnect_are_not_created" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_gateways = {
      us-central1 = { peer_asn = 65001, preshared_key = "test-key-chicago", peer_ip_addresses = ["203.0.113.10", "203.0.113.11"] }
      us-east1    = { peer_asn = 65002, preshared_key = "test-key-east", peer_ip_addresses = ["203.0.113.20"] }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.fallback) == 1
    error_message = "only regions with Interconnect attachments get VPN gateways; us-east1 has no attachment so its VPN is skipped"
  }

  assert {
    condition     = contains(keys(google_compute_ha_vpn_gateway.fallback), "us-central1")
    error_message = "the gateway must be in us-central1, the region with the Interconnect attachment"
  }
}

# R12-GCP-10014: a PSC endpoint is planned for Google APIs, and only for them.
# The forwarding rule's target is a Google API bundle, never a service
# attachment, so no PSC hop can stand between the execution node and a venue.
run "private_service_connect_plans_one_endpoint_for_the_restricted_google_apis_and_nothing_else" {
  command = plan

  variables {
    enable_private_service_connect  = true
    private_service_connect_address = "10.200.0.5"
  }

  assert {
    condition     = length(google_compute_global_address.google_apis) == 1 && length(google_compute_global_forwarding_rule.google_apis) == 1
    error_message = "enabling Private Service Connect must plan exactly one endpoint address and one forwarding rule"
  }

  assert {
    condition     = google_compute_global_address.google_apis[0].purpose == "PRIVATE_SERVICE_CONNECT" && google_compute_global_address.google_apis[0].address == "10.200.0.5"
    error_message = "the endpoint address must be the named PSC address, not one Google allocates"
  }

  assert {
    condition     = google_compute_global_forwarding_rule.google_apis[0].target == "vpc-sc" && google_compute_global_forwarding_rule.google_apis[0].load_balancing_scheme == ""
    error_message = "the endpoint must reach the restricted Google API bundle, not a service attachment a venue could sit behind"
  }
}

run "private_service_connect_left_off_plans_no_endpoint" {
  command = plan

  assert {
    condition     = length(google_compute_global_address.google_apis) == 0 && length(google_compute_global_forwarding_rule.google_apis) == 0
    error_message = "a PSC endpoint was planned while enable_private_service_connect was false"
  }
}
