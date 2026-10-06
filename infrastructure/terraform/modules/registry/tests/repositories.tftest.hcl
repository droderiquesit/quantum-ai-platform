# Container, Rust package, and deployment-bundle repositories.
#
# The registry module declares three artifact repositories:
#   * images: DOCKER format for container images, immutable tags enabled
#   * rust_packages: GENERIC format for Cargo-published crates
#   * deployment_bundles: GENERIC format for versioned deployment artifacts
#
# Each is configured with cleanup policies in dry-run mode and IAM bindings
# that allow CI to push but not delete, matching the container registry's
# discipline.
#
# Runs against a mocked provider, so it needs no credential and reaches no
# project; `command = plan` throughout, so nothing is applied even in the mock.

mock_provider "google" {}

variables {
  project_id             = "registry-plan-harness"
  project_number         = "123456789"
  region                 = "us-east4"
  environment            = "dev"
  labels                 = { "environment" = "dev" }
  ci_service_account     = "ci@project.iam.gserviceaccount.com"
  pull_service_accounts  = {}
}

run "container_registry_is_configured_with_docker_format_and_immutable_tags" {
  command = plan

  assert {
    condition     = length(google_artifact_registry_repository.images) == 1
    error_message = "container registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.images[0].format == "DOCKER"
    error_message = "container registry must use DOCKER format"
  }

  assert {
    condition     = google_artifact_registry_repository.images[0].docker_config[0].immutable_tags == true
    error_message = "container registry must have immutable tags enabled"
  }

  assert {
    condition     = google_artifact_registry_repository.images[0].cleanup_policy_dry_run == true
    error_message = "container registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = length(google_artifact_registry_repository_iam_member.ci_push) == 1
    error_message = "CI must have push access to container registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.ci_push[0].role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role (push but not delete)"
  }
}

run "rust_package_registry_is_configured_for_cargo_crates" {
  command = plan

  assert {
    condition     = length(google_artifact_registry_repository.rust_packages) == 1
    error_message = "rust package registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.rust_packages[0].format == "GENERIC"
    error_message = "rust package registry must use GENERIC format"
  }

  assert {
    condition     = google_artifact_registry_repository.rust_packages[0].cleanup_policy_dry_run == true
    error_message = "rust package registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = length(google_artifact_registry_repository_iam_member.rust_packages_ci_push) == 1
    error_message = "CI must have push access to rust package registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.rust_packages_ci_push[0].role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role on rust registry (push but not delete)"
  }
}

run "deployment_bundle_registry_is_configured_for_versioned_artifacts" {
  command = plan

  assert {
    condition     = length(google_artifact_registry_repository.deployment_bundles) == 1
    error_message = "deployment bundle registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.deployment_bundles[0].format == "GENERIC"
    error_message = "deployment bundle registry must use GENERIC format"
  }

  assert {
    condition     = google_artifact_registry_repository.deployment_bundles[0].cleanup_policy_dry_run == true
    error_message = "deployment bundle registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = length(google_artifact_registry_repository_iam_member.deployment_bundles_ci_push) == 1
    error_message = "CI must have push access to deployment bundle registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.deployment_bundles_ci_push[0].role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role on bundles registry (push but not delete)"
  }
}

run "all_repositories_inherit_environment_labels" {
  command = plan

  assert {
    condition     = contains(keys(google_artifact_registry_repository.images[0].labels), "environment")
    error_message = "container registry must inherit environment labels"
  }

  assert {
    condition     = contains(keys(google_artifact_registry_repository.rust_packages[0].labels), "environment")
    error_message = "rust package registry must inherit environment labels"
  }

  assert {
    condition     = contains(keys(google_artifact_registry_repository.deployment_bundles[0].labels), "environment")
    error_message = "deployment bundle registry must inherit environment labels"
  }
}
