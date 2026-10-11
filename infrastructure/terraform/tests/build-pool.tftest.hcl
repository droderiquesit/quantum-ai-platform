# R12 CICD-9097: the private build pool, wired from the root.
#
# Off by default; on, it peers into the Engineering VPC and nothing else;
# refused outside the build environment. Mocked providers and
# `command = plan` throughout. The three module overrides are the ones every
# root harness here carries (see fabric.tftest.hcl).

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/pool-root-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/pool-root-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/pool-root-harness/locations/us-east4/repositories/qip-dev"
    repository_name = "qip-dev"
    image_prefix    = "us-east4-docker.pkg.dev/pool-root-harness/qip-dev"
  }
}

variables {
  project_id        = "pool-root-harness"
  project_number    = 123456789012
  environment       = "dev"
  region            = "us-east4"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# Off unless the owner turns it on: no pool, no API. Mutation: make the
# module's count unconditional.
run "no_pool_is_planned_unless_the_build_environment_enables_one" {
  command = plan

  assert {
    condition     = length(module.build_pool) == 0 && output.build_pool == null
    error_message = "A build pool was planned with cloud_build_private_pool_enabled false."
  }

  assert {
    condition     = !contains(keys(module.services.enabled), "cloudbuild.googleapis.com")
    error_message = "Cloud Build's API was enabled with no pool to use it."
  }
}

# The admitting half: enabled in dev, the pool is planned, its API is on,
# and it is peered into the Engineering VPC — never the service network the
# workloads sit on. Mutation: pass `module.network.service_network_id`.
run "an_enabled_pool_is_peered_into_the_engineering_network" {
  command = plan

  variables {
    cloud_build_private_pool_enabled = true
  }

  assert {
    condition     = length(module.build_pool) == 1
    error_message = "An enabled pool was not planned."
  }

  assert {
    condition     = contains(keys(module.services.enabled), "cloudbuild.googleapis.com") && contains(keys(module.services.enabled), "servicenetworking.googleapis.com")
    error_message = "The pool's APIs are not enabled with it."
  }

  assert {
    condition     = module.build_pool[0].peered_network == "projects/pool-root-harness/global/networks/qip-dev-engineering"
    error_message = "The pool is not peered into the Engineering VPC."
  }

  assert {
    condition     = module.build_pool[0].staging_bucket == "pool-root-harness-qip-dev-build-source"
    error_message = "The staging bucket is not the one deploy.yml derives."
  }
}

# Refused: a pool outside the build environment. Mutation: delete the
# validation in build-pool.tf.
run "a_pool_outside_the_build_environment_is_refused" {
  command = plan

  variables {
    environment                      = "stage"
    cloud_build_private_pool_enabled = true
  }

  expect_failures = [var.cloud_build_private_pool_enabled]
}
