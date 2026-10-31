# The secondary image repository, planned through the root (R12-RES-9177).
#
# `modules/registry/tests/secondary.tftest.hcl` proves the module declares the
# repository when handed a region and refuses one in the primary's. What it
# cannot prove is that the root hands it anything: a `secondary_region` the
# root declared and never passed to `module.registry` would plan nothing in
# every environment, which reads exactly like the null every environment
# holds today. So the runs here plan the root, leave `module.registry`
# un-overridden so its resources are really planned, and assert on the
# root's own `secondary_image_prefix` output, which is read off the planned
# secondary repository's name rather than off the variable.
#
# Both halves, as every gate here must show (ADR 0069): null plans no
# secondary repository and a named region plans one; the region-shaped check
# and the same-region check each refuse at the root variable.
#
# What this does not prove: anything about Google. The providers are mocked,
# every run is `command = plan`, and no credential is used or project reached.
# Whether a release digest actually lands in the copy is deploy.yml's check,
# which reads the digest back from the secondary and refuses a mismatch.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "secondary-registry-harness"
  project_number    = 123456789012
  environment       = "dev"
  owner             = "platform"
  github_repository = "example/example"
  region            = "us-east4"

  # The four zones dev declares; the catalogue refuses a workload whose zone
  # this environment did not declare.
  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

# Not set, so this is the default every committed tfvars restates.
run "no_secondary_region_plans_no_secondary_repository" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/secondary-registry-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/secondary-registry-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }

  assert {
    condition     = output.secondary_image_prefix == null
    error_message = "with secondary_region unset the root still planned a secondary repository, which bills a replica no environment asked for"
  }
}

run "a_named_secondary_region_plans_the_secondary_repository_there" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/secondary-registry-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/secondary-registry-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }

  variables {
    secondary_region = "us-west1"
  }

  # The prefix is built from the planned repository, so it is null if the
  # root stopped passing the region, and names the wrong region or project
  # if the module stopped reading what the root passed. deploy.yml derives
  # the same string from the tfvars.
  assert {
    condition     = output.secondary_image_prefix == "us-west1-docker.pkg.dev/secondary-registry-harness/qip-dev-secondary"
    error_message = "a named secondary_region did not reach module.registry's secondary repository, or that repository is not where deploy.yml copies to"
  }
}

# --- the refusing half -------------------------------------------------------

run "a_secondary_region_equal_to_the_primary_stops_the_plan" {
  command = plan

  variables {
    # The region loss the replica exists for would take both.
    secondary_region = "us-east4"
  }

  expect_failures = [var.secondary_region]
}

run "a_secondary_region_that_is_not_region_shaped_stops_the_plan" {
  command = plan

  variables {
    # Upper case: Artifact Registry's locations are lower case, and deploy.yml
    # pastes this into a registry host, so a value it would refuse is refused
    # here first.
    secondary_region = "US-WEST1"
  }

  expect_failures = [var.secondary_region]
}

run "a_multi_region_secondary_stops_the_plan" {
  command = plan

  variables {
    # `us` is an Artifact Registry multi-region. The ruling is a replica in
    # one other region, not a multi-region repository standing in for the
    # signed one, so it is refused rather than admitted as region-shaped.
    secondary_region = "us"
  }

  expect_failures = [var.secondary_region]
}
