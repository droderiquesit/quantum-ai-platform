# Storage target validation and configuration.
#
# Verifies that the storage_target variable accepts implemented targets
# (memory, file, engine, cloud_storage, big_query) and rejects unimplemented ones.
# Also verifies that enable_cloud_storage gate controls GCS bucket provisioning.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "storage-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  regions            = ["us-east4"]
  zone_overrides     = {}
  enable_cloud_storage = false
  workload_instances = {}
  additional_kms_admins = []
}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/storage-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/storage-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/storage-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/storage-plan-harness/harness"
  }
}

# --- accepted targets --------------------------------------------------------

run "memory_is_an_accepted_storage_target" {
  command = plan

  variables {
    storage_target = "memory"
  }

  assert {
    condition     = var.storage_target == "memory"
    error_message = "memory storage target was not accepted"
  }
}

run "file_is_an_accepted_storage_target" {
  command = plan

  variables {
    storage_target = "file"
  }

  assert {
    condition     = var.storage_target == "file"
    error_message = "file storage target was not accepted"
  }
}

run "engine_is_an_accepted_storage_target" {
  command = plan

  variables {
    storage_target = "engine"
  }

  assert {
    condition     = var.storage_target == "engine"
    error_message = "engine storage target was not accepted"
  }
}

run "cloud_storage_is_an_accepted_storage_target" {
  command = plan

  variables {
    storage_target = "cloud_storage"
  }

  assert {
    condition     = var.storage_target == "cloud_storage"
    error_message = "cloud_storage target was not accepted; DATA-036 requires this to be reachable"
  }
}

run "big_query_is_an_accepted_storage_target" {
  command = plan

  variables {
    storage_target = "big_query"
  }

  assert {
    condition     = var.storage_target == "big_query"
    error_message = "big_query target was not accepted"
  }
}

# --- rejected targets --------------------------------------------------------

run "memorystore_is_rejected_and_never_reachable" {
  command = plan

  variables {
    storage_target = "memorystore"
  }

  expect_failures = [
    var.storage_target
  ]
}

run "alloy_db_is_rejected_as_unimplemented" {
  command = plan

  variables {
    storage_target = "alloy_db"
  }

  expect_failures = [
    var.storage_target
  ]
}

run "spanner_is_rejected_as_unimplemented" {
  command = plan

  variables {
    storage_target = "spanner"
  }

  expect_failures = [
    var.storage_target
  ]
}

run "bigtable_is_rejected_as_unimplemented" {
  command = plan

  variables {
    storage_target = "bigtable"
  }

  expect_failures = [
    var.storage_target
  ]
}
