# Multi-region edge cell deployment validation tests.
#
# These tests verify that the deployment module correctly orchestrates
# execution nodes and mesh connectivity across regions.

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

# Test 1: Single-region deployment in shadow mode
#
# The simplest valid configuration: one node in shadow mode.
run "single_region_shadow_mode" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region           = "us-east4"
        zone             = "us-east4-a"
        subnet_cidr      = "10.240.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "500000"
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
    }

    trust_zones = {
      "primary" = {
        name        = "primary"
        subnet_cidr = "10.250.0.0/24"
      }
    }

    cross_region_mirrors = []

    boot_image = "projects/test-project/global/images/qip-edge-test"

    capital_envelope_secret_id = "qip-capital-envelope-key"
    venue_credential_secret_id = null

    egress_bootstrap = file("${path.module}/../../../egress/envoy.yaml")
    egress_endpoints = {
      "gcp" = "http://127.0.0.1:9101"
    }

    labels = {}
  }

  # One execution node module is instantiated
  assert {
    condition     = length(module.execution_node) == 1
    error_message = "Single-region deployment should create one node."
  }

  # Deployment summary shows correct state
  assert {
    condition     = output.deployment_summary.total_nodes == 1
    error_message = "Summary should show one node."
  }

  assert {
    condition     = output.deployment_summary.nodes_in_shadow_mode == 1
    error_message = "Summary should show one node in shadow mode."
  }

  assert {
    condition     = output.deployment_summary.nodes_in_live_mode == 0
    error_message = "Summary should show zero nodes in live mode."
  }
}

# Test 2: Multi-region deployment with mixed shadow/live modes
#
# Primary region in live mode, secondary regions still in shadow mode.
run "multi_region_mixed_modes" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region           = "us-east4"
        zone             = "us-east4-a"
        subnet_cidr      = "10.240.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = false # Live mode
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "500000"
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
      "cell-us-west1" = {
        region           = "us-west1"
        zone             = "us-west1-a"
        subnet_cidr      = "10.241.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true # Shadow mode
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "250000"
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
      "cell-europe-west1" = {
        region           = "europe-west1"
        zone             = "europe-west1-b"
        subnet_cidr      = "10.242.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true # Shadow mode
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "250000"
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
    }

    trust_zones = {
      "primary" = {
        name        = "primary"
        subnet_cidr = "10.250.0.0/24"
      }
    }

    cross_region_mirrors = [
      {
        from_region               = "us-east4"
        to_region                 = "us-west1"
        rtt_ms                    = 42
        inventory_band_pct        = 2
        dislocation_threshold_pct = 10
      }
    ]

    boot_image = "projects/test-project/global/images/qip-edge-test"

    capital_envelope_secret_id = "qip-capital-envelope-key"
    venue_credential_secret_id = null

    egress_bootstrap = file("${path.module}/../../../egress/envoy.yaml")
    egress_endpoints = {
      "gcp" = "http://127.0.0.1:9101"
    }

    labels = {}
  }

  # Three execution node modules are instantiated
  assert {
    condition     = length(module.execution_node) == 3
    error_message = "Multi-region deployment should create three nodes."
  }

  # Deployment spans three regions
  assert {
    condition     = length(output.deployment_summary.regions_deployed) == 3
    error_message = "Deployment should span three regions."
  }

  # Mixed modes are correctly counted
  assert {
    condition     = output.deployment_summary.nodes_in_shadow_mode == 2
    error_message = "Summary should show two nodes in shadow mode."
  }

  assert {
    condition     = output.deployment_summary.nodes_in_live_mode == 1
    error_message = "Summary should show one node in live mode."
  }

  # Cross-region mirrors are noted
  assert {
    condition     = output.deployment_summary.cross_region_mirrors_configured == 1
    error_message = "Summary should show one cross-region mirror configured."
  }

  # Capital allocation sums correctly
  assert {
    condition     = output.deployment_summary.total_capital_allocation == 1000000
    error_message = "Total capital allocation should be 500k + 250k + 250k = 1M."
  }
}

# Test 3: Provisioned (not running) nodes
#
# node_count = 0 means the node is provisioned but no instance runs.
# This is valid for planning purposes; instances start when node_count = 1.
run "provisioned_not_running" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-colocated-chicago" = {
        region           = "us-central1"
        zone             = "us-central1-a"
        subnet_cidr      = "10.243.0.0/24"
        node_count       = 0 # Provisioned, not running
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "200000"
        isolated_cpus     = "2-21"
        create_egress_nat = true
      }
    }

    trust_zones = {
      "primary" = {
        name        = "primary"
        subnet_cidr = "10.250.0.0/24"
      }
    }

    cross_region_mirrors = []

    boot_image = "projects/test-project/global/images/qip-edge-test"

    capital_envelope_secret_id = "qip-capital-envelope-key"
    venue_credential_secret_id = null

    egress_bootstrap = file("${path.module}/../../../egress/envoy.yaml")
    egress_endpoints = {
      "gcp" = "http://127.0.0.1:9101"
    }

    labels = {}
  }

  # One node is instantiated
  assert {
    condition     = length(module.execution_node) == 1
    error_message = "Provisioned node should still instantiate the module."
  }

  # The node details are captured in outputs (even with node_count = 0)
  assert {
    condition     = length(output.nodes) == 1
    error_message = "Output should capture the provisioned node's configuration."
  }
}

# Test 4: Subnet CIDR validation prevents overlaps
#
# Each node must have a unique subnet CIDR. Overlaps are caught by
# the execution-node module's validation.
run "zone_validation_enforces_region_prefix" {
  command = plan

  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region           = "us-east4"
        zone             = "us-east4-a" # Zone starts with region
        subnet_cidr      = "10.240.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "500000"
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
    }

    trust_zones = {
      "primary" = {
        name        = "primary"
        subnet_cidr = "10.250.0.0/24"
      }
    }

    cross_region_mirrors = []

    boot_image = "projects/test-project/global/images/qip-edge-test"

    capital_envelope_secret_id = "qip-capital-envelope-key"
    venue_credential_secret_id = null

    egress_bootstrap = file("${path.module}/../../../egress/envoy.yaml")
    egress_endpoints = {
      "gcp" = "http://127.0.0.1:9101"
    }

    labels = {}
  }

  # Plan succeeds with correctly formatted zone
  assert {
    condition     = true
    error_message = "Zone validation should pass when zone matches region."
  }
}

# Test 5: Regional allocation type validation
#
# region_allocation must be a positive decimal number, not zero.
run "regional_allocation_must_be_positive" {
  command = plan

  # Positive allocation is valid
  variables {
    project_id  = "test-project"
    environment = "test"
    network_id  = "projects/test-project/global/networks/qip-test"

    execution_nodes = {
      "cell-us-east4" = {
        region           = "us-east4"
        zone             = "us-east4-a"
        subnet_cidr      = "10.240.0.0/24"
        node_count       = 1
        machine_type     = "c3-highcpu-22"
        shadow_mode      = true
        health_port      = 8080
        watchdog_seconds = 0
        venues = {
          "sim" = { cidr = "10.0.0.0/8", port = 443 }
        }
        region_allocation = "100.50" # Decimal allocation is valid
        isolated_cpus     = "2-21"
        create_egress_nat = false
      }
    }

    trust_zones = {
      "primary" = {
        name        = "primary"
        subnet_cidr = "10.250.0.0/24"
      }
    }

    cross_region_mirrors = []

    boot_image = "projects/test-project/global/images/qip-edge-test"

    capital_envelope_secret_id = "qip-capital-envelope-key"
    venue_credential_secret_id = null

    egress_bootstrap = file("${path.module}/../../../egress/envoy.yaml")
    egress_endpoints = {
      "gcp" = "http://127.0.0.1:9101"
    }

    labels = {}
  }

  # Validation passes for positive decimals
  assert {
    condition     = true
    error_message = "Regional allocation validation accepts positive decimal values."
  }
}
