# ADR 0110's artefact-signing keys, planned (R12 SEC-9109, SEC-9137,
# SEC-8100's KMS clause, MODEL-4023).
#
# Every gate here both refuses a bad value and admits a good one, and every
# admitting run asserts on planned resources, not on variables: a
# `var.artefact_signing.protection_level == "HSM"` assertion agrees with
# itself whatever the key was built with.
#
# Mocked providers and `command = plan`: no credential, no project, nothing
# created. What a plan can and cannot see, said so a passing run is not read
# as more than it is: every key-version name and every signer member is
# written from configuration (artefact-signing.tf says why) and is asserted
# here exactly; the public keys are Cloud KMS's answer, unknown to any plan of
# an unapplied environment, so the PEM-to-hex conversion is proven against a
# fixed vector in `modules/spki-hex/tests/` and the public-key precondition is
# not exercised here. `fixtures/harness-bundle.json` carries a throwaway key
# generated for these harnesses, private half discarded; it verifies nothing
# anywhere and is in no image.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/artefact-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/artefact-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/artefact-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/artefact-plan-harness/harness"
  }
}

variables {
  project_id        = "artefact-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  region            = "us-east4"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- off by default ----------------------------------------------------------

run "nothing_is_planned_while_artefact_signing_is_null" {
  command = plan

  assert {
    condition = (
      length(google_kms_key_ring.artefacts) == 0
      && length(google_kms_crypto_key.artefact) == 0
      && length(google_kms_crypto_key_iam_binding.artefact_signer) == 0
      && length(google_service_account.model_pack_publisher) == 0
      && length(google_iam_workload_identity_pool.model_pack_publish) == 0
      && output.artefact_trust_bundle == null
    )
    error_message = "A key, a binding, the publisher or its pool was planned with artefact_signing null; every environment's plan must be unchanged by default."
  }

  # And the API is told nothing about KMS: no signer posture, no bundle path,
  # no key-version file.
  assert {
    condition = (
      !contains(keys(module.cloud_run["api"].environment), "QIP_ARTEFACT_SIGNER")
      && !contains(keys(module.cloud_run["api"].environment), "QIP_TRUST_BUNDLE_PATH")
      && !contains(keys(module.cloud_run["api"].environment), "QIP_KMS_KEY_VERSIONS_PATH")
    )
    error_message = "The API was given a KMS signing posture with no keys and no bundle; it would stop at start-up."
  }
}

# --- the admitting halves ----------------------------------------------------

run "four_hsm_p256_signing_keys_with_disjoint_signers_are_planned" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
    }
  }

  assert {
    condition     = toset(keys(google_kms_crypto_key.artefact)) == toset(["policy", "halt", "envelope", "model-pack"])
    error_message = "The keys planned are not exactly one per ADR 0110 class."
  }

  assert {
    condition = alltrue([
      for key in values(google_kms_crypto_key.artefact) :
      key.purpose == "ASYMMETRIC_SIGN"
      && key.version_template[0].algorithm == "EC_SIGN_P256_SHA256"
      && key.version_template[0].protection_level == "HSM"
    ])
    error_message = "A class key is not an HSM-protected EC_SIGN_P256_SHA256 signing key at the resource."
  }

  assert {
    condition     = alltrue([for key in values(google_kms_crypto_key.artefact) : key.rotation_period == null])
    error_message = "An asymmetric key carries a rotation_period, which Cloud KMS refuses at apply; rotation is a new version."
  }

  # Disjoint signers (R12-SEC-9137): the API's identity on exactly the three
  # classes it dispatches, the publisher on exactly model-pack, one member on
  # each, and neither anywhere the other is.
  assert {
    condition = alltrue([
      for class in ["policy", "halt", "envelope"] :
      google_kms_crypto_key_iam_binding.artefact_signer[class].role == "roles/cloudkms.signer"
      && google_kms_crypto_key_iam_binding.artefact_signer[class].members == toset(["serviceAccount:qip-api-dev@artefact-plan-harness.iam.gserviceaccount.com"])
    ])
    error_message = "policy, halt or envelope is signable by someone other than the API's runtime identity alone."
  }

  assert {
    condition = (
      google_kms_crypto_key_iam_binding.artefact_signer["model-pack"].role == "roles/cloudkms.signer"
      && google_kms_crypto_key_iam_binding.artefact_signer["model-pack"].members == toset(["serviceAccount:qip-dev-pack-publisher@artefact-plan-harness.iam.gserviceaccount.com"])
    )
    error_message = "The model-pack key is signable by someone other than the publisher alone; the API must never sign a model pack."
  }

  # The publisher is reachable from one workflow, one branch, one environment.
  assert {
    condition = (
      strcontains(output.model_pack_publish_identity.attribute_condition, "attribute.repository == 'example/example'")
      && strcontains(output.model_pack_publish_identity.attribute_condition, "attribute.environment == 'model-pack-publish'")
      && strcontains(output.model_pack_publish_identity.attribute_condition, "attribute.ref == 'refs/heads/main'")
      && strcontains(output.model_pack_publish_identity.attribute_condition, "attribute.workflow_ref == 'example/example/.github/workflows/model-pack-publish.yml@refs/heads/main'")
    )
    error_message = "The publish provider's condition does not pin the repository, the protected environment, the branch and the publish workflow."
  }

  assert {
    condition     = output.model_pack_publish_identity.impersonator == "principal://iam.googleapis.com/projects/123456789012/locations/global/workloadIdentityPools/qip-publish-dev/subject/repo:example/example:environment:model-pack-publish"
    error_message = "The publisher is impersonable by something other than the one subject GitHub mints for the publish environment."
  }

  # The rendered bundle: one entry per class, each naming the version its
  # signer uses. (Its public keys are KMS's answer; see the header.)
  assert {
    condition = (
      [for key in output.artefact_trust_bundle.keys : key.class] == ["envelope", "halt", "model-pack", "policy"]
      && alltrue([
        for key in output.artefact_trust_bundle.keys :
        key.key_version == "projects/artefact-plan-harness/locations/us-east4/keyRings/qip-dev-artefacts/cryptoKeys/qip-dev-${key.class}/cryptoKeyVersions/1"
        && key.not_after == "2099-01-01T00:00:00Z"
      ])
      && output.artefact_trust_bundle.revoked == []
    )
    error_message = "The rendered trust bundle does not carry one entry per class naming the version its signer uses."
  }
}

run "a_committed_bundle_matching_the_keys_wires_the_api_to_kms" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
    }
    artefact_trust_bundle_file = "infrastructure/terraform/tests/fixtures/harness-bundle.json"
    google_tls_roots_file      = "infrastructure/terraform/tests/fixtures/harness-roots.pem"
  }

  assert {
    condition = (
      module.cloud_run["api"].environment["QIP_ARTEFACT_SIGNER"] == "kms"
      && module.cloud_run["api"].environment["QIP_TRUST_BUNDLE_PATH"] == "/etc/qip/trust/harness-bundle.json"
      && module.cloud_run["api"].environment["QIP_GOOGLE_TLS_ROOTS_PATH"] == "/etc/qip/trust/harness-roots.pem"
      && contains(keys(module.cloud_run["api"].environment), "QIP_KMS_KEY_VERSIONS_PATH")
    )
    error_message = "A committed, matching bundle did not move the API to KMS signing against the baked bundle."
  }

  assert {
    condition     = local.artefact_api_key_versions == { for class in ["policy", "halt", "envelope"] : class => "projects/artefact-plan-harness/locations/us-east4/keyRings/qip-dev-artefacts/cryptoKeys/qip-dev-${class}/cryptoKeyVersions/1" }
    error_message = "The API's key-version file does not name exactly the three versions it signs under."
  }

  # Not a secret: the signer posture and the paths are plain values and the
  # key-version map is a configuration file, never a mounted secret.
  assert {
    condition     = !contains(keys(module.cloud_run["api"].secret_file_paths), "kms-key-versions")
    error_message = "The key-version map reached the API as a secret."
  }
}

# --- the refusing halves -----------------------------------------------------

run "a_signing_key_below_hsm_is_refused" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
      protection_level = "SOFTWARE"
    }
  }

  expect_failures = [var.artefact_signing]
}

run "a_bundle_horizon_that_is_not_an_instant_is_refused" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "next year"
    }
  }

  expect_failures = [var.artefact_signing]
}

run "a_publish_ref_that_is_not_a_branch_is_refused" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
      publish_ref      = "refs/pull/1/merge"
    }
  }

  expect_failures = [var.artefact_signing]
}

run "a_bundle_named_without_the_keys_is_refused" {
  command = plan

  variables {
    artefact_trust_bundle_file = "infrastructure/terraform/tests/fixtures/harness-bundle.json"
    google_tls_roots_file      = "infrastructure/terraform/tests/fixtures/harness-roots.pem"
  }

  expect_failures = [terraform_data.artefact_trust_bundle_is_current]
}

run "a_bundle_without_pinned_google_roots_is_refused" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
    }
    artefact_trust_bundle_file = "infrastructure/terraform/tests/fixtures/harness-bundle.json"
  }

  expect_failures = [terraform_data.artefact_trust_bundle_is_current]
}

# Only the path is wrong here: no keys are named, so the data sources a
# Terraform 1.9 test cannot plan beside a failed variable are absent.
run "a_bundle_outside_the_baked_directory_is_refused" {
  command = plan

  variables {
    artefact_trust_bundle_file = "data/trust-bundle.json"
    google_tls_roots_file      = "infrastructure/terraform/tests/fixtures/harness-roots.pem"
  }

  expect_failures = [var.artefact_trust_bundle_file]
}

run "a_bundle_that_does_not_publish_the_signing_version_is_refused" {
  command = plan

  variables {
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
      key_versions     = { policy = 2 }
    }
    artefact_trust_bundle_file = "infrastructure/terraform/tests/fixtures/harness-bundle.json"
    google_tls_roots_file      = "infrastructure/terraform/tests/fixtures/harness-roots.pem"
  }

  # The signers moved to policy version 2 and the committed bundle still
  # publishes version 1: every policy frame would be refused by every cell.
  expect_failures = [var.artefact_trust_bundle_file]
}
