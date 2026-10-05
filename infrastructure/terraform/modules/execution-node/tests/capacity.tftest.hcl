# FINOPS-001 and FINOPS-002, planned rather than read.
#
# Reflex capacity is a reservation held in the zone the node runs in, consumed
# by name from the instance template, and it changes only when an operator
# changes node_count -- never because a metric moved. A reservation nothing
# consumes is billed for nothing, and an instance template that names no
# reservation lets a stockout take the machine a blue-green replacement needs,
# so the two halves are asserted together. Per ADR 0069 a harness proves an
# admission and a refusal: the admission is a reservation sized to the group's
# target and consumed by the template; the refusal is a node held at zero,
# which Compute would reject as a zero-size reservation and which therefore
# must plan no reservation and no affinity at all.

mock_provider "google" {}

variables {
  project_id                 = "capacity-plan-harness"
  environment                = "dev"
  node_id                    = "harness"
  region                     = "us-east4"
  zone                       = "us-east4-a"
  network_id                 = "projects/capacity-plan-harness/global/networks/harness"
  subnet_cidr                = "10.0.48.0/24"
  boot_image                 = "projects/capacity-plan-harness/global/images/qip-node-harness"
  venues                     = { "sim" = { cidr = "192.0.2.0/24", port = 443 } }
  egress_endpoints           = { "gcp" = "http://127.0.0.1:9105" }
  capital_envelope_secret_id = "qip-capital-envelope-key"
  region_allocation          = "0.25"
  egress_bootstrap           = "static_resources:\n${join("\n", [for i in range(60) : "  # bootstrap line ${i} keeps this fixture past the one-kilobyte floor the module refuses below"])}\n"
  machine_type               = "c3-highcpu-8"
}

run "a_node_with_an_instance_holds_a_reservation_sized_to_its_target_and_consumes_it" {
  command = plan

  variables {
    node_count = 2
  }

  assert {
    condition     = length(google_compute_reservation.node) == 1
    error_message = "a group that holds instances plans no capacity reservation, so a zonal stockout can refuse the machine the node needs"
  }

  assert {
    condition     = google_compute_reservation.node[0].specific_reservation[0].count == 2
    error_message = "the reservation is not sized to the group's target_size (node_count = 2)"
  }

  assert {
    condition     = google_compute_reservation.node[0].specific_reservation[0].instance_properties[0].machine_type == "c3-highcpu-8"
    error_message = "the reservation holds a different machine type from the one the template boots"
  }

  assert {
    condition     = google_compute_reservation.node[0].zone == "us-east4-a"
    error_message = "the reservation is in a different zone from the group, so it holds capacity nothing can use"
  }

  assert {
    condition     = google_compute_reservation.node[0].specific_reservation_required == true
    error_message = "the reservation admits any matching instance in the project, not only this node"
  }

  assert {
    condition     = length(google_compute_instance_template.node.reservation_affinity) == 1
    error_message = "the instance template carries no reservation_affinity, so the reservation is billed and never consumed"
  }

  assert {
    condition     = google_compute_instance_template.node.reservation_affinity[0].type == "SPECIFIC_RESERVATION"
    error_message = "the template's affinity is not SPECIFIC_RESERVATION"
  }

  assert {
    condition     = contains(google_compute_instance_template.node.reservation_affinity[0].specific_reservation[0].values, google_compute_reservation.node[0].name)
    error_message = "the template's affinity names a reservation other than the one this module declares"
  }
}

run "a_node_held_at_zero_plans_no_reservation_and_no_affinity" {
  command = plan

  variables {
    node_count = 0
  }

  assert {
    condition     = length(google_compute_reservation.node) == 0
    error_message = "a node provisioned but not running plans a reservation; Compute refuses a zero-size one and a nonzero one bills for a machine nobody runs"
  }

  assert {
    condition     = length(google_compute_instance_template.node.reservation_affinity) == 0
    error_message = "the template names a reservation that does not exist"
  }
}

# The refusing half: a size no reservation is declared for is refused at
# validation, so capacity can only move by a number a person typed within the
# range the module admits.
run "a_group_larger_than_the_one_machine_per_region_ceiling_is_refused" {
  command = plan

  variables {
    node_count = 3
  }

  expect_failures = [var.node_count]
}
