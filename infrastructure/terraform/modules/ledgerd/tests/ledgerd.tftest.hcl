# W4.2: qip-ledgerd is provisionable from committed configuration.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created.

mock_provider "google" {}

variables {
  project_id       = "test-project"
  environment      = "dev"
  zone             = "us-east4-a"
  network_id       = "projects/test-project/global/networks/service"
  subnet_id        = "projects/test-project/regions/us-east4/subnetworks/qip-dev-tz-ledger"
  network_tags     = ["qip-dev-tz-ledger"]
  internal_address = "10.0.21.10"
  boot_image       = "projects/test-project/global/images/qip-ledgerd-20261011"
  account_id       = "paper-book"
}

run "the_ledger_is_one_machine_with_no_external_address_on_its_own_archive_disk" {
  command = plan

  assert {
    condition     = length(google_compute_instance.ledgerd.network_interface[0].access_config) == 0
    error_message = "The ledger machine has an external address."
  }

  assert {
    condition     = google_compute_instance.ledgerd.attached_disk[0].device_name == "qip-ledger"
    error_message = "The archive disk is not attached as `qip-ledger`, so the startup script mounts nothing."
  }

  assert {
    condition     = strcontains(google_compute_instance.ledgerd.metadata["startup-script"], "/dev/disk/by-id/google-qip-ledger")
    error_message = "The startup script does not mount the disk the machine is given."
  }

  assert {
    condition     = strcontains(google_compute_instance.ledgerd.metadata["startup-script"], "QIP_LEDGER_ACCOUNT_ID=paper-book")
    error_message = "The book name does not reach the composition root's configuration."
  }

  assert {
    condition     = google_compute_instance.ledgerd.shielded_instance_config[0].enable_secure_boot
    error_message = "The ledger writer boots without secure boot."
  }

  assert {
    condition     = contains(google_compute_instance.ledgerd.tags, "qip-dev-tz-ledger")
    error_message = "The ledger does not carry its trust zone's tag."
  }
}

# Telemetry and nothing else.
run "the_ledger_identity_holds_only_telemetry_roles" {
  command = plan

  assert {
    condition = (
      google_project_iam_member.logging.role == "roles/logging.logWriter"
      && google_project_iam_member.metrics.role == "roles/monitoring.metricWriter"
    )
    error_message = "The ledger's identity holds something other than log and metric writing."
  }
}

# The refusals that keep `templatefile` from writing a command into a
# root-run script, and a real account number out of committed config.
run "a_book_name_carrying_shell_syntax_is_refused" {
  command = plan

  variables {
    account_id = "paper$(reboot)"
  }

  expect_failures = [var.account_id]
}

run "a_numeric_account_number_is_refused" {
  command = plan

  variables {
    account_id = "12345678"
  }

  expect_failures = [var.account_id]
}

run "an_image_family_is_refused" {
  command = plan

  variables {
    boot_image = "projects/test-project/global/images/family/qip-ledgerd"
  }

  expect_failures = [var.boot_image]
}

run "a_ledger_with_no_zone_tag_is_refused" {
  command = plan

  variables {
    network_tags = []
  }

  expect_failures = [var.network_tags]
}
