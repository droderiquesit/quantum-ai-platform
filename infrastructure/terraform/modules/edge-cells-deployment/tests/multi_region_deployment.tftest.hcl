# Plans proving the edge cell topology contract refuses a bad cell and admits
# a good one.
#
# Run from `infrastructure/terraform/modules/edge-cells-deployment`:
#
#   terraform init -backend=false && terraform test
#
# The provider is mocked, so no run needs a credential or reaches a project.
#
# This harness used to declare a `terraform {}` block and a real `provider`
# block, which `terraform test` refuses outright, and its two "validation"
# runs asserted `condition = true` — which Terraform also refuses, because an
# assertion that names nothing checks nothing. It never ran, so it proved
# nothing, and every run in it admitted: not one showed that a gate fired.
# The refusals below are the half it was missing; the admissions are the half
# that proves the gates do not refuse everything.

mock_provider "google" {}

variables {
  trust_zones = {
    "primary" = {
      name        = "primary"
      subnet_cidr = "10.250.0.0/24"
    }
  }

  cross_region_mirrors = []

  execution_nodes = {
    "cell-us-east4" = {
      region            = "us-east4"
      zone              = "us-east4-a"
      subnet_cidr       = "10.240.0.0/24"
      node_count        = 1
      machine_type      = "c3-highcpu-22"
      shadow_mode       = true
      venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
      region_allocation = "500000"
    }
  }
}

# --- Admissions --------------------------------------------------------------

run "one_cell_in_shadow_mode_is_admitted" {
  command = plan

  assert {
    condition     = output.deployment_summary.total_nodes == 1 && output.deployment_summary.nodes_in_shadow_mode == 1
    error_message = "One shadow-mode cell should be admitted and counted as in shadow mode."
  }

  assert {
    condition     = output.deployment_summary.nodes_with_venue_paths == 0
    error_message = "A cell in shadow mode has no venue paths."
  }

  assert {
    condition     = output.central_plane_ranges == tolist(["10.250.0.0/24", "199.36.153.8/30"])
    error_message = "The central plane ranges are every trust-zone subnet followed by the Google APIs range, and nothing else."
  }
}

run "three_regions_with_a_mirror_are_admitted" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region            = "us-east4"
        zone              = "us-east4-a"
        subnet_cidr       = "10.240.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = false
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "500000"
      }
      "cell-us-west1" = {
        region            = "us-west1"
        zone              = "us-west1-a"
        subnet_cidr       = "10.241.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "250000"
      }
      "cell-europe-west1" = {
        region            = "europe-west1"
        zone              = "europe-west1-b"
        subnet_cidr       = "10.242.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "250000"
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
  }

  assert {
    condition     = output.deployment_summary.regions_deployed == tolist(["europe-west1", "us-east4", "us-west1"])
    error_message = "Three cells in three regions should report three regions, sorted."
  }

  assert {
    condition = output.psc_addresses == {
      "europe-west1" = "10.255.0.1"
      "us-east4"     = "10.255.0.2"
      "us-west1"     = "10.255.0.3"
    }
    error_message = "PSC addresses are assigned one per region, in sorted region order, so the far end can compute them."
  }

  assert {
    condition     = output.deployment_summary.nodes_in_shadow_mode == 2 && output.deployment_summary.nodes_with_venue_paths == 1
    error_message = "Two cells in shadow mode and one with venue paths should be counted as such."
  }

  assert {
    condition     = output.deployment_summary.total_capital_allocation == 1000000
    error_message = "Regional ceilings of 500000, 250000 and 250000 sum to 1000000."
  }
}

run "a_provisioned_cell_with_no_running_instance_is_admitted" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-central1" = {
        region            = "us-central1"
        zone              = "us-central1-a"
        subnet_cidr       = "10.243.0.0/24"
        node_count        = 0
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "100.50"
      }
    }
  }

  assert {
    condition     = output.deployment_summary.total_nodes == 1 && output.deployment_summary.total_capital_allocation == 100.5
    error_message = "A provisioned cell with a positive decimal ceiling should be admitted and counted."
  }
}

# --- Refusals ----------------------------------------------------------------

run "a_zone_outside_its_region_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region            = "us-east4"
        zone              = "us-west1-a"
        subnet_cidr       = "10.240.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "500000"
      }
    }
  }

  expect_failures = [var.execution_nodes]
}

run "a_zero_regional_ceiling_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region            = "us-east4"
        zone              = "us-east4-a"
        subnet_cidr       = "10.240.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "0"
      }
    }
  }

  expect_failures = [var.execution_nodes]
}

run "a_regional_ceiling_that_is_not_a_number_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region            = "us-east4"
        zone              = "us-east4-a"
        subnet_cidr       = "10.240.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
        region_allocation = "half a million"
      }
    }
  }

  expect_failures = [var.execution_nodes]
}

run "a_cell_with_no_venue_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region            = "us-east4"
        zone              = "us-east4-a"
        subnet_cidr       = "10.240.0.0/24"
        node_count        = 1
        machine_type      = "c3-highcpu-22"
        shadow_mode       = true
        venues            = {}
        region_allocation = "500000"
      }
    }
  }

  expect_failures = [var.execution_nodes]
}
