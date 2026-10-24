# R12-GCP-8114: no Privileged Access Manager entitlement can hand a workload
# account, or anybody, a road to the venue credential.
#
# `pam_entitlements` admitted a `serviceAccount:` requester for any non-basic
# role until 2026-10-24, so a Cloud Run workload's own account could be made
# eligible for `roles/secretmanager.secretAccessor` at project scope behind
# one approval click, and no grant scanner saw it — an entitlement is not a
# binding until somebody asks. Each refusing run below names one role or one
# principal, never two, so each proves its own element of the rule:
# `expect_failures` is satisfied by any validation on the variable failing,
# and a run carrying two bad values would pass with either refusal deleted.
# That is the paper-boundary harness's discipline, for the reason it gives.
#
# The admitting runs plan to the end and carry the near misses — the viewer
# roles that share a prefix with a refused one, a `user:` and a `group:` on
# both sides — so a rule loosened into a prefix or a substring match refuses
# a good value here and fails.
#
# Mocked providers and `command = plan`: no credential, no project, nothing
# created. The module overrides are the ones every root harness here carries.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/pam-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/pam-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/pam-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/pam-plan-harness/harness"
  }
}

variables {
  project_id        = "pam-plan-harness"
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

# --- the admitting halves ----------------------------------------------------

# The near misses are admitted and reach the entitlement exactly as written:
# `serviceAccountViewer` beside `serviceAccountUser`/`serviceAccountAdmin`,
# `workloadIdentityPoolViewer` beside `workloadIdentityUser`/
# `workloadIdentityPoolAdmin`, `securityReviewer` beside `securityAdmin`, and
# a `.v1` role whose id carries a second dot. A rule matching
# `roles/iam.serviceAccount` or `roles/iam.workloadIdentity` as a prefix, or
# refusing a dotted id as custom, refuses this run.
run "an_entitlement_for_people_naming_service_roles_is_admitted" {
  command = plan

  variables {
    pam_entitlements = {
      investigate = {
        roles = [
          "roles/run.admin",
          "roles/compute.viewer",
          "roles/iam.serviceAccountViewer",
          "roles/iam.workloadIdentityPoolViewer",
          "roles/iam.securityReviewer",
          "roles/logging.viewer",
          "roles/compute.instanceAdmin.v1",
        ]
        requesters = ["group:qip-operators@example.com", "user:operator@example.com"]
        approvers  = ["group:qip-approvers@example.com", "user:approver@example.com"]
      }
    }
  }

  assert {
    condition     = length(google_privileged_access_manager_entitlement.this) == 1
    error_message = "An entitlement for people naming service roles was not planned."
  }

  assert {
    condition = (
      toset([for binding in google_privileged_access_manager_entitlement.this["investigate"].privileged_access[0].gcp_iam_access[0].role_bindings : binding.role])
      == toset([
        "roles/run.admin",
        "roles/compute.viewer",
        "roles/iam.serviceAccountViewer",
        "roles/iam.workloadIdentityPoolViewer",
        "roles/iam.securityReviewer",
        "roles/logging.viewer",
        "roles/compute.instanceAdmin.v1",
      ])
    )
    error_message = "The planned entitlement does not grant exactly the seven admitted roles."
  }

  assert {
    condition = (
      toset(google_privileged_access_manager_entitlement.this["investigate"].eligible_users[0].principals)
      == toset(["group:qip-operators@example.com", "user:operator@example.com"])
      && toset(google_privileged_access_manager_entitlement.this["investigate"].approval_workflow[0].manual_approvals[0].steps[0].approvers[0].principals)
      == toset(["group:qip-approvers@example.com", "user:approver@example.com"])
    )
    error_message = "A user: or group: requester or approver did not reach the entitlement as written."
  }
}

# --- the refusing halves: principals -------------------------------------------

run "a_service_account_requester_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      workload = {
        roles      = ["roles/run.viewer"]
        requesters = ["serviceAccount:qip-dev-api@pam-plan-harness.iam.gserviceaccount.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_service_account_approver_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      automated = {
        roles      = ["roles/run.viewer"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["serviceAccount:qip-ci-dev@pam-plan-harness.iam.gserviceaccount.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

# --- the refusing halves: roles that read a secret ---------------------------

run "a_secret_accessor_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      venue = {
        roles      = ["roles/secretmanager.secretAccessor"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_secret_manager_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      secrets = {
        roles      = ["roles/secretmanager.admin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

# A custom role's permissions are whatever was last written into it; one
# holding `secretmanager.versions.access` would pass every name rule.
run "a_custom_role_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      custom = {
        roles      = ["projects/pam-plan-harness/roles/venueReader"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

# --- the refusing halves: impersonation and IAM administration, one each -----

run "a_token_creator_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.serviceAccountTokenCreator"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "an_open_id_token_creator_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.serviceAccountOpenIdTokenCreator"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_service_account_user_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.serviceAccountUser"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_service_account_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.serviceAccountAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_service_account_key_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.serviceAccountKeyAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_workload_identity_user_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.workloadIdentityUser"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_workload_identity_pool_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.workloadIdentityPoolAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_security_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      rebind = {
        roles      = ["roles/iam.securityAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_role_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      rebind = {
        roles      = ["roles/iam.roleAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_project_iam_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      rebind = {
        roles      = ["roles/resourcemanager.projectIamAdmin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

run "a_privileged_access_manager_admin_entitlement_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      rebind = {
        roles      = ["roles/privilegedaccessmanager.admin"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}

# The id compared case-folded: IAM would refuse this spelling at apply, and a
# plan that admitted it would be a refusal the reviewed plan never showed.
run "a_refused_role_spelt_in_another_case_is_refused" {
  command = plan

  variables {
    pam_entitlements = {
      act = {
        roles      = ["roles/iam.ServiceAccountTokenCreator"]
        requesters = ["group:qip-operators@example.com"]
        approvers  = ["group:qip-approvers@example.com"]
      }
    }
  }

  expect_failures = [var.pam_entitlements]
}
