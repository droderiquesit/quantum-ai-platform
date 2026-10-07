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
mock_provider "google-beta" {}

# Every repository and CI binding in `modules/registry` is a single resource
# with neither `count` nor `for_each`. This harness addressed them as counted
# — `images[0]`, `length(images) == 1` — which Terraform refuses for a
# single instance ("Unexpected resource instance key"), and `length()` of a
# single object counts its attributes, not its instances. The references
# below name each resource as it is declared; "is declared" is a non-null
# planned object, and a resource removed from the module fails the run at
# the reference itself.

variables {
  project_id            = "registry-plan-harness"
  project_number        = "123456789"
  region                = "us-east4"
  environment           = "dev"
  labels                = { "environment" = "dev" }
  ci_service_account    = "ci@project.iam.gserviceaccount.com"
  pull_service_accounts = {}
  key_ring_id           = "projects/registry-plan-harness/locations/us-east4/keyRings/qip-dev"
}

run "container_registry_is_configured_with_docker_format_and_immutable_tags" {
  command = plan

  assert {
    condition     = google_artifact_registry_repository.images != null
    error_message = "container registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.images.format == "DOCKER"
    error_message = "container registry must use DOCKER format"
  }

  assert {
    condition     = google_artifact_registry_repository.images.docker_config[0].immutable_tags == true
    error_message = "container registry must have immutable tags enabled"
  }

  assert {
    condition     = google_artifact_registry_repository.images.cleanup_policy_dry_run == true
    error_message = "container registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.ci_push != null
    error_message = "CI must have push access to container registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.ci_push.role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role (push but not delete)"
  }
}

run "rust_package_registry_is_configured_for_cargo_crates" {
  command = plan

  assert {
    condition     = google_artifact_registry_repository.rust_packages != null
    error_message = "rust package registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.rust_packages.format == "GENERIC"
    error_message = "rust package registry must use GENERIC format"
  }

  assert {
    condition     = google_artifact_registry_repository.rust_packages.cleanup_policy_dry_run == true
    error_message = "rust package registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.rust_packages_ci_push != null
    error_message = "CI must have push access to rust package registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.rust_packages_ci_push.role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role on rust registry (push but not delete)"
  }
}

run "deployment_bundle_registry_is_configured_for_versioned_artifacts" {
  command = plan

  assert {
    condition     = google_artifact_registry_repository.deployment_bundles != null
    error_message = "deployment bundle registry not declared"
  }

  assert {
    condition     = google_artifact_registry_repository.deployment_bundles.format == "GENERIC"
    error_message = "deployment bundle registry must use GENERIC format"
  }

  assert {
    condition     = google_artifact_registry_repository.deployment_bundles.cleanup_policy_dry_run == true
    error_message = "deployment bundle registry cleanup policy must be in dry-run mode"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.deployment_bundles_ci_push != null
    error_message = "CI must have push access to deployment bundle registry"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.deployment_bundles_ci_push.role == "roles/artifactregistry.writer"
    error_message = "CI must have writer role on bundles registry (push but not delete)"
  }
}

run "all_repositories_inherit_environment_labels" {
  command = plan

  assert {
    condition     = contains(keys(google_artifact_registry_repository.images.labels), "environment")
    error_message = "container registry must inherit environment labels"
  }

  assert {
    condition     = contains(keys(google_artifact_registry_repository.rust_packages.labels), "environment")
    error_message = "rust package registry must inherit environment labels"
  }

  assert {
    condition     = contains(keys(google_artifact_registry_repository.deployment_bundles.labels), "environment")
    error_message = "deployment bundle registry must inherit environment labels"
  }
}

# --- the key the GENERIC repositories are encrypted with (SEC-046) ----------
#
# Whether each repository names the key is asserted on the configuration, by
# `qip-acceptance`'s `security_controls` suite: `kms_key_name` is the key's
# id, unknown until apply, and a plan run comparing it would be refused.
# What a plan can see is the gate on the ring and where the key lands.

# The admitting half: a ring in the repositories' own region is accepted, and
# the key is planned in that ring at the root's protection level.
run "a_ring_in_the_region_is_admitted_and_the_key_is_planned_in_it" {
  command = plan

  assert {
    condition     = google_kms_crypto_key.registry.key_ring == var.key_ring_id
    error_message = "the registry key is not planned in the ring the module was given"
  }

  assert {
    condition     = google_kms_crypto_key.registry.version_template[0].protection_level == "SOFTWARE"
    error_message = "the registry key does not carry the protection level the module was given"
  }
}

# The refusing half. Artifact Registry encrypts a repository only with a key
# in the repository's location and refuses any other at apply; the module
# refuses it at plan instead.
run "a_ring_in_another_region_is_refused" {
  command = plan

  variables {
    key_ring_id = "projects/registry-plan-harness/locations/us-central1/keyRings/qip-dev"
  }

  expect_failures = [var.key_ring_id]
}
