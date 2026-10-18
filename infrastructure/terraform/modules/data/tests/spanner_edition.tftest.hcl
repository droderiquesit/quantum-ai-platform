# Terraform test: the ledger's Spanner instance is planned on Enterprise Plus
#
# R12-LEDGER-8132. The `nam-eur-asea` multi-region configuration carries the
# external-consistency guarantee the ledger of record is placed on Spanner
# for (ADR 0113) only on Enterprise Plus; an instance with no edition plans
# the project's default and reads in review as the same resource while
# holding a weaker contract. `enable_spanner` is false in every committed
# environment, so no environment's plan shows the instance at all: this run
# turns it on against a mocked provider and reads the planned attribute, which
# the acceptance suite's text scan cannot do. `command = plan` throughout.
mock_provider "google" {}

variables {
  project_id           = "test-project"
  environment          = "test"
  region               = "us-central1"
  enable_cloud_storage = false
  enable_bigquery      = false
  enable_bigtable      = false
  enable_alloydb       = false
  enable_memorystore   = false
  key_ring_id          = "/projects/test-project/locations/us-central1/keyRings/test"
  network_id           = "projects/test-project/global/networks/default"
  labels               = {}
}

run "the_ledger_instance_is_planned_on_enterprise_plus" {
  command = plan

  variables {
    enable_spanner = true
  }

  assert {
    condition     = length(google_spanner_instance.global) == 1
    error_message = "enable_spanner = true planned no Spanner instance, so the edition below is not being read."
  }

  assert {
    condition     = google_spanner_instance.global[0].edition == "ENTERPRISE_PLUS"
    error_message = "the ledger's Spanner instance is not planned on ENTERPRISE_PLUS; its multi-region consistency guarantee is the edition's."
  }
}

run "no_instance_is_planned_while_spanner_is_off" {
  command = plan

  variables {
    enable_spanner = false
  }

  assert {
    condition     = length(google_spanner_instance.global) == 0
    error_message = "enable_spanner = false planned a Spanner instance."
  }
}

# The harness policy asks every plan file to prove a refusal beside its
# admissions. The edition is not itself a validation, so the refusal here is
# the neighbouring floor on the same instance: a sliver of a node is refused
# while the default is admitted by the first run above.
run "a_floor_below_one_node_is_refused_on_the_same_instance" {
  command = plan

  variables {
    enable_spanner               = true
    spanner_min_processing_units = 100
  }

  expect_failures = [var.spanner_min_processing_units]
}
