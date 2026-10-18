# The planes this module can name, planned.
#
# `execution` is not one of them: the execution node is a Compute Engine
# machine, not a Cloud Run workload, and a Cloud Run service on the venue path
# would put a cold start and a shared network in front of an order. The
# Rust acceptance suite reads this validation as text and so cannot tell a
# list that refuses `execution` from one that merely does not spell it; only a
# plan shows the validation fire on the bad value and stay quiet on a good one.
#
# Runs against a mocked provider, so it needs no credential and reaches no
# project; `command = plan` throughout, so nothing is applied even in the mock.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id     = "cloudrun-plan-harness"
  project_number = "1"
  region         = "us-east4"
  environment    = "dev"
  labels         = {}
  name           = "harness"
  traffic_class  = "platform"
}

run "a_plane_the_module_names_is_admitted" {
  command = plan

  variables {
    plane = "cognition"
  }
}

run "the_execution_plane_is_refused" {
  command = plan

  variables {
    plane = "execution"
  }

  expect_failures = [var.plane]
}
