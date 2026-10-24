# NCC hubs (R12-GCP-8057, R12-GCP-8058), planned against mocks: no
# credential, no project, `command = plan` throughout.

mock_provider "google" {}

variables {
  project_id  = "ncc-harness"
  environment = "dev"
}

run "no_hub_is_declared_by_default" {
  command = plan

  assert {
    condition     = length(google_network_connectivity_hub.latency) == 0 && length(google_network_connectivity_hub.services) == 0
    error_message = "a hub was declared with no spokes named"
  }
}

run "the_latency_hub_is_a_mesh_exporting_only_narrow_prefixes" {
  command = plan

  variables {
    latency_spokes = {
      newyork = { network_uri = "projects/p/global/networks/reflex-us", export = ["10.10.0.0/28"] }
      london  = { network_uri = "projects/p/global/networks/reflex-eu", export = ["10.20.0.0/28", "10.20.1.0/29"] }
    }
  }

  assert {
    condition     = google_network_connectivity_hub.latency[0].preset_topology == "MESH"
    error_message = "the latency hub is not a mesh"
  }

  assert {
    condition     = google_network_connectivity_spoke.latency["london"].linked_vpc_network[0].include_export_ranges == tolist(["10.20.0.0/28", "10.20.1.0/29"])
    error_message = "a latency spoke exports something other than its named prefixes"
  }
}

run "a_latency_spoke_exporting_a_wide_prefix_is_refused" {
  command = plan

  variables {
    latency_spokes = {
      newyork = { network_uri = "projects/p/global/networks/reflex-us", export = ["10.10.0.0/16"] }
    }
  }

  expect_failures = [var.latency_spokes]
}

run "the_service_hub_is_a_star_with_center_and_edge_groups" {
  command = plan

  variables {
    star_spokes = {
      inspection = { network_uri = "projects/p/global/networks/mgmt", group = "center" }
      data       = { network_uri = "projects/p/global/networks/data", group = "edge" }
    }
  }

  assert {
    condition     = google_network_connectivity_hub.services[0].preset_topology == "STAR"
    error_message = "the service hub is not a star"
  }

  assert {
    condition     = endswith(google_network_connectivity_spoke.services["inspection"].group, "/groups/center") && endswith(google_network_connectivity_spoke.services["data"].group, "/groups/edge")
    error_message = "a star spoke is not in its group"
  }
}

run "a_star_with_no_center_is_refused" {
  command = plan

  variables {
    star_spokes = {
      data = { network_uri = "projects/p/global/networks/data", group = "edge" }
      ai   = { network_uri = "projects/p/global/networks/ai", group = "edge" }
    }
  }

  expect_failures = [var.star_spokes]
}
