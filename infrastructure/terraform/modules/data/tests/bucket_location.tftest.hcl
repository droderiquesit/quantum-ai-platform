# Test: bucket location must be dual-region or multi-region, never single-region

# A plan needs no credentials. Without this the harness asked for application
# default credentials and could not run anywhere, CI included.
mock_provider "google" {}

run "archive_bucket_location_is_dual_region" {
  command = plan

  variables {
    project_id           = "test-project"
    region               = "us-central1"
    environment          = "test"
    labels               = { env = "test" }
    key_ring_id          = "projects/p/locations/l/keyRings/kr"
    network_id           = "projects/p/global/networks/n"
    enable_cloud_storage = true
    bucket_location      = "US" # Dual-region
  }

  assert {
    condition     = google_storage_bucket.archive[0].location == "US"
    error_message = "Archive bucket location must be a dual-region or multi-region location, not a single region"
  }
}

run "archive_bucket_rejects_single_region" {
  command = plan

  variables {
    project_id           = "test-project"
    region               = "us-central1"
    environment          = "test"
    labels               = { env = "test" }
    key_ring_id          = "projects/p/locations/l/keyRings/kr"
    network_id           = "projects/p/global/networks/n"
    enable_cloud_storage = true
    bucket_location      = "us-central1" # Single-region - should fail
  }

  # The refusing half. `bucket_location`'s validation admits only
  # `^[A-Z0-9]+$` — `US`, `EU`, `NAM4` — and every single-region name carries
  # a hyphen, so DATA-067's "never single-region" is enforced at plan time.
  # This run said `expect_failures = []` with a note that Terraform accepts a
  # single region; the module refuses one, so that run could never pass and
  # asserted the opposite of the requirement it is named for.
  expect_failures = [var.bucket_location]
}

run "artifacts_bucket_uses_same_location" {
  command = plan

  variables {
    project_id           = "test-project"
    region               = "us-central1"
    environment          = "test"
    labels               = { env = "test" }
    key_ring_id          = "projects/p/locations/l/keyRings/kr"
    network_id           = "projects/p/global/networks/n"
    enable_cloud_storage = true
    bucket_location      = "EU" # Dual-region EU
  }

  assert {
    condition     = google_storage_bucket.artifacts[0].location == "EU"
    error_message = "Artifacts bucket must use the same bucket_location variable as archive"
  }
}
