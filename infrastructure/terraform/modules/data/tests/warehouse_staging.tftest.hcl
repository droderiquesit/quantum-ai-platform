# Test: warehouse load batches expire (ADR 0240, R12-DATA-8127)
#
# The deep brain stages each batch of resolved outcomes under
# `warehouse-staging/` in the regional segment bucket, where the research
# dataset lives, and loads it with one BigQuery job. The staged object is a
# transport copy: the table and the event log are the records, so the
# prefix must expire, and at the bucket's own seven-day retention floor.

mock_provider "google" {}

run "warehouse_staging_batches_expire_after_seven_days" {
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
    condition = length([
      for rule in google_storage_bucket.segments[0].lifecycle_rule : rule
      if one(rule.action).type == "Delete"
      && try(contains(one(rule.condition).matches_prefix, "warehouse-staging/"), false)
      && one(rule.condition).age == 7
    ]) == 1
    error_message = "The segment bucket must delete warehouse-staging/ batches at seven days; a staged batch is a transport copy, not a record"
  }

  assert {
    condition     = google_storage_bucket.segments[0].location == "us-central1"
    error_message = "Batches are staged in the dataset's region, which is the segment bucket's"
  }
}

# The load path's grants, and only with both stores: a writer may create a
# load job, and create and read objects under warehouse-staging/ alone, so it
# can neither read nor add a segment.
run "a_writer_may_create_load_jobs_and_touch_only_the_staging_prefix" {
  command = plan

  variables {
    project_id              = "test-project"
    region                  = "us-central1"
    environment             = "test"
    labels                  = { env = "test" }
    key_ring_id             = "projects/p/locations/l/keyRings/kr"
    network_id              = "projects/p/global/networks/n"
    enable_cloud_storage    = true
    enable_bigquery         = true
    bucket_location         = "US"
    writer_service_accounts = ["deepbrain@test-project.iam.gserviceaccount.com"]
  }

  # The mock's random email would trip the warehouse key's own precondition,
  # which is not what this run is about.
  override_data {
    target = data.google_bigquery_default_service_account.bigquery[0]
    values = {
      email = "bq-123456789012@bigquery-encryption.iam.gserviceaccount.com"
    }
  }

  assert {
    condition     = google_project_iam_member.warehouse_load_jobs["deepbrain@test-project.iam.gserviceaccount.com"].role == "roles/bigquery.jobUser"
    error_message = "A writer that loads the warehouse must be able to create a load job"
  }

  assert {
    condition = alltrue([
      for grant in [
        google_storage_bucket_iam_member.warehouse_staging_writers["deepbrain@test-project.iam.gserviceaccount.com"],
        google_storage_bucket_iam_member.warehouse_staging_readers["deepbrain@test-project.iam.gserviceaccount.com"],
      ] : strcontains(try(one(grant.condition).expression, ""), "/objects/warehouse-staging/")
    ])
    error_message = "A writer's object grants on the segment bucket must be confined to warehouse-staging/"
  }
}

run "no_load_grant_exists_while_either_store_is_off" {
  command = plan

  variables {
    project_id              = "test-project"
    region                  = "us-central1"
    environment             = "test"
    labels                  = { env = "test" }
    key_ring_id             = "projects/p/locations/l/keyRings/kr"
    network_id              = "projects/p/global/networks/n"
    enable_cloud_storage    = true
    bucket_location         = "US"
    writer_service_accounts = ["deepbrain@test-project.iam.gserviceaccount.com"]
  }

  assert {
    condition     = length(google_project_iam_member.warehouse_load_jobs) == 0 && length(google_storage_bucket_iam_member.warehouse_staging_writers) == 0
    error_message = "No load grant may exist until both the dataset and the staging bucket do"
  }
}

# The refusing half: the project-wide load-job role goes only to a workload's
# own service account.
run "a_load_job_grant_to_anything_but_a_service_account_is_refused" {
  command = plan

  variables {
    project_id              = "test-project"
    region                  = "us-central1"
    environment             = "test"
    labels                  = { env = "test" }
    key_ring_id             = "projects/p/locations/l/keyRings/kr"
    network_id              = "projects/p/global/networks/n"
    enable_cloud_storage    = true
    enable_bigquery         = true
    bucket_location         = "US"
    writer_service_accounts = ["analyst@example.com"]
  }

  override_data {
    target = data.google_bigquery_default_service_account.bigquery[0]
    values = {
      email = "bq-123456789012@bigquery-encryption.iam.gserviceaccount.com"
    }
  }

  expect_failures = [google_project_iam_member.warehouse_load_jobs]
}
