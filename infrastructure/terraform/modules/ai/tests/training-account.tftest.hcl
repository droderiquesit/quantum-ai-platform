# The training grants' empty-account refusal, planned.
#
# The three grants in main.tf used to carry
# `count = var.enable_vertex_ai && var.training_service_account != "" ? 1 : 0`.
# That expression is unresolvable when the plan is saved — the account is a
# Cloud Run service account email, unknown until apply — so `plan -out` failed
# on all three and `infra.yml`'s `up` never reached an apply. The gate is now
# `var.enable_vertex_ai` alone, and the empty-account half is a precondition.
#
# A precondition is not visible to `terraform validate` and not visible to a
# Rust test reading the HCL: both see the text, and text cannot tell a rule
# that runs from one that cannot. Only a plan can. These runs are that plan.
#
# The provider is mocked, so this needs no credential, reaches no project and
# creates nothing; `command = plan` throughout.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id  = "ai-plan-harness"
  region      = "us-east4"
  environment = "dev"
  labels      = {}
  key_ring_id = "projects/ai-plan-harness/locations/us-east4/keyRings/qip-dev"
  network_id  = "projects/ai-plan-harness/global/networks/qip-dev"
}

run "the_module_is_off_by_default_and_binds_nothing" {
  command = plan

  variables {
    enable_vertex_ai         = false
    training_service_account = ""
  }

  # The admitting half for the disabled case, and the reason the precondition
  # is on the resources rather than on the variable: with the module off there
  # is nothing to bind to, so an empty account is not a mistake and must not
  # stop a plan. Every environment in this repository is in exactly this state.
  assert {
    condition     = length(google_storage_bucket_iam_member.training_writer) == 0
    error_message = "a disabled module bound a training writer, so `enable_vertex_ai = false` no longer means nothing is provisioned"
  }

  assert {
    condition     = length(google_project_iam_member.training_user) == 0
    error_message = "a disabled module granted aiplatform.user at the project"
  }
}

run "an_enabled_module_with_an_account_binds_all_three_grants" {
  command = plan

  variables {
    enable_vertex_ai         = true
    training_service_account = "qip-dev-deepbrain@ai-plan-harness.iam.gserviceaccount.com"
  }

  # The half that distinguishes a working gate from one that refuses
  # everything. All three grants exist, and the two on the bucket are the
  # narrow roles: a training identity that could delete could erase the inputs
  # of the run that produced a model.
  assert {
    condition     = google_storage_bucket_iam_member.training_writer[0].role == "roles/storage.objectCreator"
    error_message = "the training writer no longer holds object creation alone"
  }

  assert {
    condition     = google_storage_bucket_iam_member.training_reader[0].role == "roles/storage.objectViewer"
    error_message = "the training reader no longer holds object read alone"
  }

  assert {
    condition     = google_project_iam_member.training_user[0].role == "roles/aiplatform.user"
    error_message = "the training identity no longer holds aiplatform.user; admin would let a workload replace the model serving live traffic"
  }

  assert {
    condition     = google_project_iam_member.training_user[0].member == "serviceAccount:qip-dev-deepbrain@ai-plan-harness.iam.gserviceaccount.com"
    error_message = "the grant names an account other than the one passed in"
  }
}

run "an_enabled_module_with_no_account_is_refused" {
  command = plan

  variables {
    enable_vertex_ai         = true
    training_service_account = ""
  }

  # What this configuration did silently before: provision the bucket, the
  # endpoint and the metadata store, create zero bindings, and report success.
  # The refusal has to fire on all three grants, because each one would
  # otherwise have written `serviceAccount:` with nothing after it.
  expect_failures = [
    google_storage_bucket_iam_member.training_writer,
    google_storage_bucket_iam_member.training_reader,
    google_project_iam_member.training_user,
  ]
}
