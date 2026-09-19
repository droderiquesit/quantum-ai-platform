# ADR 0079's dark-region window, planned.
#
# `region_dark_after` is the one number that arms the centre's derivation of
# a dark region: every cell of a region silent past it, and nothing new is
# granted into the region, its share is frozen and every mirror into it
# suspends. The variable's validation refuses zero and a negative — a region
# silent for no time at all is every region between two reports — and the
# catalogue renders the value into `QIP_REGION_DARK_AFTER` on the API alone,
# because the API is the one workload that ingests cell reports. The Rust
# acceptance suite reads both facts as text; this file plans them, for the
# reason `paper-boundary.tftest.hcl` gives: a validation that read correctly
# once killed every plan by handing a null to a function, and only a plan
# could have shown it.
#
# Both halves. A harness that only refused would pass on a validation that
# refuses every window, and a deployment that could never arm the control
# would look, from the harness, like one that always refuses bad values.
#
# Mocked providers, plan only, three modules overridden with fixed outputs —
# the same shape as the sibling harnesses and for the same reasons: nothing
# here can reach a project, and the three overrides replace `for_each` inputs
# a mock cannot compute. None of the three is on the variable's path.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "dark-window-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

run "an_unset_window_leaves_the_api_without_the_variable_and_the_derivation_off" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/dark-window-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/dark-window-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/dark-window-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/dark-window-plan-harness/harness"
    }
  }

  # The default: every environment's tfvars leaves the variable null. The
  # API must then receive no `QIP_REGION_DARK_AFTER` at all — not an empty
  # string, which the binary would read as unset only by the grace of its
  # trim, and not a rendered "null", which it would refuse at start-up.
  assert {
    condition     = !contains(keys(output.cloud_run_services["api"].environment), "QIP_REGION_DARK_AFTER")
    error_message = "the API's manifest carries QIP_REGION_DARK_AFTER with the root variable null; the arm is not conditional and the binary would be handed a value nobody stated"
  }
}

run "a_stated_window_reaches_the_api_as_whole_seconds_and_reaches_neither_brain" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/dark-window-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/dark-window-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/dark-window-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/dark-window-plan-harness/harness"
    }
  }

  variables {
    # Written as a number on purpose: it is how an operator will write it in
    # a tfvars, and the string-typed variable has to take it as "300" rather
    # than refuse a value the binary would have read.
    region_dark_after = 300
  }

  assert {
    condition     = output.cloud_run_services["api"].environment["QIP_REGION_DARK_AFTER"] == "300"
    error_message = "the window an operator stated is not the one the API's manifest carries"
  }

  # The window is the API's alone. The brains build no mesh and ingest no
  # cell report, so a value there would be a number over an empty map and a
  # banner claiming a control that cannot fire — and `manifest_wiring.rs`
  # would refuse a deployment setting a variable a binary never reads.
  assert {
    condition     = !contains(keys(output.cloud_run_services["fastbrain"].environment), "QIP_REGION_DARK_AFTER")
    error_message = "the fast brain's manifest carries QIP_REGION_DARK_AFTER; only the API ingests cell reports"
  }
  assert {
    condition     = !contains(keys(output.cloud_run_services["deepbrain"].environment), "QIP_REGION_DARK_AFTER")
    error_message = "the deep brain's manifest carries QIP_REGION_DARK_AFTER; only the API ingests cell reports"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per shape of bad value. Zero and a negative are the two the ADR
# names; a fraction is the one an operator reaching for "half a minute"
# would write, and the binary would refuse it at start-up — better refused
# at plan, where the message names the shape wanted.

run "a_zero_window_stops_the_plan" {
  command = plan

  variables {
    region_dark_after = 0
  }

  expect_failures = [var.region_dark_after]
}

run "a_negative_window_stops_the_plan" {
  command = plan

  variables {
    region_dark_after = -300
  }

  expect_failures = [var.region_dark_after]
}

run "a_fractional_window_stops_the_plan" {
  command = plan

  variables {
    region_dark_after = "1.5"
  }

  expect_failures = [var.region_dark_after]
}
