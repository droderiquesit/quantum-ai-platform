# FABRIC-087 & FABRIC-089: Broker bootstrap and addressing validation.
# Tests that brokers have stable internal addresses (no public IP) and SRV records exist.

terraform {
  required_version = ">= 1.9.8"
}

mock_provider "google" {}

variables {
  project_id                = "test-project"
  region                    = "us-central1"
  enabled                   = true
  network_id                = "projects/test-project/global/networks/default"
  subnet_id                 = "projects/test-project/regions/us-central1/subnetworks/default"
  machine_type              = "c3-standard-4"
  boot_image                = "projects/test-project/global/images/qip-broker-image"
  service_account_email     = "qip-broker@test-project.iam.gserviceaccount.com"
  dns_zone_name             = "event-fabric.internal"
  managed_zone              = "projects/test-project/managedZones/event-fabric-internal"
  broker_internal_addresses = ["10.0.1.10"]
  qip_events_config         = "base64encodedconfig"
  qip_fabricd_config        = "base64encodedconfig"
}

run "fabric_087_srv_records_created" {
  # FABRIC-087: Clients bootstrap from Cloud DNS SRV records.
  assert {
    condition = try(
      google_dns_record_set.event_fabric_broker_srv[0].name == "qip-event-fabric-broker.event-fabric.internal",
      false
    )
    error_message = "A record for broker discovery (FABRIC-087) was not created."
  }

  assert {
    condition = try(
      google_dns_record_set.event_fabric_broker_srv_tcp[0].name == "_qip-event-fabric._tcp.event-fabric.internal",
      false
    )
    error_message = "SRV record for broker service discovery (FABRIC-087) was not created."
  }

  assert {
    condition = try(
      contains(google_dns_record_set.event_fabric_broker_srv_tcp[0].rrdatas[0], "qip-event-fabric-broker.event-fabric.internal"),
      false
    )
    error_message = "SRV record does not point to broker DNS name (FABRIC-087)."
  }
}

run "fabric_089_no_external_ip" {
  # FABRIC-089: Brokers have stable internal addresses and no public endpoint.
  assert {
    condition = try(
      length(google_compute_instance_template.event_fabric_broker[0].network_interface[0].access_config) == 0,
      false
    )
    error_message = "Broker instance template has external IP (violates FABRIC-089)."
  }

  assert {
    condition = try(
      google_compute_instance_template.event_fabric_broker[0].network_interface[0].stack_type == "IPV4_ONLY",
      false
    )
    error_message = "Broker network interface stack type not set correctly."
  }
}

run "broker_disabled_when_not_enabled" {
  variables {
    enabled = false
  }

  assert {
    condition = try(
      length(google_compute_instance_template.event_fabric_broker) == 0,
      true
    )
    error_message = "Broker resources should not be created when enabled=false."
  }
}
