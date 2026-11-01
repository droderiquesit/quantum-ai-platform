# The fleet root, planned (ADR 0102 decision 3 and the Validation section).
#
# Three properties a reader of the text cannot establish and a plan can:
#
#   1. Parallelism above 40 is refused, and 8, 40 are admitted. 41 is the
#      boundary case: a `<= 40` mistyped as `< 40` would pass 41 and refuse
#      the owner's own ceiling, so both edges are run.
#   2. The worker's bindings are exactly roles/aiplatform.user and
#      roles/monitoring.viewer at the project and prefix-conditioned object
#      access on its own bucket (ADR 0102 amendment of 2026-10-11), and none of the forbidden families
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
  image      = "us-east4-docker.pkg.dev/algorik-platform-dev/fleet/worker@sha256:0000000000000000000000000000000000000000000000000000000000000000"
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
    image = "us-east4-docker.pkg.dev/algorik-platform-dev/fleet/worker:latest"
  }

  expect_failures = [var.image]
}

run "the_worker_holds_aiplatform_user_read_only_telemetry_and_its_bucket_prefixes_and_nothing_else" {
  command = plan

  assert {
    condition = toset(output.worker_roles) == toset([
      "roles/aiplatform.user",
      "roles/monitoring.viewer",
      "roles/storage.objectViewer",
      "roles/storage.objectUser",
    ])
    error_message = "the worker's roles must be exactly aiplatform.user, monitoring.viewer, and objectViewer and objectUser on its bucket (ADR 0102 amendment of 2026-10-11)"
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
    condition     = toset(output.worker_roles) == toset(["roles/monitoring.viewer", "roles/storage.objectViewer", "roles/storage.objectUser"])
    error_message = "with worker_may_call_models = false the aiplatform.user binding must be gone and the read-only telemetry role must remain"
  }
}

# ADR 0102 amendment, item 3: the bucket grant is narrowed by object prefix. A
# plan cannot tell whether Google evaluates a condition as intended, but it
# can pin what is asked for, and it fails if the grant widens back to the
# whole bucket: a binding without a condition, or a condition that names a
# prefix it should not.
run "the_workers_bucket_grants_are_conditioned_on_object_prefixes" {
  command = plan

  assert {
    condition     = length(google_storage_bucket_iam_member.worker_reads.condition) == 1 && length(google_storage_bucket_iam_member.worker_writes.condition) == 1
    error_message = "both bucket bindings must carry a condition: an unconditioned objectViewer or objectUser is the whole bucket"
  }

  assert {
    condition = (
      strcontains(output.worker_bucket_conditions.reads, "/objects/packets/") &&
      strcontains(output.worker_bucket_conditions.reads, "/objects/HALT") &&
      !strcontains(output.worker_bucket_conditions.reads, "/objects/output/") &&
      !strcontains(output.worker_bucket_conditions.reads, "/objects/ledger/")
    )
    error_message = "the read grant must reach packets/ and HALT and nothing else"
  }

  assert {
    condition = (
      strcontains(output.worker_bucket_conditions.writes, "/objects/output/") &&
      strcontains(output.worker_bucket_conditions.writes, "/objects/ledger/") &&
      !strcontains(output.worker_bucket_conditions.writes, "/objects/packets/") &&
      !strcontains(output.worker_bucket_conditions.writes, "/objects/HALT")
    )
    error_message = "the write grant must reach output/ and ledger/ and never packets/ or HALT: a worker that can delete HALT can un-halt the fleet"
  }
}

# ADR 0102 amendment, item 2: egress is restricted to Google APIs. The Job
# sends all traffic through the fleet VPC; the VPC's one allow rule admits TCP
# 443 to the Private Google Access range, and a lower-priority rule denies
# everything else. Both edges are pinned: an allow widened to 0.0.0.0/0, or a
# deny that is gone, is the open egress this closes.
run "the_job_reaches_google_apis_and_nothing_else" {
  command = plan

  assert {
    condition     = output.job_egress == "ALL_TRAFFIC"
    error_message = "the Job must route all egress through the VPC; PRIVATE_RANGES_ONLY would leave the internet open"
  }

  assert {
    condition     = toset(output.egress_allowed_ranges) == toset(["199.36.153.8/30"])
    error_message = "the allow rule must admit only the private.googleapis.com range"
  }

  assert {
    condition     = toset(output.egress_denied_ranges) == toset(["0.0.0.0/0"])
    error_message = "the deny rule must refuse every other destination"
  }

  assert {
    condition     = google_compute_firewall.deny_all_egress.priority > google_compute_firewall.allow_google_apis_egress.priority
    error_message = "the deny rule must have the larger priority number, so the allow rule is evaluated first"
  }

  assert {
    condition     = google_compute_firewall.allow_google_apis_egress.direction == "EGRESS" && google_compute_firewall.deny_all_egress.direction == "EGRESS"
    error_message = "both firewall rules must be egress rules"
  }

  assert {
    condition     = google_compute_subnetwork.fleet.private_ip_google_access == true
    error_message = "the subnet must have Private Google Access, or the Job reaches no API at all"
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
      contains(keys(google_storage_bucket.fleet.labels), k) && contains(keys(google_cloud_run_v2_job.fleet[0].labels), k) && contains(keys(google_artifact_registry_repository.fleet.labels), k)
    ])
    error_message = "the bucket, the Job and the image repository must carry env, service, owner and cost-center"
  }
}

# The first apply happens before any image exists: it has to create the
# repository the image is pushed to. Without this the root could only ever be
# applied by somebody who already had an image, which nobody can have.
run "without_an_image_there_is_no_job_and_the_rest_still_plans" {
  command = plan

  variables {
    image = null
  }

  assert {
    condition     = length(google_cloud_run_v2_job.fleet) == 0
    error_message = "with no image the Job must not be planned: it would name an image that does not exist"
  }

  assert {
    condition     = output.job_name == null
    error_message = "job_name must be null while there is no Job"
  }

  assert {
    condition     = google_artifact_registry_repository.fleet.format == "DOCKER"
    error_message = "the image repository must still be planned without an image, or nothing can ever be pushed"
  }

  assert {
    condition = toset(output.worker_roles) == toset([
      "roles/aiplatform.user",
      "roles/monitoring.viewer",
      "roles/storage.objectViewer",
      "roles/storage.objectUser",
    ])
    error_message = "the worker's fence must be the same with and without a Job"
  }
}

run "with_an_image_there_is_exactly_one_job" {
  command = plan

  assert {
    condition     = length(google_cloud_run_v2_job.fleet) == 1
    error_message = "a digest-pinned image must produce the one Job"
  }
}

# R12-SEC-9110: Cloud Run enforces the project's Binary Authorization policy
# only on a job that asks for it. Without the block the worker image ran
# unattested in a project whose default rule requires an attestation.
run "the_job_is_held_to_the_projects_binary_authorization_policy" {
  command = plan

  assert {
    condition     = length(google_cloud_run_v2_job.fleet[0].binary_authorization) == 1 && google_cloud_run_v2_job.fleet[0].binary_authorization[0].use_default == true
    error_message = "the fleet Job must ask for the project's Binary Authorization policy (use_default = true)"
  }

  assert {
    condition     = google_cloud_run_v2_job.fleet[0].binary_authorization[0].breakglass_justification == null
    error_message = "the fleet Job must not carry a breakglass justification"
  }
}
