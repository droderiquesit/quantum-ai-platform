# Reflex standby promotion (GCP-043).
#
# Two nodes hold the same cell: primary in one zone, standby in another. The
# standby monitors the primary's liveness via a fencing token. When the primary
# zone fails (health check timeout) or the primary loses its fencing token, the
# standby is promoted to hold the cell. This test verifies the infrastructure
# shape for the standby node.

mock_provider "google" {}

# Every value here is one the module admits. Nine were not — a machine shape
# outside §41.4's allowlist, a pricing word the node refuses, two `s3://`
# "paths" the absolute-path guard refuses, a map where `region_allocation` is
# a string, a full resource path where a bare secret id is required, an
# egress endpoint that was not `http://127.0.0.1:<port>`, and an empty
# bootstrap below the module's floor — so the first run stopped on nine
# validation errors and no run here ever reached the standby. The values that
# replaced them are the fixtures `affinity.tftest.hcl` and
# `capacity.tftest.hcl` use, or the module's own defaults.
variables {
  project_id                 = "standby-test-harness"
  environment                = "dev"
  node_id                    = "primary"
  region                     = "us-east4"
  zone                       = "us-east4-a"
  standby_zone               = "us-east4-c"
  network_id                 = "projects/standby-test-harness/global/networks/qip"
  subnet_cidr                = "10.41.0.0/24"
  standby_subnet_cidr        = "10.41.1.0/24"
  machine_type               = "c3-highcpu-8"
  boot_image                 = "projects/standby-test-harness/global/images/qip-edge-node"
  node_count                 = 1
  create_egress_nat          = false
  shadow_mode                = true
  default_pricing            = ""
  strategy_plan_path         = ""
  cross_region_mirror_path   = ""
  region_allocation          = "0.25"
  health_port                = 8080
  required_hugepages_gb      = 16
  watchdog_seconds           = 60
  capital_envelope_secret_id = "qip-capital-envelope-key"
  venue_credential_secret_id = null
  venue_credential_readable  = false
  venues = {
    testex = { cidr = "203.0.113.0/24", port = 443 }
  }
  labels               = {}
  google_apis_range    = "199.36.153.8/30"
  central_plane_ranges = []
  evidence_bucket      = null
  egress_endpoints     = { "gcp" = "http://127.0.0.1:9105" }
  egress_bootstrap     = "static_resources:\n${join("\n", [for i in range(60) : "  # bootstrap line ${i} keeps this fixture past the one-kilobyte floor the module refuses below"])}\n"
}

run "a_standby_node_is_provisioned_in_a_different_zone" {
  command = plan

  variables {
    standby_enabled = true
  }

  assert {
    condition     = length(google_compute_instance_group_manager.standby) == 1
    error_message = "Standby instance group manager must exist when standby_enabled = true"
  }

  assert {
    condition     = google_compute_instance_group_manager.standby[0].zone == var.standby_zone
    error_message = "Standby must be in standby_zone, not the primary zone"
  }

  assert {
    condition     = google_compute_instance_group_manager.standby[0].zone != google_compute_instance_group_manager.node.zone
    error_message = "Standby zone must differ from primary zone for geographic diversity"
  }

  assert {
    condition     = google_compute_instance_group_manager.standby[0].target_size == var.node_count
    error_message = "Standby target size must match primary node count"
  }
}

run "standby_is_not_created_when_flag_is_off" {
  command = plan

  variables {
    standby_enabled = false
  }

  assert {
    condition     = length(google_compute_instance_group_manager.standby) == 0
    error_message = "Standby instance group must not exist when standby_enabled = false"
  }
}

# --- the standby inputs: required when enabled, null otherwise --------------
#
# `standby_zone` and `standby_subnet_cidr` default to null, so a caller that
# never enables a standby — the root module among them — need not invent
# values it would never read. The refusals below are what keep that default
# from becoming a standby planned with nowhere to run; the admission is the
# null path itself, which the root plans in every environment.

run "with_no_standby_both_standby_inputs_may_be_null" {
  command = plan

  variables {
    standby_enabled     = false
    standby_zone        = null
    standby_subnet_cidr = null
  }

  assert {
    condition     = length(google_compute_instance_group_manager.standby) == 0 && length(google_compute_subnetwork.standby) == 0
    error_message = "a node with no standby planned standby resources, or refused the null standby inputs it never reads"
  }
}

run "an_enabled_standby_with_no_zone_is_refused" {
  command = plan

  variables {
    standby_enabled = true
    standby_zone    = null
  }

  expect_failures = [var.standby_zone]
}

run "an_enabled_standby_with_no_subnet_is_refused" {
  command = plan

  variables {
    standby_enabled     = true
    standby_subnet_cidr = null
  }

  expect_failures = [var.standby_subnet_cidr]
}

run "a_standby_zone_outside_the_region_is_refused" {
  command = plan

  variables {
    standby_enabled = true
    standby_zone    = "us-west1-a"
  }

  expect_failures = [var.standby_zone]
}

run "a_standby_in_the_primary_zone_is_refused" {
  command = plan

  variables {
    standby_enabled = true
    standby_zone    = "us-east4-a"
  }

  expect_failures = [var.standby_zone]
}

run "standby_subnet_is_in_same_region_but_different_zone" {
  command = plan

  variables {
    standby_enabled = true
  }

  assert {
    condition     = google_compute_subnetwork.standby[0].region == google_compute_subnetwork.node.region
    error_message = "Standby subnet must be in the same region as primary"
  }

  assert {
    condition     = google_compute_subnetwork.standby[0].region == var.region
    error_message = "Standby subnet region must match var.region"
  }

  assert {
    condition     = google_compute_subnetwork.standby[0].ip_cidr_range == var.standby_subnet_cidr
    error_message = "Standby subnet must use standby_subnet_cidr"
  }
}

run "standby_has_same_firewall_posture_as_primary" {
  command = plan

  variables {
    standby_enabled = true
  }

  assert {
    condition = alltrue([
      for rule in google_compute_firewall.standby_deny_egress : rule.direction == "EGRESS" && rule.priority == 65000
    ])
    error_message = "Standby deny-all egress rule must match primary: EGRESS at priority 65000"
  }

  assert {
    condition = alltrue([
      for rule in google_compute_firewall.standby_google_apis : rule.direction == "EGRESS" && rule.priority == 1000
    ])
    error_message = "Standby Google APIs rule must match primary: EGRESS at priority 1000"
  }

  assert {
    condition = alltrue([
      for rule in google_compute_firewall.standby_health_checks : rule.direction == "INGRESS" && rule.priority == 1000
    ])
    error_message = "Standby health check rule must match primary: INGRESS at priority 1000"
  }
}

run "standby_node_uses_same_service_account_as_primary" {
  command = plan

  variables {
    standby_enabled = true
  }

  assert {
    condition     = google_compute_instance_template.standby[0].service_account[0].email == google_compute_instance_template.node.service_account[0].email
    error_message = "Standby template must use the same service account as primary"
  }
}

run "standby_fencing_rules_allow_primary_to_standby_communication" {
  command = plan

  variables {
    standby_enabled = true
  }

  assert {
    condition     = length(google_compute_firewall.fencing) == 1
    error_message = "Fencing rule allowing primary-to-standby communication must exist"
  }

  assert {
    condition     = google_compute_firewall.fencing[0].direction == "INGRESS"
    error_message = "Fencing rule must be INGRESS (into the standby)"
  }

  # `destination_ranges` and `allow` are sets, which have no index; the
  # `[0]` these two read could not be evaluated. The same properties, read
  # across the set: the destination is the standby subnet and nothing else,
  # and some allow block opens 9443.
  assert {
    condition     = google_compute_firewall.fencing[0].destination_ranges == toset([var.standby_subnet_cidr])
    error_message = "Fencing rule must target the standby subnet"
  }

  assert {
    condition     = anytrue([for rule in google_compute_firewall.fencing[0].allow : contains(rule.ports, "9443")])
    error_message = "Fencing rule must allow port 9443 for fencing protocol"
  }
}
