# Evidence bucket retention and lock enforcement.
#
# The evidence bucket must be append-only to the people who run the platform.
# Four controls enforce it: retention policy (locked), versioning, uniform
# bucket-level access, and narrow IAM bindings. This harness tests the lock.
#
# Mocked provider, plan only: no credential, no project, nothing created.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id           = "evidence-plan-harness"
  environment          = "dev"
  region               = "us-east4"
  labels               = {}
  key_ring_id          = "projects/evidence-plan/locations/us/keyRings/qip-dev"
  kms_protection_level = "SOFTWARE"
}

# --- Default values: retention locked, minimum 2557 days -----------------------

run "default_retention_locked_and_at_minimum_2557_days" {
  command = plan

  # No retention variables set: defaults apply.
  variables {
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition = (
      google_storage_bucket.evidence.retention_policy[0].is_locked == true &&
      google_storage_bucket.evidence.retention_policy[0].retention_period == 2557 * 24 * 60 * 60
    )
    error_message = "default must be retention locked and >= 2557 days (2557*86400 seconds)"
  }
}

# --- Explicit lock and standard retention period ------

run "explicit_lock_true_standard_retention" {
  command = plan

  variables {
    retention_days          = 2557
    retention_locked        = true
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition = (
      google_storage_bucket.evidence.retention_policy[0].is_locked == true &&
      google_storage_bucket.evidence.retention_policy[0].retention_period == 2557 * 24 * 60 * 60
    )
    error_message = "explicit true lock and 2557 days must apply to the bucket"
  }
}

# --- Longer retention period (10 years) still locked ------

run "longer_retention_period_remains_locked" {
  command = plan

  variables {
    retention_days          = 3650 # ~10 years
    retention_locked        = true
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition = (
      google_storage_bucket.evidence.retention_policy[0].is_locked == true &&
      google_storage_bucket.evidence.retention_policy[0].retention_period == 3650 * 24 * 60 * 60
    )
    error_message = "longer retention period (10 years) must remain locked"
  }
}

# --- Mutation: setting retention_locked=false is refused at plan ----------
#
# This run was named `retention_locked_false_plans_but_loses_structural_protection`
# and asserted the plan admitted `false` with an unlocked policy. It was
# written (02c434be) against a module that already refused `false`: GOV-023
# (03a3b7db), earlier the same day, added the validation on
# `retention_locked`, and `immutable-retention.tftest.hcl` proves it fires.
# An evidence store that can be planned unlocked is append-only by
# convention, which is what GOV-023 exists to stop, so the run keeps its
# inputs and now proves the refusal rather than the opposite of it.

run "retention_locked_false_is_refused_and_never_plans_unlocked" {
  command = plan

  variables {
    retention_days          = 2557
    retention_locked        = false # Mutation: structural protection removed
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  expect_failures = [var.retention_locked]
}

# --- Versioning is enabled by default ----------

run "versioning_enabled" {
  command = plan

  variables {
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition     = google_storage_bucket.evidence.versioning[0].enabled == true
    error_message = "versioning must be enabled so overwrites leave originals readable"
  }
}

# --- Uniform bucket-level access is enforced ----------

run "uniform_bucket_level_access_enforced" {
  command = plan

  variables {
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  # A boolean argument on `google_storage_bucket`, not a block: the `[0].enabled`
  # this read is the shape of the older `bucket_policy_only` block and has no
  # index to take.
  assert {
    condition     = google_storage_bucket.evidence.uniform_bucket_level_access == true
    error_message = "uniform bucket-level access must be enabled"
  }
}

# --- Public access is prevented ----------

run "public_access_prevented" {
  command = plan

  variables {
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition     = google_storage_bucket.evidence.public_access_prevention == "enforced"
    error_message = "public access prevention must be enforced"
  }
}

# --- Retention policy must be >= 1 day ----------

run "retention_days_minimum_one" {
  command = plan

  variables {
    retention_days          = 1
    retention_locked        = true
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition = (
      google_storage_bucket.evidence.retention_policy[0].retention_period == 1 * 24 * 60 * 60
    )
    error_message = "retention period must accept minimum 1 day"
  }
}

# --- Retention days zero is refused ----------

run "retention_days_zero_is_refused" {
  command = plan

  variables {
    retention_days          = 0
    retention_locked        = true
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  expect_failures = [var.retention_days]
}

# --- KMS protection level validation ----------

run "kms_protection_level_software_is_accepted" {
  command = plan

  variables {
    kms_protection_level    = "SOFTWARE"
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  # Plan succeeds if SOFTWARE is valid; KMS key resource will be created
  assert {
    condition     = google_kms_crypto_key.evidence != null
    error_message = "kms_protection_level SOFTWARE must be accepted"
  }
}

# --- KMS protection level HSM is valid ----------

run "kms_protection_level_hsm_is_accepted" {
  command = plan

  variables {
    kms_protection_level    = "HSM"
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  assert {
    condition     = google_kms_crypto_key.evidence != null
    error_message = "kms_protection_level HSM must be accepted"
  }
}

# --- KMS protection level invalid value is refused ----------

run "kms_protection_level_invalid_is_refused" {
  command = plan

  variables {
    kms_protection_level    = "EXTERNAL"
    writer_service_accounts = {}
    reader_service_accounts = {}
  }

  expect_failures = [var.kms_protection_level]
}
