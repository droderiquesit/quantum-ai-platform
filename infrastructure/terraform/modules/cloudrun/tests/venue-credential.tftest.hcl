# No Cloud Run workload reads the venue credential, planned (R12-GCP-8114).
#
# The precondition on `google_service_account.workload` refuses the secret by
# either input and in every traffic class. It used to refuse only a
# customer-class mount, and an acceptance test reading the text could not
# tell that from a refusal of every class; only a plan shows the
# precondition fire on each way in and stay quiet on a secret that is not
# the venue's.
#
# Runs against a mocked provider, so it needs no credential and reaches no
# project; `command = plan` throughout, so nothing is applied even in the mock.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id     = "cloudrun-plan-harness"
  project_number = "1"
  region         = "us-east4"
  environment    = "dev"
  labels         = {}
  name           = "harness"
  traffic_class  = "platform"
  plane          = "cognition"
}

run "a_workload_mounting_another_secret_is_admitted" {
  command = plan

  variables {
    secret_mounts = {
      model = {
        secret_id         = "qip-model-api-key"
        file_name         = "key"
        env_file_variable = "QIP_MODEL_API_KEY_FILE"
      }
    }
  }

  assert {
    condition     = length(google_secret_manager_secret_iam_member.mounted) == 1
    error_message = "a mount of a secret that is not the venue credential planned no accessor grant, so the admission above was not exercised."
  }
}

run "a_platform_workload_mounting_the_venue_credential_is_refused" {
  command = plan

  variables {
    secret_mounts = {
      venue = {
        secret_id         = "qip-venue-credential"
        file_name         = "credential"
        env_file_variable = "QIP_VENUE_CREDENTIAL_FILE"
      }
    }
  }

  expect_failures = [google_service_account.workload]
}

run "a_workload_taking_the_venue_credential_as_an_environment_value_is_refused" {
  command = plan

  variables {
    secret_env = {
      QIP_VENUE = {
        secret_id = "qip-venue-credential"
      }
    }
  }

  expect_failures = [google_service_account.workload]
}

run "a_customer_workload_mounting_the_venue_credential_is_still_refused" {
  command = plan

  variables {
    traffic_class = "customer"
    secret_mounts = {
      venue = {
        secret_id         = "qip-venue-credential"
        file_name         = "credential"
        env_file_variable = "QIP_VENUE_CREDENTIAL_FILE"
      }
    }
  }

  expect_failures = [google_service_account.workload]
}
