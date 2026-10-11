# R12 CICD-9112: the build registry admits later environments as readers,
# and only as readers, and only service accounts.
#
# Mocked provider, `command = plan` throughout: no credential, no project.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id            = "registry-plan-harness"
  project_number        = "123456789"
  region                = "us-east4"
  environment           = "dev"
  labels                = { "environment" = "dev" }
  ci_service_account    = "ci@project.iam.gserviceaccount.com"
  pull_service_accounts = {}
  key_ring_id           = "projects/registry-plan-harness/locations/us-east4/keyRings/qip-dev"
}

# The admitting half: a service account is granted reader and only reader.
run "a_later_environments_service_account_is_admitted_as_a_reader_only" {
  command = plan

  variables {
    promotion_readers = ["serviceAccount:qip-ci-test@algorik-platform-test.iam.gserviceaccount.com"]
  }

  assert {
    condition     = length(google_artifact_registry_repository_iam_member.promotion_read) == 1
    error_message = "One promotion reader was named and the plan does not grant exactly one."
  }

  assert {
    condition = alltrue([
      for grant in google_artifact_registry_repository_iam_member.promotion_read :
      grant.role == "roles/artifactregistry.reader"
    ])
    error_message = "A promotion reader was granted something other than artifactregistry.reader; a later environment that can write here can replace the bytes the earlier ones tested."
  }
}

# The refusing half: anything but one service account is refused at plan.
run "a_promotion_reader_that_is_not_a_service_account_is_refused" {
  command = plan

  variables {
    promotion_readers = ["allAuthenticatedUsers"]
  }

  expect_failures = [var.promotion_readers]
}

run "a_person_as_a_promotion_reader_is_refused" {
  command = plan

  variables {
    promotion_readers = ["user:someone@example.com"]
  }

  expect_failures = [var.promotion_readers]
}

# And by default nothing outside the environment reads the registry.
run "no_promotion_reader_is_granted_by_default" {
  command = plan

  assert {
    condition     = length(google_artifact_registry_repository_iam_member.promotion_read) == 0
    error_message = "A promotion reader was granted with none named."
  }
}
