# The resource hierarchy (R12-GCP-10001, 8043-8050, 8052), planned against
# mocks: no credential, no organisation, `command = plan` throughout.

mock_provider "google" {}

variables {
  parent          = "organizations/123456789"
  billing_account = "000000-AAAAAA-BBBBBB"
  prefix          = "qip"
  environments = {
    prod = ["us-east4", "europe-west2"]
    dev  = ["us-east4"]
  }
}

run "every_environment_gets_the_same_planes_as_their_own_projects" {
  command = plan

  assert {
    # prod: five global planes and three per region over two regions; dev:
    # five and three.
    condition     = length(google_project.plane) == 19
    error_message = "the hierarchy did not plan one project per plane per environment"
  }

  assert {
    condition = (
      toset([for k, p in google_project.plane : p.labels["plane"] if p.labels["environment"] == "prod"]) ==
      toset([for k, p in google_project.plane : p.labels["plane"] if p.labels["environment"] == "dev"])
    )
    error_message = "non-production does not mirror production's planes"
  }

  assert {
    condition     = google_project.plane["prod/ledger"].project_id == "qip-prod-ledger" && google_project.plane["prod/exec-europe-west2"].project_id == "qip-prod-exec-europe-west2"
    error_message = "a plane's project is not named for its plane"
  }

  assert {
    condition     = google_project.plane["prod/ledger"].project_id != google_project.plane["dev/ledger"].project_id
    error_message = "two environments share a project"
  }

  assert {
    condition     = alltrue([for p in values(google_project.plane) : p.auto_create_network == false])
    error_message = "a plane's project gets the default network"
  }
}

run "an_id_longer_than_google_allows_is_refused" {
  command = plan

  variables {
    prefix       = "qipxy"
    environments = { prod = ["northamerica-northeast1"] }
  }

  expect_failures = [var.environments]
}

run "a_hierarchy_without_production_is_refused" {
  command = plan

  variables {
    environments = { dev = ["us-east4"] }
  }

  expect_failures = [var.environments]
}
