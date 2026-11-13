# Terraform test: the ledger is placed where its writer runs (ADR 0393)
#
# R12-LEDGER-8136 asks that the Spanner configuration and its leader be chosen
# by write origin and residency, and that the plan set the leader. The
# configuration was `nam-eur-asea` with no default leader, so Spanner chose
# one and the ledger was replicated to three continents nobody reviewed.
# `qip-ledgerd`, the only writer, runs in `var.region`; the one decided
# placement is `us-east4` on `nam3` with the leader there. A region with no
# decided placement is refused at plan rather than defaulted, and the refusal
# is proven beside the admission, because a gate that only refuses is
# indistinguishable from one that refuses everything. `command = plan`
# throughout against a mocked provider.
mock_provider "google" {}

variables {
  project_id           = "test-project"
  environment          = "test"
  region               = "us-east4"
  enable_cloud_storage = false
  enable_bigquery      = false
  enable_bigtable      = false
  enable_alloydb       = false
  enable_memorystore   = false
  key_ring_id          = "/projects/test-project/locations/us-east4/keyRings/test"
  network_id           = "projects/test-project/global/networks/default"
  labels               = {}
}

run "the_ledger_written_from_us_east4_is_placed_on_nam3_with_its_leader_there" {
  command = plan

  variables {
    enable_spanner = true
  }

  assert {
    condition     = length(google_spanner_instance.global) == 1 && length(google_spanner_database.positions) == 1
    error_message = "enable_spanner = true planned no ledger instance and database, so the placement below is not being read."
  }

  assert {
    condition     = google_spanner_instance.global[0].config == "nam3"
    error_message = "the ledger's instance is not planned on nam3, the United States configuration ADR 0393 decides for a us-east4 writer."
  }

  assert {
    condition     = contains(google_spanner_database.positions[0].ddl, "ALTER DATABASE `positions` SET OPTIONS (default_leader = 'us-east4')")
    error_message = "the ledger database's plan sets no default leader at us-east4, so Spanner would choose a leader away from the writer."
  }
}

run "spanner_in_a_region_with_no_decided_placement_is_refused" {
  command = plan

  variables {
    region         = "europe-west2"
    key_ring_id    = "/projects/test-project/locations/europe-west2/keyRings/test"
    enable_spanner = true
  }

  expect_failures = [google_spanner_instance.global]
}

run "a_region_with_no_decided_placement_plans_while_spanner_is_off" {
  command = plan

  variables {
    region         = "europe-west2"
    key_ring_id    = "/projects/test-project/locations/europe-west2/keyRings/test"
    enable_spanner = false
  }

  assert {
    condition     = length(google_spanner_instance.global) == 0
    error_message = "enable_spanner = false planned a Spanner instance."
  }
}

# ADR 0394: the leader is a validated variable. The pair below proves the
# validation refuses a region outside nam3 and admits the write origin; a
# validation proven only to refuse is indistinguishable from one that refuses
# everything.
run "a_default_leader_outside_nam3s_read_write_regions_is_refused" {
  command = plan

  variables {
    enable_spanner         = true
    spanner_default_leader = "europe-west1"
  }

  expect_failures = [var.spanner_default_leader]
}

run "nam3s_witness_region_is_refused_as_the_default_leader" {
  command = plan

  variables {
    enable_spanner         = true
    spanner_default_leader = "us-central1"
  }

  expect_failures = [var.spanner_default_leader]
}

run "the_default_leader_us_east4_is_admitted_and_reaches_the_ddl" {
  command = plan

  variables {
    enable_spanner         = true
    spanner_default_leader = "us-east4"
  }

  assert {
    condition     = contains(google_spanner_database.positions[0].ddl, "ALTER DATABASE `positions` SET OPTIONS (default_leader = 'us-east4')")
    error_message = "spanner_default_leader = us-east4 was admitted but did not reach the positions database's DDL."
  }
}

run "the_default_leader_us_east1_reaches_the_ddl" {
  command = plan

  variables {
    enable_spanner         = true
    spanner_default_leader = "us-east1"
  }

  assert {
    condition     = contains(google_spanner_database.positions[0].ddl, "ALTER DATABASE `positions` SET OPTIONS (default_leader = 'us-east1')")
    error_message = "spanner_default_leader = us-east1 did not reach the DDL, so the variable is not what sets the leader."
  }
}
