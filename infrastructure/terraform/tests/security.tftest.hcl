# W4.6: §15's security services (R12 SEC-9123, 9128, 9129, 9134) and
# mandatory MFA, planned.
#
# Every control is off unless an environment names it; each admits a sound
# configuration and refuses the hollow one. Mocked providers and
# `command = plan`: no credential, no project, nothing created. The module
# overrides are the ones every root harness here carries.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/security-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/security-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/security-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/security-plan-harness/harness"
  }
}

variables {
  project_id        = "security-plan-harness"
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

run "every_security_service_is_off_unless_named" {
  command = plan

  assert {
    condition = (
      length(google_privileged_access_manager_entitlement.this) == 0
      && length(google_privateca_ca_pool.internal) == 0
      && length(google_access_context_manager_service_perimeter.data) == 0
      && length(google_data_loss_prevention_discovery_config.lake) == 0
      && length(google_project_service.security) == 0
    )
    error_message = "A security service, or its API, was planned with none named."
  }
}

# --- the admitting halves ----------------------------------------------------

run "every_security_service_plans_when_named_soundly" {
  command = plan

  variables {
    pam_entitlements = {
      break-glass = {
        roles      = ["roles/run.admin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
    certificate_authority = {
      location     = "us-east4"
      organization = "Algorik"
      requesters   = ["serviceAccount:qip-dev-fabricd@security-plan-harness.iam.gserviceaccount.com"]
    }
    vpc_service_controls = {
      access_policy      = "123456"
      ingress_identities = ["serviceAccount:qip-ci-dev@security-plan-harness.iam.gserviceaccount.com"]
    }
    sensitive_data_protection = {}
  }

  assert {
    condition     = length(google_privileged_access_manager_entitlement.this) == 1
    error_message = "A named entitlement was not planned."
  }

  assert {
    condition     = google_privileged_access_manager_entitlement.this["break-glass"].approval_workflow[0].manual_approvals[0].steps[0].approvals_needed == 1
    error_message = "The entitlement does not require an approval."
  }

  assert {
    condition     = google_privateca_ca_pool.internal[0].issuance_policy[0].baseline_values[0].ca_options[0].is_ca == false
    error_message = "The pool may issue a CA certificate; a leaked workload key would then mint identities."
  }

  assert {
    condition     = google_privateca_ca_pool.internal[0].issuance_policy[0].maximum_lifetime == "2592000s"
    error_message = "The pool issues leaves for longer than thirty days."
  }

  assert {
    condition     = google_access_context_manager_service_perimeter.data[0].use_explicit_dry_run_spec == false && length(google_access_context_manager_service_perimeter.data[0].status) == 1
    error_message = "The perimeter is not enforced by default; a dry-run perimeter blocks nothing."
  }

  assert {
    condition     = contains(google_access_context_manager_service_perimeter.data[0].status[0].restricted_services, "spanner.googleapis.com")
    error_message = "The perimeter does not restrict the ledger's database."
  }

  assert {
    condition     = google_data_loss_prevention_discovery_config.lake[0].status == "RUNNING"
    error_message = "Discovery is planned and not running."
  }

  assert {
    condition     = length(google_project_service.security) == 4
    error_message = "The four APIs were not enabled with their four controls."
  }

  # And MFA, which W4.6 makes mandatory, is mandatory by default.
  assert {
    condition     = var.identity_mfa_state == "MANDATORY"
    error_message = "MFA is not mandatory by default."
  }
}

# --- the refusing halves -----------------------------------------------------

run "an_entitlement_nobody_approves_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      unapproved = {
        roles      = ["roles/run.admin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = []
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "self_approval_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      self = {
        roles      = ["roles/run.admin"]
        requesters = ["user:operator@example.com"]
        approvers  = ["user:operator@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "an_elevation_longer_than_eight_hours_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      long = {
        roles                = ["roles/run.admin"]
        requesters           = ["group:qip-operators@example.com"]
        approvers            = ["group:qip-approvers@example.com"]
        max_request_duration = "86400s"
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_basic_role_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      owner = {
        roles      = ["roles/owner"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_perimeter_restricting_nothing_is_refused" {
  command = plan

  variables {
    vpc_service_controls = {
      access_policy       = "123456"
      restricted_services = []
    }
  }

  expect_failures = [var.vpc_service_controls]
}

run "perimeter_ingress_for_a_person_is_refused" {
  command = plan

  variables {
    vpc_service_controls = {
      access_policy      = "123456"
      ingress_identities = ["user:someone@example.com"]
    }
  }

  expect_failures = [var.vpc_service_controls]
}

run "a_person_requesting_workload_certificates_is_refused" {
  command = plan

  variables {
    certificate_authority = {
      location     = "us-east4"
      organization = "Algorik"
      requesters   = ["user:someone@example.com"]
    }
  }

  expect_failures = [var.certificate_authority]
}

run "a_discovery_scan_for_nothing_is_refused" {
  command = plan

  variables {
    sensitive_data_protection = {
      info_types = []
    }
  }

  expect_failures = [var.sensitive_data_protection]
}

run "optional_mfa_is_refused_at_the_root" {
  command = plan

  variables {
    identity_mfa_state = "ENABLED"
  }

  expect_failures = [var.identity_mfa_state]
}
