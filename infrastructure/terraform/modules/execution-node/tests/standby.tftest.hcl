# Reflex standby promotion (GCP-043).
#
# Two nodes hold the same cell: primary in one zone, standby in another. The
# standby monitors the primary's liveness via a fencing token. When the primary
# zone fails (health check timeout) or the primary loses its fencing token, the
# standby is promoted to hold the cell. This test verifies the infrastructure
# shape for the standby node.

mock_provider "google" {}

variables {
  project_id                   = "standby-test-harness"
  environment                  = "dev"
  node_id                      = "primary"
  region                       = "us-east4"
  zone                         = "us-east4-a"
  standby_zone                 = "us-east4-c"
  network_id                   = "projects/standby-test-harness/global/networks/qip"
  subnet_cidr                  = "10.41.0.0/24"
  standby_subnet_cidr          = "10.41.1.0/24"
  machine_type                 = "c3d-standard-16"
  boot_image                   = "projects/standby-test-harness/global/images/qip-edge-node"
  node_count                   = 1
  create_egress_nat            = false
  shadow_mode                  = true
  default_pricing              = "flat"
  strategy_plan_path           = "s3://unused"
  cross_region_mirror_path     = "s3://unused"
  region_allocation            = {}
  health_port                  = 8080
  required_hugepages_gb        = 16
  watchdog_seconds             = 60
  capital_envelope_secret_id   = "projects/standby-test-harness/secrets/capital-envelope"
  venue_credential_secret_id   = null
  venue_credential_readable    = false
  venues = {
    testex = { cidr = "203.0.113.0/24", port = 443 }
  }
  labels                       = {}
  google_apis_range            = "199.36.153.8/30"
  central_plane_ranges         = []
  evidence_bucket              = null
  egress_endpoints             = { gcp = "127.0.0.1" }
  egress_bootstrap             = ""
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
    condition     = google_compute_instance_group_manager.standby[0].zone != google_compute_instance_group_manager.node[0].zone
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
    condition = length(google_compute_firewall.fencing) == 1
    error_message = "Fencing rule allowing primary-to-standby communication must exist"
  }

  assert {
    condition     = google_compute_firewall.fencing[0].direction == "INGRESS"
    error_message = "Fencing rule must be INGRESS (into the standby)"
  }

  assert {
    condition     = google_compute_firewall.fencing[0].destination_ranges[0] == var.standby_subnet_cidr
    error_message = "Fencing rule must target the standby subnet"
  }

  assert {
    condition     = contains(google_compute_firewall.fencing[0].allow[0].ports, "9443")
    error_message = "Fencing rule must allow port 9443 for fencing protocol"
  }
}
