# The evidence bucket retention lock validation, planned.
#
# GOV-023: Archive buckets whose retention must be immutable use Bucket Lock
# with versioning. A plan declaring an immutable-class bucket without the lock
# must fail validation, and one with the lock must pass.
#
# The evidence bucket is always immutable by design; this test asserts that
# setting retention_locked = false is refused at plan time.
#
# The provider is mocked, so this needs no credential, reaches no project and
# creates nothing; `command = plan` throughout.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id  = "evidence-plan-harness"
  region      = "us-central1"
  environment = "dev"
  labels      = {}
  key_ring_id = "projects/evidence-plan-harness/locations/us-central1/keyRings/qip-dev"
}

run "retention_locked_defaults_to_true_and_succeeds" {
  command = plan

  # The default case: retention_locked is not passed, so it defaults to true.
  # This should succeed.
  assert {
    condition     = google_storage_bucket.evidence.versioning[0].enabled == true
    error_message = "evidence bucket versioning is not enabled"
  }

  assert {
    condition     = google_storage_bucket.evidence.retention_policy[0].is_locked == true
    error_message = "evidence bucket retention policy is not locked with default value"
  }
}

run "retention_locked_explicitly_true_succeeds" {
  command = plan

  variables {
    retention_locked = true
  }

  assert {
    condition     = google_storage_bucket.evidence.retention_policy[0].is_locked == true
    error_message = "evidence bucket retention policy is not locked when explicitly set to true"
  }
}

run "retention_locked_false_is_refused" {
  command = plan

  variables {
    retention_locked = false
  }

  # The validation block on the retention_locked variable should fire and
  # refuse this plan.
  expect_failures = [
    var.retention_locked,
  ]
}
