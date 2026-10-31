# Test: the event archive is locked and versioned (R12-DATA-8125)
#
# The archive receives each sealed segment from fabricd's own archive path,
# never from a landing bucket: object lifecycle cannot move objects between
# buckets, and no transfer service is bought for it (owner-delegated ruling
# of 2026-10-31, decision 10). What this plan proves is the half a plan can:
# the bucket that receives the record keeps every version and holds a locked
# retention policy, so nothing — a lifecycle rule, a cost-response job, an
# operator — can shorten the window and delete the one segment an
# investigation needs. Storage stays off by default because the seven-year
# lock cannot be undone once applied.

# A plan needs no credentials.
mock_provider "google" {}

run "the_archive_holds_a_locked_retention_policy_and_keeps_every_version" {
  command = plan

  variables {
    project_id           = "test-project"
    region               = "us-central1"
    environment          = "test"
    labels               = { env = "test" }
    key_ring_id          = "projects/p/locations/l/keyRings/kr"
    network_id           = "projects/p/global/networks/n"
    enable_cloud_storage = true
    bucket_location      = "US"
  }

  assert {
    condition     = google_storage_bucket.archive[0].retention_policy[0].is_locked == true
    error_message = "The event archive's retention policy must be locked; an unlocked one can be shortened and the record deleted"
  }

  assert {
    condition     = google_storage_bucket.archive[0].retention_policy[0].retention_period == var.archive_retention_days * 24 * 60 * 60
    error_message = "The archive's retention period must be the declared archive_retention_days"
  }

  assert {
    condition     = google_storage_bucket.archive[0].versioning[0].enabled == true
    error_message = "The event archive must keep every object version"
  }
}

run "no_archive_is_planned_while_storage_is_off" {
  command = plan

  variables {
    project_id  = "test-project"
    region      = "us-central1"
    environment = "test"
    labels      = { env = "test" }
    key_ring_id = "projects/p/locations/l/keyRings/kr"
    network_id  = "projects/p/global/networks/n"
  }

  assert {
    condition     = length(google_storage_bucket.archive) == 0
    error_message = "The archive's seven-year lock cannot be undone; storage must stay off until the change that lands its first writer"
  }
}

run "a_retention_shorter_than_seven_years_is_refused" {
  command = plan

  variables {
    project_id             = "test-project"
    region                 = "us-central1"
    environment            = "test"
    labels                 = { env = "test" }
    key_ring_id            = "projects/p/locations/l/keyRings/kr"
    network_id             = "projects/p/global/networks/n"
    enable_cloud_storage   = true
    bucket_location        = "US"
    archive_retention_days = 365
  }

  expect_failures = [var.archive_retention_days]
}
