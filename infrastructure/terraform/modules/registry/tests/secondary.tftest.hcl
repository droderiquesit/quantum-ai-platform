# The secondary image repository (R12-RES-9177), planned against mocks: no
# credential, no project, `command = plan` throughout.

mock_provider "google" {}
mock_provider "google-beta" {}

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

run "no_secondary_repository_is_declared_by_default" {
  command = plan

  assert {
    condition     = length(google_artifact_registry_repository.images_secondary) == 0
    error_message = "a secondary repository was declared with secondary_region null"
  }

  assert {
    condition     = length(google_kms_key_ring.registry_secondary) == 0 && length(google_kms_crypto_key.registry_secondary) == 0
    error_message = "a secondary key ring or key was declared with secondary_region null; a key ring can never be deleted, so one declared for nothing is permanent"
  }
}

run "the_secondary_repository_is_encrypted_under_its_own_regions_key_and_the_agent_holds_it" {
  command = plan

  variables {
    secondary_region = "us-west1"
  }

  assert {
    condition     = google_kms_key_ring.registry_secondary[0].location == "us-west1"
    error_message = "the secondary key ring is not in the secondary region; Artifact Registry refuses a key from another location, and only at apply"
  }

  assert {
    condition     = google_kms_crypto_key.registry_secondary[0].rotation_period == "7776000s" && google_kms_crypto_key.registry_secondary[0].version_template[0].protection_level == "SOFTWARE"
    error_message = "the secondary key does not rotate every ninety days at the root's protection level, as every other key does"
  }

  assert {
    condition     = google_kms_crypto_key_iam_member.artifact_registry_agent_secondary[0].role == "roles/cloudkms.cryptoKeyEncrypterDecrypter" && google_kms_crypto_key_iam_member.artifact_registry_agent_secondary[0].member == "serviceAccount:service-123456789@gcp-sa-artifactregistry.iam.gserviceaccount.com"
    error_message = "the Artifact Registry service agent does not hold the secondary key, so the repository create fails at apply"
  }
}

run "a_secondary_region_declares_an_immutable_repository_ci_can_push_to_and_not_delete_from" {
  command = plan

  variables {
    secondary_region = "us-west1"
  }

  assert {
    condition     = google_artifact_registry_repository.images_secondary[0].location == "us-west1"
    error_message = "the secondary repository is not in the secondary region"
  }

  assert {
    condition     = google_artifact_registry_repository.images_secondary[0].format == "DOCKER" && google_artifact_registry_repository.images_secondary[0].docker_config[0].immutable_tags == true
    error_message = "the secondary repository's tags are mutable"
  }

  assert {
    condition     = google_artifact_registry_repository.images_secondary[0].cleanup_policy_dry_run == true
    error_message = "the secondary repository's cleanup may delete"
  }

  assert {
    condition     = google_artifact_registry_repository_iam_member.ci_push_secondary[0].role == "roles/artifactregistry.writer"
    error_message = "CI holds a role on the secondary repository other than writer"
  }
}

run "a_secondary_repository_in_the_primarys_region_is_refused" {
  command = plan

  variables {
    secondary_region = "us-east4"
  }

  expect_failures = [var.secondary_region]
}
