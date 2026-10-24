# Connectivity tests (R12-GCP-10060), planned against mocks: no credential,
# no project, `command = plan` throughout.

mock_provider "google" {}

variables {
  project_id  = "connectivity-tests-harness"
  environment = "dev"
}

run "nothing_is_declared_by_default" {
  command = plan

  assert {
    condition     = length(google_network_management_connectivity_test.this) == 0
    error_message = "a connectivity test was declared with none named"
  }
}

run "an_intended_and_a_forbidden_path_are_declared_with_their_intent" {
  command = plan

  variables {
    tests = {
      node-to-api = {
        source_ip      = "10.10.0.5"
        destination_ip = "10.20.0.8"
        protocol       = "TCP"
        port           = 443
        expect         = "reachable"
      }
      node-to-internet = {
        source_ip      = "10.10.0.5"
        destination_ip = "203.0.113.10"
        protocol       = "TCP"
        port           = 443
        expect         = "unreachable"
      }
    }
  }

  assert {
    condition     = length(google_network_management_connectivity_test.this) == 2
    error_message = "both paths were not declared"
  }

  assert {
    condition     = google_network_management_connectivity_test.this["node-to-internet"].labels["expect"] == "unreachable" && google_network_management_connectivity_test.this["node-to-api"].labels["expect"] == "reachable"
    error_message = "a test does not carry its intent"
  }

  assert {
    condition     = google_network_management_connectivity_test.this["node-to-api"].destination[0].port == 443
    error_message = "the destination port was not planned"
  }
}

run "a_suite_of_only_intended_paths_is_refused" {
  command = plan

  variables {
    tests = {
      node-to-api = {
        source_ip      = "10.10.0.5"
        destination_ip = "10.20.0.8"
        protocol       = "TCP"
        port           = 443
        expect         = "reachable"
      }
    }
  }

  expect_failures = [var.tests]
}

run "a_test_with_no_intent_is_refused" {
  command = plan

  variables {
    tests = {
      node-to-api = {
        source_ip      = "10.10.0.5"
        destination_ip = "10.20.0.8"
        protocol       = "TCP"
        port           = 443
        expect         = "maybe"
      }
    }
  }

  expect_failures = [var.tests]
}
