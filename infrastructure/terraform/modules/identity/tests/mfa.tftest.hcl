# W4.6: Identity Platform MFA is mandatory, not optional.
#
# Mocked provider, `command = plan`: no credential, no project.

mock_provider "google" {}

variables {
  enabled            = true
  project_id         = "identity-plan-harness"
  authorized_domains = ["localhost"]
}

# The admitting half: by default, MANDATORY with a TOTP factor.
run "mfa_is_mandatory_by_default_with_a_factor_that_is_not_sms" {
  command = plan

  assert {
    condition     = google_identity_platform_config.algorik[0].mfa[0].state == "MANDATORY"
    error_message = "MFA is not mandatory by default."
  }

  assert {
    condition     = google_identity_platform_config.algorik[0].mfa[0].provider_configs[0].state == "ENABLED"
    error_message = "No TOTP factor is enabled; SMS alone is reachable by a SIM swap."
  }
}

# The refusing halves: optional and off are both refused at plan.
run "optional_mfa_is_refused" {
  command = plan

  variables {
    mfa_state = "ENABLED"
  }

  expect_failures = [var.mfa_state]
}

run "mfa_off_is_refused" {
  command = plan

  variables {
    mfa_state = "OFF"
  }

  expect_failures = [var.mfa_state]
}
