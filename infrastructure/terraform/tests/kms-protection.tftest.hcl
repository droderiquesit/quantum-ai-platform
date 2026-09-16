# Cloud HSM, planned: the protection level both refuses a bad value and admits
# a good one.
#
# Blueprint §45.1 lists Cloud HSM beside Secret Manager and KMS. There is no
# `google_cloud_hsm` resource in any provider — Cloud HSM *is* a KMS key whose
# version template says `protection_level = "HSM"` — so the row is declared by
# making that level a choice rather than a literal, and the gate that choice
# needs is the subject of this file.
#
# ADR 0069 requires a harness to prove both halves, because a gate proven only
# to refuse may refuse everything: `modules/network`'s prefix check read
# correctly, had a Rust test asserting it "both fires and admits", and killed
# the plan outright in three of four environments. So there are admitting runs
# here as well as refusing ones, and the admitting ones assert on an output
# read off the planned key rather than on the variable.
#
# That distinction is the point of the two admitting runs. An assertion like
# `var.kms_protection_level == "HSM"` agrees with itself whatever the key was
# built with. `output.kms_protection_level` comes from
# `google_kms_crypto_key.secrets.version_template[0].protection_level`, so it
# is false the moment the variable stops reaching the resource — which is the
# failure a reviewer cannot see by reading, since a hardcoded `"SOFTWARE"` and
# a threaded `var.kms_protection_level` look equally correct in a diff.
#
# What this file does not prove, stated because a mocked plan invites being
# read as more than it is:
#
#   - Nothing about Google. Every provider here is mocked; this says the
#     configuration is coherent and its refusal fires on the values given, and
#     it has never spoken to Cloud KMS. Whether HSM is offered in a given
#     location is Google's answer to give and no plan here asks for it.
#   - Nothing about an environment already applied. Raising the level on one
#     does not upgrade a key: `version_template` is immutable, Terraform plans
#     a replacement, and `prevent_destroy` stops the apply. A variable
#     validation is handed the value and never the prior state, so that is
#     documented on the root variable and deliberately not gated — a check
#     that could never fire is the shape this repository refuses to add.
#
# It needs no credential and reaches no project. `mock_provider` replaces both
# providers and every run is `command = plan`.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "kms-protection-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  # The four zones dev declares. The catalogue refuses a workload whose zone
  # this environment did not declare, so a harness with no zones would fail on
  # that precondition long before it reached a key.
  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

run "the_default_software_level_plans_to_the_end_and_reaches_the_key" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/kms-protection-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/kms-protection-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/kms-protection-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/kms-protection-harness/harness"
    }
  }

  # Not set at all, so this is the default arriving at the key. The default is
  # the level every environment holds today, and a harness that only ever
  # named the value explicitly would not notice the default being changed.
  assert {
    condition     = output.kms_protection_level == "SOFTWARE"
    error_message = "the default protection level is not SOFTWARE at the key, so either the default moved or the variable stopped reaching the resource"
  }
}

run "an_hsm_level_plans_to_the_end_and_reaches_the_key" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/kms-protection-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/kms-protection-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/kms-protection-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/kms-protection-harness/harness"
    }
  }

  variables {
    kms_protection_level = "HSM"
  }

  # The half that matters, and the reason the row can be called declared at
  # all: a configuration in which HSM is spelled but refused is a row nobody
  # can turn on, which is worse than an absent one because it reads as
  # available.
  assert {
    condition     = output.kms_protection_level == "HSM"
    error_message = "HSM was admitted by the validation but did not reach the key, so the platform would plan a software key while reporting HSM"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per mistake rather than one naming the worst, for the reason the
# ceiling harness gives: a single case can pass while its neighbours are
# admitted.

run "a_lower_case_hsm_stops_the_plan" {
  command = plan

  variables {
    # Cloud KMS's enum is upper case and the provider does not normalise. This
    # is the mistake somebody actually makes, and without the gate it fails
    # inside the provider naming a field rather than the value.
    kms_protection_level = "hsm"
  }

  expect_failures = [var.kms_protection_level]
}

run "an_external_key_manager_level_stops_the_plan" {
  command = plan

  variables {
    # A real Cloud KMS protection level, and refused deliberately: EXTERNAL
    # needs a `google_kms_ekm_connection` and a key management partner outside
    # Google, and this configuration declares neither. Admitting it would
    # produce an apply that fails naming a key URI nobody configured.
    kms_protection_level = "EXTERNAL"
  }

  expect_failures = [var.kms_protection_level]
}

run "an_external_vpc_key_manager_level_stops_the_plan" {
  command = plan

  variables {
    # The second EKM level, named separately because `EXTERNAL` is a prefix of
    # nothing here but the pair is easy to half-handle: a check written against
    # only one of them admits the other.
    kms_protection_level = "EXTERNAL_VPC"
  }

  expect_failures = [var.kms_protection_level]
}

run "an_empty_protection_level_stops_the_plan" {
  command = plan

  variables {
    # An unset value arriving as an empty string rather than as the default —
    # a tfvars line somebody cleared. Without the gate it reaches the provider
    # as a blank enum.
    kms_protection_level = ""
  }

  expect_failures = [var.kms_protection_level]
}
