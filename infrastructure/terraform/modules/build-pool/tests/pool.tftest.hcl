# The private build pool (R12 CICD-9097), planned.
#
# Mocked provider and `command = plan` throughout: no credential, no
# project, nothing created. Each run names the mutation of `main.tf` or
# `variables.tf` that should make it fail.

mock_provider "google" {}

variables {
  project_id         = "pool-plan-harness"
  region             = "us-east4"
  environment        = "dev"
  peered_network_id  = "projects/pool-plan-harness/global/networks/qip-dev-engineering"
  ci_service_account = "qip-ci-dev@pool-plan-harness.iam.gserviceaccount.com"
  key_ring_id        = "projects/pool-plan-harness/locations/us-east4/keyRings/qip-dev"
}

# The names `deploy.yml` derives from the tfvars. If either side renames
# one, the workflow submits to a pool, an identity or a bucket that does not
# exist; this run fails first. Mutation: rename `local.pool_name`.
run "the_pool_identity_and_bucket_have_the_names_the_workflow_derives" {
  command = plan

  assert {
    condition     = google_cloudbuild_worker_pool.pool.name == "qip-dev-build"
    error_message = "The pool is not named qip-<env>-build, which is the name deploy.yml submits to."
  }

  assert {
    condition     = google_service_account.build.account_id == "qip-build-dev"
    error_message = "The build identity is not qip-build-<env>, which is the account deploy.yml runs builds as."
  }

  assert {
    condition     = google_storage_bucket.staging.name == "pool-plan-harness-qip-dev-build-source"
    error_message = "The staging bucket is not <project>-qip-<env>-build-source, which is where deploy.yml stages the checkout."
  }
}

# Private networking: the pool is peered into the network it was given, on
# the range reserved there, and nowhere else. Mutation: drop
# `network_config`, which leaves a pool in Google's shared network.
run "the_pool_is_peered_into_the_engineering_network" {
  command = plan

  assert {
    condition     = google_cloudbuild_worker_pool.pool.network_config[0].peered_network == "projects/pool-plan-harness/global/networks/qip-dev-engineering"
    error_message = "The pool is not peered into the network it was given."
  }

  assert {
    condition     = google_compute_global_address.pool.purpose == "VPC_PEERING" && google_compute_global_address.pool.network == "projects/pool-plan-harness/global/networks/qip-dev-engineering"
    error_message = "The peering range is not a VPC_PEERING range in the pool's network."
  }

  assert {
    condition     = google_cloudbuild_worker_pool.pool.network_config[0].peered_network_ip_range == "/24"
    error_message = "The pool's worker range is not the /24 the harness reserved."
  }
}

# The build identity writes its own logs and its saved image under `out/`,
# reads the staged source, and nothing else: no registry write, no Secret
# Manager, no KMS, no attestor. Mutations that must fail this run: drop the
# `condition` on the output grant (the build could overwrite another
# build's staged source), or widen the project-level role.
run "the_build_identity_holds_only_what_a_build_needs" {
  command = plan

  assert {
    condition     = google_project_iam_member.build_writes_logs.role == "roles/logging.logWriter"
    error_message = "The build identity's only project-level role is not logging.logWriter."
  }

  assert {
    condition     = google_storage_bucket_iam_member.build_writes_output.role == "roles/storage.objectCreator" && length(google_storage_bucket_iam_member.build_writes_output.condition) == 1
    error_message = "The build identity's write on the staging bucket is not object create."
  }

  assert {
    condition     = strcontains(google_storage_bucket_iam_member.build_writes_output.condition[0].expression, "/objects/out/\")")
    error_message = "The build identity may write anywhere in the staging bucket, not only under out/."
  }

  assert {
    condition     = google_storage_bucket_iam_member.build_reads_source.role == "roles/storage.objectViewer"
    error_message = "The build identity cannot read the staged source."
  }
}

# The pipeline may run builds as the build identity — on that account, not
# project-wide. Mutation: replace the account-scoped binding with a project
# `iam.serviceAccountUser`, which would let the pipeline act as every account.
run "the_pipeline_acts_as_the_build_identity_alone" {
  command = plan

  assert {
    condition     = google_service_account_iam_member.ci_runs_builds_as_the_build_identity.role == "roles/iam.serviceAccountUser"
    error_message = "The pipeline cannot run builds as the build identity."
  }

  assert {
    condition     = google_service_account_iam_member.ci_runs_builds_as_the_build_identity.member == "serviceAccount:qip-ci-dev@pool-plan-harness.iam.gserviceaccount.com"
    error_message = "The actAs grant names an identity other than the pipeline's."
  }
}

# Admits a valid peering size, refuses one Cloud Build rejects at apply.
run "a_peering_range_smaller_than_a_slash_24_is_refused" {
  command = plan

  variables {
    peering_prefix_length = 28
  }

  expect_failures = [var.peering_prefix_length]
}

run "a_service_network_given_as_a_name_is_refused" {
  command = plan

  variables {
    peered_network_id = "qip-dev-service"
  }

  expect_failures = [var.peered_network_id]
}
