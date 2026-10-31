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
  owner             = "platform"
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
      && length(google_privateca_certificate_template.workload) == 0
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

  # ADR 0112 §2: hours, not months — a day by default, at the pool and at
  # the template alike.
  assert {
    condition = (
      google_privateca_ca_pool.internal[0].issuance_policy[0].maximum_lifetime == "86400s"
      && google_privateca_certificate_template.workload[0].maximum_lifetime == "86400s"
    )
    error_message = "The pool or the workload template issues leaves for longer than a day."
  }

  # The template: no subject from the requester, exactly one service-account
  # shaped DNS SAN, never a CA.
  assert {
    condition = (
      google_privateca_certificate_template.workload[0].identity_constraints[0].allow_subject_passthrough == false
      && google_privateca_certificate_template.workload[0].identity_constraints[0].allow_subject_alt_names_passthrough == true
      && google_privateca_certificate_template.workload[0].identity_constraints[0].cel_expression[0].expression == "subject_alt_names.size() == 1 && subject_alt_names.all(san, san.type == DNS && san.value.endsWith('.iam.gserviceaccount.com'))"
      && google_privateca_certificate_template.workload[0].predefined_values[0].ca_options[0].is_ca == false
    )
    error_message = "The workload template does not confine a certificate to one SAN shaped as a service account, or may issue a CA."
  }

  # No machine is declared here, so nobody may request a certificate at all.
  assert {
    condition     = length(google_privateca_ca_pool_iam_member.requesters) == 0
    error_message = "A certificate requester was granted with no broker, ledger or node declared."
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

# Requesters are derived, never listed: a listed one would be an
# unconditioned grant able to request any name.
run "a_listed_certificate_requester_is_refused" {
  command = plan

  variables {
    certificate_authority = {
      location     = "us-east4"
      organization = "Algorik"
      requesters   = ["serviceAccount:qip-dev-fabricd@security-plan-harness.iam.gserviceaccount.com"]
    }
  }

  expect_failures = [var.certificate_authority]
}

run "a_workload_certificate_living_longer_than_a_day_is_refused" {
  command = plan

  variables {
    certificate_authority = {
      location                       = "us-east4"
      organization                   = "Algorik"
      max_certificate_lifetime_hours = 720
    }
  }

  expect_failures = [var.certificate_authority]
}

run "a_workload_certificate_lifetime_of_hours_is_admitted" {
  command = plan

  variables {
    certificate_authority = {
      location                       = "us-east4"
      organization                   = "Algorik"
      max_certificate_lifetime_hours = 8
    }
  }

  assert {
    condition     = google_privateca_ca_pool.internal[0].issuance_policy[0].maximum_lifetime == "28800s" && google_privateca_certificate_template.workload[0].maximum_lifetime == "28800s"
    error_message = "An eight-hour lifetime did not reach the pool and the template."
  }
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

# R12-GOV-10005: the asset change feed plans both content types, each
# publishing to the change topic. The feed lived in audit-logs.tf unasserted;
# a content type dropped from its map would stop half the changes being
# published and no plan would have said so.
run "the_asset_change_feed_plans_resource_and_iam_policy_changes_to_the_change_topic" {
  command = plan

  assert {
    condition     = toset([for feed in values(google_cloud_asset_project_feed.changes) : feed.content_type]) == toset(["RESOURCE", "IAM_POLICY"])
    error_message = "the asset change feed must plan exactly a RESOURCE and an IAM_POLICY feed"
  }

  # The topic's id is computed and unknown at plan, so the feed-to-topic
  # link is held on the configuration by qip-acceptance's
  # infrastructure::every_asset_change_feed_publishes_to_the_change_topic.
  assert {
    condition     = google_pubsub_topic.asset_changes.name == "qip-dev-asset-changes"
    error_message = "the asset change topic must plan under its environment's name"
  }
}
