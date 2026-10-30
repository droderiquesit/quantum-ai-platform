# ADR 0116's peer mesh, planned rather than read (R12-MESH-1073, 1074,
# 1075, 1166, 5052, 5063, 5065, 5070, 10112, 10113).
#
# `qip-edge-node` spawns the peer link only when QIP_PEER_MESH_LISTEN and
# QIP_PEER_MESH_PEERS are both set, and until this module rendered them no
# committed configuration could set them: the saga that reserves, declares
# FIRE and unwinds ran on no deployable node. The harness proves both halves
# of the switch — null renders no line and opens no port, a declared map
# renders both lines exactly as the binary parses them and opens the firewall
# for the peers named alone — and that every value the binary would refuse at
# start is refused at plan instead.

mock_provider "google" {}

variables {
  project_id            = "peer-mesh-plan-harness"
  environment           = "stage"
  node_id               = "newyork-1"
  region                = "us-east4"
  zone                  = "us-east4-a"
  network_id            = "projects/peer-mesh-plan-harness/global/networks/harness"
  subnet_cidr           = "10.0.48.0/24"
  boot_image            = "projects/peer-mesh-plan-harness/global/images/qip-node-harness"
  venues                = { "sim" = { cidr = "192.0.2.0/24", port = 443 } }
  egress_endpoints      = { "gcp" = "http://127.0.0.1:9105" }
  trust_bundle_path     = "/etc/qip/trust/harness.json"
  google_tls_roots_path = "/etc/qip/trust/google-roots.pem"
  workload_certificates = {
    ca_pool              = "projects/harness-project/locations/us-east4/caPools/qip-harness-internal"
    certificate_template = "projects/harness-project/locations/us-east4/certificateTemplates/qip-harness-workload"
    lifetime_secs        = 86400
    trust_roots_pem      = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
  }
  region_allocation = "0.25"
  egress_bootstrap  = "static_resources:\n${join("\n", [for i in range(60) : "  # bootstrap line ${i} keeps this fixture past the one-kilobyte floor the module refuses below"])}\n"
  machine_type      = "c4-highcpu-8"
}

run "a_node_with_no_peer_mesh_renders_no_mesh_variable_and_opens_no_port" {
  command = plan

  assert {
    condition     = !strcontains(google_compute_instance_template.node.metadata["startup-script"], "QIP_PEER_MESH_")
    error_message = "node.env carries a QIP_PEER_MESH_ line with peer_mesh null; the binary would refuse a half-configured mesh or join one nobody declared"
  }

  assert {
    condition     = length(google_compute_firewall.peer_mesh_ingress) == 0 && length(google_compute_firewall.peer_mesh_egress) == 0
    error_message = "a node with no peers has a peer-mesh firewall rule"
  }
}

run "two_declared_peers_render_both_variables_as_the_binary_parses_them_and_open_the_firewall_for_them_alone" {
  command = plan

  variables {
    peer_mesh = {
      listen_port    = 7441
      clock_bound_ms = 250
      peers = {
        "london-1"    = { address = "10.68.0.10", port = 7441 }
        "frankfurt-1" = { address = "10.69.0.10", port = 7442 }
      }
    }
  }

  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nQIP_PEER_MESH_LISTEN=0.0.0.0:7441\n")
    error_message = "node.env does not carry QIP_PEER_MESH_LISTEN=0.0.0.0:7441"
  }

  # Name order, as a map iterates: the binary parses `name=ip:port` entries
  # split on commas and refuses anything else.
  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nQIP_PEER_MESH_PEERS=frankfurt-1=10.69.0.10:7442,london-1=10.68.0.10:7441\n")
    error_message = "node.env does not carry QIP_PEER_MESH_PEERS in the name=ip:port,name=ip:port form the binary reads"
  }

  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nQIP_PEER_MESH_CLOCK_BOUND_MS=250\n")
    error_message = "node.env does not carry the stated clock bound"
  }

  assert {
    condition     = google_compute_firewall.peer_mesh_ingress[0].source_ranges == toset(["10.68.0.10/32", "10.69.0.10/32"]) && google_compute_firewall.peer_mesh_ingress[0].direction == "INGRESS"
    error_message = "the peer-mesh ingress rule does not admit exactly the two peers' addresses"
  }

  assert {
    condition     = google_compute_firewall.peer_mesh_egress[0].destination_ranges == toset(["10.68.0.10/32", "10.69.0.10/32"]) && google_compute_firewall.peer_mesh_egress[0].direction == "EGRESS"
    error_message = "the peer-mesh egress rule does not reach exactly the two peers' addresses"
  }

  assert {
    condition     = toset(flatten([for rule in google_compute_firewall.peer_mesh_ingress[0].allow : rule.ports])) == toset(["7441"]) && toset(flatten([for rule in google_compute_firewall.peer_mesh_egress[0].allow : rule.ports])) == toset(["7441", "7442"])
    error_message = "the peer-mesh rules do not name the listen port inbound and the peers' ports outbound"
  }
}

run "a_peer_mesh_with_no_clock_bound_leaves_the_binary_its_default" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = { "london-1" = { address = "10.68.0.10", port = 7441 } }
    }
  }

  assert {
    condition     = !strcontains(google_compute_instance_template.node.metadata["startup-script"], "QIP_PEER_MESH_CLOCK_BOUND_MS")
    error_message = "node.env states a clock bound nobody chose"
  }

  assert {
    condition     = strcontains(google_compute_instance_template.node.metadata["startup-script"], "\nQIP_PEER_MESH_PEERS=london-1=10.68.0.10:7441\n")
    error_message = "node.env does not carry the one peer"
  }
}

run "a_node_named_as_its_own_peer_is_refused_at_plan" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = { "newyork-1" = { address = "10.67.0.10", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_mesh_naming_no_peer_is_refused_rather_than_rendered_empty" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = {}
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_peer_at_a_hostname_is_refused_because_the_binary_reads_ip_port" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = { "london-1" = { address = "london-1.internal", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_clock_bound_of_zero_or_past_a_second_is_refused_at_plan" {
  command = plan

  variables {
    peer_mesh = {
      listen_port    = 7441
      clock_bound_ms = 0
      peers          = { "london-1" = { address = "10.68.0.10", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_peer_name_a_certificate_cannot_carry_is_refused_at_plan" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = { "London_1" = { address = "10.68.0.10", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_peer_at_a_public_address_is_refused_because_the_mesh_rule_must_stay_inside_the_proxys_control" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 7441
      peers       = { "london-1" = { address = "203.0.113.10", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}

run "a_mesh_on_a_remote_administration_port_is_refused_at_plan" {
  command = plan

  variables {
    peer_mesh = {
      listen_port = 22
      peers       = { "london-1" = { address = "10.68.0.10", port = 7441 } }
    }
  }

  expect_failures = [var.peer_mesh]
}
