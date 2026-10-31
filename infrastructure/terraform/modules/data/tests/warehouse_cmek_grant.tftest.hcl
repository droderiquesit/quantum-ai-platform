# Terraform test: the research dataset's key is granted to BigQuery's
# encryption account
#
# `research` sets `default_encryption_configuration` on the warehouse key, and
# BigQuery uses that key as its own per-project encryption service account,
# not as the caller. Until 2026-10-24 nothing granted it the key, so dev — the
# one environment with `enable_bigquery = true` — planned a dataset that the
# first apply had no way to create. The text of `main.tf` cannot show which
# account a data source resolves to; a plan with the data source overridden
# can. `command = plan` throughout, against a mocked provider.
mock_provider "google" {}

variables {
  project_id           = "test-project"
  environment          = "test"
  region               = "us-central1"
  enable_cloud_storage = false
  enable_bigtable      = false
  enable_alloydb       = false
  enable_memorystore   = false
  enable_spanner       = false
  key_ring_id          = "projects/test-project/locations/us-central1/keyRings/test"
  network_id           = "projects/test-project/global/networks/default"
  labels               = {}
}

run "the_warehouse_key_is_granted_to_bigquerys_encryption_account" {
  command = plan

  variables {
    enable_bigquery = true
  }

  override_data {
    target = data.google_bigquery_default_service_account.bigquery[0]
    values = {
      email = "bq-123456789012@bigquery-encryption.iam.gserviceaccount.com"
    }
  }

  # Premise: the dataset that needs the key is planned at all.
  assert {
    condition     = length(google_bigquery_dataset.research) == 1
    error_message = "enable_bigquery = true planned no research dataset, so nothing below is being read."
  }

  assert {
    condition     = length(google_kms_crypto_key_iam_member.warehouse_bigquery_agent) == 1
    error_message = "the research dataset is planned under the warehouse key and nothing grants that key to BigQuery's encryption account."
  }

  # Which key the grant is on is not asserted here: a key's id is computed,
  # Terraform 1.9.8's mocked plan leaves it unknown, and an assertion over an
  # unknown cannot be evaluated. `crypto_key_id` names
  # `google_kms_crypto_key.warehouse[0].id` in main.tf, read as text.

  assert {
    condition     = google_kms_crypto_key_iam_member.warehouse_bigquery_agent[0].role == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
    error_message = "the BigQuery encryption grant is not cryptoKeyEncrypterDecrypter, which is the role CMEK needs: encrypt for writes, decrypt for reads."
  }

  assert {
    condition     = google_kms_crypto_key_iam_member.warehouse_bigquery_agent[0].member == "serviceAccount:bq-123456789012@bigquery-encryption.iam.gserviceaccount.com"
    error_message = "the warehouse key is granted to an account other than the one BigQuery reports as its encryption account."
  }
}

# The refusing half. A grant to Cloud Storage's agent — the shape every other
# key grant in this tree has — would apply cleanly and leave the dataset as
# unable to use its key as no grant at all.
run "a_grant_to_an_account_that_is_not_bigquerys_is_refused" {
  command = plan

  variables {
    enable_bigquery = true
  }

  override_data {
    target = data.google_bigquery_default_service_account.bigquery[0]
    values = {
      email = "service-123456789012@gs-project-accounts.iam.gserviceaccount.com"
    }
  }

  expect_failures = [google_kms_crypto_key_iam_member.warehouse_bigquery_agent]
}

run "no_grant_is_planned_while_bigquery_is_off" {
  command = plan

  variables {
    enable_bigquery = false
  }

  assert {
    condition     = length(google_kms_crypto_key_iam_member.warehouse_bigquery_agent) == 0
    error_message = "enable_bigquery = false planned a grant on a warehouse key that does not exist."
  }
}
