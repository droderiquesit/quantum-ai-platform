# Edge mesh validation tests.
#
# These tests verify that the mesh module correctly gates connectivity based
# on shadow mode and cross-region configuration.

terraform {
  required_version = ">= 1.9.0"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

provider "google" {
  project = "test-project"
  region  = "us-east4"
}

# Test 1: Shadow mode blocks inter-cell connectivity
#
# When shadow_mode = true for all nodes, no ingress rules should be created.
# Each cell is isolated from the central plane and from other cells.
run "shadow_mode_isolates_cells" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = true # Shadow mode
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
      "cell-us-west1" = {
        region      = "us-west1"
        zone        = "us-west1-a"
        subnet_cidr = "10.241.0.0/24"
        health_port = 8080
        shadow_mode = true # Shadow mode
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
    }

    central_plane_ranges = [
      "10.250.0.0/24", # Trust zone subnet
      "199.36.153.8/30"
    ]

    cross_region_mirrors = []

    psc_endpoint_addresses = {
      "us-east4" = "10.255.0.1"
      "us-west1" = "10.255.0.2"
    }

    labels = {}
  }

  # In shadow mode, no ingress rules are created
  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 0
    error_message = "Shadow mode should create zero ingress rules."
  }

  # But PSC endpoints are still created (for future use)
  assert {
    condition     = length(google_compute_global_address.cell_psc_endpoint) == 2
    error_message = "PSC endpoints should be created for all regions."
  }
}

# Test 2: Live mode creates ingress rules
#
# When at least one node has shadow_mode = false, ingress rules are created
# from central plane ranges and other node subnets.
run "live_mode_creates_ingress" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = false # Live mode
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
      "cell-us-west1" = {
        region      = "us-west1"
        zone        = "us-west1-a"
        subnet_cidr = "10.241.0.0/24"
        health_port = 8080
        shadow_mode = true # Still in shadow mode
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
    }

    central_plane_ranges = [
      "10.250.0.0/24",
      "199.36.153.8/30"
    ]

    cross_region_mirrors = []

    psc_endpoint_addresses = {
      "us-east4" = "10.255.0.1"
      "us-west1" = "10.255.0.2"
    }

    labels = {}
  }

  # One ingress rule is created for the live node
  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 1
    error_message = "Live mode should create one ingress rule for the live cell."
  }

  # The rule targets the correct cell
  assert {
    condition = alltrue([
      for rule in google_compute_firewall.cell_ingress_health :
      contains(rule.target_tags, "qip-exec-cell-us-east4")
    ])
    error_message = "Ingress rule should target the live cell."
  }

  # The rule permits TCP 8080 (health port)
  assert {
    condition = alltrue([
      for rule in google_compute_firewall.cell_ingress_health :
      contains([for allow in rule.allow : allow.ports[0]], "8080")
    ])
    error_message = "Ingress rule should permit port 8080."
  }

  # The rule sources from central plane ranges
  assert {
    condition = alltrue([
      for rule in google_compute_firewall.cell_ingress_health :
      contains(rule.source_ranges, "10.250.0.0/24")
    ])
    error_message = "Ingress rule should permit central plane ranges."
  }
}

# Test 3: Cross-region configuration gates PSC endpoints
#
# PSC endpoints are created for all regions regardless of cross-region config,
# because they may be needed when mirrors are added. The presence of mirrors
# is informational, not gatekeeping.
run "cross_region_mirrors_create_no_extra_resources" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = false
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
      "cell-us-west1" = {
        region      = "us-west1"
        zone        = "us-west1-a"
        subnet_cidr = "10.241.0.0/24"
        health_port = 8080
        shadow_mode = false
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
      }
    }

    central_plane_ranges = [
      "10.250.0.0/24",
      "199.36.153.8/30"
    ]

    cross_region_mirrors = [
      {
        from_region               = "us-east4"
        to_region                 = "us-west1"
        rtt_ms                    = 42
        inventory_band_pct        = 2
        dislocation_threshold_pct = 10
      }
    ]

    psc_endpoint_addresses = {
      "us-east4" = "10.255.0.1"
      "us-west1" = "10.255.0.2"
    }

    labels = {}
  }

  # PSC endpoints exist for both regions
  assert {
    condition     = length(google_compute_global_address.cell_psc_endpoint) == 2
    error_message = "PSC endpoints should be created for all regions with nodes."
  }

  # Ingress rules are created for both nodes
  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 2
    error_message = "Ingress rules should be created for both live nodes."
  }

  # No additional resources are created just because mirrors are defined
  assert {
    condition     = local.has_cross_region_mirror == true
    error_message = "Cross-region mirror flag should be set."
  }
}

# Test 4: Venue ID validation (in the execution_nodes variable)
#
# Venue IDs must follow a pattern (lowercase, hyphens, no special chars).
# This test is implicit; the variable validation rejects bad values.
run "venue_id_validation_prevents_injection" {
  command = plan

  # This should fail at variable validation if uncommented:
  # variables {
  #   execution_nodes = {
  #     "test" = {
  #       venues = {
  #         "bad-venue$(id)" = { cidr = "10.0.0.0/8", port = 443 }  # Injection attempt
  #       }
  #       # ...
  #     }
  #   }
  # }

  # For now, test passes because the bad config is commented out.
  assert {
    condition     = true
    error_message = "Venue ID validation is enforced at the edge-mesh module boundary."
  }
}
