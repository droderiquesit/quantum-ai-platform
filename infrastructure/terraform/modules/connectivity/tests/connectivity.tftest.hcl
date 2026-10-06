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

# --- HA VPN fallback (GCP-032) -----------------------------------------------

run "ha_vpn_disabled_plans_no_gateways_or_tunnels" {
  command = plan

  variables {
    enable_ha_vpn = false
    ha_vpn_peer_gateways = {
      chicago-vpn = {
        region                 = "us-central1"
        peer_asn               = 65000
        peer_gateway_addresses = ["203.0.113.1"]
        tunnel_1_shared_secret = "shared-secret-1"
        tunnel_2_shared_secret = "shared-secret-2"
      }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.backup) == 0 && length(google_compute_vpn_tunnel.backup) == 0
    error_message = "HA VPN gateways and tunnels must not be created when enable_ha_vpn is false"
  }
}

run "ha_vpn_enabled_creates_gateway_and_two_tunnels" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago-a = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_peer_gateways = {
      chicago-vpn = {
        region                 = "us-central1"
        peer_asn               = 65000
        peer_gateway_addresses = ["203.0.113.1", "203.0.113.2"]
        tunnel_1_shared_secret = "shared-secret-1-minimum-8-chars"
        tunnel_2_shared_secret = "shared-secret-2-minimum-8-chars"
      }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.backup) == 1
    error_message = "exactly one HA VPN gateway must be created for one peer config"
  }

  assert {
    condition     = length(google_compute_vpn_tunnel.backup) == 2
    error_message = "exactly two tunnels must be created per HA VPN gateway"
  }

  assert {
    condition     = length(google_compute_router_interface.backup_tunnel) == 2 && length(google_compute_router_peer.backup_tunnel) == 2
    error_message = "each tunnel must have a router interface and BGP peer session"
  }

  assert {
    condition     = alltrue([for t in values(google_compute_vpn_tunnel.backup) : t.ike_version == 2])
    error_message = "all tunnels must use IKE version 2"
  }

  assert {
    condition = alltrue([
      for t in values(google_compute_vpn_tunnel.backup) :
      contains(["203.0.113.1", "203.0.113.2"], t.peer_ip)
    ])
    error_message = "tunnel peer IPs must be from the configured peer gateway addresses"
  }
}

run "ha_vpn_with_single_peer_address_reuses_it_for_both_tunnels" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      backup-circuit = { region = "us-east1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_peer_gateways = {
      backup = {
        region                 = "us-east1"
        peer_asn               = 64999
        peer_gateway_addresses = ["198.51.100.1"]
        tunnel_1_shared_secret = "backup-secret-1-minimum-8-chars"
        tunnel_2_shared_secret = "backup-secret-2-minimum-8-chars"
      }
    }
  }

  assert {
    condition     = length(google_compute_vpn_tunnel.backup) == 2
    error_message = "two tunnels must still be created even with a single peer address"
  }

  assert {
    condition = alltrue([
      for t in values(google_compute_vpn_tunnel.backup) :
      t.peer_ip == "198.51.100.1"
    ])
    error_message = "both tunnels must use the same peer address when only one is configured"
  }
}

run "ha_vpn_in_region_without_interconnect_is_not_created" {
  command = plan

  variables {
    enable_partner_interconnect = true
    partner_interconnects = {
      chicago = { region = "us-central1", edge_availability_domain = "AVAILABILITY_DOMAIN_1" }
    }
    enable_ha_vpn = true
    ha_vpn_peer_gateways = {
      chicago-vpn = {
        region                 = "us-central1"
        peer_asn               = 65000
        peer_gateway_addresses = ["203.0.113.1"]
        tunnel_1_shared_secret = "chicago-secret-1-chars"
        tunnel_2_shared_secret = "chicago-secret-2-chars"
      }
      east-vpn = {
        region                 = "us-east1"
        peer_asn               = 65001
        peer_gateway_addresses = ["203.0.113.2"]
        tunnel_1_shared_secret = "east-secret-1-minimum-chars"
        tunnel_2_shared_secret = "east-secret-2-minimum-chars"
      }
    }
  }

  assert {
    condition     = length(google_compute_ha_vpn_gateway.backup) == 1 && length(google_compute_vpn_tunnel.backup) == 2
    error_message = "only the chicago-vpn should be created since only us-central1 has an interconnect; us-east1 vpn config is ignored"
  }

  assert {
    condition = google_compute_ha_vpn_gateway.backup["chicago-vpn"].region == "us-central1"
    error_message = "the created vpn gateway must be in the region with the interconnect"
  }
}

run "ha_vpn_with_invalid_peer_asn_is_refused" {
  command = plan

  variables {
    enable_ha_vpn = true
    ha_vpn_peer_gateways = {
      invalid = {
        region                 = "us-west1"
        peer_asn               = 99999
        peer_gateway_addresses = ["192.0.2.1"]
        tunnel_1_shared_secret = "invalid-secret-1"
        tunnel_2_shared_secret = "invalid-secret-2"
      }
    }
  }

  expect_failures = [var.ha_vpn_peer_gateways]
}

run "ha_vpn_with_short_shared_secret_is_refused" {
  command = plan

  variables {
    enable_ha_vpn = true
    ha_vpn_peer_gateways = {
      short = {
        region                 = "us-west1"
        peer_asn               = 65000
        peer_gateway_addresses = ["192.0.2.1"]
        tunnel_1_shared_secret = "short"
        tunnel_2_shared_secret = "ok-secret-longer"
      }
    }
  }

  expect_failures = [var.ha_vpn_peer_gateways]
}
