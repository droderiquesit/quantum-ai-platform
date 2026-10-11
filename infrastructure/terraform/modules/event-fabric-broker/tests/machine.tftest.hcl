# W4.2: the broker is a machine, not only a template.
#
# Until this harness the module declared an instance template and two DNS
# records and no instance, so `enabled = true` published a name for an
# address no machine held, and the template's startup script named a
# journal device (`/dev/sdb`) the template did not declare. Mocked
# provider, `command = plan`: no credential, no project.

mock_provider "google" {}

variables {
  project_id                = "test-project"
  region                    = "us-east4"
  zone                      = "us-east4-a"
  enabled                   = true
  network_id                = "projects/test-project/global/networks/fabric"
  subnet_id                 = "projects/test-project/regions/us-east4/subnetworks/control-fabric"
  boot_image                = "projects/test-project/global/images/qip-broker-image"
  service_account_email     = "qip-dev-fabricd@test-project.iam.gserviceaccount.com"
  dns_zone_name             = "fabric.qip-dev.internal"
  managed_zone              = "qip-dev-fabric"
  broker_internal_addresses = ["10.0.20.10"]
  network_tags              = ["qip-dev-tz-control-fabric"]
  qip_events_config         = "{}"
  qip_fabricd_config        = "{}"
}

run "an_enabled_broker_with_a_zone_is_one_machine_on_its_stable_address" {
  command = plan

  assert {
    condition     = length(google_compute_instance_from_template.event_fabric_broker) == 1
    error_message = "One address and a zone were given and the plan does not create exactly one broker machine."
  }

  assert {
    condition     = google_compute_instance_from_template.event_fabric_broker["10.0.20.10"].network_interface[0].network_ip == "10.0.20.10"
    error_message = "The broker machine is not on the address the A record names."
  }

  assert {
    condition     = length(google_compute_instance_from_template.event_fabric_broker["10.0.20.10"].network_interface[0].access_config) == 0
    error_message = "The broker machine has an external address (FABRIC-089)."
  }
}

run "the_template_declares_the_journal_disk_its_script_mounts" {
  command = plan

  # By position: the boot disk's device name is chosen by Google at apply,
  # so a filter over both disks cannot be evaluated at plan. The journal is
  # the second disk the template declares.
  assert {
    condition = (
      length(google_compute_instance_template.event_fabric_broker[0].disk) == 2
      && google_compute_instance_template.event_fabric_broker[0].disk[1].device_name == "qip-journal"
      && !google_compute_instance_template.event_fabric_broker[0].disk[1].boot
      && !google_compute_instance_template.event_fabric_broker[0].disk[1].auto_delete
    )
    error_message = "The template declares no persistent `qip-journal` disk, so the journal device the script mounts does not exist."
  }

  assert {
    condition     = strcontains(google_compute_instance_template.event_fabric_broker[0].metadata["startup-script"], "/dev/disk/by-id/google-qip-journal")
    error_message = "The startup script does not mount the disk the template declares."
  }

  assert {
    condition     = contains(google_compute_instance_template.event_fabric_broker[0].tags, "qip-dev-tz-control-fabric")
    error_message = "The broker does not carry its trust zone's tag, so the zone's firewall rules do not apply to it."
  }
}

run "no_zone_is_no_machine" {
  command = plan

  variables {
    zone = ""
  }

  assert {
    condition     = length(google_compute_instance_from_template.event_fabric_broker) == 0
    error_message = "A broker machine was planned with no zone named."
  }
}

# RF1: a second address is a second writer of one chain.
run "a_second_broker_is_refused_under_rf1" {
  command = plan

  variables {
    broker_internal_addresses = ["10.0.20.10", "10.0.20.11"]
  }

  expect_failures = [var.broker_internal_addresses]
}

run "a_journal_device_carrying_shell_syntax_is_refused" {
  command = plan

  variables {
    qip_storage_journal_device = "/dev/sdb; reboot"
  }

  expect_failures = [var.qip_storage_journal_device]
}
