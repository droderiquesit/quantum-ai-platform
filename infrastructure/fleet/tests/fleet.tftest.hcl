# The fleet root, planned (ADR 0102 decision 3 and the Validation section).
#
# Three properties a reader of the text cannot establish and a plan can:
#
#   1. Parallelism above 40 is refused, and 8, 40 are admitted. 41 is the
#      boundary case: a `<= 40` mistyped as `< 40` would pass 41 and refuse
#      the owner's own ceiling, so both edges are run.
#   2. The worker's bindings are exactly roles/aiplatform.user at the project
#      and object access on its own bucket, and none of the forbidden families
#      appears. The assertion reads `output.worker_roles`, which is built from
#      the roles on the IAM resources themselves, not from a variable, so
#      adding a binding to the resources changes the list.
#   3. The bucket is uniform-access, read off the bucket resource.
#
# Every provider is mocked and every run is `command = plan`: no credential,
# no project, nothing created. This proves the configuration is coherent and
# its refusals fire. It says nothing about what Google does with it.

mock_provider "google" {}

variables {
  project_id = "algorik-platform-dev"
  image      = "us-docker.pkg.dev/algorik-platform-dev/fleet/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000"
}

run "parallelism_over_forty_is_refused" {
  command = plan

  variables {
    parallelism = 41
  }

  expect_failures = [var.parallelism]
}

run "parallelism_of_forty_is_admitted" {
  command = plan

  variables {
    parallelism = 40
  }

  assert {
    condition     = output.job_parallelism == 40
    error_message = "parallelism 40 is the owner's ceiling and must be admitted and reach the Job"
  }
}

run "the_default_parallelism_is_eight_and_retries_are_off" {
  command = plan

  assert {
    condition     = output.job_parallelism == 8
    error_message = "the default rung is 8 (ADR 0102 decision 1)"
  }

  assert {
    condition     = output.job_max_retries == 0
    error_message = "the Job must not retry a task blindly (ADR 0102 decision 2)"
  }
}

run "the_unprovisioned_placeholder_is_refused" {
  command = plan

  variables {
    project_id = "unprovisioned"
  }

  expect_failures = [var.project_id]
}

run "the_ceilings_above_the_owners_are_refused" {
  command = plan

  variables {
    daily_usd_ceiling = 26
    per_slot_usd_cap  = 0.51
  }

  expect_failures = [var.daily_usd_ceiling, var.per_slot_usd_cap]
}

run "an_image_pinned_by_tag_is_refused" {
  command = plan

  variables {
    image = "us-docker.pkg.dev/algorik-platform-dev/fleet/worker:latest"
  }

  expect_failures = [var.image]
}

run "the_worker_holds_aiplatform_user_and_its_bucket_and_nothing_else" {
  command = plan

  assert {
    condition     = toset(output.worker_roles) == toset(["roles/aiplatform.user", "roles/storage.objectUser"])
    error_message = "the worker's roles must be exactly aiplatform.user and objectUser on its bucket"
  }

  # Substring families, so a broader role in the same family is caught as well
  # as the named one: secretAccessor, cloudkms.cryptoKeyDecrypter, artifactregistry.writer,
  # run.admin, binaryauthorization.policyEditor, owner, editor.
  assert {
    condition = length([
      for r in output.worker_roles : r
      if length(regexall("secretmanager|cloudkms|artifactregistry|run\\.|binaryauthorization|iam\\.|owner|editor|source", r)) > 0
    ]) == 0
    error_message = "the worker holds a role in a forbidden family (secrets, KMS, Artifact Registry, Cloud Run, Binary Authorization, IAM, git)"
  }

  assert {
    condition     = !contains(output.worker_roles, "roles/storage.admin")
    error_message = "the worker must not hold storage.admin"
  }
}

run "the_harder_stop_removes_the_model_binding" {
  command = plan

  variables {
    worker_may_call_models = false
  }

  assert {
    condition     = toset(output.worker_roles) == toset(["roles/storage.objectUser"])
    error_message = "with worker_may_call_models = false the aiplatform.user binding must be gone"
  }
}

run "the_bucket_is_uniform_access" {
  command = plan

  assert {
    condition     = output.bucket_uniform_access == true
    error_message = "the fleet bucket must be uniform-access: per-object ACLs would let one object escape the bucket's IAM"
  }

  assert {
    condition     = google_storage_bucket.fleet.public_access_prevention == "enforced"
    error_message = "the fleet bucket must prevent public access"
  }
}

run "everything_is_labelled" {
  command = plan

  assert {
    condition = alltrue([
      for k in ["env", "service", "owner", "cost-center"] :
      contains(keys(google_storage_bucket.fleet.labels), k) && contains(keys(google_cloud_run_v2_job.fleet.labels), k)
    ])
    error_message = "the bucket and the Job must carry env, service, owner and cost-center"
  }
}
