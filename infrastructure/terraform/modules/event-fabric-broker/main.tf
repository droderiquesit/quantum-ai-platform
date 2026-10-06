# Event Fabric Broker module.
#
# ADR 0100 §2 assigns qip-fabricd the sole writer role of every partition's
# batch chain. ADR 0100 §3 specifies RF1 with fsync before acknowledgement,
# static leader epoch, and no consensus replication (blocked on C2).
# FABRIC-008 requires brokers on dedicated Compute Engine VMs spread across
# three zones in a region. FABRIC-089 requires stable internal addresses and
# no public endpoint. FABRIC-087 requires bootstrap through Cloud DNS SRV records.

terraform {
  required_version = ">= 1.9.8"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

locals {
  broker_count = var.enabled ? 1 : 0 # FABRIC-086 blocked(C2) = RF1 for now
}

# FABRIC-089: Brokers have stable internal addresses and no public endpoint.
# Instance template for event-fabric brokers with no external IP.
resource "google_compute_instance_template" "event_fabric_broker" {
  count       = local.broker_count
  name_prefix = "qip-event-fabric-broker-"
  description = "Event fabric broker instance template, no external IP"

  machine_type = var.machine_type

  service_account {
    email  = var.service_account_email
    scopes = ["https://www.googleapis.com/auth/cloud-platform"]
  }

  boot_disk {
    source_image = var.boot_image
    disk_type    = var.disk_type
    disk_size_gb = var.disk_size_gb

    initialize_params {
      resource_labels = {
        component = "event-fabric-broker"
      }
    }
  }

  network_interface {
    network            = var.network_id
    subnet             = var.subnet_id
    stack_type         = "IPV4_ONLY"
    access_config      = [] # No external IP: FABRIC-089
  }

  metadata = {
    user-data = base64encode(templatefile("${path.module}/startup.sh.tftpl", {
      qip_events_config          = base64encode(var.qip_events_config)
      qip_fabricd_config         = base64encode(var.qip_fabricd_config)
      qip_storage_journal_device = var.qip_storage_journal_device
      broker_instance_name       = "qip-fabricd"
    }))
  }

  tags = ["event-fabric-broker", "internal-only"]

  scheduling {
    automatic_restart   = true
    on_host_maintenance = "MIGRATE"
  }

  lifecycle {
    create_before_destroy = true
  }
}

# FABRIC-087: Clients bootstrap from Cloud DNS SRV records.
# Create internal DNS name for broker discovery via SRV records.
resource "google_dns_record_set" "event_fabric_broker_srv" {
  count = local.broker_count

  name             = "qip-event-fabric-broker.${var.dns_zone_name}"
  type             = "A"
  ttl              = 300
  managed_zone     = var.managed_zone
  rrdatas          = var.broker_internal_addresses
}

# FABRIC-087: SRV record for broker discovery.
resource "google_dns_record_set" "event_fabric_broker_srv_tcp" {
  count = local.broker_count

  name             = "_qip-event-fabric._tcp.${var.dns_zone_name}"
  type             = "SRV"
  ttl              = 300
  managed_zone     = var.managed_zone
  rrdatas = [
    "0 10 ${var.broker_port} qip-event-fabric-broker.${var.dns_zone_name}"
  ]
}

output "instance_template_self_link" {
  value       = try(google_compute_instance_template.event_fabric_broker[0].self_link, null)
  description = "Self link of the event fabric broker instance template"
}

output "broker_dns_name" {
  value       = try(google_dns_record_set.event_fabric_broker_srv[0].name, null)
  description = "Internal DNS name for broker discovery (FABRIC-089)"
}

output "broker_srv_record" {
  value       = try(google_dns_record_set.event_fabric_broker_srv_tcp[0].name, null)
  description = "SRV record name for service discovery (FABRIC-087)"
}
