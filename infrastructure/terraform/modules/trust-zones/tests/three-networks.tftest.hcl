# Three separate VPC networks per environment (GCP-009): Reflex for execution
# nodes, Fabric for control-plane and event-fabric, Service for all other
# workloads. This harness verifies zones are placed on the correct networks.

mock_provider "google" {}

variables {
  project_id  = "tz-three-networks-harness"
  environment = "dev"
  region      = "us-east4"

  # Three mock networks for Reflex, Fabric, and Service.
  reflex_network_id  = "projects/tz-three-networks-harness/global/networks/reflex"
  fabric_network_id  = "projects/tz-three-networks-harness/global/networks/fabric"
  service_network_id = "projects/tz-three-networks-harness/global/networks/service"
}

run "execution_zone_is_placed_on_reflex_network" {
  command = plan

  variables {
    zones = {
      "execution" = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
    }
  }

  assert {
    condition     = length(google_compute_subnetwork.zone) == 1
    error_message = "Execution zone subnet not created"
  }

  assert {
    condition     = google_compute_subnetwork.zone["execution"].network == "projects/tz-three-networks-harness/global/networks/reflex"
    error_message = "Execution zone should be on Reflex network"
  }
}

run "control_fabric_zone_is_placed_on_fabric_network" {
  command = plan

  variables {
    zones = {
      "control-fabric" = { region = "us-east4", subnet_cidr = "10.90.7.0/24" }
    }
  }

  assert {
    condition     = length(google_compute_subnetwork.zone) == 1
    error_message = "Control-fabric zone subnet not created"
  }

  assert {
    condition     = google_compute_subnetwork.zone["control-fabric"].network == "projects/tz-three-networks-harness/global/networks/fabric"
    error_message = "Control-fabric zone should be on Fabric network"
  }
}

run "all_other_zones_are_placed_on_service_network" {
  command = plan

  variables {
    zones = {
      "public-edge"          = { region = "us-east4", subnet_cidr = "10.90.0.0/24" }
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
      "ingestion-discovery"  = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
      "cognition"            = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
      "valuation"            = { region = "us-east4", subnet_cidr = "10.90.4.0/24" }
      "intelligence"         = { region = "us-east4", subnet_cidr = "10.90.5.0/24" }
      "optimisation"         = { region = "us-east4", subnet_cidr = "10.90.6.0/24" }
      "ledger"               = { region = "us-east4", subnet_cidr = "10.90.9.0/24" }
      "wallet-read"          = { region = "us-east4", subnet_cidr = "10.90.10.0/24" }
      "treasury-write"       = { region = "us-east4", subnet_cidr = "10.90.11.0/24" }
      "management"           = { region = "us-east4", subnet_cidr = "10.90.12.0/24" }
    }
  }

  assert {
    condition     = alltrue([for zone, subnet in google_compute_subnetwork.zone : subnet.network == "projects/tz-three-networks-harness/global/networks/service" if zone != "execution" && zone != "control-fabric"])
    error_message = "All non-execution, non-fabric zones should be on Service network"
  }
}

run "execution_firewall_rules_are_on_reflex_network" {
  command = plan

  variables {
    zones = {
      "execution" = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
    }
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_egress : rule.network == "projects/tz-three-networks-harness/global/networks/reflex" if strcontains(rule.name, "execution")])
    error_message = "Execution zone firewall rules should be on Reflex network"
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_ingress : rule.network == "projects/tz-three-networks-harness/global/networks/reflex" if strcontains(rule.name, "execution")])
    error_message = "Execution zone firewall rules should be on Reflex network"
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.google_apis : rule.network == "projects/tz-three-networks-harness/global/networks/reflex" if strcontains(rule.name, "execution")])
    error_message = "Execution zone Google APIs rule should be on Reflex network"
  }
}

run "control_fabric_firewall_rules_are_on_fabric_network" {
  command = plan

  variables {
    zones = {
      "control-fabric" = { region = "us-east4", subnet_cidr = "10.90.7.0/24" }
    }
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_egress : rule.network == "projects/tz-three-networks-harness/global/networks/fabric" if strcontains(rule.name, "fabric")])
    error_message = "Control-fabric zone firewall rules should be on Fabric network"
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_ingress : rule.network == "projects/tz-three-networks-harness/global/networks/fabric" if strcontains(rule.name, "fabric")])
    error_message = "Control-fabric zone firewall rules should be on Fabric network"
  }
}

run "service_zone_firewall_rules_are_on_service_network" {
  command = plan

  variables {
    zones = {
      "public-edge" = { region = "us-east4", subnet_cidr = "10.90.0.0/24" }
    }
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_egress : rule.network == "projects/tz-three-networks-harness/global/networks/service" if strcontains(rule.name, "public-edge")])
    error_message = "Public-edge zone firewall rules should be on Service network"
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.deny_ingress : rule.network == "projects/tz-three-networks-harness/global/networks/service" if strcontains(rule.name, "public-edge")])
    error_message = "Public-edge zone firewall rules should be on Service network"
  }
}

run "path_rules_use_source_zone_network_for_egress" {
  command = plan

  variables {
    zones = {
      "execution"      = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
      "control-fabric" = { region = "us-east4", subnet_cidr = "10.90.7.0/24" }
    }

    permitted_paths = {
      "fabric-to-execution" = {
        from  = "control-fabric"
        to    = "execution"
        mode  = "publish"
        ports = [5005]
        note  = "Control fabric publishes to execution"
      }
    }
  }

  # Path egress rule uses the source zone's network (control-fabric on Fabric).
  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.path_egress : rule.network == "projects/tz-three-networks-harness/global/networks/fabric" if strcontains(rule.name, "fabric-to-execution")])
    error_message = "Path egress rule should use source zone's network (Fabric)"
  }

  # Path ingress rule uses the destination zone's network (execution on Reflex).
  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.path_ingress : rule.network == "projects/tz-three-networks-harness/global/networks/reflex" if strcontains(rule.name, "fabric-to-execution")])
    error_message = "Path ingress rule should use destination zone's network (Reflex)"
  }
}

run "external_egress_rules_use_zone_network" {
  command = plan

  variables {
    zones = {
      "execution" = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
    }

    external_egress = {
      "execution-to-venue" = {
        zone    = "execution"
        cidr    = "198.51.100.0/24"
        port    = 443
        purpose = "venue"
        note    = "Execution can reach venues"
      }
    }
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.external_egress : rule.network == "projects/tz-three-networks-harness/global/networks/reflex" if strcontains(rule.name, "execution-to-venue")])
    error_message = "External egress rule should use zone's network (Reflex for execution)"
  }
}

run "public_ingress_rules_use_zone_network" {
  command = plan

  variables {
    zones = {
      "public-edge" = { region = "us-east4", subnet_cidr = "10.90.0.0/24" }
    }

    public_ingress = {
      "public-edge-https" = {
        zone = "public-edge"
        port = 443
        note = "HTTPS from load balancer"
      }
    }
  }

  assert {
    condition     = alltrue([for _, rule in google_compute_firewall.public_ingress : rule.network == "projects/tz-three-networks-harness/global/networks/service" if strcontains(rule.name, "public-edge-https")])
    error_message = "Public ingress rule should use zone's network (Service for public-edge)"
  }
}

# The refusing half, which this file did not have. Placement is a lookup by
# zone name, so a misspelt execution zone would have no network the hot path
# was meant for. The module refuses a name outside §46.1's thirteen at plan,
# before any subnet is placed on any of the three networks.
run "a_misspelt_execution_zone_is_refused_rather_than_placed" {
  command = plan

  variables {
    zones = {
      "executon" = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
    }
  }

  expect_failures = [var.zones]
}
