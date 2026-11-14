# The desk mandate's risk tolerance, planned through the root (ADR 0405).
#
# `desk_risk_tolerance` is a fraction from 0 to 1 that narrows every maximum
# in the operator's limit set; unset, the binaries run at 1, the limits
# exactly as written. The three roots that assemble a platform stop at
# start-up on a value outside the range, so an environment applied with one
# is a platform that does not start. These runs put that refusal at plan
# time, and show the value reaches the workloads that read it and no other.
#
# Both halves, as every gate here must show (ADR 0069): a fraction in range
# is admitted and reaches `QIP_DESK_RISK_TOLERANCE` on qip-api, qip-fastbrain
# and qip-deepbrain; null sets it on none of them; a value above one, a
# negative one and a word each refuse at the root variable.
#
# What this does not prove: anything about Google, or that a binary narrows
# a limit. The providers are mocked and every run is `command = plan`. The
# narrowing itself is `qip-kernel`'s to prove, and does
# (`limit_narrowing`'s tests); the manifest carrying the variable is
# `manifest_wiring`'s parity check.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "desk-risk-tolerance-harness"
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

# Not set: the default test, stage and prod still hold.
run "no_tolerance_sets_the_variable_on_no_workload" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/desk-risk-tolerance-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/desk-risk-tolerance-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }

  assert {
    # Premise: the three roots are in the catalogue, or "none carries it"
    # below would hold of an empty set.
    condition = alltrue([
      for name in ["api", "fastbrain", "deepbrain"] :
      contains(keys(output.cloud_run_services), name)
    ])
    error_message = "the catalogue no longer holds api, fastbrain and deepbrain, so this harness is asserting about workloads that do not exist"
  }
  assert {
    condition = alltrue([
      for name, workload in output.cloud_run_services :
      !contains(keys(workload.environment), "QIP_DESK_RISK_TOLERANCE")
    ])
    error_message = "with desk_risk_tolerance unset a workload still carries QIP_DESK_RISK_TOLERANCE, so an environment that stated nothing is running a tolerance somebody else chose"
  }
}

run "a_stated_tolerance_reaches_the_three_roots_and_no_other_workload" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/desk-risk-tolerance-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/desk-risk-tolerance-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }

  variables {
    desk_risk_tolerance = "0.5"
  }

  assert {
    condition = alltrue([
      for name in ["api", "fastbrain", "deepbrain"] :
      lookup(output.cloud_run_services[name].environment, "QIP_DESK_RISK_TOLERANCE", null) == "0.5"
    ])
    error_message = "a stated desk_risk_tolerance did not reach QIP_DESK_RISK_TOLERANCE on api, fastbrain and deepbrain, so the roots would run the operator's limits un-narrowed while the tfvars said otherwise"
  }
  assert {
    condition = alltrue([
      for name, workload in output.cloud_run_services :
      contains(["api", "fastbrain", "deepbrain"], name) || !contains(keys(workload.environment), "QIP_DESK_RISK_TOLERANCE")
    ])
    error_message = "a workload that assembles no platform carries QIP_DESK_RISK_TOLERANCE; only the three roots read it"
  }
}

run "a_tolerance_of_one_is_admitted" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/desk-risk-tolerance-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/desk-risk-tolerance-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }

  variables {
    # The figure dev states: the operator's limits exactly as written.
    desk_risk_tolerance = "1"
  }

  assert {
    condition     = output.cloud_run_services["api"].environment["QIP_DESK_RISK_TOLERANCE"] == "1"
    error_message = "a tolerance of one, the upper bound of the range, did not reach the API"
  }
}

# --- the refusing half -------------------------------------------------------

run "a_tolerance_above_one_stops_the_plan" {
  command = plan

  variables {
    # A tolerance above one would raise a limit over the operator's ceiling.
    desk_risk_tolerance = "1.5"
  }

  expect_failures = [var.desk_risk_tolerance]
}

run "a_negative_tolerance_stops_the_plan" {
  command = plan

  variables {
    desk_risk_tolerance = "-0.1"
  }

  expect_failures = [var.desk_risk_tolerance]
}

run "a_tolerance_that_is_not_a_number_stops_the_plan" {
  command = plan

  variables {
    # A percentage written as a word, or "50%", is the likely mistake.
    desk_risk_tolerance = "half"
  }

  expect_failures = [var.desk_risk_tolerance]
}
