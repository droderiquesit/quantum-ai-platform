# Three separate VPC networks per environment (GCP-009): Reflex for execution
# nodes, Fabric for control-plane and event-fabric, Service for all other
# workloads. This harness verifies the network module creates and exposes the
# three networks correctly.

mock_provider "google" {}

variables {
  project_id  = "network-three-vpcs-harness"
  region      = "us-central1"
  environment = "dev"
  labels      = {}
}

run "three_networks_are_created" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = length(google_compute_network.vpc) == 3
    error_message = "Expected three networks (Reflex, Fabric, Service), got ${length(google_compute_network.vpc)}"
  }

  assert {
    condition     = contains(keys(google_compute_network.vpc), "reflex")
    error_message = "Reflex network not found"
  }

  assert {
    condition     = contains(keys(google_compute_network.vpc), "fabric")
    error_message = "Fabric network not found"
  }

  assert {
    condition     = contains(keys(google_compute_network.vpc), "service")
    error_message = "Service network not found"
  }
}

run "each_network_has_a_deny_ingress_rule" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = length(google_compute_firewall.deny_ingress) == 3
    error_message = "Each network needs a deny-ingress rule; got ${length(google_compute_firewall.deny_ingress)}"
  }
}

run "reflex_network_is_named_correctly" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = google_compute_network.vpc["reflex"].name == "qip-dev-reflex"
    error_message = "Reflex network name is ${google_compute_network.vpc["reflex"].name}, expected qip-dev-reflex"
  }
}

run "fabric_network_is_named_correctly" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = google_compute_network.vpc["fabric"].name == "qip-dev-fabric"
    error_message = "Fabric network name is ${google_compute_network.vpc["fabric"].name}, expected qip-dev-fabric"
  }
}

run "service_network_is_named_correctly" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = google_compute_network.vpc["service"].name == "qip-dev-service"
    error_message = "Service network name is ${google_compute_network.vpc["service"].name}, expected qip-dev-service"
  }
}

run "console_egress_subnet_is_on_service_network" {
  command = plan

  variables {
    console_egress_cidr = "10.0.16.0/26"
  }

  assert {
    condition     = length(google_compute_subnetwork.console_egress) == 1
    error_message = "Console egress subnet not created"
  }

  assert {
    condition     = google_compute_subnetwork.console_egress[0].network == google_compute_network.vpc["service"].id
    error_message = "Console egress subnet is not on the Service network"
  }
}

run "private_googleapis_zone_covers_all_three_networks" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = length(google_dns_managed_zone.googleapis.private_visibility_config[0].networks) == 3
    error_message = "Private Google APIs zone must be visible on all three networks"
  }
}

run "console_egress_rules_are_on_service_network" {
  command = plan

  variables {
    console_egress_cidr = "10.0.16.0/26"
  }

  assert {
    condition     = length(google_compute_firewall.console_egress_deny_egress) == 1
    error_message = "Console egress deny rule not created"
  }

  assert {
    condition     = google_compute_firewall.console_egress_deny_egress[0].network == google_compute_network.vpc["service"].id
    error_message = "Console egress deny rule is not on the Service network"
  }

  assert {
    condition     = length(google_compute_firewall.console_egress_google_apis) == 1
    error_message = "Console Google APIs rule not created"
  }

  assert {
    condition     = google_compute_firewall.console_egress_google_apis[0].network == google_compute_network.vpc["service"].id
    error_message = "Console Google APIs rule is not on the Service network"
  }
}

run "backward_compatibility_outputs_point_to_service_network" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = output.network_id == google_compute_network.vpc["service"].id
    error_message = "Backward compatibility network_id output should point to Service network"
  }

  assert {
    condition     = output.network_name == google_compute_network.vpc["service"].name
    error_message = "Backward compatibility network_name output should point to Service network"
  }
}

run "new_network_outputs_are_available" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = output.reflex_network_id == google_compute_network.vpc["reflex"].id
    error_message = "reflex_network_id output is missing or incorrect"
  }

  assert {
    condition     = output.fabric_network_id == google_compute_network.vpc["fabric"].id
    error_message = "fabric_network_id output is missing or incorrect"
  }

  assert {
    condition     = output.service_network_id == google_compute_network.vpc["service"].id
    error_message = "service_network_id output is missing or incorrect"
  }
}
