# Plans proving the edge mesh gates connectivity on shadow mode, opens only
# the one health port it names, and refuses a cell configuration that would
# make it open anything else.
#
# Run from `infrastructure/terraform/modules/edge-mesh`:
#
#   terraform init -backend=false && terraform test
#
# The provider is mocked, so no run needs a credential or reaches a project.
#
# This harness used to declare a `terraform {}` block and a real `provider`
# block, which `terraform test` refuses outright, and its fourth run was a
# commented-out bad value above `condition = true` — an assertion Terraform
# also refuses, because it names nothing. It never ran. "Shadow mode off" is
# not live trading: every cell is paper-only (ADR 0003), and a cell out of
# shadow mode reaches only its configured simulated venues.

mock_provider "google" {}

variables {
  project_id  = "test-project"
  environment = "test"
  network_id  = "projects/test-project/global/networks/qip-test"

  central_plane_ranges = ["10.250.0.0/24", "199.36.153.8/30"]
  cross_region_mirrors = []

  psc_endpoint_addresses = {
    "us-east4" = "10.255.0.1"
    "us-west1" = "10.255.0.2"
  }

  labels = {}

  execution_nodes = {
    "cell-us-east4" = {
      region      = "us-east4"
      zone        = "us-east4-a"
      subnet_cidr = "10.240.0.0/24"
      health_port = 8080
      shadow_mode = true
      venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
    }
    "cell-us-west1" = {
      region      = "us-west1"
      zone        = "us-west1-a"
      subnet_cidr = "10.241.0.0/24"
      health_port = 8080
      shadow_mode = true
      venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
    }
  }
}

# --- Admissions --------------------------------------------------------------

# The admit half of the central-plane range gate: the specific subnets every
# other run uses, and a /8 at the boundary, all plan. Without this a gate that
# refused every range would pass the refusal runs below.
run "specific_central_plane_ranges_and_a_slash_8_are_admitted" {
  command = plan

  variables {
    central_plane_ranges = ["10.250.0.0/24", "199.36.153.8/30", "10.0.0.0/8"]
  }
}

run "shadow_mode_isolates_cells" {
  command = plan

  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 0
    error_message = "Shadow mode should create zero ingress rules."
  }

  assert {
    condition     = length(google_compute_global_address.cell_psc_endpoint) == 2
    error_message = "PSC endpoints should be created for all regions."
  }
}

run "a_cell_out_of_shadow_admits_only_its_health_port" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = false
        venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
      }
      "cell-us-west1" = {
        region      = "us-west1"
        zone        = "us-west1-a"
        subnet_cidr = "10.241.0.0/24"
        health_port = 8080
        shadow_mode = true
        venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
      }
    }
  }

  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 1
    error_message = "One cell out of shadow mode should get exactly one ingress rule."
  }

  assert {
    condition     = google_compute_firewall.cell_ingress_health["cell-us-east4"].target_tags == toset(["qip-exec-cell-us-east4"])
    error_message = "The ingress rule should target only the cell out of shadow mode."
  }

  # Exactly one allow block, TCP, exactly one port — not merely "contains
  # 8080", which a rule opening 22 beside it would also satisfy.
  assert {
    condition = alltrue([
      for allow in google_compute_firewall.cell_ingress_health["cell-us-east4"].allow :
      allow.protocol == "tcp" && allow.ports == tolist(["8080"])
    ]) && length(google_compute_firewall.cell_ingress_health["cell-us-east4"].allow) == 1
    error_message = "The ingress rule should open TCP 8080 and nothing else."
  }

  assert {
    condition     = google_compute_firewall.cell_ingress_health["cell-us-east4"].source_ranges == toset(["10.250.0.0/24", "199.36.153.8/30", "10.241.0.0/24"])
    error_message = "The ingress rule should admit the central plane ranges and the other cell's subnet, and nothing else."
  }
}

run "cross_region_mirrors_create_no_extra_resources" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = false
        venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
      }
      "cell-us-west1" = {
        region      = "us-west1"
        zone        = "us-west1-a"
        subnet_cidr = "10.241.0.0/24"
        health_port = 8080
        shadow_mode = false
        venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
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
    condition     = length(google_compute_global_address.cell_psc_endpoint) == 2
    error_message = "PSC endpoints should be created for all regions with nodes."
  }

  assert {
    condition     = length(google_compute_firewall.cell_ingress_health) == 2
    error_message = "Ingress rules should be created for both cells out of shadow mode."
  }

  assert {
    condition     = local.has_cross_region_mirror
    error_message = "Cross-region mirror flag should be set."
  }
}

# --- Refusals ----------------------------------------------------------------

# The rule used to open `tostring(each.value.health_port)`, so this value
# would have opened SSH to the central plane and every other cell's subnet.
run "a_health_port_of_22_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "cell-us-east4" = {
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 22
        shadow_mode = false
        venues      = { "sim" = { cidr = "10.0.0.0/8", port = 443 } }
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
        region      = "us-east4"
        zone        = "us-east4-a"
        subnet_cidr = "10.240.0.0/24"
        health_port = 8080
        shadow_mode = true
        venues      = {}
      }
    }
  }

  expect_failures = [var.execution_nodes]
}

# The gate used to compare against the literal "0.0.0.0/0", so half the
# internet in one range passed it.
run "a_central_plane_range_of_0_0_0_0_slash_1_is_refused" {
  command = plan

  variables {
    central_plane_ranges = ["10.250.0.0/24", "0.0.0.0/1"]
  }

  expect_failures = [var.central_plane_ranges]
}

run "a_central_plane_range_of_the_whole_internet_is_still_refused" {
  command = plan

  variables {
    central_plane_ranges = ["0.0.0.0/0"]
  }

  expect_failures = [var.central_plane_ranges]
}

run "a_central_plane_range_wider_than_a_slash_8_is_refused" {
  command = plan

  variables {
    central_plane_ranges = ["10.0.0.0/7"]
  }

  expect_failures = [var.central_plane_ranges]
}

run "a_central_plane_range_that_is_not_a_cidr_is_refused" {
  command = plan

  variables {
    central_plane_ranges = ["not-a-range"]
  }

  expect_failures = [var.central_plane_ranges]
}
