# Terraform test: the Bigtable topology and the Spanner autoscaler
#
# R12-DATA-8126 and R12-DATA-9184 ask for a Bigtable instance of at least two
# clusters with app profiles chosen per workload; R12-DATA-9185 asks for a
# Spanner autoscaler whose floor is held, not merely written. Both are plan
# checks, so the provider is mocked; the floor run also proves the variable
# refuses a value below one node and admits one at it, because a gate that
# only refuses is indistinguishable from one that refuses everything.
mock_provider "google" {}

variables {
  project_id           = "test-project"
  environment          = "test"
  region               = "us-central1"
  enable_cloud_storage = false
  enable_bigquery      = false
  enable_alloydb       = false
  enable_memorystore   = false
  key_ring_id          = "/projects/test-project/locations/us-central1/keyRings/test"
  network_id           = "projects/test-project/global/networks/default"
  labels               = {}
}

run "bigtable_has_two_clusters_in_different_zones_and_a_profile_per_workload" {
  command = plan

  variables {
    enable_bigtable = true
    enable_spanner  = false
  }

  assert {
    condition     = length(google_bigtable_instance.timeseries[0].cluster) >= 2
    error_message = "The Bigtable instance must declare at least two clusters (R12-DATA-8126)."
  }

  assert {
    condition     = length(distinct([for c in google_bigtable_instance.timeseries[0].cluster : c.zone])) == length(google_bigtable_instance.timeseries[0].cluster)
    error_message = "Each Bigtable cluster must sit in its own zone, or a zonal outage takes both."
  }

  assert {
    condition     = toset(keys(google_bigtable_app_profile.timeseries)) == toset(["ingest", "serve", "batch"])
    error_message = "The Bigtable instance must carry exactly the ingest, serve and batch app profiles (R12-DATA-9184)."
  }

  # The writer's profile is the read-your-writes guarantee: single-cluster
  # routing, never multi-cluster.
  assert {
    condition     = google_bigtable_app_profile.timeseries["ingest"].multi_cluster_routing_use_any != true && length(google_bigtable_app_profile.timeseries["ingest"].single_cluster_routing) == 1
    error_message = "The ingest profile must route to a single cluster; multi-cluster routing lets a replay miss acknowledged writes."
  }

  assert {
    condition     = google_bigtable_app_profile.timeseries["serve"].multi_cluster_routing_use_any == true
    error_message = "The serve profile must use multi-cluster routing so lag-tolerant reads survive a zonal outage (R12-DATA-8126)."
  }

  # Every single-cluster profile names a cluster the instance declares.
  assert {
    condition = alltrue(flatten([
      for p in values(google_bigtable_app_profile.timeseries) : [
        for r in p.single_cluster_routing : contains([for c in google_bigtable_instance.timeseries[0].cluster : c.cluster_id], r.cluster_id)
      ]
    ]))
    error_message = "A single-cluster app profile names a cluster the instance does not declare."
  }
}

run "bigtable_disabled_plans_no_profile" {
  command = plan

  variables {
    enable_bigtable = false
    enable_spanner  = false
  }

  assert {
    condition     = length(google_bigtable_app_profile.timeseries) == 0
    error_message = "An app profile must not be planned without its instance."
  }
}

run "spanner_autoscales_from_the_floor" {
  command = plan

  variables {
    enable_bigtable = false
    enable_spanner  = true
  }

  assert {
    condition     = length(google_spanner_instance.global[0].autoscaling_config) == 1
    error_message = "The Spanner instance must autoscale rather than hold a fixed size (R12-DATA-9185)."
  }

  assert {
    condition     = google_spanner_instance.global[0].autoscaling_config[0].autoscaling_limits[0].min_processing_units >= 1000
    error_message = "The Spanner autoscaler's floor must be at least one node (1000 processing units) (R12-DATA-9185)."
  }

  assert {
    condition     = google_spanner_instance.global[0].autoscaling_config[0].autoscaling_limits[0].max_processing_units >= google_spanner_instance.global[0].autoscaling_config[0].autoscaling_limits[0].min_processing_units
    error_message = "The Spanner autoscaler's ceiling must not sit below its floor."
  }
}

run "spanner_floor_below_one_node_is_refused" {
  command = plan

  variables {
    enable_bigtable = false
    enable_spanner  = true
    # Zero, not 100: 100 is also refused by the whole-node clause, so a test
    # on 100 passed with the floor clause deleted. Zero is a multiple of 1000
    # and below the floor, so only the floor can refuse it.
    spanner_min_processing_units = 0
  }

  expect_failures = [var.spanner_min_processing_units]
}

run "spanner_ceiling_below_the_floor_is_refused" {
  command = plan

  variables {
    enable_bigtable              = false
    enable_spanner               = true
    spanner_min_processing_units = 2000
    spanner_max_processing_units = 1000
  }

  expect_failures = [var.spanner_max_processing_units]
}

run "spanner_floor_of_two_nodes_is_admitted" {
  command = plan

  variables {
    enable_bigtable              = false
    enable_spanner               = true
    spanner_min_processing_units = 2000
    spanner_max_processing_units = 4000
  }

  assert {
    condition     = google_spanner_instance.global[0].autoscaling_config[0].autoscaling_limits[0].min_processing_units == 2000
    error_message = "A floor of two nodes must be admitted and planned as written."
  }
}
