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
  deployment                = "dev"
  google_tls_roots_path     = "/etc/qip/trust/google-roots.pem"
  admitted_peers            = ["qip-dev-ledgerd.test-project.iam.gserviceaccount.com"]
  workload_certificates = {
    ca_pool              = "projects/test-project/locations/us-east4/caPools/qip-dev-internal"
    certificate_template = "projects/test-project/locations/us-east4/certificateTemplates/qip-dev-workload"
    lifetime_secs        = 86400
    trust_roots_pem      = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n"
  }
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

# RF1 on the local fabric: a second address with no managed log is a second
# writer of one chain.
run "a_second_broker_is_refused_under_rf1" {
  command = plan

  variables {
    broker_internal_addresses = ["10.0.20.10", "10.0.20.11"]
  }

  expect_failures = [var.broker_internal_addresses]
}

# FABRIC-8096, ADR 0111: two to three instances per region behind the
# Spanner lease are admitted, and each is told the lease it contends for.
run "three_brokers_behind_the_spanner_lease_are_three_machines" {
  command = plan

  variables {
    broker_internal_addresses = ["10.0.20.10", "10.0.20.11", "10.0.20.12"]
    managed_log = {
      spanner_database = "projects/test-project/instances/qip-dev/databases/fabric"
    }
  }

  assert {
    condition     = length(google_compute_instance_from_template.event_fabric_broker) == 3
    error_message = "Three addresses behind the managed log did not plan three machines."
  }

  assert {
    condition = alltrue([
      for line in [
        "\nQIP_EVENT_FABRIC_MANAGED_LOG=spanner\n",
        "\nQIP_EVENT_FABRIC_SPANNER_DATABASE=projects/test-project/instances/qip-dev/databases/fabric\n",
        "\nQIP_EVENT_FABRIC_LEASE_TTL_MS=10000\n",
        "\nQIP_EVENT_FABRIC_REGION=us-east4\n",
        "\nQIP_EVENT_FABRIC_INSTANCE=$instance_name\n",
      ] : strcontains(google_compute_instance_template.event_fabric_broker[0].metadata["startup-script"], line)
    ])
    error_message = "The brokers are not told the managed log and the lease they contend for."
  }
}

run "a_fourth_broker_in_a_region_is_refused" {
  command = plan

  variables {
    broker_internal_addresses = ["10.0.20.10", "10.0.20.11", "10.0.20.12", "10.0.20.13"]
    managed_log = {
      spanner_database = "projects/test-project/instances/qip-dev/databases/fabric"
    }
  }

  expect_failures = [var.broker_internal_addresses]
}

# Items 8 and 10: what qip-fabricd refuses to start without outside `local`.
run "the_broker_is_told_its_deployment_its_certificate_and_whom_it_admits" {
  command = plan

  assert {
    condition = alltrue([
      for line in [
        "\nQIP_DEPLOYMENT=dev\n",
        "\nQIP_TLS_MODE=cas\n",
        "\nQIP_CAS_CA_POOL=projects/test-project/locations/us-east4/caPools/qip-dev-internal\n",
        "\nQIP_CAS_CERTIFICATE_TEMPLATE=projects/test-project/locations/us-east4/certificateTemplates/qip-dev-workload\n",
        "\nQIP_CAS_LIFETIME_SECS=86400\n",
        "\nQIP_TLS_TRUST_ROOTS_PATH=/etc/qip/tls-trust-roots.pem\n",
        "\nQIP_GOOGLE_TLS_ROOTS_PATH=/etc/qip/trust/google-roots.pem\n",
        "\nQIP_EVENT_FABRIC_ADMITTED_PEERS=qip-dev-ledgerd.test-project.iam.gserviceaccount.com\n",
      ] : strcontains(google_compute_instance_template.event_fabric_broker[0].metadata["startup-script"], line)
    ])
    error_message = "The broker's environment file lacks a variable qip-fabricd refuses to start without."
  }

  # The local fabric's single broker names no lease.
  assert {
    condition     = !strcontains(google_compute_instance_template.event_fabric_broker[0].metadata["startup-script"], "QIP_EVENT_FABRIC_MANAGED_LOG")
    error_message = "A single local broker was told a managed log nobody configured."
  }
}

run "a_broker_admitting_nobody_by_name_is_refused" {
  command = plan

  variables {
    admitted_peers = []
  }

  expect_failures = [var.admitted_peers]
}

run "an_admitted_peer_that_is_not_a_service_account_san_is_refused" {
  command = plan

  variables {
    admitted_peers = ["qip-dev-ledgerd.test-project.iam.gserviceaccount.com,evil.example.com"]
  }

  expect_failures = [var.admitted_peers]
}

run "a_broker_deployed_as_local_is_refused" {
  command = plan

  variables {
    deployment = "local"
  }

  expect_failures = [var.deployment]
}

run "a_journal_device_carrying_shell_syntax_is_refused" {
  command = plan

  variables {
    qip_storage_journal_device = "/dev/sdb; reboot"
  }

  expect_failures = [var.qip_storage_journal_device]
}
