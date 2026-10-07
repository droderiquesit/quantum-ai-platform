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

  # Which network the subnet attaches to is not asserted here: its `network`
  # and the Service network's `id` are both unknown until apply, and
  # Terraform refuses a `plan` run whose condition compares two unknowns
  # ("Unknown condition value") — which also skipped every run below it. It
  # is asserted on the configuration instead, in `qip-acceptance`'s
  # `terraform_plan::the_console_egress_subnet_and_its_rules_attach_to_the_service_network`.
}

# The refusing half, which this file did not have: a range below the /26
# floor direct VPC egress accepts is refused at plan, before any subnet is
# placed on the Service network.
run "a_console_egress_range_below_the_floor_is_refused_before_it_reaches_the_service_network" {
  command = plan

  variables {
    console_egress_cidr = "10.0.16.0/28"
  }

  expect_failures = [var.console_egress_cidr]
}

run "private_googleapis_zone_covers_all_three_networks" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  # Counted through a `for`, not `length()` of the set: each element carries a
  # network id that is unknown at plan, and `length` of a set holding unknowns
  # is itself unknown, because two of them might turn out equal. The module
  # renders one `networks` block per network, and this counts those blocks.
  assert {
    condition     = length([for network in google_dns_managed_zone.googleapis.private_visibility_config[0].networks : network]) == 3
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
    condition     = length(google_compute_firewall.console_egress_google_apis) == 1
    error_message = "Console Google APIs rule not created"
  }

  # That both rules sit on the Service network is asserted on the
  # configuration, for the reason the subnet's attachment is: in
  # `terraform_plan::the_console_egress_subnet_and_its_rules_attach_to_the_service_network`.
}

run "backward_compatibility_outputs_point_to_service_network" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  assert {
    condition     = output.network_name == google_compute_network.vpc["service"].name
    error_message = "Backward compatibility network_name output should point to Service network"
  }

  # The `network_id` half is a network's `id`, unknown until apply, so it is
  # asserted on the configuration instead, beside the three per-network ids
  # this file's `new_network_outputs_are_available` run compared for the same
  # reason and could not evaluate: in
  # `terraform_plan::each_network_id_output_names_the_network_it_is_called_after`.
}
