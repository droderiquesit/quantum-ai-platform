# Terraform test: the workload log bucket has a bounded retention
#
# R12-OBS-10058. The retention is a plan-time number, so it is read from a
# plan against a mocked provider; the variable's bound is exercised on both
# sides, because a gate that only refuses is indistinguishable from one that
# refuses everything.
mock_provider "google" {}

variables {
  project_id            = "test-project"
  environment           = "test"
  labels                = {}
  notification_channels = []
}

run "the_default_retention_is_planned_on_the_dedicated_bucket" {
  command = plan

  assert {
    condition     = google_logging_project_bucket_config.workloads.retention_days == 90
    error_message = "the workload log bucket is not planned with the default 90-day retention."
  }

  assert {
    condition     = google_logging_project_bucket_config.workloads.bucket_id != "_Default"
    error_message = "the module manages the project's _Default bucket, which this repository did not create."
  }
}

run "a_retention_at_the_ceiling_is_admitted" {
  command = plan

  variables {
    log_retention_days = 400
  }

  assert {
    condition     = google_logging_project_bucket_config.workloads.retention_days == 400
    error_message = "a retention of 400 days was not planned."
  }
}

run "a_retention_past_the_ceiling_is_refused" {
  command = plan

  variables {
    log_retention_days = 401
  }

  expect_failures = [var.log_retention_days]
}

run "a_zero_retention_is_refused" {
  command = plan

  variables {
    log_retention_days = 0
  }

  expect_failures = [var.log_retention_days]
}
