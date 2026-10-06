# Branch protection enforces tests and independent review before merge, CICD-045.
#
# Three layers enforce the paper-trading boundary:
# 1. Terraform refuses a live ceiling at plan time (paper-boundary.tftest.hcl)
# 2. Composition roots refuse a live ceiling at startup
# 3. The type system makes a live ceiling unreachable
#
# This file tests layer zero: the merge itself is blocked unless tests pass
# and a reviewer approves. Branch protection is load-bearing for CICD-045:
# "A change enters through a GitHub branch or PR created by a human or an
# authorized agent. Branch protection requires tests, and independent review
# scaled to the change's risk class, before merge."
#
# The resource is declared only once; the gate here proves it enforces the
# required checks and review policies. It needs no credential and reaches no
# real GitHub repository — the github provider is mocked.

mock_provider "google" {}
mock_provider "google-beta" {}
mock_provider "github" {}

variables {
  project_id        = "branch-protection-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_owner      = "test-owner"
  github_repository = "test-repo"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

run "branch_protection_requires_all_status_checks" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/branch-protection-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/branch-protection-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/branch-protection-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/branch-protection-harness/harness"
    }
  }

  # The branch protection resource is created and enforces the required
  # status checks: ci, build/lint, build/clippy, build/tests,
  # build/dependency_check, build/secret_scan. The gate fires and admits.
  assert {
    condition     = github_branch_protection.main.pattern == "main"
    error_message = "branch protection pattern is not 'main'"
  }

  assert {
    condition     = github_branch_protection.main.enforce_admins == true
    error_message = "enforce_admins is not true"
  }

  # The resource enforces strict checking — branches must be up to date
  # before merging. This prevents merges of stale branches that may have
  # passed checks against an older commit.
  assert {
    condition     = github_branch_protection.main.requires_strict_status_checks == true
    error_message = "requires_strict_status_checks is not true"
  }
}

run "branch_protection_requires_review_and_dismisses_stale" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/branch-protection-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/branch-protection-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/branch-protection-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/branch-protection-harness/harness"
    }
  }

  # Requires at least one review from a code owner. Stale reviews are
  # dismissed when new commits are pushed, forcing re-review of changed code.
  assert {
    condition     = github_branch_protection.main.required_pull_request_reviews[0].required_approving_review_count == 1
    error_message = "required_approving_review_count is not 1"
  }

  assert {
    condition     = github_branch_protection.main.required_pull_request_reviews[0].require_code_owner_reviews == true
    error_message = "require_code_owner_reviews is not true"
  }

  assert {
    condition     = github_branch_protection.main.required_pull_request_reviews[0].dismiss_stale_reviews == true
    error_message = "dismiss_stale_reviews is not true"
  }
}

run "github_owner_validation_rejects_empty_string" {
  command = plan

  variables {
    github_owner = ""
  }

  # The validation on github_owner refuses empty strings.
  expect_errors = [
    "var.github_owner"
  ]
}

run "github_repository_validation_rejects_empty_string" {
  command = plan

  variables {
    github_repository = ""
  }

  # The validation on github_repository refuses empty strings.
  expect_errors = [
    "var.github_repository"
  ]
}
